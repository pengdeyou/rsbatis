//! 窗口函数示例：每组 Top-N、累积和、排名。
//!
//! 跑：`cargo run -p rsbatis --features sqlite --example window`
//!
//! Top-N 必须走派生表再在外层过滤 —— 窗口函数不能出现在 `WHERE` 里。

use rsbatis::{BinOp, Entity, Expr, FromRow, SelectStmt, Session, SortOrder, SqliteDriver, Window};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "sales")]
struct Sale {
    id: i64,
    user_id: i64,
    amount: i64,
}

#[derive(FromRow, Debug)]
struct Ranked {
    user_id: i64,
    amount: i64,
    rn: i64,
}

fn main() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE sales (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount INTEGER NOT NULL);
         INSERT INTO sales (id, user_id, amount) VALUES
           (1, 1, 100), (2, 1, 200), (3, 1, 50), (4, 2, 300), (5, 2, 10);",
    )
    .unwrap();

    // 1) 每个用户内按金额降序排名
    let stmt = SelectStmt::new("sales")
        .column("user_id")
        .column("amount")
        .column_expr(
            Expr::row_number()
                .over(
                    Window::new()
                        .partition_by(Expr::col("sales", "user_id"))
                        .order_by(Expr::col("sales", "amount"), SortOrder::Desc),
                )
                .as_("rn"),
        );
    println!("1) 分区排名\nSQL: {}\n", db.explain(&stmt).text);
    let rows: Vec<Ranked> = db.all(&stmt).unwrap();
    for r in rows {
        println!("   user={} amount={} rn={}", r.user_id, r.amount, r.rn);
    }

    // 2) 每组 Top-2：派生表 + 外层过滤
    let inner = SelectStmt::new("sales")
        .column("user_id")
        .column("amount")
        .column_expr(
            Expr::row_number()
                .over(
                    Window::new()
                        .partition_by(Expr::col("sales", "user_id"))
                        .order_by(Expr::col("sales", "amount"), SortOrder::Desc),
                )
                .as_("rn"),
        );
    let outer = SelectStmt::from_subquery(&inner, "t")
        .column("user_id")
        .column("amount")
        .and_filter(Expr::cmp(BinOp::Le, Expr::col("t", "rn"), Expr::lit(2_i64)))
        .order_by(Expr::col("t", "user_id"), SortOrder::Asc)
        .order_by(Expr::col("t", "amount"), SortOrder::Desc);
    println!("\n2) 每组 Top-2\nSQL: {}\n", db.explain(&outer).text);
    #[derive(FromRow, Debug)]
    struct Top {
        user_id: i64,
        amount: i64,
    }
    for r in db.all::<Top>(&outer).unwrap() {
        println!("   user={} amount={}", r.user_id, r.amount);
    }

    // 3) 累积和（显式帧）
    //    注意输出的 number 看着"乱"：累积是按 amount 升序算的，
    //    而结果按 id 排序显示 —— 窗口的排序与结果集的排序是两件独立的事。
    let stmt = SelectStmt::new("sales")
        .column("user_id")
        .column("amount")
        .column_expr(
            Expr::sum(Expr::col("sales", "amount"))
                .over(
                    Window::new()
                        .partition_by(Expr::col("sales", "user_id"))
                        .order_by(Expr::col("sales", "amount"), SortOrder::Asc)
                        .rows_cumulative(),
                )
                .as_("running"),
        )
        .order_by(Expr::col("sales", "id"), SortOrder::Asc);
    println!("\n3) 累积和\nSQL: {}\n", db.explain(&stmt).text);
    #[derive(FromRow, Debug)]
    struct Run {
        user_id: i64,
        amount: i64,
        running: i64,
    }
    for r in db.all::<Run>(&stmt).unwrap() {
        println!(
            "   user={} amount={} running={}",
            r.user_id, r.amount, r.running
        );
    }
}
