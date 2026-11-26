//! 递归 CTE 在真实 MariaDB 上（10.6，支持 `WITH RECURSIVE`）。
//!
//! 没有 `RSBATIS_MYSQL_URL` 时直接跳过。

#![cfg(feature = "mysql")]

use rsbatis::{
    ArithOp, BinOp, Entity, Expr, FromRow, JoinKind, MySqlDriver, SelectStmt, Session, SortOrder,
};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "v_tree")]
#[allow(dead_code)] // 只用到 TABLE 元数据
struct Node {
    id: i64,
    parent_id: i64,
    name: String,
}

fn seeded() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接 MySQL"));
    db.execute_batch(
        "DROP TABLE IF EXISTS v_tree;
         CREATE TABLE v_tree (id BIGINT PRIMARY KEY, parent_id BIGINT NOT NULL, name VARCHAR(32) NOT NULL) ENGINE=InnoDB;
         INSERT INTO v_tree (id, parent_id, name) VALUES
           (1, 0, 'root'), (2, 1, 'a'), (3, 1, 'b'),
           (4, 2, 'a1'), (5, 2, 'a2'), (6, 3, 'b1');",
    )
    .expect("建表与灌数据");
    Some(db)
}

fn numbers(limit: i64) -> SelectStmt {
    let base = SelectStmt::no_table().column_expr(Expr::lit(1_i64).as_("n"));
    let step = SelectStmt::from_cte("nums")
        .column_expr(Expr::arith(ArithOp::Add, Expr::col("nums", "n"), Expr::lit(1_i64)).as_("n"))
        .and_filter(Expr::cmp(
            BinOp::Lt,
            Expr::col("nums", "n"),
            Expr::lit(limit),
        ));
    SelectStmt::new("nums")
        .column("n")
        .recursive_cte("nums", &base, &step)
        .order_by(Expr::bare_col("n"), SortOrder::Asc)
}

#[test]
fn recursive_numbers_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    #[derive(FromRow, Debug)]
    struct N {
        n: i64,
    }
    let rows: Vec<N> = db.fetch_all(numbers(5).build(db.dialect())).unwrap();
    assert_eq!(
        vec![1, 2, 3, 4, 5],
        rows.into_iter().map(|r| r.n).collect::<Vec<_>>()
    );
}

#[test]
fn recursive_tree_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let base = SelectStmt::new("v_tree")
        .column("id")
        .column("parent_id")
        .column("name")
        .and_filter(Expr::cmp(
            BinOp::Eq,
            Expr::col("v_tree", "id"),
            Expr::lit(1_i64),
        ));
    let step = SelectStmt::from_cte("d")
        .column_expr(Expr::col("v_tree", "id").as_("id"))
        .column_expr(Expr::col("v_tree", "parent_id").as_("parent_id"))
        .column_expr(Expr::col("v_tree", "name").as_("name"))
        .join_table(
            JoinKind::Inner,
            "v_tree",
            Expr::cmp(
                BinOp::Eq,
                Expr::col("v_tree", "parent_id"),
                Expr::col("d", "id"),
            ),
        );
    let stmt = SelectStmt::new("d")
        .column("id")
        .column("name")
        .recursive_cte("d", &base, &step)
        .order_by(Expr::col("d", "id"), SortOrder::Asc);

    #[derive(FromRow, Debug)]
    struct Named {
        id: i64,
        name: String,
    }
    let rows: Vec<Named> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    let got: Vec<(i64, String)> = rows.into_iter().map(|r| (r.id, r.name)).collect();
    assert_eq!(6, got.len(), "root 展开应得 6 个节点，got: {got:?}");
    assert_eq!((1, "root".to_string()), got[0]);
}

#[test]
fn recursive_count_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let count = numbers(10).build_count(db.dialect());
    let total: i64 = db.fetch_one(count).unwrap().unwrap().get::<i64>(0).unwrap();
    assert_eq!(10, total);
}

#[test]
fn recursive_cte_with_plain_cte_mysql() {
    // 普通 CTE + 递归 CTE 混用：RECURSIVE 只在 WITH 后出现一次
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let plain = SelectStmt::new("v_tree").column("id");
    let base = SelectStmt::no_table().column_expr(Expr::lit(1_i64).as_("n"));
    let step = SelectStmt::from_cte("nums")
        .column_expr(Expr::arith(ArithOp::Add, Expr::col("nums", "n"), Expr::lit(1_i64)).as_("n"))
        .and_filter(Expr::cmp(
            BinOp::Lt,
            Expr::col("nums", "n"),
            Expr::lit(3_i64),
        ));
    let stmt = SelectStmt::new("ids")
        .column("id")
        .cte("ids", &plain)
        .recursive_cte("nums", &base, &step);
    let sql = stmt.build(db.dialect());
    assert_eq!(1, sql.text.matches("RECURSIVE").count());
    #[derive(FromRow, Debug)]
    #[allow(dead_code)]
    struct IdOnly {
        id: i64,
    }
    let rows: Vec<IdOnly> = db.fetch_all(sql).unwrap();
    assert_eq!(6, rows.len());
}
