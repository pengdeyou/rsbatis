//! 递归 CTE 在真实 SQLite 上。
//!
//! 两个真实场景：数字序列、组织树（父子层级展开）。

#![cfg(feature = "sqlite")]

use rsbatis::{
    ArithOp, BinOp, Entity, Expr, FromRow, SelectStmt, Session, SortOrder, SqliteDriver,
};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "tree")]
#[allow(dead_code)]
struct Node {
    id: i64,
    parent_id: i64,
    name: String,
}

#[derive(FromRow, Debug)]
struct Named {
    id: i64,
    name: String,
}

fn seeded() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE tree (id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL, name TEXT NOT NULL);
         INSERT INTO tree (id, parent_id, name) VALUES
           (1, 0, 'root'),
           (2, 1, 'a'), (3, 1, 'b'),
           (4, 2, 'a1'), (5, 2, 'a2'), (6, 3, 'b1');",
    )
    .unwrap();
    db
}

#[test]
fn recursive_numbers_one_to_five() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    let base = SelectStmt::no_table().column_expr(Expr::lit(1_i64).as_("n"));
    let step = SelectStmt::from_cte("nums")
        .column_expr(Expr::arith(ArithOp::Add, Expr::col("nums", "n"), Expr::lit(1_i64)).as_("n"))
        .and_filter(Expr::cmp(
            BinOp::Lt,
            Expr::col("nums", "n"),
            Expr::lit(5_i64),
        ));
    let stmt = SelectStmt::new("nums")
        .column("n")
        .recursive_cte("nums", &base, &step)
        .order_by(Expr::bare_col("n"), SortOrder::Asc);

    #[derive(FromRow, Debug)]
    struct N {
        n: i64,
    }
    let rows: Vec<N> = db.all(stmt.build(db.dialect())).unwrap();
    assert_eq!(
        vec![1, 2, 3, 4, 5],
        rows.into_iter().map(|r| r.n).collect::<Vec<_>>()
    );
}

#[test]
fn recursive_tree_descendants() {
    let mut db = seeded();
    // 从 root 出发，逐层展开所有后代（含自己）
    let base = SelectStmt::new("tree")
        .column("id")
        .column("parent_id")
        .column("name")
        .and_filter(Expr::cmp(
            BinOp::Eq,
            Expr::col("tree", "id"),
            Expr::lit(1_i64),
        ));
    // 递归段：从已展开的 d 里取一条，join 出它的直接子节点
    let step = SelectStmt::from_cte("d")
        .column_expr(Expr::col("tree", "id").as_("id"))
        .column_expr(Expr::col("tree", "parent_id").as_("parent_id"))
        .column_expr(Expr::col("tree", "name").as_("name"))
        .join_table(
            rsbatis::JoinKind::Inner,
            "tree",
            Expr::cmp(
                BinOp::Eq,
                Expr::col("tree", "parent_id"),
                Expr::col("d", "id"),
            ),
        );

    let stmt = SelectStmt::new("d")
        .column("id")
        .column("name")
        .recursive_cte("d", &base, &step)
        .order_by(Expr::col("d", "id"), SortOrder::Asc);

    let rows: Vec<Named> = db.all(stmt.build(db.dialect())).unwrap();
    let got: Vec<(i64, String)> = rows.into_iter().map(|r| (r.id, r.name)).collect();
    assert_eq!(
        vec![
            (1, "root".into()),
            (2, "a".into()),
            (3, "b".into()),
            (4, "a1".into()),
            (5, "a2".into()),
            (6, "b1".into()),
        ],
        got,
        "从 root 展开应拿到全部 6 个节点"
    );
}

#[test]
fn recursive_cte_count_is_total_after_expansion() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    let base = SelectStmt::no_table().column_expr(Expr::lit(1_i64).as_("n"));
    let step = SelectStmt::from_cte("nums")
        .column_expr(Expr::arith(ArithOp::Add, Expr::col("nums", "n"), Expr::lit(1_i64)).as_("n"))
        .and_filter(Expr::cmp(
            BinOp::Lt,
            Expr::col("nums", "n"),
            Expr::lit(10_i64),
        ));
    let stmt = SelectStmt::new("nums")
        .column("n")
        .recursive_cte("nums", &base, &step);
    let count = stmt.build_count(db.dialect());
    let total: i64 = db.first(count).unwrap().unwrap().get::<i64>(0).unwrap();
    assert_eq!(10, total);
}

#[test]
fn recursive_cte_paginates_over_expanded_rows() {
    // 分页跑在递归结果上：count 必须等于展开后的行数，否则页数算错
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    let base = SelectStmt::no_table().column_expr(Expr::lit(1_i64).as_("n"));
    let step = SelectStmt::from_cte("nums")
        .column_expr(Expr::arith(ArithOp::Add, Expr::col("nums", "n"), Expr::lit(1_i64)).as_("n"))
        .and_filter(Expr::cmp(
            BinOp::Lt,
            Expr::col("nums", "n"),
            Expr::lit(7_i64),
        ));
    let stmt = SelectStmt::new("nums")
        .column("n")
        .recursive_cte("nums", &base, &step)
        .order_by(Expr::bare_col("n"), SortOrder::Asc);

    #[derive(FromRow, Debug)]
    struct N {
        n: i64,
    }
    let page = db
        .paginate::<N>(&stmt, rsbatis::PageRequest::new(2, 3))
        .unwrap();
    assert_eq!(7, page.total, "总数应是递归展开后的 7 行");
    assert_eq!(3, page.pages(), "7 行、每页 3 行 → 3 页");
    assert_eq!(
        vec![4, 5, 6],
        page.items.into_iter().map(|r| r.n).collect::<Vec<_>>()
    );
}
