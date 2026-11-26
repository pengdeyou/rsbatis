//! Golden 测试：把"某表达式在某方言下必须渲染成什么"固定下来。
//!
//! 这些断言是回归保护网——之后改渲染器时，任何一处输出变化都会在这里失败。
//! 同时它们也是文档：看一遍就知道每个语法特性最终长什么样。

use rsbatis_core::{select, ArithOp, BinOp, Column, Dialect, Entity, Expr, Order, Value};

struct User;

impl Entity for User {
    const TABLE: &'static str = "user";
    const COLUMNS: &'static [&'static str] = &["id", "name", "age"];
}

fn age() -> Column<i32> {
    Column::new(User::TABLE, "age")
}

fn name() -> Column<String> {
    Column::new(User::TABLE, "name")
}

fn id() -> Column<i64> {
    Column::new(User::TABLE, "id")
}

fn render(e: Expr) -> String {
    select::<User>().filter(e).build(Dialect::Sqlite).text
}

fn pg(e: Expr) -> String {
    select::<User>().filter(e).build(Dialect::Postgres).text
}

fn my(e: Expr) -> String {
    select::<User>().filter(e).build(Dialect::MySql).text
}

// ---------- 括号 ----------

#[test]
fn explicit_parens_are_preserved() {
    // E1 F1 修复：用户手写的括号必须原样保留
    let e = Expr::paren(Expr::or(vec![age().lt(18), age().gt(65)]));
    assert_eq!(
        render(e),
        r#"SELECT * FROM "user" WHERE ("age" < ? OR "age" > ?)"#
    );
}

#[test]
fn or_inside_and_is_auto_parenthesized() {
    let e = Expr::and(vec![
        age().ge(18),
        Expr::or(vec![name().eq("a"), name().eq("b")]),
    ]);
    assert_eq!(
        render(e),
        r#"SELECT * FROM "user" WHERE "age" >= ? AND ("name" = ? OR "name" = ?)"#
    );
}

#[test]
fn and_inside_or_needs_no_parens() {
    // AND 优先级高于 OR，所以不需要括号
    let e = Expr::or(vec![
        Expr::and(vec![age().ge(18), name().eq("a")]),
        name().eq("b"),
    ]);
    assert_eq!(
        render(e),
        r#"SELECT * FROM "user" WHERE "age" >= ? AND "name" = ? OR "name" = ?"#
    );
}

#[test]
fn not_wraps_comparison_in_parens() {
    // E1 F3 修复：`NOT "age" >= ?` 虽然语义正确但难读，加括号
    assert_eq!(
        render(Expr::not(age().ge(18))),
        r#"SELECT * FROM "user" WHERE NOT ("age" >= ?)"#
    );
}

#[test]
fn not_wraps_compound_condition() {
    let e = Expr::not(Expr::and(vec![age().ge(18), name().like("a%")]));
    assert_eq!(
        render(e),
        r#"SELECT * FROM "user" WHERE NOT ("age" >= ? AND "name" LIKE ?)"#
    );
}

// ---------- 集合 ----------

#[test]
fn in_list_renders_one_placeholder_per_value() {
    let e = age().in_([18, 19, 20]);
    assert_eq!(
        render(e),
        r#"SELECT * FROM "user" WHERE "age" IN (?, ?, ?)"#
    );
}

