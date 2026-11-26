//! `UNION` / `UNION ALL` 示例：合并两个来源后统一统计。
//!
//! 跑：`cargo run -p rsbatis --features sqlite --example union`

use rsbatis::{BinOp, Entity, Expr, FromRow, SelectStmt, Session, SortOrder, SqliteDriver};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "online_orders")]
#[allow(dead_code)]
struct Online {
    amount: i64,
}

fn main() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE online_orders (amount INTEGER NOT NULL);
         CREATE TABLE offline_orders (amount INTEGER NOT NULL);
         INSERT INTO online_orders (amount) VALUES (100), (250);
         INSERT INTO offline_orders (amount) VALUES (100), (50), (400);",
    )
    .unwrap();

    let online = SelectStmt::new("online_orders").column("amount");
    let offline = SelectStmt::new("offline_orders").column("amount");

    // 1) UNION ALL：两渠道流水全部保留（同一笔金额重复也各算一次）
    let all = online.clone().union_all(&offline);
    println!("1) UNION ALL\nSQL: {}\n", db.explain(&all).text);
    #[derive(FromRow, Debug)]
    struct Amount {
        amount: i64,
    }
    let rows: Vec<Amount> = db.fetch_all(&all).unwrap();
    println!("   行数 = {}（2 + 3）", rows.len());

    // 2) UNION：金额去重 —— 两个渠道都有 100，只算一次
    let distinct = online.clone().union(&offline);
    println!(
        "2) UNION（去重）\nSQL: {}\n",
        db.explain(&distinct).text
    );
    let rows: Vec<Amount> = db.fetch_all(&distinct).unwrap();
    println!("   行数 = {}（100 被合并）", rows.len());

    // 3) 复合查询的总数：count 必须包一层，否则算出来的是"每段各一行"
    let count = all.build_count(db.dialect());
    println!("3) 总数\nSQL: {}\n", count.text);
    let total: i64 = db.fetch_one(count).unwrap().unwrap().get::<i64>(0).unwrap();
    println!("   total = {total}");

    // 4) 合并后统一过滤 + 排序 + 截断
    //    注意：直接 `.union_all(&b).and_filter(…)` 的 WHERE 只作用于**第一段**！
    //    要对整条复合查询过滤，必须把 UNION 包成派生表再在外层过滤。
    let union = online.clone().union_all(&offline);
    let top = SelectStmt::from_subquery(&union, "t")
        .column("amount")
        .and_filter(Expr::cmp(
            BinOp::Ge,
            Expr::col("t", "amount"),
            Expr::lit(100_i64),
        ))
        .order_by(Expr::col("t", "amount"), SortOrder::Desc)
        .limit(2);
    println!("4) 合并后取前二\nSQL: {}\n", db.explain(&top).text);
    let rows: Vec<Amount> = db.fetch_all(&top).unwrap();
    for r in rows {
        println!("   amount = {}", r.amount);
    }
}
