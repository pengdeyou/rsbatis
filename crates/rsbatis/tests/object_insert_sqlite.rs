//! 对象式插入（`Session::add` / `add_all`）在 SQLite 上。
//!
//! 要验证的核心不是"能插入"，而是两条容易被实现悄悄做错的语义：
//! - `Option::None` 字段**不进 INSERT**（列用数据库默认值），而不是写一个 NULL；
//! - `#[rsbatis(column = "...")]` 的重命名对 `add` 同样生效（列名来自 derive）。

#![cfg(feature = "sqlite")]
#![allow(dead_code)]

use rsbatis::{select, Entity, FromRow, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "oi_users")]
struct User {
    id: Option<i64>, // None → 交给 AUTOINCREMENT
    name: String,
    #[rsbatis(column = "age_years")]
    age: Option<i32>, // None → 该列不写，用表默认值 21
}

/// 手写 Entity 且不实现 `column_values`：`add` 应当拒绝，而不是插入一行空值。
struct Empty;

impl Entity for Empty {
    const TABLE: &'static str = "oi_users";
    const COLUMNS: &'static [&'static str] = &["name"];
}

fn db() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().expect("内存库"));
    db.execute_batch(
        r#"CREATE TABLE "oi_users" (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             age_years INTEGER DEFAULT 21
           );"#,
    )
    .expect("建表");
    db
}

#[test]
fn add_uses_renamed_column_and_autoincrement() {
    let mut db = db();
    let n = db
        .add(&User {
            id: None,
            name: "张三".into(),
            age: Some(20),
        })
        .expect("插入");
    assert_eq!(n, 1);

    let rows: Vec<User> = db.fetch_all(&select! { User }).expect("回读");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, Some(1), "id 由 AUTOINCREMENT 生成");
    assert_eq!(rows[0].age, Some(20), "重命名列 age_years 被正确写入");
}

#[test]
fn none_means_column_omitted_not_null() {
    let mut db = db();
    db.add(&User {
        id: None,
        name: "李四".into(),
        age: None,
    })
    .expect("插入");

    let rows: Vec<User> = db.fetch_all(&select! { User }).expect("回读");
    // 表定义里 age_years DEFAULT 21：读回 21 说明这一列压根没出现在 INSERT 里。
    // 如果实现把 None 写成 NULL，这里会是 None。
    assert_eq!(
        rows[0].age,
        Some(21),
        "None 应当跳过该列，让数据库默认值生效"
    );
}

#[test]
fn add_all_inserts_every_model() {
    let mut db = db();
    let models = vec![
        User {
            id: None,
            name: "A".into(),
            age: Some(1),
        },
        User {
            id: None,
            name: "B".into(),
            age: Some(2),
        },
        User {
            id: None,
            name: "C".into(),
            age: None,
        },
    ];
    let n = db.add_all(&models).expect("批量插入");
    assert_eq!(n, 3);

    let rows: Vec<User> = db
        .fetch_all(&select! { User order_by User.id asc })
        .expect("回读");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2].age, Some(21));
}

#[test]
fn add_on_entity_without_column_values_is_an_error() {
    let mut db = db();
    let err = db.add(&Empty).expect_err("没有可写列时应当报错");
    assert!(
        err.to_string().contains("没有可写的列"),
        "错误信息要说清原因，实际：{err}"
    );
}

#[test]
fn add_explicit_null_still_possible_via_builder() {
    // add 的 None 语义是"跳过列"；真要写 NULL 走显式 builder，两条路不能混
    let mut db = db();
    db.execute(
        rsbatis::insert::<User>()
            .set(User::col_name(), "王五")
            .set_null(User::col_age())
            .build(rsbatis::Dialect::Sqlite),
    )
    .expect("显式 NULL");

    let rows: Vec<User> = db.fetch_all(&select! { User }).expect("回读");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].age, None, "set_null 写的是真正的 NULL");
}
