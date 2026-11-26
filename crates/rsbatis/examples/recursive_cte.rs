//! 递归 CTE 示例：组织树展开 + 数字序列。
//!
//! 跑：`cargo run -p rsbatis --features sqlite --example recursive_cte`

use rsbatis::{
    ArithOp, BinOp, Entity, Expr, FromRow, JoinKind, SelectStmt, Session, SortOrder, SqliteDriver,
};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "tree")]
#[allow(dead_code)]
struct Node {
    id: i64,
    parent_id: i64,
    name: String,
}

fn main() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE tree (id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL, name TEXT NOT NULL);
         INSERT INTO tree (id, parent_id, name) VALUES
           (1, 0, 'root'), (2, 1, 'a'), (3, 1, 'b'),
           (4, 2, 'a1'), (5, 2, 'a2'), (6, 3, 'b1');",
    )
    .unwrap();

    // 1) 数字序列：基础段无 FROM，递归段引用自己（必须用 from_cte）
    let base = SelectStmt::no_table().column_expr(Expr::lit(1_i64).as_("n"));
    let step = SelectStmt::from_cte("nums")
        .column_expr(Expr::arith(ArithOp::Add, Expr::col("nums", "n"), Expr::lit(1_i64)).as_("n"))
        .and_filter(Expr::cmp(
            BinOp::Lt,
            Expr::col("nums", "n"),
            Expr::lit(5_i64),
        ));
    let nums = SelectStmt::new("nums")
        .column("n")
        .recursive_cte("nums", &base, &step)
        .order_by(Expr::bare_col("n"), SortOrder::Asc);
    println!("1) 数字序列 1..5\nSQL: {}\n", db.explain(&nums).text);
    #[derive(FromRow, Debug)]
    struct N {
        n: i64,
    }
    let rows: Vec<N> = db.fetch_all(&nums).unwrap();
    println!(
        "   n = {:?}\n",
        rows.into_iter().map(|r| r.n).collect::<Vec<_>>()
    );

    // 2) 组织树：从 root 出发展开全部后代
    let base = SelectStmt::new("tree")
        .column("id")
        .column("parent_id")
        .column("name")
        .and_filter(Expr::cmp(
            BinOp::Eq,
            Expr::col("tree", "id"),
            Expr::lit(1_i64),
        ));
    // 递归段：从已展开的 d 里 join 出直接子节点
    let step = SelectStmt::from_cte("d")
        .column_expr(Expr::col("tree", "id").as_("id"))
        .column_expr(Expr::col("tree", "parent_id").as_("parent_id"))
        .column_expr(Expr::col("tree", "name").as_("name"))
        .join_table(
            JoinKind::Inner,
            "tree",
            Expr::cmp(
                BinOp::Eq,
                Expr::col("tree", "parent_id"),
                Expr::col("d", "id"),
            ),
        );
    let tree = SelectStmt::new("d")
        .column("id")
        .column("name")
        .recursive_cte("d", &base, &step)
        .order_by(Expr::col("d", "id"), SortOrder::Asc);
    println!("2) 组织树展开\nSQL: {}\n", db.explain(&tree).text);

    #[derive(FromRow, Debug)]
    struct Named {
        id: i64,
        name: String,
    }
    for r in db.fetch_all::<Named>(&tree).unwrap() {
        println!("   #{} {}", r.id, r.name);
    }
}
