//! 窗口函数在真实 MySQL / MariaDB 上（MariaDB 10.6）。
//!
//! 没有 `RSBATIS_MYSQL_URL` 时直接跳过。

#![cfg(feature = "mysql")]

use rsbatis::{BinOp, Entity, Expr, FromRow, MySqlDriver, SelectStmt, Session, SortOrder, Window};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "v_w_orders")]
#[allow(dead_code)] // 只用到 TABLE 元数据，字段不参与本文件的读取
struct Order {
    id: i64,
    user_id: i64,
    amount: i64,
}

fn seeded() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接 MySQL"));
    db.execute_batch(
        "DROP TABLE IF EXISTS v_w_orders;
         CREATE TABLE v_w_orders (id BIGINT PRIMARY KEY, user_id BIGINT NOT NULL, amount BIGINT NOT NULL) ENGINE=InnoDB;
         INSERT INTO v_w_orders (id, user_id, amount) VALUES
           (1, 1, 100), (2, 1, 200), (3, 1, 50), (4, 2, 300), (5, 2, 10);",
    )
    .expect("建表与灌数据");
    Some(db)
}

#[test]
fn row_number_within_partition_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let stmt = SelectStmt::new("v_w_orders")
        .column("user_id")
        .column("amount")
        .column_expr(
            Expr::row_number()
                .over(
                    Window::new()
                        .partition_by(Expr::col("v_w_orders", "user_id"))
                        .order_by(Expr::col("v_w_orders", "amount"), SortOrder::Desc),
                )
                .as_("rn"),
        )
        .order_by(Expr::col("v_w_orders", "id"), SortOrder::Asc);
    #[derive(FromRow, Debug)]
    struct Row {
        user_id: i64,
        amount: i64,
        rn: i64,
    }
    let rows: Vec<Row> = db.all(stmt.build(db.dialect())).unwrap();
    let got: Vec<(i64, i64, i64)> = rows
        .into_iter()
        .map(|r| (r.user_id, r.amount, r.rn))
        .collect();
    assert_eq!(
        vec![
            (1, 100, 2),
            (1, 200, 1),
            (1, 50, 3),
            (2, 300, 1),
            (2, 10, 2)
        ],
        got
    );
}

#[test]
fn top_n_per_group_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let inner = SelectStmt::new("v_w_orders")
        .column("user_id")
        .column("amount")
        .column_expr(
            Expr::row_number()
                .over(
                    Window::new()
                        .partition_by(Expr::col("v_w_orders", "user_id"))
                        .order_by(Expr::col("v_w_orders", "amount"), SortOrder::Desc),
                )
                .as_("rn"),
        );
    let outer = SelectStmt::from_subquery(&inner, "t")
        .column("user_id")
        .column("amount")
        .and_filter(Expr::cmp(BinOp::Le, Expr::col("t", "rn"), Expr::lit(2_i64)))
        // 第二排序键必须是 amount：只按 user_id 排，同一 user 内两行的顺序由引擎决定，
        // 断言会随查询计划变化而红（这条断言就曾在 MariaDB 上把 100 和 200 排反）。
        .order_by(Expr::col("t", "user_id"), SortOrder::Asc)
        .order_by(Expr::col("t", "amount"), SortOrder::Desc);
    #[derive(FromRow, Debug)]
    struct Row {
        user_id: i64,
        amount: i64,
    }
    let rows: Vec<Row> = db.all(outer.build(db.dialect())).unwrap();
    let got: Vec<(i64, i64)> = rows.into_iter().map(|r| (r.user_id, r.amount)).collect();
    assert_eq!(vec![(1, 200), (1, 100), (2, 300), (2, 10)], got);
}

#[test]
fn cumulative_sum_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let stmt = SelectStmt::new("v_w_orders")
        .column("user_id")
        .column("amount")
        // MySQL 的 SUM(BIGINT) 返回 DECIMAL（回读是 text），必须显式 cast 成整数；
        // SQLite 直接返回整数，所以那边不需要。CAST 包窗口是合法的，
        // 渲染成 `CAST(SUM(…) OVER (…) AS SIGNED)`。
        .column_expr(
            Expr::cast(
                Expr::sum(Expr::col("v_w_orders", "amount")).over(
                    Window::new()
                        .partition_by(Expr::col("v_w_orders", "user_id"))
                        .order_by(Expr::col("v_w_orders", "amount"), SortOrder::Asc)
                        .rows_cumulative(),
                ),
                "INTEGER",
            )
            .as_("running"),
        )
        .order_by(Expr::col("v_w_orders", "id"), SortOrder::Asc);
    #[derive(FromRow, Debug)]
    struct Row {
        user_id: i64,
        amount: i64,
        running: i64,
    }
    let rows: Vec<Row> = db.all(stmt.build(db.dialect())).unwrap();
    let got: Vec<(i64, i64, i64)> = rows
        .into_iter()
        .map(|r| (r.user_id, r.amount, r.running))
        .collect();
    assert_eq!(
        vec![
            (1, 100, 150),
            (1, 200, 350),
            (1, 50, 50),
            (2, 300, 310),
            (2, 10, 10)
        ],
        got
    );
}

#[test]
fn rank_ties_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    db.execute_batch(
        "DROP TABLE IF EXISTS v_w_ties;
         CREATE TABLE v_w_ties (id BIGINT PRIMARY KEY, v INT NOT NULL) ENGINE=InnoDB;
         INSERT INTO v_w_ties (id, v) VALUES (1, 10), (2, 20), (3, 20), (4, 30);",
    )
    .unwrap();
    let stmt = SelectStmt::new("v_w_ties")
        .column("id")
        .column_expr(
            Expr::rank()
                .over(Window::new().order_by(Expr::col("v_w_ties", "v"), SortOrder::Asc))
                .as_("rk"),
        )
        .column_expr(
            Expr::dense_rank()
                .over(Window::new().order_by(Expr::col("v_w_ties", "v"), SortOrder::Asc))
                .as_("dr"),
        )
        .order_by(Expr::col("v_w_ties", "id"), SortOrder::Asc);
    #[derive(FromRow, Debug)]
    struct Row {
        id: i64,
        rk: i64,
        dr: i64,
    }
    let rows: Vec<Row> = db.all(stmt.build(db.dialect())).unwrap();
    let got: Vec<(i64, i64, i64)> = rows.into_iter().map(|r| (r.id, r.rk, r.dr)).collect();
    assert_eq!(vec![(1, 1, 1), (2, 2, 2), (3, 2, 2), (4, 4, 3)], got);
}

#[test]
fn ntile_and_empty_over_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    // `OVER ()` 空括号在 MariaDB 上必须合法，否则 `row_number()` 的默认形态就是坑
    // 只需要 bucket：ORDER BY 可以引用没投影的列，不必为了排序把 id 也投影出来
    let stmt = SelectStmt::new("v_w_orders")
        .column_expr(Expr::ntile(2).as_("bucket"))
        .order_by(Expr::col("v_w_orders", "id"), SortOrder::Asc);
    #[derive(FromRow, Debug)]
    struct Row {
        bucket: i64,
    }
    let rows: Vec<Row> = db.all(stmt.build(db.dialect())).unwrap();
    let buckets: Vec<i64> = rows.into_iter().map(|r| r.bucket).collect();
    assert_eq!(vec![1, 1, 1, 2, 2], buckets);
}
