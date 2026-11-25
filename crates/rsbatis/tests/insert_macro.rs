//! `insert!` 字面量式插入（SQLite）。
//!
//! 重点不是"能插入"，而是三件事：
//! - 列名走 `col_<field>()`，所以 `#[rsbatis(column = "...")]` 的重命名一定生效；
//! - 不写的字段就是不写该列（数据库默认值生效）；
//! - 值位置接受任意表达式（算术、函数调用），不是只能塞字面量。

#![cfg(feature = "sqlite")]
#![allow(dead_code)]

use rsbatis::{insert, select, Entity, FromRow, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "im_users")]
struct User {
    id: Option<i64>,
    name: String,
    #[rsbatis(column = "age_years")]
    age: Option<i32>,
}

fn db() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().expect("内存库"));
    db.execute_batch(
        r#"CREATE TABLE "im_users" (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             age_years INTEGER DEFAULT 21
           );"#,
    )
    .expect("建表");
    db
}

#[test]
fn insert_macro_writes_renamed_columns() {
    let mut db = db();
    db.execute(insert! { User { name: "张三", age: 20 } })
        .expect("插入");

    let rows: Vec<User> = db.fetch_all(&select! { User }).expect("回读");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "张三");
    assert_eq!(rows[0].age, Some(20), "age_years 列被写入");
    assert_eq!(rows[0].id, Some(1), "id 没写 → AUTOINCREMENT 生效");
}

#[test]
fn omitted_field_lets_database_default_win() {
    let mut db = db();
    db.execute(insert! { User { name: "李四" } }).expect("插入");
    let rows: Vec<User> = db.fetch_all(&select! { User }).expect("回读");
    assert_eq!(rows[0].age, Some(21), "没写 age → DEFAULT 21");
}

#[test]
fn field_values_accept_expressions() {
    let mut db = db();
    let base = 18;
    db.execute(insert! { User { name: "王五", age: base + 2 } })
        .expect("插入");
    let rows: Vec<User> = db.fetch_all(&select! { User }).expect("回读");
    assert_eq!(rows[0].age, Some(20));
}

#[test]
fn insert_macro_and_object_add_produce_same_row() {
    // 两条写入路径（宏 / 对象）必须产出一致的行，否则用户会在两者之间看到差异
    let mut db = db();
    db.execute(insert! { User { name: "宏", age: 30 } })
        .expect("宏插入");
    db.add(&User {
        id: None,
        name: "对象".into(),
        age: Some(30),
    })
    .expect("对象插入");

    let rows: Vec<User> = db
        .fetch_all(&select! { User order_by User.id asc })
        .expect("回读");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].age, rows[1].age);
    assert_eq!(rows[0].id, Some(1));
    assert_eq!(rows[1].id, Some(2));
}
