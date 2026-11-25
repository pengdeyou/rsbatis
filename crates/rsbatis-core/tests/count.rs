//! `build_count` 的 golden 测试：分页用的总数查询必须与主查询的 FROM / WHERE 保持一致。
//!
//! 这类断言的价值在于：`COUNT` 只要少了一个 `WHERE` 条件，用户看到的就是"总数 100、
//! 但只有 3 条符合条件"——而页面不会报错，只是数字不对。

use rsbatis_core::{select, Column, Dialect, Entity, JoinKind, Order};

struct User;
impl Entity for User {
    const TABLE: &'static str = "users";
    const COLUMNS: &'static [&'static str] = &["id", "name", "age", "status"];
}

struct Order_;
impl Entity for Order_ {
    const TABLE: &'static str = "orders";
    const COLUMNS: &'static [&'static str] = &["id", "user_id", "amount"];
}

fn age() -> Column<i32> {
    Column::new(User::TABLE, "age")
}

#[test]
fn count_without_join_has_no_table_qualifier() {
    let stmt = select::<User>().where_(age().ge(18));
    let sql = stmt.build_count(Dialect::Sqlite);
    assert_eq!(sql.text, r#"SELECT COUNT(*) FROM "users" WHERE "age" >= ?"#);
    assert_eq!(sql.params.len(), 1);
}

#[test]
fn count_drops_order_limit_offset() {
    // 分页时把 LIMIT 9 留在 count 里，总数就会永远不超过 9
    let stmt = select::<User>()
        .where_(age().ge(18))
        .order_by(Column::<i64>::new(User::TABLE, "id").expr(), Order::Desc)
        .limit(9)
        .offset(18);
    let sql = stmt.build_count(Dialect::Sqlite);
    assert!(!sql.text.contains("ORDER BY"), "{}", sql.text);
    assert!(!sql.text.contains("LIMIT"), "{}", sql.text);
    assert!(!sql.text.contains("OFFSET"), "{}", sql.text);
    assert_eq!(sql.text, r#"SELECT COUNT(*) FROM "users" WHERE "age" >= ?"#);
}

#[test]
fn count_keeps_join_and_qualifies_columns() {
    let stmt = select::<User>()
        .join::<Order_>(
            JoinKind::Inner,
            Column::<i64>::new(User::TABLE, "id")
                .eq_col(Column::<i64>::new(Order_::TABLE, "user_id")),
        )
        .where_(age().ge(18));
    let sql = stmt.build_count(Dialect::MySql);
    assert!(
        sql.text
            .starts_with("SELECT COUNT(*) FROM `users` INNER JOIN `orders` ON"),
        "{}",
        sql.text
    );
    // 有 join 时必须限定表名，否则同名列会让数据库选错表
    assert!(
        sql.text.contains("`age` >= ?") || sql.text.contains("`users`.`age` >= ?"),
        "{}",
        sql.text
    );
    assert!(!sql.text.contains("LIMIT"), "{}", sql.text);
}

#[test]
fn count_with_distinct_wraps_a_subquery() {
    // COUNT(*) + DISTINCT 在 SQL 里不合法；去重后再计数只能用子查询表达
    let stmt = select::<User>().distinct().where_(age().ge(18)).limit(5);
    let sql = stmt.build_count(Dialect::Sqlite);
    assert!(
        sql.text
            .starts_with("SELECT COUNT(*) FROM (SELECT DISTINCT"),
        "{}",
        sql.text
    );
    assert!(
        sql.text.ends_with(r#") AS "__rsbatis_count""#),
        "{}",
        sql.text
    );
    assert_eq!(sql.params.len(), 1);
}

#[test]
fn count_keeps_param_order_for_multi_condition_filters() {
    let stmt = select::<User>().where_(rsbatis_core::Expr::and(vec![
        age().ge(18),
        Column::<String>::new(User::TABLE, "name").like("zh%"),
    ]));
    let sql = stmt.build_count(Dialect::Sqlite);
    assert_eq!(
        sql.text,
        r#"SELECT COUNT(*) FROM "users" WHERE "age" >= ? AND "name" LIKE ?"#
    );
    assert_eq!(sql.params.len(), 2);
}
