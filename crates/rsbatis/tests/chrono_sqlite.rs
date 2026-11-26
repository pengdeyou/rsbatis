//! 日期时间：写入 / 回读 / 比较 / 容错（SQLite，无外部依赖）。
#![cfg(all(feature = "sqlite", feature = "chrono"))]
// 测试里的实体字段只用来声明「列」，不会被业务代码读取；
// 派生宏拿的是字段名，clippy 看不到「读」，因此在这里关掉 dead_code。
#![allow(dead_code)]

use chrono::{NaiveDate, NaiveDateTime, Timelike};
use rsbatis::{insert, select, Entity, ErrorKind, FromRow, Session, Sql, SqliteDriver};

#[derive(Entity, FromRow)]
#[rsbatis(table = "events")]
struct Event {
    id: i64,
    name: String,
    created: chrono::NaiveDateTime,
}

fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, s)
        .unwrap()
}

#[test]
fn roundtrip_keeps_microseconds_and_compares_as_time() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        r#"CREATE TABLE "events" (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, created TEXT NOT NULL)"#,
    )
    .unwrap();

    // 精度上限是微秒（MySQL 的 Date 也只有微秒），纳秒部分落库时被截断
    let early = dt(2026, 1, 5, 9, 30, 0).with_nanosecond(123_456).unwrap();
    let early_stored = dt(2026, 1, 5, 9, 30, 0).with_nanosecond(123_000).unwrap();
    let late = dt(2026, 1, 5, 18, 0, 0);

    db.execute(
        insert::<Event>()
            .set(Event::name(), "早")
            .set(Event::created(), early),
    )
    .unwrap();
    db.execute(
        insert::<Event>()
            .set(Event::name(), "晚")
            .set(Event::created(), late),
    )
    .unwrap();

    // 比较走字符串比较：格式定长到微秒，字典序 == 时间序
    let after_noon: Vec<Event> = db
        .all(select! { Event where Event.created >= late })
        .unwrap();
    assert_eq!(1, after_noon.len());
    assert_eq!("晚", after_noon[0].name);

    // 微秒必须原样回来
    let all: Vec<Event> = db
        .all(select! { Event order_by Event.created asc })
        .unwrap();
    assert_eq!(2, all.len());
    assert_eq!(early_stored, all[0].created, "纳秒被截到微秒，不应丢整秒");
    assert_eq!(late, all[1].created);
    assert_eq!(123_000, all[0].created.nanosecond(), "截断到微秒而不是纳秒");

    // 更新一个日期字段
    db.execute(
        rsbatis::update::<Event>()
            .set(Event::created(), late)
            .filter(Event::name().eq("早")),
    )
    .unwrap();
    let all2: Vec<Event> = db
        .all(select! { Event order_by Event.id asc })
        .unwrap();
    assert_eq!(late, all2[0].created);
}

#[test]
fn unix_seconds_are_accepted_but_loose_text_is_not() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(r#"CREATE TABLE "t" (a INTEGER, b TEXT)"#)
        .unwrap();
    db.execute_batch(r#"INSERT INTO "t" VALUES (1700000000, '2026/1/5')"#)
        .unwrap();

    let rows = db.fetch(Sql::raw("SELECT a, b FROM t")).unwrap();

    // unix 秒：当作 UTC 解释（SQLite 没有时区概念，我们不自作主张换算）
    let from_secs: NaiveDateTime = rows[0].get_by_name("a").unwrap();
    assert_eq!(dt(2023, 11, 14, 22, 13, 20), from_secs);

    // 宽松格式（2026/1/5）必须报错，而不是"尽力猜测"
    match rows[0].get_by_name::<NaiveDateTime>("b") {
        Ok(v) => panic!("不该解析出 {v}"),
        Err(e) => assert_eq!(ErrorKind::TypeMismatch, e.kind),
    }
}

#[test]
fn date_only_column_maps_to_midnight() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(r#"CREATE TABLE "d" (day TEXT)"#).unwrap();
    db.execute_batch(r#"INSERT INTO "d" VALUES ('2026-01-05')"#)
        .unwrap();
    let rows = db.fetch(Sql::raw("SELECT day FROM d")).unwrap();
    let v: NaiveDateTime = rows[0].get_by_name("day").unwrap();
    assert_eq!(dt(2026, 1, 5, 0, 0, 0), v);
}

#[test]
fn entity_meta_records_datetime() {
    assert_eq!(vec!["id", "name", "created"], <Event as Entity>::COLUMNS);
    assert!(<Event as Entity>::NULLABLE.is_empty(), "三个字段都不可空");
}
