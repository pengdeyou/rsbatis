//! 别名联表的元组映射（回归测试）。
//!
//! 曾经的缺陷：元组 `(A, B)` 用 `Entity::TABLE` 当前缀，而查询一旦写了别名
//! （`User as u join Order as o`），投影前缀是 `u` / `o`，于是元组去找 `users__id`
//! 这种根本不存在的列，报错"结果集中没有列 users__id"——看上去像用户把查询写错了。
//!
//! 现在的判据：前缀**按投影顺序从结果集列名里推断**，别名与表名都能对上。

// 测试实体字段只用于声明「列」，不会被业务代码读。
#![allow(dead_code)]
#![cfg(feature = "sqlite")]

use rsbatis::{select, Driver, Entity, FromRow, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "t_user")]
struct User {
    id: i64,
    name: String,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "t_order")]
struct Order {
    id: i64,
    user_id: i64,
    amount: i64,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "t_item")]
struct Item {
    id: i64,
    order_id: i64,
}

fn db() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().expect("内存库"));
    db.driver_mut()
        .execute_batch(
            r#"
            CREATE TABLE "t_user" (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE "t_order" (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount INTEGER NOT NULL);
            CREATE TABLE "t_item" (id INTEGER PRIMARY KEY, order_id INTEGER NOT NULL);
            INSERT INTO "t_user" VALUES (1, '张三'), (2, '李四');
            INSERT INTO "t_order" VALUES (1, 1, 100), (2, 2, 50);
            INSERT INTO "t_item" VALUES (1, 1), (2, 1), (3, 2);
            "#,
        )
        .expect("建表");
    db
}

#[test]
fn aliased_two_table_tuple_maps_by_projection_order() {
    let mut db = db();
    let rows: Vec<(User, Order)> = db
        .fetch_all(&select! {
            User as u join Order as o on o.user_id == u.id
            order_by o.id asc
        })
        .expect("别名联表");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0.name, "张三");
    assert_eq!(rows[0].1.amount, 100);
    assert_eq!(rows[1].0.name, "李四");
}

#[test]
fn aliased_three_table_tuple_maps_by_projection_order() {
    let mut db = db();
    let rows: Vec<(User, Order, Item)> = db
        .fetch_all(&select! {
            User as u join Order as o on o.user_id == u.id
            join Item as i on i.order_id == o.id
            order_by i.id asc
        })
        .expect("三表别名联表");
    assert_eq!(rows.len(), 3);
    // 订单 1 有两件货，两个元组共享同一个订单对象
    assert_eq!(rows[0].1.id, 1);
    assert_eq!(rows[1].1.id, 1);
    assert_eq!(rows[2].0.name, "李四");
}

#[test]
fn mixed_alias_and_plain_table_still_maps() {
    // 主表用别名、join 表不用：前缀序列是 ["u", "t_order"]
    let mut db = db();
    let rows: Vec<(User, Order)> = db
        .fetch_all(&select! {
            User as u join Order on Order.user_id == u.id
            order_by Order.id asc
        })
        .expect("混合别名");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0.name, "张三");
}

#[test]
fn no_alias_join_still_works() {
    // 全部不用别名：前缀就是表名，与修复前的行为一致（不能因为改推断就破坏旧路径）
    let mut db = db();
    let rows: Vec<(User, Order)> = db
        .fetch_all(&select! {
            User join Order on Order.user_id == User.id
            order_by Order.id asc
        })
        .expect("无别名联表");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0.name, "张三");
}
