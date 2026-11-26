//! 联表与表别名的可编译示例（对应 README 的 "Joins and aliases" 一节）。
//!
//! 要点：没有 join 时投影仍是 `SELECT *`；一旦有 join，每列都投影成 `"u"."id" AS "u__id"`，
//! 元组 `(User, Order)` 的 FromRow 实现按前缀把列读回来。

// 示例里的实体字段只用于声明「列」，不会被业务代码读；派生宏读字段名，clippy 看不到「读」。
#![allow(dead_code)]

use rsbatis::{select, Driver, Entity, FromRow, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    name: String,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "orders")]
struct Order {
    id: i64,
    user_id: i64,
    amount: i64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Session::new(SqliteDriver::open_in_memory()?);
    db.driver_mut().execute_batch(
        r#"
        CREATE TABLE "users" (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
        CREATE TABLE "orders" (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount INTEGER NOT NULL);
        INSERT INTO "users" (id, name) VALUES (1, '张三'), (2, '李四');
        INSERT INTO "orders" (id, user_id, amount) VALUES (1, 1, 100), (2, 1, 300), (3, 2, 50);
        "#,
    )?;

    let rows: Vec<(User, Order)> = db.all(&select! {
        User as u join Order as o on o.user_id == u.id
        where u.id == 1 order_by o.amount desc
    })?;

    for (user, order) in &rows {
        println!("{} 的订单：{}", user.name, order.amount);
    }
    assert_eq!(rows.len(), 2);

    // LEFT JOIN：右表可能缺失，模型里的字段必须写成 Option 才接得住
    #[derive(Entity, FromRow, Clone, Debug)]
    #[rsbatis(table = "orders")]
    struct OrderOpt {
        id: Option<i64>,
        user_id: Option<i64>,
        amount: Option<i64>,
    }
    let left: Vec<(User, OrderOpt)> = db.all(&select! {
        User as u left_join OrderOpt as o on o.user_id == u.id
    })?;
    assert_eq!(left.len(), 3); // 2 + 1 重复父行：LEFT JOIN 不去重，去重是关系加载的职责

    Ok(())
}
