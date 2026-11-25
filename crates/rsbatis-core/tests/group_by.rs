//! `GROUP BY` / `HAVING` / 聚合函数的渲染契约。
//!
//! 两个容易出事的点：
//! 1. 子句顺序：`WHERE` → `GROUP BY` → `HAVING` → `ORDER BY`，写错就是语法错误；
//! 2. `build_count`：有 `GROUP BY` 时 `COUNT(*)` 数是"组数"，不是总行数，
//!    必须把原查询包一层子查询。

use rsbatis_core::{BinOp, Dialect, Expr, Order, SelectStmt, Value};

#[test]
fn group_by_and_having_render_in_order() {
    let sql = SelectStmt::new("orders")
        .column("user_id")
        .column_expr(Expr::count_star().as_("cnt"))
        .where_(Expr::cmp(
            BinOp::Ge,
            Expr::Col("orders", "amount"),
            Expr::lit(10_i64),
        ))
        .group_by(Expr::Col("orders", "user_id"))
        .having(Expr::cmp(BinOp::Gt, Expr::count_star(), Expr::lit(2_i64)))
        .order_by(Expr::Col("orders", "user_id"), Order::Asc)
        .build(Dialect::Sqlite);
    assert_eq!(
        "SELECT \"user_id\", COUNT(*) AS \"cnt\" FROM \"orders\" \
         WHERE \"amount\" >= ? GROUP BY \"user_id\" HAVING COUNT(*) > ? ORDER BY \"user_id\" ASC",
        sql.text
    );
    // WHERE 的参数在前、HAVING 的参数在后
    assert_eq!(2, sql.params.len());
    assert_eq!(Value::I64(10), sql.params[0]);
    assert_eq!(Value::I64(2), sql.params[1]);
}

#[test]
fn aggregate_helpers_render_expected_sql() {
    // 单表查询的 qualify=false，列不带表名前缀（这是既有的一致行为，不是这轮的改动）
    let cases: Vec<(Expr, &str)> = vec![
        (Expr::count(Expr::Col("orders", "id")), "COUNT(\"id\")"),
        (Expr::count_star(), "COUNT(*)"),
        (Expr::sum(Expr::Col("orders", "amount")), "SUM(\"amount\")"),
        (Expr::avg(Expr::Col("orders", "amount")), "AVG(\"amount\")"),
        (Expr::min(Expr::Col("orders", "amount")), "MIN(\"amount\")"),
        (Expr::max(Expr::Col("orders", "amount")), "MAX(\"amount\")"),
    ];
    for (e, want) in cases {
        let sql = SelectStmt::new("orders")
            .column_expr(e)
            .build(Dialect::Sqlite);
        assert!(
            sql.text.contains(want),
            "期望包含 {want}，实际 {}",
            sql.text
        );
    }
}

#[test]
fn multiple_group_by_columns() {
    let sql = SelectStmt::new("orders")
        .column("user_id")
        .column("status")
        .group_by(Expr::Col("orders", "user_id"))
        .group_by(Expr::Col("orders", "status"))
        .build(Dialect::Sqlite);
    assert!(
        sql.text.contains("GROUP BY \"user_id\", \"status\""),
        "got: {}",
        sql.text
    );
}

#[test]
fn count_with_group_by_wraps_subquery() {
    // `SELECT COUNT(*) … GROUP BY x` 数的是组数，语义错。必须包一层。
    let sql = SelectStmt::new("orders")
        .column("user_id")
        .group_by(Expr::Col("orders", "user_id"))
        .build_count(Dialect::Sqlite);
    assert!(
        sql.text.starts_with("SELECT COUNT(*) FROM (SELECT"),
        "got: {}",
        sql.text
    );
    assert!(
        sql.text.ends_with(") AS \"__rsbatis_count\""),
        "got: {}",
        sql.text
    );
}

#[test]
fn count_without_group_by_stays_flat() {
    let sql = SelectStmt::new("orders")
        .column("user_id")
        .where_(Expr::cmp(
            BinOp::Ge,
            Expr::Col("orders", "amount"),
            Expr::lit(10_i64),
        ))
        .build_count(Dialect::Sqlite);
    assert_eq!(
        "SELECT COUNT(*) FROM \"orders\" WHERE \"amount\" >= ?",
        sql.text
    );
}

#[test]
fn group_by_with_join_qualifies_columns() {
    let sql = SelectStmt::new("users")
        .column("name")
        .join_table(
            rsbatis_core::JoinKind::Inner,
            "orders",
            Expr::cmp(
                BinOp::Eq,
                Expr::Col("users", "id"),
                Expr::Col("orders", "user_id"),
            ),
        )
        .column_expr(Expr::sum(Expr::Col("orders", "amount")).as_("total"))
        .group_by(Expr::Col("users", "name"))
        .build(Dialect::Sqlite);
    assert!(
        sql.text.contains("GROUP BY \"users\".\"name\""),
        "联表时分组列必须带表名，got: {}",
        sql.text
    );
}

#[test]
fn count_subquery_drops_limit_and_order() {
    // 总数不该被用户设的 LIMIT 影响；ORDER BY 对计数也无意义
    let sql = SelectStmt::new("orders")
        .column("user_id")
        .group_by(Expr::Col("orders", "user_id"))
        .order_by(Expr::Col("orders", "user_id"), Order::Desc)
        .limit(5)
        .build_count(Dialect::Sqlite);
    assert!(
        !sql.text.contains("LIMIT"),
        "计数子查询不应带 LIMIT：{}",
        sql.text
    );
    assert!(
        !sql.text.contains("ORDER BY"),
        "计数子查询不应带 ORDER BY：{}",
        sql.text
    );
    assert!(
        sql.text.starts_with("SELECT COUNT(*) FROM (SELECT"),
        "got: {}",
        sql.text
    );
}
