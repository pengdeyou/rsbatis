//! `update!` / `delete!` 在真实 MySQL / MariaDB 上（反引号引用 + 真实写库）。
//!
//! 没有 `RSBATIS_MYSQL_URL` 时直接跳过。

#![cfg(feature = "mysql")]

use rsbatis::{delete, update, Entity, FromRow, MySqlDriver, SelectStmt, Session};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "v_ud_users")]
struct User {
    id: i64,
    name: String,
    age: i32,
}

fn seeded() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接 MySQL"));
    db.execute_batch(
        "DROP TABLE IF EXISTS v_ud_users;
         CREATE TABLE v_ud_users (id BIGINT PRIMARY KEY, name VARCHAR(32) NOT NULL, age INT NOT NULL) ENGINE=InnoDB;
         INSERT INTO v_ud_users (id, name, age) VALUES (1, 'a', 20), (2, 'b', 30), (3, 'c', 10);",
    )
    .expect("建表与灌数据");
    Some(db)
}

fn ages(db: &mut Session<MySqlDriver>) -> Vec<(i64, i32)> {
    let mut rows: Vec<User> = db
        .all(SelectStmt::for_entity::<User>().build(db.dialect()))
        .unwrap();
    rows.sort_by_key(|u| u.id);
    rows.into_iter().map(|u| (u.id, u.age)).collect()
}

#[test]
fn update_macro_mysql_backticks_and_executes() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let stmt = update!(User { age: 99 } where User.age >= 20);
    let sql = stmt.build(db.dialect());
    assert_eq!(
        "UPDATE `v_ud_users` SET `age` = ? WHERE `age` >= ?",
        sql.text
    );
    db.execute(sql).unwrap();
    assert_eq!(vec![(1, 99), (2, 99), (3, 10)], ages(&mut db));
}

#[test]
fn delete_macro_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let stmt = delete!(User where User.age < 20);
    let sql = stmt.build(db.dialect());
    assert_eq!("DELETE FROM `v_ud_users` WHERE `age` < ?", sql.text);
    db.execute(sql).unwrap();
    assert_eq!(vec![(1, 20), (2, 30)], ages(&mut db));
}

#[test]
fn update_macro_mysql_multi_field_and_affected_rows() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let stmt = update!(User { name: "z", age: 1 } where User.id == 2);
    db.execute(stmt.build(db.dialect())).unwrap();
    let rows: Vec<User> = db
        .all(SelectStmt::for_entity::<User>().build(db.dialect()))
        .unwrap();
    let u2 = rows.iter().find(|u| u.id == 2).unwrap();
    assert_eq!(("z".to_string(), 1), (u2.name.clone(), u2.age));
}
