//! `update!` / `delete!` 宏的端到端（SQLite）：SQL 文本 + 真实执行结果。
//!
//! 这两个宏的价值是"字段名写错就编译失败"，所以除了正确路径，还要有 trybuild
//! 诊断用例（见 `tests/ui/update_bad_field.rs`）。

#![cfg(feature = "sqlite")]

use rsbatis::{delete, update, Entity, FromRow, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "ud_users")]
struct User {
    id: i64,
    name: String,
    age: i32,
}

fn setup() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        r#"
        DROP TABLE IF EXISTS ud_users;
        CREATE TABLE ud_users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER NOT NULL);
        INSERT INTO ud_users (id, name, age) VALUES (1, 'a', 20), (2, 'b', 30), (3, 'c', 10);
        "#,
    )
    .unwrap();
    db
}

fn ages(db: &mut Session<SqliteDriver>) -> Vec<(i64, i32)> {
    let mut rows: Vec<User> = db
        .fetch_all(rsbatis::SelectStmt::for_entity::<User>().build(db.dialect()))
        .unwrap();
    rows.sort_by_key(|u| u.id);
    rows.into_iter().map(|u| (u.id, u.age)).collect()
}

#[test]
fn update_macro_sets_fields_with_where() {
    let mut db = setup();
    let stmt = update!(User { age: 99 } where User.age >= 20);
    let sql = stmt.build(db.dialect());
    assert_eq!(
        "UPDATE \"ud_users\" SET \"age\" = ? WHERE \"age\" >= ?",
        sql.text
    );
    db.execute(sql).unwrap();
    assert_eq!(vec![(1, 99), (2, 99), (3, 10)], ages(&mut db));
}

#[test]
fn update_macro_multiple_fields() {
    let mut db = setup();
    let stmt = update!(User { name: "x", age: 7 } where User.id == 1);
    let sql = stmt.build(db.dialect());
    assert!(
        sql.text.contains("SET \"name\" = ?") && sql.text.contains("\"age\" = ?"),
        "got: {}",
        sql.text
    );
    db.execute(sql).unwrap();
    let rows: Vec<User> = db
        .fetch_all(rsbatis::SelectStmt::for_entity::<User>().build(db.dialect()))
        .unwrap();
    let u1 = rows.iter().find(|u| u.id == 1).unwrap();
    assert_eq!(("x".to_string(), 7), (u1.name.clone(), u1.age));
}

#[test]
fn update_macro_without_where_updates_all() {
    let mut db = setup();
    let stmt = update!(User { age: 0 });
    db.execute(stmt.build(db.dialect())).unwrap();
    assert_eq!(vec![(1, 0), (2, 0), (3, 0)], ages(&mut db));
}

#[test]
fn delete_macro_with_where() {
    let mut db = setup();
    let stmt = delete!(User where User.age < 20);
    let sql = stmt.build(db.dialect());
    assert_eq!("DELETE FROM \"ud_users\" WHERE \"age\" < ?", sql.text);
    db.execute(sql).unwrap();
    assert_eq!(vec![(1, 20), (2, 30)], ages(&mut db));
}

#[test]
fn delete_macro_without_where_removes_all() {
    let mut db = setup();
    let stmt = delete!(User);
    db.execute(stmt.build(db.dialect())).unwrap();
    assert!(ages(&mut db).is_empty());
}

#[test]
fn update_macro_accepts_expression_values() {
    let mut db = setup();
    // 值可以是任意表达式（这里用局部变量），类型仍由 IntoValue<T> 校验
    let bump = 5_i32;
    // 值可以是任意 Rust 表达式，在调用点求值后再绑定
    let stmt = update!(User { age: bump + 5 } where User.id == 3);
    db.execute(stmt.build(db.dialect())).unwrap();
    assert_eq!(vec![(1, 20), (2, 30), (3, 10)], ages(&mut db));
}
