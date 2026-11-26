//! 子查询示例：`IN` / `NOT IN` / `EXISTS`，对应 SQLAlchemy 的 `User.id.in_(select(...))` 与 `exists()`。
//!
//! SQLAlchemy 里写：
//! ```python
//! select(User).where(User.id.in_(select(Order.user_id).where(Order.amount >= 100)))
//! select(User).where(exists().where(Order.user_id == User.id))
//! ```
//! 这里对应：
//! ```ignore
//! SelectStmt::for_entity::<User>().where_(User::id().in_subquery(&sub))
//! SelectStmt::for_entity::<User>().where_(Expr::exists(&sub))
//! ```
//! 差别只在写法风格：Rust 没有 `==` 重载，所以比较用 `Expr::cmp` / `Column::eq`。

// 实体字段只用于声明「列」，业务代码不读；派生宏读字段名，clippy 看不到「读」。
#![allow(dead_code)]

use rsbatis::{BinOp, Column, Entity, Expr, FromRow, SelectStmt, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    name: String,
    age: i32,
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
    db.execute_batch(
        r#"
        CREATE TABLE "users" (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER NOT NULL);
        CREATE TABLE "orders" (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount INTEGER NOT NULL);
        INSERT INTO "users" (id, name, age) VALUES (1, '张三', 20), (2, '李四', 30), (3, '王五', 40);
        INSERT INTO "orders" (id, user_id, amount) VALUES (1, 1, 100), (2, 1, 200), (3, 3, 50);
        "#,
    )?;

    // ── 1. IN 子查询：买过 100 元以上东西的人 ──
    let big_buyers = SelectStmt::new("orders")
        .column("user_id")
        .where_(Column::<i64>::new("orders", "amount").ge(100_i64));

    let stmt = SelectStmt::for_entity::<User>().where_(User::id().in_subquery(&big_buyers));
    let sql = stmt.build(db.dialect());
    println!("IN 子查询 SQL:\n  {}\n", sql.text);
    let users: Vec<User> = db.fetch_all(&sql)?;
    println!(
        "  → 买过大额的: {:?}\n",
        users.iter().map(|u| &u.name).collect::<Vec<_>>()
    );

    // ── 2. NOT IN：没买过大额的（含从没下过单的人）──
    let stmt = SelectStmt::for_entity::<User>().where_(User::id().not_in_subquery(&big_buyers));
    let users: Vec<User> = db.fetch_all(stmt.build(db.dialect()))?;
    println!(
        "NOT IN 结果: {:?}\n",
        users.iter().map(|u| &u.name).collect::<Vec<_>>()
    );

    // ── 3. EXISTS 相关子查询：等价于"有订单的人" ──
    // 注意：子查询里引用外层表必须写全表名（users.id）。渲染层会强制带表名限定，
    // 否则 `id` 会被解析成 orders 自己的列——这是相关子查询最经典的静默错误。
    let exists_sub = SelectStmt::new("orders")
        .column_expr(Expr::lit(1_i64))
        .where_(Expr::cmp(
            BinOp::Eq,
            Expr::Col("orders", "user_id"),
            Expr::Col("users", "id"),
        ))
        .and_where(Column::<i64>::new("orders", "amount").ge(200_i64));

    let stmt = SelectStmt::for_entity::<User>()
        .where_(Column::<i32>::new("users", "age").ge(20_i32))
        .and_where(Expr::exists(&exists_sub));
    let sql = stmt.build(db.dialect());
    println!("EXISTS SQL:\n  {}\n", sql.text);
    let users: Vec<User> = db.fetch_all(&sql)?;
    println!(
        "  → 满足的: {:?}\n",
        users.iter().map(|u| &u.name).collect::<Vec<_>>()
    );

    // ── 4. 子查询里带 LIMIT：MySQL/MariaDB 会拒绝，我们提前告警 ──
    let limited = SelectStmt::new("orders").column("user_id").limit(1);
    let sql = SelectStmt::for_entity::<User>()
        .where_(User::id().in_subquery(&limited))
        .build(rsbatis::Dialect::MySql);
    if !sql.warnings.is_empty() {
        println!("MySQL 侧提示: {}", sql.warnings[0]);
    }

    Ok(())
}
