//! 派生表与 CTE 示例：`FROM (子查询) AS t` 与 `WITH t AS (子查询)`。
//!
//! 对应 SQLAlchemy 的 `.subquery()`（用作 FROM/join 的来源）与 `.cte("t")`。
//! 两者共用同一套"子查询装配"基础设施：共享参数缓冲、强制表名限定。

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

    // ── 1. 派生表：先筛一遍，外层再筛一遍 ──
    let inner = SelectStmt::new("users")
        .column("id")
        .column("name")
        .column("age")
        .where_(Column::<i32>::new("users", "age").ge(20_i32));

    let outer = SelectStmt::from_subquery(&inner, "t")
        .column("name")
        .where_(Expr::cmp(
            BinOp::Ge,
            Expr::Col("t", "age"),
            Expr::lit(30_i32),
        ));

    let sql = outer.build(db.dialect());
    println!("派生表 SQL:\n  {}\n", sql.text);
    let rows = db.fetch_all::<NameOnly>(&sql)?;
    println!(
        "  → 30 岁以上且在派生表里的人: {:?}\n",
        rows.into_iter().map(|r| r.name).collect::<Vec<_>>()
    );

    // ── 2. CTE：给子查询起名，再在 FROM 里引用 ──
    let cte_stmt = SelectStmt::new("t").cte("t", &inner).where_(Expr::cmp(
        BinOp::Ge,
        Expr::Col("t", "age"),
        Expr::lit(40_i32),
    ));
    let sql = cte_stmt.build(db.dialect());
    println!("CTE SQL:\n  {}\n", sql.text);
    let rows = db.fetch_all::<NameOnly>(&sql)?;
    println!(
        "  → 40 岁以上: {:?}\n",
        rows.into_iter().map(|r| r.name).collect::<Vec<_>>()
    );

    // ── 3. JOIN 派生表：子查询算一次，按别名连回来 ──
    let big_orders = SelectStmt::new("orders")
        .column("user_id")
        .column("amount")
        .where_(Column::<i64>::new("orders", "amount").ge(150_i64));

    let joined = SelectStmt::new("users").column("name").join_subquery(
        rsbatis::JoinKind::Inner,
        &big_orders,
        "o",
        Expr::cmp(
            BinOp::Eq,
            Expr::Col("users", "id"),
            Expr::Col("o", "user_id"),
        ),
    );
    let sql = joined.build(db.dialect());
    println!("JOIN 派生表 SQL:\n  {}\n", sql.text);
    let rows = db.fetch_all::<NameOnly>(&sql)?;
    println!(
        "  → 有大额订单的人: {:?}\n",
        rows.into_iter().map(|r| r.name).collect::<Vec<_>>()
    );

    // ── 4. 带 CTE 的 COUNT：WITH 必须一起渲染，否则表名不存在 ──
    let cnt = SelectStmt::new("t")
        .cte("t", &inner)
        .build_count(db.dialect());
    println!("带 CTE 的 COUNT SQL:\n  {}", cnt.text);

    Ok(())
}

#[derive(FromRow, Debug)]
struct NameOnly {
    name: String,
}
