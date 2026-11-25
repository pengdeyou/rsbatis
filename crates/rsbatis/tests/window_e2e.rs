//! 窗口函数在真实 SQLite 上（结果断言，不是 SQL 文本）。
//!
//! 重点验"派生表 + ROW_NUMBER 取每组 Top-N"：窗口函数不能出现在 WHERE，
//! 这是分页/排名类需求最容易踩的一步，所以 e2e 就是照着推荐写法跑的。

#![cfg(feature = "sqlite")]

use rsbatis::{
    BinOp, Entity, Expr, FrameBound, FromRow, SelectStmt, Session, SortOrder, SqliteDriver, Window,
};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "w_orders")]
#[allow(dead_code)] // 只用到 TABLE 元数据
struct Order {
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

fn seeded() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE w_orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount INTEGER NOT NULL);
         INSERT INTO w_orders (id, user_id, amount) VALUES
           (1, 1, 100), (2, 1, 200), (3, 1, 50), (4, 2, 300), (5, 2, 10);",
    )
    .unwrap();
    db
}

#[test]
fn row_number_ranks_within_partition() {
    let mut db = seeded();
    let stmt = SelectStmt::new("w_orders")
        .column("user_id")
        .column("amount")
        .column_expr(
            Expr::row_number()
                .over(
                    Window::new()
                        .partition_by(Expr::col("w_orders", "user_id"))
                        .order_by(Expr::col("w_orders", "amount"), SortOrder::Desc),
                )
                .as_("rn"),
        );
    let rows: Vec<Ranked> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    let mut got: Vec<(i64, i64, i64)> = rows
        .into_iter()
        .map(|r| (r.user_id, r.amount, r.rn))
        .collect();
    got.sort();
    assert_eq!(
        vec![
            (1, 50, 3),
            (1, 100, 2),
            (1, 200, 1),
            (2, 10, 2),
            (2, 300, 1)
        ],
        got
    );
}

#[test]
fn top_n_per_group_via_derived_table() {
    let mut db = seeded();
    let inner = SelectStmt::new("w_orders")
        .column("user_id")
        .column("amount")
        .column_expr(
            Expr::row_number()
                .over(
                    Window::new()
                        .partition_by(Expr::col("w_orders", "user_id"))
                        .order_by(Expr::col("w_orders", "amount"), SortOrder::Desc),
                )
                .as_("rn"),
        );
    let outer = SelectStmt::from_subquery(&inner, "t")
        .column("user_id")
        .column("amount")
        .and_where(Expr::cmp(BinOp::Le, Expr::col("t", "rn"), Expr::lit(2_i64)));

    #[derive(FromRow, Debug)]
    struct Top {
        user_id: i64,
        amount: i64,
    }
    let rows: Vec<Top> = db.fetch_all(outer.build(db.dialect())).unwrap();
    let mut got: Vec<(i64, i64)> = rows.into_iter().map(|r| (r.user_id, r.amount)).collect();
    got.sort();
    assert_eq!(vec![(1, 100), (1, 200), (2, 10), (2, 300)], got);
}

#[test]
fn cumulative_sum_frame() {
    let mut db = seeded();
    let stmt = SelectStmt::new("w_orders")
        .column("user_id")
        .column("amount")
        .column_expr(
            Expr::sum(Expr::col("w_orders", "amount"))
                .over(
                    Window::new()
                        .partition_by(Expr::col("w_orders", "user_id"))
                        .order_by(Expr::col("w_orders", "amount"), SortOrder::Asc)
                        .rows_cumulative(),
                )
                .as_("running"),
        );
    #[derive(FromRow, Debug)]
    struct Row {
        user_id: i64,
        amount: i64,
        running: i64,
    }
    let rows: Vec<Row> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    let mut got: Vec<(i64, i64, i64)> = rows
        .into_iter()
        .map(|r| (r.user_id, r.amount, r.running))
        .collect();
    got.sort();
    // 用户 1：50 → 150 → 350；用户 2：10 → 310
    assert_eq!(
        vec![
            (1, 50, 50),
            (1, 100, 150),
            (1, 200, 350),
            (2, 10, 10),
            (2, 300, 310)
        ],
        got
    );
}

