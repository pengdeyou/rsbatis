//! 表别名端到端：宏 `as` 语法、自连接、同表两次 join。
//!
//! 自连接不能用元组映射（两个前缀必须不同），所以这里走手工
//! `Entity::from_row_prefix(row, "别名")`——这条路一直是公开的，只是现在才真正用得上。
#![cfg(feature = "sqlite")]

use rsbatis::{select, Dialect, Entity, FromRow, Session, SqliteDriver, Value};

#[derive(Entity, FromRow)]
#[rsbatis(table = "emp")]
#[allow(dead_code)] // id 只用于 SQL 侧，测试不需要读它
struct Emp {
    id: i64,
    name: String,
    manager_id: Option<i64>,
}

fn setup() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        r#"CREATE TABLE "emp" (id INTEGER PRIMARY KEY, name TEXT NOT NULL, manager_id INTEGER);
           INSERT INTO "emp" VALUES
             (1, '老板', NULL),
             (2, '张三', 1),
             (3, '李四', 1),
             (4, '王五', 2);"#,
    )
    .unwrap();
    db
}

#[test]
fn macro_renders_self_join_sql() {
    let sql = select! {
        Emp as e join Emp as m on e.manager_id == m.id
        where m.name == "老板" order_by e.id asc
    }
    .build(Dialect::Sqlite);

    assert_eq!(
        sql.text,
        "SELECT \"e\".\"id\" AS \"e__id\", \"e\".\"name\" AS \"e__name\", \
         \"e\".\"manager_id\" AS \"e__manager_id\", \"m\".\"id\" AS \"m__id\", \
         \"m\".\"name\" AS \"m__name\", \"m\".\"manager_id\" AS \"m__manager_id\" \
         FROM \"emp\" AS \"e\" INNER JOIN \"emp\" AS \"m\" ON \"e\".\"manager_id\" = \"m\".\"id\" \
         WHERE \"m\".\"name\" = ? ORDER BY \"e\".\"id\" ASC"
    );
    assert_eq!(sql.params, vec![Value::Text("老板".into())]);
}

#[test]
fn self_join_reads_two_prefixes() {
    let mut db = setup();
    let rows = db
        .fetch(select! {
            Emp as e join Emp as m on e.manager_id == m.id
            where m.name == "老板" order_by e.id asc
        })
        .expect("自连接");
    assert_eq!(2, rows.len(), "老板手下两个直接下属");

    let first = Emp::from_row_prefix(&rows[0], "e").expect("被查的人");
    let boss = Emp::from_row_prefix(&rows[0], "m").expect("上级");
    assert_eq!("张三", first.name);
    assert_eq!("老板", boss.name);
    assert_eq!(Some(1), first.manager_id);

    let second = Emp::from_row_prefix(&rows[1], "e").unwrap();
    assert_eq!("李四", second.name);

    // 左侧前缀取到的一定是下属，右侧一定是上级——不是同一行被读了两次
    assert_ne!(first.name, boss.name);
}

#[test]
fn aliased_main_table_without_join() {
    let mut db = setup();
    let rows = db
        .fetch(select! { Emp as e where e.manager_id == null })
        .expect("查没有上级的人");
    assert_eq!(1, rows.len());
    let boss = Emp::from_row_prefix(&rows[0], "e").unwrap();
    assert_eq!("老板", boss.name);
    assert_eq!(None, boss.manager_id);
}

#[test]
fn two_joins_of_same_entity_with_different_aliases() {
    // 三层：王五 -> 张三 -> 老板。同一实体连两次，别名必须不同
    let mut db = setup();
    let rows = db
        .fetch(select! {
            Emp as a join Emp as b on a.manager_id == b.id
                    join Emp as c on b.manager_id == c.id
            where a.name == "王五"
        })
        .expect("多级别名连接");
    assert_eq!(1, rows.len());
    assert_eq!("王五", Emp::from_row_prefix(&rows[0], "a").unwrap().name);
    assert_eq!("张三", Emp::from_row_prefix(&rows[0], "b").unwrap().name);
    assert_eq!("老板", Emp::from_row_prefix(&rows[0], "c").unwrap().name);
}

#[test]
fn alias_mixed_with_bare_entity_still_works() {
    // 主表不起别名、join 起别名：ON 里裸表名与别名可以共存
    let sql = select! { Emp join Emp as m on Emp.manager_id == m.id }.build(Dialect::Sqlite);
    assert!(
        sql.text.contains(
            "FROM \"emp\" INNER JOIN \"emp\" AS \"m\" ON \"emp\".\"manager_id\" = \"m\".\"id\""
        ),
        "实际：{}",
        sql.text
    );
    assert!(
        !sql.text.contains("FROM \"emp\" AS"),
        "主表没写别名就不该出现 AS"
    );
}
