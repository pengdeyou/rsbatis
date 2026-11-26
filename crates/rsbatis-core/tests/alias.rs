//! 表别名：自连接与"同一实体多次 join"的唯一解法。
//!
//! 核心约束：`Column` 是 `Copy` 的纯数据，不持有上下文，所以"该用哪个别名"
//! 只能由调用点通过 `Column::of_table(别名)` 指定。

use rsbatis_core::{select, Column, Dialect, Entity, JoinKind, Order};

struct User;

impl Entity for User {
    const TABLE: &'static str = "user";
    const COLUMNS: &'static [&'static str] = &["id", "manager_id", "age"];
}

fn id() -> Column<i64> {
    Column::new(User::TABLE, "id")
}
fn manager_id() -> Column<i64> {
    Column::new(User::TABLE, "manager_id")
}
fn age() -> Column<i32> {
    Column::new(User::TABLE, "age")
}

#[test]
fn self_join_uses_two_aliases() {
    // 自己连自己：u 是被查的人，m 是他的上级
    let sql = select::<User>()
        .from_alias("u")
        .join_as::<User>(
            JoinKind::Inner,
            "m",
            manager_id().of_table("u").eq_col(id().of_table("m")),
        )
        .filter(age().of_table("u").ge(18))
        .order_by(id().of_table("u").expr(), Order::Asc)
        .build(Dialect::Sqlite);

    assert_eq!(
        sql.text,
        "SELECT \"u\".\"id\" AS \"u__id\", \"u\".\"manager_id\" AS \"u__manager_id\", \
         \"u\".\"age\" AS \"u__age\", \"m\".\"id\" AS \"m__id\", \
         \"m\".\"manager_id\" AS \"m__manager_id\", \"m\".\"age\" AS \"m__age\" \
         FROM \"user\" AS \"u\" INNER JOIN \"user\" AS \"m\" ON \"u\".\"manager_id\" = \"m\".\"id\" \
         WHERE \"u\".\"age\" >= ? ORDER BY \"u\".\"id\" ASC"
    );
    assert_eq!(sql.params, vec![rsbatis_core::Value::I32(18)]);
}

#[test]
fn alias_without_join_still_qualifies() {
    // 只给主表起别名也必须走限定列名，否则投影和过滤会指向裸表名
    let sql = select::<User>()
        .from_alias("u")
        .filter(age().of_table("u").ge(18))
        .build(Dialect::Sqlite);
    assert_eq!(
        sql.text,
        "SELECT \"u\".\"id\" AS \"u__id\", \"u\".\"manager_id\" AS \"u__manager_id\", \
         \"u\".\"age\" AS \"u__age\" FROM \"user\" AS \"u\" WHERE \"u\".\"age\" >= ?"
    );
    assert_eq!(Some("u"), sql_alias(&sql.text), "主表别名要出现在 FROM 里");
}

fn sql_alias(s: &str) -> Option<&str> {
    s.split(" FROM \"user\" AS \"")
        .nth(1)
        .map(|r| r.split('"').next().unwrap())
}

#[test]
fn alias_across_dialects() {
    let build = |d| {
        select::<User>()
            .from_alias("u")
            .join_as::<User>(
                JoinKind::Left,
                "m",
                manager_id().of_table("u").eq_col(id().of_table("m")),
            )
            .build(d)
            .text
    };
    let my = build(Dialect::MySql);
    assert!(
        my.contains("FROM `user` AS `u` LEFT JOIN `user` AS `m`"),
        "实际：{my}"
    );
    let pg = build(Dialect::Postgres);
    assert!(
        pg.contains("FROM \"user\" AS \"u\" LEFT JOIN \"user\" AS \"m\""),
        "实际：{pg}"
    );
}

#[test]
fn mapped_columns_do_not_leak_into_other_query() {
    // 别名只作用在这一份 Column 上；原列不受影响（Copy 语义）
    let aliased = id().of_table("u");
    assert_eq!("u", aliased.table());
    assert_eq!("user", id().table(), "原来那个列还是裸表名");
}

#[test]
fn alias_on_plain_join_leaves_baseline_untouched() {
    // 不带别名时输出必须与 M5 完全一致（回归保护）
    let sql = select::<User>()
        .join::<User>(JoinKind::Inner, manager_id().eq_col(id()))
        .build(Dialect::Sqlite)
        .text;
    assert!(
        sql.contains("INNER JOIN \"user\" ON"),
        "无别名时不应出现 AS，实际：{sql}"
    );
    assert!(!sql.contains(" AS \"user\""), "实际：{sql}");
}
