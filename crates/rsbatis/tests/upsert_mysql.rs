//! upsert 在真实 MySQL / MariaDB 上：`ON DUPLICATE KEY UPDATE` 的语义验证。
//!
//! MySQL 与 SQLite 的差异点都集中在这几条：
//! - 冲突目标由主键/唯一键自动判定，`VALUES(col)` 而不是 `excluded.col`；
//! - `ON DUPLICATE KEY UPDATE c = c`（do nothing）不会吞掉其他错误；
//! - 冲突被更新时受影响行数是 **2**（MySQL 的约定），不是 1——测试要显式承认这点，
//!   否则"受影响行数"这个返回值会被下游误读成"插入了 2 行"。

#![cfg(feature = "mysql")]
#![allow(dead_code)]

use rsbatis::{Entity, FromRow, MySqlDriver, Session};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "v_ups_users")]
struct User {
    #[rsbatis(primary_key)]
    id: Option<i64>,
    name: String,
    age: Option<i32>,
}

fn session() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接 MySQL"));
    db.execute_batch(
        "DROP TABLE IF EXISTS v_ups_users;
         CREATE TABLE v_ups_users (
             id   BIGINT PRIMARY KEY AUTO_INCREMENT,
             name VARCHAR(32) NOT NULL,
             age  INT DEFAULT 21
         ) ENGINE=InnoDB;",
    )
    .expect("建表");
    Some(db)
}

fn load(db: &mut Session<MySqlDriver>) -> Vec<User> {
    db.fetch_all(rsbatis::select! { User order_by User.id asc })
        .expect("回读")
}

#[test]
fn insert_new_row_uses_default() {
    let Some(mut db) = session() else { return };
    db.save(&User {
        id: None,
        name: "张三".into(),
        age: None,
    })
    .expect("插入");

    let rows = load(&mut db);
    assert_eq!(1, rows.len());
    assert!(rows[0].id.is_some(), "自增主键应回填");
    assert_eq!(Some(21), rows[0].age, "DEFAULT 21 必须生效");
}

#[test]
fn conflict_updates_only_provided_columns() {
    let Some(mut db) = session() else { return };
    db.save(&User {
        id: Some(7),
        name: "原名".into(),
        age: Some(30),
    })
    .expect("首次插入");
    db.save(&User {
        id: Some(7),
        name: "改名".into(),
        age: None,
    })
    .expect("冲突更新");

    let rows = load(&mut db);
    assert_eq!(1, rows.len(), "upsert 不能变成两行");
    assert_eq!("改名", rows[0].name);
    assert_eq!(Some(30), rows[0].age, "None 的列不应被 VALUES 覆盖成 NULL");
}

#[test]
fn conflict_affected_rows_is_two_in_mysql() {
    // MySQL 的约定：插入 1 行=1，更新 1 行=2，数据和原来完全一样=0。
    // 这个返回值语义与 SQLite/PG 不同，必须写进测试，否则调用方会误判。
    let Some(mut db) = session() else { return };
    let first = db
        .save(&User {
            id: Some(11),
            name: "A".into(),
            age: Some(1),
        })
        .expect("首次插入");
    let second = db
        .save(&User {
            id: Some(11),
            name: "B".into(),
            age: Some(2),
        })
        .expect("冲突更新");
    assert_eq!(1, first);
    assert_eq!(2, second, "MySQL 更新已存在的行返回 2");
}

#[test]
fn do_nothing_mysql_is_real_no_op_and_keeps_data() {
    let Some(mut db) = session() else { return };
    db.execute(
        rsbatis::insert::<User>()
            .set(User::id(), 5_i64)
            .set(User::name(), "原值")
            .set(User::age(), 9_i32),
    )
    .expect("首次插入");

    // ON DUPLICATE KEY UPDATE `id` = `id`：冲突时什么都不做
    db.execute(
        rsbatis::insert::<User>()
            .set(User::id(), 5_i64)
            .set(User::name(), "被覆盖了就会有罪")
            .on_conflict_do_nothing(),
    )
    .expect("do nothing 不应抛唯一键冲突");

    let rows = load(&mut db);
    assert_eq!(1, rows.len());
    assert_eq!("原值", rows[0].name);
}