#[test]
fn lag_with_explicit_order() {
    let mut db = seeded();
    let stmt = SelectStmt::new("w_orders")
        .column("id")
        .column_expr(
            Expr::lag(Expr::col("w_orders", "amount"), 1)
                .over(
                    Window::new()
                        .partition_by(Expr::col("w_orders", "user_id"))
                        .order_by(Expr::col("w_orders", "amount"), SortOrder::Asc),
                )
                .as_("prev"),
        )
        .order_by(Expr::col("w_orders", "id"), SortOrder::Asc);
    #[derive(FromRow, Debug)]
    struct Row {
        id: i64,
        prev: Option<i64>,
    }
    let rows: Vec<Row> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    let by_id: Vec<(i64, Option<i64>)> = rows.into_iter().map(|r| (r.id, r.prev)).collect();
    // 用户 1 按 amount 升序是 id3(50) → id1(100) → id2(200)，
    // 所以 id1 的前一行是 50、id2 的前一行是 100、id3 是分区首行( NULL )。
    // 结果按 id 排序输出，因此出现 (1,50) (2,100) (3,None)。
    // 用户 2 按 amount 升序是 id5(10) → id4(300)：id4 的 prev=10，id5=NULL。
    assert_eq!(
        vec![
            (1, Some(50)),
            (2, Some(100)),
            (3, None),
            (4, Some(10)),
            (5, None)
        ],
        by_id
    );
}

#[test]
fn rank_vs_dense_rank_on_ties() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE w_ties (id INTEGER PRIMARY KEY, v INTEGER NOT NULL);
         INSERT INTO w_ties (id, v) VALUES (1, 10), (2, 20), (3, 20), (4, 30);",
    )
    .unwrap();
    let stmt = SelectStmt::new("w_ties")
        .column("id")
        .column_expr(
            Expr::rank()
                .over(Window::new().order_by(Expr::col("w_ties", "v"), SortOrder::Asc))
                .as_("rk"),
        )
        .column_expr(
            Expr::dense_rank()
                .over(Window::new().order_by(Expr::col("w_ties", "v"), SortOrder::Asc))
                .as_("dr"),
        )
        .order_by(Expr::col("w_ties", "id"), SortOrder::Asc);
    #[derive(FromRow, Debug)]
    struct Row {
        id: i64,
        rk: i64,
        dr: i64,
    }
    let rows: Vec<Row> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    let got: Vec<(i64, i64, i64)> = rows.into_iter().map(|r| (r.id, r.rk, r.dr)).collect();
    // RANK 跳号（1,2,2,4），DENSE_RANK 不跳（1,2,2,3）
    assert_eq!(vec![(1, 1, 1), (2, 2, 2), (3, 2, 2), (4, 4, 3)], got);
}

#[test]
fn ntile_buckets_total_sum() {
    let mut db = seeded();
    let stmt = SelectStmt::new("w_orders")
        .column_expr(
            Expr::ntile(2)
                .over(Window::new().order_by(Expr::col("w_orders", "id"), SortOrder::Asc))
                .as_("bucket"),
        )
        .order_by(Expr::col("w_orders", "id"), SortOrder::Asc);
    #[derive(FromRow, Debug)]
    struct Row {
        bucket: i64,
    }
    let rows: Vec<Row> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    // 5 行切 2 桶：3 + 2
    let buckets: Vec<i64> = rows.into_iter().map(|r| r.bucket).collect();
    assert_eq!(vec![1, 1, 1, 2, 2], buckets);
}

#[test]
fn whole_partition_last_value() {
    let mut db = seeded();
    let stmt = SelectStmt::new("w_orders")
        .column("user_id")
        .column_expr(
            Expr::last_value(Expr::col("w_orders", "amount"))
                .over(
                    Window::new()
                        .partition_by(Expr::col("w_orders", "user_id"))
                        .order_by(Expr::col("w_orders", "amount"), SortOrder::Asc)
                        .rows_between(
                            FrameBound::UnboundedPreceding,
                            FrameBound::UnboundedFollowing,
                        ),
                )
                .as_("mx"),
        )
        .order_by(Expr::col("w_orders", "id"), SortOrder::Asc);
    #[derive(FromRow, Debug)]
    struct Row {
        user_id: i64,
        mx: i64,
    }
    let rows: Vec<Row> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    // 显式整分区帧之后，每行拿到的是分区最大值，而不是"到当前行为止"
    let got: Vec<(i64, i64)> = rows.into_iter().map(|r| (r.user_id, r.mx)).collect();
    assert_eq!(vec![(1, 200), (1, 200), (1, 200), (2, 300), (2, 300)], got);
}
