//! 子查询（`EXISTS` / `IN (…)`）的渲染契约。
//!
//! 这里最关键的断言不是"能渲染出来"，而是**参数编号与外层连续**：
//! 子查询如果自己从 1 开始编号，最终 SQL 里的 `?` 与参数列表会整体错位——
//! 这种错误在 SQLite 上往往表现为"查不到数据"，在 MySQL 上甚至可能返回看似合理的结果，
//! 是子查询实现里最难发现的一类缺陷。

use rsbatis_core::{Dialect, Expr, SelectStmt, Value};

fn outer() -> SelectStmt {
    SelectStmt::new("users").column("id")
}

fn inner() -> SelectStmt {
    SelectStmt::new("orders")
        .column("user_id")
        .filter(Expr::cmp(
            rsbatis_core::BinOp::Ge,
            Expr::Col("orders", "amount"),
            Expr::lit(100_i64),
        ))
}

#[test]
fn in_subquery_renders_and_keeps_param_numbering() {
    let mut q = outer();
    q = q.filter(Expr::cmp(
        rsbatis_core::BinOp::Ge,
        Expr::Col("users", "id"),
        Expr::lit(1_i64),
    ));
    q = q.and_filter(Expr::in_subquery(Expr::Col("users", "id"), &inner()));

    let sql = q.build(Dialect::Sqlite);
    assert_eq!(
        "SELECT \"id\" FROM \"users\" WHERE \"id\" >= ? AND \"id\" IN (SELECT \"orders\".\"user_id\" FROM \"orders\" WHERE \"orders\".\"amount\" >= ?)",
        sql.text
    );
    // 外层 id>=1 是第 1 个参数，子查询 amount>=100 必须是第 2 个
    assert_eq!(2, sql.params.len());
    assert_eq!(Value::I64(1), sql.params[0]);
    assert_eq!(Value::I64(100), sql.params[1]);
}

#[test]
fn subquery_params_start_after_outer_params() {
    // 反向验证：外层有 3 个参数时，子查询参数必须从第 4 个开始
    let mut q = SelectStmt::new("users").column("id");
    q = q.filter(Expr::cmp(
        rsbatis_core::BinOp::Ge,
        Expr::Col("users", "age"),
        Expr::lit(18_i64),
    ));
    // 用 and_filter 追加（filter 是覆盖语义，重复调用会替换条件）
    q = q.and_filter(Expr::lit("x"));
    q = q.and_filter(Expr::lit("y"));
    q = q.and_filter(Expr::in_subquery(Expr::Col("users", "id"), &inner()));

    let sql = q.build(Dialect::Postgres);
    // PG 用 $N 编号，正好能一眼看出编号是否连续
    assert!(sql.text.contains("$2"), "got: {}", sql.text);
    assert!(
        sql.text.contains("$4"),
        "子查询的第一个参数应是 $4，got: {}",
        sql.text
    );
    assert_eq!(4, sql.params.len());
    assert_eq!(Value::I64(100), sql.params[3]);
}

#[test]
fn not_in_subquery_renders_with_negation() {
    let sql = outer()
        .filter(Expr::not_in_subquery(Expr::Col("users", "id"), &inner()))
        .build(Dialect::MySql);
    assert!(
        sql.text.contains("NOT IN (SELECT"),
        "MySQL 侧也要正确渲染 NOT IN，got: {}",
        sql.text
    );
}

#[test]
fn exists_forces_table_qualification() {
    // 相关子查询：子查询里引用外层 users.id。若不强制带表名，会渲染成裸 "id"，
    // 被解析成 orders 表自己的 id 列——静默查错表。
    let sub = SelectStmt::new("orders")
        .column_expr(Expr::lit(1_i64))
        .filter(Expr::cmp(
            rsbatis_core::BinOp::Eq,
            Expr::Col("orders", "user_id"),
            Expr::Col("users", "id"),
        ));
    let sql = outer().filter(Expr::exists(&sub)).build(Dialect::Sqlite);
    assert_eq!(
        "SELECT \"id\" FROM \"users\" WHERE EXISTS (SELECT ? FROM \"orders\" WHERE \"orders\".\"user_id\" = \"users\".\"id\")",
        sql.text
    );
}

#[test]
fn exists_inside_or_branch_keeps_parentheses() {
    // EXISTS 与比较同级（4），放进 OR 里要保留结构，不能被优先级规则吃掉括号
    let sub = SelectStmt::new("orders").column_expr(Expr::lit(1_i64));
    let expr = Expr::or(vec![
        Expr::cmp(
            rsbatis_core::BinOp::Eq,
            Expr::Col("users", "id"),
            Expr::lit(1_i64),
        ),
        Expr::exists(&sub),
    ]);
    let sql = outer().filter(expr).build(Dialect::Sqlite);
    // OR 的右分支本身是谓词级表达式，不需要额外括号
    assert!(
        sql.text.contains("= ? OR EXISTS (SELECT"),
        "got: {}",
        sql.text
    );
}

#[test]
fn nested_subquery_keeps_numbering() {
    // 两层嵌套：users IN (orders WHERE user_id IN (payments …))
    let deepest = SelectStmt::new("payments")
        .column("order_id")
        .filter(Expr::cmp(
            rsbatis_core::BinOp::Ge,
            Expr::Col("payments", "amount"),
            Expr::lit(50_i64),
        ));
    let mid = SelectStmt::new("orders")
        .column("user_id")
        .filter(Expr::in_subquery(Expr::Col("orders", "id"), &deepest));

    let sql = outer()
        .filter(Expr::cmp(
            rsbatis_core::BinOp::Ge,
            Expr::Col("users", "age"),
            Expr::lit(1_i64),
        ))
        .and_filter(Expr::in_subquery(Expr::Col("users", "id"), &mid))
        .build(Dialect::Sqlite);

    assert_eq!(2, sql.params.len(), "两层子查询也只能有 2 个参数");
    assert_eq!(Value::I64(1), sql.params[0]);
    assert_eq!(Value::I64(50), sql.params[1]);
    assert_eq!(
        2,
        sql.text.matches('?').count(),
        "SQL 里也必须是 2 个占位符"
    );
}

#[test]
fn mysql_warns_on_in_with_limit() {
    // MariaDB / MySQL 会直接报 1235 拒绝这条 SQL；渲染阶段必须给出可读的告警，
    // 而不是让用户拿着一条正常-looking 的 SQL 去撞数据库错误。
    let sub = SelectStmt::new("orders").column("user_id").limit(1);
    let sql = outer()
        .filter(Expr::in_subquery(Expr::Col("users", "id"), &sub))
        .build(Dialect::MySql);
    assert_eq!(1, sql.warnings.len(), "MySQL 侧应有一条告警");
    assert!(sql.warnings[0].contains("1235"), "告警要带上错误码便于搜索");

    // 同样的 SQL 在 SQLite / PG 上没有这个限制，不应该告警
    let sql = outer()
        .filter(Expr::in_subquery(Expr::Col("users", "id"), &sub))
        .build(Dialect::Sqlite);
    assert!(sql.warnings.is_empty(), "SQLite 不该告警");
}
