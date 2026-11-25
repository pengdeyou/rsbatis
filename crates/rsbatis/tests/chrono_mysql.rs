//! 日期时间在真实 MySQL 上：DATETIME(6) 往返、跨类型比较、DATE 列映射。
#![cfg(all(feature = "mysql", feature = "chrono"))]
// 测试里的实体字段只用来声明「列」，不会被业务代码读取；
// 派生宏拿的是字段名，clippy 看不到「读」，因此在这里关掉 dead_code。
#![allow(dead_code)]

use chrono::{NaiveDate, NaiveDateTime, Timelike};
use rsbatis::{insert, select, Entity, FromRow, MySqlDriver, Session, Sql};

fn url() -> Option<String> {
    std::env::var("RSBATIS_MYSQL_URL").ok()
}

#[derive(Entity, FromRow)]
#[rsbatis(table = "v_events")]
struct Event {
    id: i64,
    name: String,
    created: chrono::NaiveDateTime,
}

fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32, us: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_micro_opt(h, mi, s, us)
        .unwrap()
}

#[test]
fn datetime_roundtrip_on_real_mysql() {
    let Some(url) = url() else {
        eprintln!("跳过：未设置 RSBATIS_MYSQL_URL");
        return;
    };
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接失败"));
    db.execute_batch(
        "DROP TABLE IF EXISTS v_events;
         CREATE TABLE v_events (
           id BIGINT AUTO_INCREMENT PRIMARY KEY,
           name VARCHAR(32) NOT NULL,
           created DATETIME(6) NOT NULL
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .unwrap();

    let early = dt(2026, 1, 5, 9, 30, 0, 123_456);
    let late = dt(2026, 1, 5, 18, 0, 0, 0);

    db.execute(
        insert::<Event>()
            .set(Event::col_name(), "早")
            .set(Event::col_created(), early),
    )
    .unwrap();
    db.execute(
        insert::<Event>()
            .set(Event::col_name(), "晚")
            .set(Event::col_created(), late),
    )
    .unwrap();

    // MySQL 服务端按 DATETIME 语义比较，而不是字符串比较
    let after_noon: Vec<Event> = db
        .fetch_all(select! { Event where Event.created >= late })
        .unwrap();
    assert_eq!(1, after_noon.len());
    assert_eq!("晚", after_noon[0].name);

    let all: Vec<Event> = db
        .fetch_all(select! { Event order_by Event.created asc })
        .unwrap();
    assert_eq!(2, all.len());
    assert_eq!(early, all[0].created, "微秒必须无损往返");
    assert_eq!(
        123_456,
        all[0].created.nanosecond() / 1000,
        "微秒部分应原样保留"
    );
    assert_eq!(late, all[1].created);

    // DATE 列只有日期，映射成当日 0 点
    db.execute_batch(
        "DROP TABLE IF EXISTS v_days;
         CREATE TABLE v_days (day DATE NOT NULL);
         INSERT INTO v_days VALUES ('2026-01-05')",
    )
    .unwrap();
    let rows = db.fetch(Sql::raw("SELECT day FROM v_days")).unwrap();
    let day: NaiveDateTime = rows[0].get_by_name("day").unwrap();
    assert_eq!(dt(2026, 1, 5, 0, 0, 0, 0), day);
}
