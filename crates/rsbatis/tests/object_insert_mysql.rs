//! 对象式插入在真实 MySQL / MariaDB 上：`AUTO_INCREMENT`、列重命名、
//! `None` 跳过列（`DEFAULT` 生效）、事务内批量。

#![cfg(feature = "mysql")]
#![allow(dead_code)]

use rsbatis::{select, Entity, FromRow, MySqlDriver, Session};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "v_oi_users")]
struct User {
    id: Option<i64>,
    name: String,
    #[rsbatis(column = "age_years")]
    age: Option<i32>,
}

fn seeded() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接"));
    db.execute_batch(
        "DROP TABLE IF EXISTS v_oi_users;
         CREATE TABLE v_oi_users (
             id BIGINT PRIMARY KEY AUTO_INCREMENT,
             name VARCHAR(32) NOT NULL,
             age_years INT DEFAULT 21
         ) ENGINE=InnoDB;",
    )
    .expect("建表");
    Some(db)
}

#[test]
fn mysql_add_and_read_back() {
    let Some(mut db) = seeded() else { return };
    db.add(&User {
        id: None,
        name: "张三".into(),
        age: Some(20),
    })
    .expect("插入");

    let rows: Vec<User> = db.fetch_all(&select! { User }).expect("回读");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, Some(1));
    assert_eq!(rows[0].age, Some(20), "重命名列 age_years 写入成功");
}

#[test]
fn mysql_none_skips_column_and_default_applies() {
    let Some(mut db) = seeded() else { return };
    db.add(&User {
        id: None,
        name: "李四".into(),
        age: None,
    })
    .expect("插入");
    let rows: Vec<User> = db.fetch_all(&select! { User }).expect("回读");
    assert_eq!(rows[0].age, Some(21), "None 应跳过该列，让 DEFAULT 21 生效");
}

#[test]
fn mysql_add_all_inside_transaction() {
    let Some(mut db) = seeded() else { return };
    {
        let mut tx = db.transaction().expect("开事务");
        tx.add(&User {
            id: None,
            name: "A".into(),
            age: Some(1),
        })
        .expect("插入 A");
        tx.add(&User {
            id: None,
            name: "B".into(),
            age: None,
        })
        .expect("插入 B");
        tx.commit().expect("提交");
    }
    let rows: Vec<User> = db
        .fetch_all(&select! { User order_by User.id asc })
        .expect("回读");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].age, Some(21));
}
