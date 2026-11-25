//! upsert 端到端（SQLite 内存库）：新行、冲突覆盖、`None` 不冲掉已有值、退化路径。
//!
//! 重点不是"能跑通"，而是三条容易被写错的语义：
//! 1. 主键 `None` 时是插入新行，不触发冲突；
//! 2. 冲突时 `None` 字段**不参与 SET**，所以不会把已有值冲成 NULL；
//! 3. 只有主键值、没有其他列时，不能渲染出空的 `SET` 子句。

#![cfg(feature = "sqlite")]
#![allow(dead_code)]

use rsbatis::{Entity, FromRow, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug, PartialEq)]
#[rsbatis(table = "ups_users")]
struct User {
    #[rsbatis(primary_key)]
    id: Option<i64>,
    name: String,
    age: Option<i32>,
}

/// 除主键外全可空：用来测"只有主键值"的退化路径。
#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "ups_pk_users")]
struct PkOnly {
    #[rsbatis(primary_key)]
    id: Option<i64>,
    name: Option<String>,
    age: Option<i32>,
}

/// 没有声明主键：`save` 必须拒绝，而不是猜一列。
#[derive(Entity, FromRow)]
#[rsbatis(table = "ups_users")]
struct NoPk {
    id: Option<i64>,
    name: String,
}

fn session() -> Session<SqliteDriver> {
    let mut s = Session::new(SqliteDriver::open_in_memory().expect("内存库"));
    s.execute_batch(
        "CREATE TABLE ups_users (
             id   INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             age  INTEGER DEFAULT 21
         );
         CREATE TABLE ups_pk_users (
             id   INTEGER PRIMARY KEY,
             name TEXT,
             age  INTEGER
         );",
    )
    .expect("建表");
    s
}

fn load(s: &mut Session<SqliteDriver>) -> Vec<User> {
    s.fetch_all(rsbatis::select! { User order_by User.id asc })
        .expect("回读")
}

#[test]
fn save_without_pk_inserts_and_uses_default_value() {
    let mut s = session();
    s.save(&User {
        id: None,
        name: "张三".into(),
        age: None,
    })
    .expect("插入新行");

    let rows = load(&mut s);
    assert_eq!(1, rows.len());
    assert_eq!(Some(1), rows[0].id, "AUTOINCREMENT 应该生成主键");
    assert_eq!(
        Some(21),
        rows[0].age,
        "None 必须走数据库默认值而不是写 NULL"
    );
}

#[test]
fn save_with_existing_pk_overwrites_given_columns_only() {
    let mut s = session();
    s.save(&User {
        id: Some(7),
        name: "原名".into(),
        age: Some(30),
    })
    .expect("首次插入");
    // 同一个主键、只给 name：age 必须保持 30
    s.save(&User {
        id: Some(7),
        name: "改名".into(),
        age: None,
    })
    .expect("冲突更新");

    let rows = load(&mut s);
    assert_eq!(1, rows.len(), "upsert 不能变成两行");
    assert_eq!("改名", rows[0].name);
    assert_eq!(Some(30), rows[0].age, "None 的列不应被 SET 冲成 NULL");
}

#[test]
fn save_with_existing_pk_and_all_columns_updates_everything() {
    let mut s = session();
    s.save(&User {
        id: Some(9),
        name: "A".into(),
        age: Some(18),
    })
    .expect("首次插入");
    s.save(&User {
        id: Some(9),
        name: "B".into(),
        age: Some(40),
    })
    .expect("冲突更新");

    let rows = load(&mut s);
    assert_eq!(1, rows.len());
    assert_eq!("B", rows[0].name);
    assert_eq!(Some(40), rows[0].age);
}

#[test]
fn save_with_only_pk_does_nothing_on_conflict() {
    let mut s = session();
    s.save(&PkOnly {
        id: Some(3),
        name: Some("原值".into()),
        age: Some(1),
    })
    .expect("首次插入");
    // 只给主键：没有可 SET 的列，必须退化成"什么都不做"而不是发空 SET
    s.save(&PkOnly {
        id: Some(3),
        name: None,
        age: None,
    })
    .expect("只给主键的 save 不应报错");

    let rows: Vec<PkOnly> = s.fetch_all(rsbatis::select! { PkOnly }).expect("回读");
    assert_eq!(1, rows.len());
    assert_eq!(
        Some("原值".to_string()),
        rows[0].name,
        "空 SET 的退化路径不能改动数据"
    );
}

#[test]
fn save_without_primary_key_declaration_is_rejected() {
    let mut s = session();
    let err = s
        .save(&NoPk {
            id: Some(1),
            name: "x".into(),
        })
        .expect_err("没有主键声明必须拒绝");
    assert!(
        err.message.contains("primary_key"),
        "错误消息要直接告诉用户怎么修：{}",
        err.message
    );
}

#[test]
fn save_returns_rows_affected() {
    let mut s = session();
    let n = s
        .save(&User {
            id: None,
            name: "计数".into(),
            age: None,
        })
        .expect("插入");
    assert_eq!(1, n, "单行 insert 的受影响行数应为 1");
}