#[test]
fn empty_in_becomes_always_false() {
    // 绝不允许把空 IN 悄悄丢掉——那会让 `NOT IN ()` 变成全表命中
    let e = age().in_(Vec::<i32>::new());
    assert_eq!(render(e), r#"SELECT * FROM "user" WHERE 0 = 1"#);
}

#[test]
fn empty_not_in_becomes_always_true() {
    let e = age().not_in(Vec::<i32>::new());
    assert_eq!(render(e), r#"SELECT * FROM "user" WHERE 1 = 1"#);
}

#[test]
fn between_renders_two_params() {
    let e = age().between(18, 65);
    assert_eq!(
        render(e),
        r#"SELECT * FROM "user" WHERE "age" BETWEEN ? AND ?"#
    );
}

#[test]
fn not_between() {
    let e = age().not_between(18, 65);
    assert_eq!(
        render(e),
        r#"SELECT * FROM "user" WHERE "age" NOT BETWEEN ? AND ?"#
    );
}

// ---------- 算术 ----------

#[test]
fn arithmetic_precedence_matches_sql() {
    // age + 1 * 2 → 不补括号（* 优先级高于 +）
    let e = Expr::arith(
        ArithOp::Add,
        age().expr(),
        Expr::arith(ArithOp::Mul, Expr::lit(1), Expr::lit(2)),
    );
    assert_eq!(render(e), r#"SELECT * FROM "user" WHERE "age" + ? * ?"#);
}

#[test]
fn arithmetic_that_needs_parens() {
    // (age + 1) * 2 → 必须补括号
    let e = Expr::arith(
        ArithOp::Mul,
        Expr::arith(ArithOp::Add, age().expr(), Expr::lit(1)),
        Expr::lit(2),
    );
    assert_eq!(render(e), r#"SELECT * FROM "user" WHERE ("age" + ?) * ?"#);
}

#[test]
fn unary_neg() {
    let e = Expr::cmp(BinOp::Lt, Expr::neg(age().expr()), Expr::lit(0));
    assert_eq!(render(e), r#"SELECT * FROM "user" WHERE -"age" < ?"#);
}

// ---------- 函数 ----------

#[test]
fn count_star() {
    let e = Expr::cmp(
        BinOp::Gt,
        Expr::func("COUNT", vec![Expr::Wildcard]),
        Expr::lit(0i64),
    );
    assert_eq!(render(e), r#"SELECT * FROM "user" WHERE COUNT(*) > ?"#);
}

#[test]
fn scalar_function_with_column_arg() {
    let e = Expr::cmp(
        BinOp::Eq,
        Expr::func("LOWER", vec![name().expr()]),
        Expr::lit("lisi"),
    );
    assert_eq!(render(e), r#"SELECT * FROM "user" WHERE LOWER("name") = ?"#);
}

// ---------- CASE / CAST ----------

#[test]
fn searched_case() {
    let e = Expr::case(
        None,
        vec![
            (age().ge(18), Expr::lit("adult")),
            (age().lt(18), Expr::lit("minor")),
        ],
        Some(Expr::lit("unknown")),
    );
    assert_eq!(
        render(e),
        r#"SELECT * FROM "user" WHERE CASE WHEN "age" >= ? THEN ? WHEN "age" < ? THEN ? ELSE ? END"#
    );
}

#[test]
fn simple_case_with_operand() {
    let e = Expr::case(
        Some(age().expr()),
        vec![(Expr::lit(18), Expr::lit("eighteen"))],
        None,
    );
    assert_eq!(
        render(e),
        r#"SELECT * FROM "user" WHERE CASE "age" WHEN ? THEN ? END"#
    );
}

#[test]
fn cast() {
    let e = Expr::cmp(BinOp::Eq, Expr::cast(age().expr(), "TEXT"), Expr::lit("18"));
    assert_eq!(
        render(e),
        r#"SELECT * FROM "user" WHERE CAST("age" AS TEXT) = ?"#
    );
}

// ---------- 三方言占位符编号 ----------

#[test]
fn postgres_numbers_all_params_in_order() {
    let e = Expr::and(vec![
        age().ge(18),
        name().like("a%"),
        id().in_([1i64, 2, 3]),
    ]);
    let sql = select::<User>().filter(e).build(Dialect::Postgres);
    assert_eq!(
        sql.text,
        r#"SELECT * FROM "user" WHERE "age" >= $1 AND "name" LIKE $2 AND "id" IN ($3, $4, $5)"#
    );
    assert_eq!(sql.params.len(), 5);
    assert_eq!(sql.params[0], Value::I32(18));
    assert_eq!(sql.params[1], Value::Text("a%".into()));
    assert_eq!(sql.params[2], Value::I64(1));
}

#[test]
fn mysql_uses_backticks_and_question_marks() {
    let e = Expr::and(vec![age().ge(18), name().like("a%")]);
    assert_eq!(
        my(e),
        "SELECT * FROM `user` WHERE `age` >= ? AND `name` LIKE ?"
    );
}

#[test]
fn sqlite_uses_double_quotes() {
    let e = Expr::and(vec![age().ge(18), name().like("a%")]);
    assert_eq!(
        render(e),
        r#"SELECT * FROM "user" WHERE "age" >= ? AND "name" LIKE ?"#
    );
}

#[test]
fn postgres_between_numbering() {
    let e = Expr::and(vec![age().between(18, 65), name().like("a%")]);
    assert_eq!(
        pg(e),
        r#"SELECT * FROM "user" WHERE "age" BETWEEN $1 AND $2 AND "name" LIKE $3"#
    );
}

// ---------- SELECT 的其他子句 ----------

#[test]
fn distinct_columns_order_limit_offset() {
    let sql = select::<User>()
        .distinct()
        .column("name")
        .order_by(name().expr(), Order::Desc)
        .limit(10)
        .offset(20)
        .build(Dialect::Sqlite);
    assert_eq!(
        sql.text,
        r#"SELECT DISTINCT "name" FROM "user" ORDER BY "name" DESC LIMIT 10 OFFSET 20"#
    );
    assert!(sql.params.is_empty());
}

#[test]
fn column_alias_in_projection() {
    let sql = select::<User>()
        .column_expr(Expr::func("COUNT", vec![Expr::Wildcard]).as_("total"))
        .build(Dialect::MySql);
    assert_eq!(sql.text, "SELECT COUNT(*) AS `total` FROM `user`");
}

#[test]
fn multi_column_projection_with_alias() {
    let sql = select::<User>()
        .column("id")
        .column_expr(name().as_("nickname"))
        .build(Dialect::Postgres);
    assert_eq!(sql.text, r#"SELECT "id", "name" AS "nickname" FROM "user""#);
}

#[test]
fn and_filter_chains_conditions() {
    let sql = select::<User>()
        .filter(age().ge(18))
        .and_filter(name().like("a%"))
        .and_filter(Expr::not(age().gt(60)))
        .build(Dialect::Sqlite);
    assert_eq!(
        sql.text,
        r#"SELECT * FROM "user" WHERE "age" >= ? AND "name" LIKE ? AND NOT ("age" > ?)"#
    );
    assert_eq!(sql.params.len(), 3);
}

// ---------- 参数覆盖不全的检查 ----------

#[test]
fn null_param_is_bound_as_null() {
    let e = name().eq(Option::<String>::None);
    let sql = select::<User>().filter(e).build(Dialect::Sqlite);
    assert_eq!(sql.text, r#"SELECT * FROM "user" WHERE "name" = ?"#);
    assert_eq!(sql.params, vec![Value::Null]);
}

#[test]
fn bool_param_and_literal_forms() {
    let active = Column::<bool>::new("user", "active");
    let sql = select::<User>()
        .filter(active.eq(true))
        .build(Dialect::MySql);
    assert_eq!(sql.text, "SELECT * FROM `user` WHERE `active` = ?");
    assert_eq!(sql.params, vec![Value::Bool(true)]);
    assert_eq!(Dialect::MySql.bool_literal(true), "1");
    assert_eq!(Dialect::Sqlite.bool_literal(true), "TRUE");
}
