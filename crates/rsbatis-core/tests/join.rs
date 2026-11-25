//! 联表渲染的 golden 基线。
use rsbatis_core::{select, Column, Dialect, Entity, JoinKind, Order, SelectStmt};

struct Users;
impl Entity for Users {
    const TABLE: &'static str = "users";
    const COLUMNS: &'static [&'static str] = &["id", "name", "age"];
}

struct Orders;
impl Entity for Orders {
    const TABLE: &'static str = "orders";
    const COLUMNS: &'static [&'static str] = &["id", "user_id", "amount"];
}

fn col<T>(table: &'static str, name: &'static str) -> Column<T> {
    Column::new(table, name)
}

#[test]
fn inner_join_auto_projects_with_prefixed_alias() {
    let sql = select::<Users>()
        .join::<Orders>(
            JoinKind::Inner,
            col::<i64>("orders", "user_id").eq_col(col::<i64>("users", "id")),
        )
        .where_(col::<i32>("users", "age").ge(18))
        .order_by(col::<i64>("orders", "amount").expr(), Order::Desc)
        .build(Dialect::Sqlite);

    assert_eq!(
        "SELECT \"users\".\"id\" AS \"users__id\", \"users\".\"name\" AS \"users__name\", \
         \"users\".\"age\" AS \"users__age\", \"orders\".\"id\" AS \"orders__id\", \
         \"orders\".\"user_id\" AS \"orders__user_id\", \"orders\".\"amount\" AS \"orders__amount\" \
         FROM \"users\" INNER JOIN \"orders\" ON \"orders\".\"user_id\" = \"users\".\"id\" \
         WHERE \"users\".\"age\" >= ? ORDER BY \"orders\".\"amount\" DESC",
        sql.text
    );
    assert_eq!(1, sql.params.len(), "只有 where 的值进参数");
    assert!(sql.warnings.is_empty());
}

#[test]
fn mysql_uses_backticks_and_left_join_with_paging() {
    let sql = select::<Users>()
        .join::<Orders>(
            JoinKind::Left,
            col::<i64>("orders", "user_id").eq_col(col::<i64>("users", "id")),
        )
        .limit(10)
        .offset(20)
        .build(Dialect::MySql);

    assert!(sql.text.starts_with(
        "SELECT `users`.`id` AS `users__id`, `users`.`name` AS `users__name`, `users`.`age` AS `users__age`"
    ));
    assert!(sql
        .text
        .contains("FROM `users` LEFT JOIN `orders` ON `orders`.`user_id` = `users`.`id`"));
    assert!(sql.text.ends_with("LIMIT 10 OFFSET 20"));
    assert!(sql.warnings.is_empty());
}

#[test]
fn single_table_keeps_star_and_bare_column_names() {
    // 回归：没有 join 时行为必须和以前逐字节一致
    let sql = select::<Users>()
        .where_(col::<i32>("users", "age").ge(18))
        .build(Dialect::Sqlite);
    assert_eq!("SELECT * FROM \"users\" WHERE \"age\" >= ?", sql.text);
}

#[test]
fn cross_join_has_no_on_clause() {
    let sql = select::<Users>()
        .cross_join::<Orders>()
        .build(Dialect::Sqlite);
    assert!(
        sql.text.contains("FROM \"users\" CROSS JOIN \"orders\""),
        "{}",
        sql.text
    );
    assert!(
        !sql.text.contains(" ON "),
        "CROSS JOIN 不该有 ON：{}",
        sql.text
    );
}

#[test]
fn missing_main_columns_is_warned_not_silent() {
    // 用 new() 构造（没有实体列信息）再联表：投影会漏主表，必须告警
    let sql = SelectStmt::new("users")
        .join::<Orders>(
            JoinKind::Inner,
            col::<i64>("orders", "user_id").eq_col(col::<i64>("users", "id")),
        )
        .build(Dialect::Sqlite);
    assert!(sql.text.contains("\"orders\".\"id\" AS \"orders__id\""));
    assert_eq!(1, sql.warnings.len());
    assert!(sql.warnings[0].contains("缺少主表列信息"));
}
