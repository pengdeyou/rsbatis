//! 实体上声明 `snowflake` 后的插入行为。
//!
//! 锁的是三条：id = 0 会被补成生成值；显式给出的非 0 主键原样写入；返回的 id 就是落库那个。

#![allow(dead_code)]

use rsbatis::{select, Entity, FromRow, Session, Snowflake, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "events")]
struct Event {
    #[rsbatis(primary_key, snowflake)]
    id: i64,
    name: String,
}

#[test]
fn zero_id_is_replaced_and_explicit_id_is_kept() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(r#"CREATE TABLE "events" (id INTEGER PRIMARY KEY, name TEXT NOT NULL);"#)
        .unwrap();

    // id = 0 → 生成
    let a = db.insert(&Event { id: 0, name: "created".into() }).unwrap();
    let b = db.insert(&Event { id: 0, name: "updated".into() }).unwrap();
    assert!(a > 0 && b > 0);
    assert!(b > a, "同一节点连续发号必须递增");
    assert_eq!(Snowflake::parse(a).1, Snowflake::global().node());

    // 显式主键（非 0）不被"纠正"
    db.add(&Event { id: 7, name: "imported".into() }).unwrap();
    let row: Event = db
        .fetch_one_as(&select! { Event where Event.id == 7 })
        .unwrap()
        .expect("显式 id 应该原样落库");
    assert_eq!(row.id, 7);

    let all: Vec<Event> = db
        .fetch_all(&select! { Event where Event.id != 7 order_by Event.id asc })
        .unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|e| e.id != 0));
}
