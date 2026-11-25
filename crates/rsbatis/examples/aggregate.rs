//! 分组与聚合示例：`GROUP BY` / `HAVING` / `COUNT` / `SUM`。
//!
//! 对应 SQLAlchemy 的 `.group_by(...)` / `.having(func.count(...) > 2)`。
//! 注意两个方言层面的真实差异：
//! - MySQL 的 `SUM(BIGINT)` 返回 DECIMAL（字符串），要读整数得 `CAST(... AS INTEGER)`
//!   （`INTEGER` 在 MySQL 侧会被映射成 `SIGNED`）；
//! - `COUNT(*)` 与 `COUNT(col)` 不同：后者不数 NULL。

#![allow(dead_code)]

use rsbatis::{BinOp, Entity, Expr, FromRow, SelectStmt, Session, SortOrder, SqliteDriver};

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
    db.execute_batch(
        r#"
        CREATE TABLE "users" (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
        CREATE TABLE "orders" (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount INTEGER NOT NULL);
        INSERT INTO "users" (id, name) VALUES (1, '张三'), (2, '李四'), (3, '王五');
        INSERT INTO "orders" (id, user_id, amount) VALUES (1, 1, 100), (2, 1, 200), (3, 2, 50);
        "#,
    )?;

    // ── 1. 每个用户的订单数与总额 ──
    let stmt = SelectStmt::new("orders")
        .column("user_id")
        .column_expr(Expr::count_star().as_("cnt"))
        .column_expr(Expr::sum(Expr::Col("orders", "amount")).as_("total"))
        .group_by(Expr::Col("orders", "user_id"))
        .order_by(Expr::Col("orders", "user_id"), SortOrder::Asc);

    let sql = stmt.build(db.dialect());
    println!("分组统计 SQL:\n  {}\n", sql.text);
    let rows: Vec<UserStats> = db.fetch_all(sql)?;
    println!("  → 每组: {:?}\n", rows);

    // ── 2. HAVING：只要下单 >= 2 次的用户 ──
    let stmt = SelectStmt::new("orders")
        .column("user_id")
        .column_expr(Expr::count_star().as_("cnt"))
        .group_by(Expr::Col("orders", "user_id"))
        .having(Expr::cmp(BinOp::Gt, Expr::count_star(), Expr::lit(1_i64)));

    let sql = stmt.build(db.dialect());
    println!("带 HAVING 的 SQL:\n  {}\n", sql.text);
    let rows: Vec<UserCount> = db.fetch_all(sql)?;
    println!("  → 下单 >= 2: {:?}\n", rows);

    // ── 3. 联表分组：按用户名统计（LEFT JOIN 让没下单的人也出现）──
    let stmt = SelectStmt::new("users")
        .column("name")
        .column_expr(Expr::count(Expr::Col("orders", "id")).as_("orders"))
        .join_table(
            rsbatis::JoinKind::Left,
            "orders",
            Expr::cmp(
                BinOp::Eq,
                Expr::Col("users", "id"),
                Expr::Col("orders", "user_id"),
            ),
        )
        .group_by(Expr::Col("users", "name"))
        .order_by(Expr::Col("users", "name"), SortOrder::Asc);

    let sql = stmt.build(db.dialect());
    println!("联表分组 SQL:\n  {}\n", sql.text);
    let rows: Vec<NameOrders> = db.fetch_all(sql)?;
    println!(
        "  → 每人订单数（COUNT(orders.id) 不数 NULL，所以没下单的是 0）: {:?}\n",
        rows.iter()
            .map(|r| (r.name.clone(), r.orders))
            .collect::<Vec<_>>()
    );

    // ── 4. 分页总数 = 组数，不是行数 ──
    let count_sql = SelectStmt::new("orders")
        .column("user_id")
        .group_by(Expr::Col("orders", "user_id"))
        .build_count(db.dialect());
    println!("分组查询的 COUNT（走子查询）:\n  {}", count_sql.text);

    Ok(())
}

#[derive(FromRow, Debug)]
struct UserStats {
    user_id: i64,
    cnt: i64,
    total: i64,
}

#[derive(FromRow, Debug)]
struct UserCount {
    user_id: i64,
    cnt: i64,
}

#[derive(FromRow, Debug)]
struct NameOrders {
    name: String,
    orders: i64,
}
