//! 递归 CTE 渲染（`WITH RECURSIVE`）。
//!
//! 结果正确性由 `rsbatis` 的 `recursive_e2e` / `recursive_mysql` 用真库验证；
//! 这里钉文本与告警。

use rsbatis_core::{ArithOp, BinOp, Dialect, Expr, Order, SelectStmt};

fn sql(s: &SelectStmt, d: Dialect) -> String {
    s.build(d).text
}

/// 1..=5 的计数器（最标准的递归 CTE 用例）。
fn numbers() -> SelectStmt {
    let base = SelectStmt::no_table().column_expr(Expr::lit(1_i64).as_("n"));
    let step = SelectStmt::from_cte("nums")
        .column_expr(Expr::arith(ArithOp::Add, Expr::col("nums", "n"), Expr::lit(1_i64)).as_("n"))
        .and_filter(Expr::cmp(
            BinOp::Lt,
            Expr::col("nums", "n"),
            Expr::lit(5_i64),
        ));
    SelectStmt::new("nums")
        .column("n")
        .recursive_cte("nums", &base, &step)
}

#[test]
fn recursive_cte_renders_with_recursive_keyword() {
    let out = sql(&numbers(), Dialect::Sqlite);
    assert!(
        out.starts_with(r#"WITH RECURSIVE "nums" AS ("#),
        "got: {out}"
    );
    assert!(
        out.contains(r#"UNION ALL SELECT ("nums"."n" + ?)"#),
        "递归段必须带 CTE 名限定，got: {out}"
    );
    assert!(out.contains(r#"WHERE "nums"."n" < ?"#), "got: {out}");
}

#[test]
fn recursive_cte_full_sql() {
    let out = sql(&numbers(), Dialect::Sqlite);
    assert_eq!(
        // 基础段无 FROM；递归段带 CTE 名限定；外层因为引用 CTE 也带限定
        r#"WITH RECURSIVE "nums" AS (SELECT ? AS "n" UNION ALL SELECT ("nums"."n" + ?) AS "n" FROM "nums" WHERE "nums"."n" < ?) SELECT "nums"."n" FROM "nums""#,
        out
    );
}

#[test]
fn recursive_cte_params_in_order() {
    // 参数顺序：基础段 1 → 递归段 +1 → 递归段条件 5 → 外层无参数
    let s = numbers().build(Dialect::Postgres);
    assert!(s.text.contains("SELECT $1 AS"), "got: {}", s.text);
    assert!(s.text.contains("+ $2)"), "got: {}", s.text);
    assert!(s.text.contains("< $3"), "got: {}", s.text);
    assert_eq!(3, s.params.len());
}

#[test]
fn non_recursive_cte_keeps_plain_with() {
    let sub = SelectStmt::new("users").column("id");
    let q = SelectStmt::new("t").column("id").cte("t", &sub);
    let out = sql(&q, Dialect::Sqlite);
    assert!(out.starts_with(r#"WITH "t" AS ("#), "got: {out}");
    assert!(!out.contains("RECURSIVE"), "不递归就不该有 RECURSIVE");
}

#[test]
fn mixed_ctes_put_recursive_once_at_with_level() {
    // 一个递归 + 一个普通 CTE：RECURSIVE 只能出现在 WITH 之后一次
    let plain = SelectStmt::new("users").column("id");
    let base = SelectStmt::no_table().column_expr(Expr::lit(1_i64).as_("n"));
    let step = SelectStmt::from_cte("nums")
        .column_expr(Expr::arith(ArithOp::Add, Expr::col("nums", "n"), Expr::lit(1_i64)).as_("n"));
    let q = SelectStmt::new("u")
        .column("id")
        .cte("u", &plain)
        .recursive_cte("nums", &base, &step);
    let out = sql(&q, Dialect::Sqlite);
    assert_eq!(1, out.matches("RECURSIVE").count(), "got: {out}");
    assert!(out.starts_with(r#"WITH RECURSIVE "u" AS"#), "got: {out}");
}

#[test]
fn build_count_carries_recursive_with() {
    // COUNT 也必须带 WITH RECURSIVE，否则引用的 CTE 名不存在
    let c = numbers().build_count(Dialect::Sqlite);
    assert!(
        c.text.starts_with(r#"WITH RECURSIVE "nums" AS ("#),
        "got: {}",
        c.text
    );
    // 外层的 UNION 在 CTE 体里，不在本层：外层只是 `COUNT(*) FROM nums`，
    // 数的是 CTE 求值出来的行数 —— 这里不需要再包一层子查询。
    assert!(
        c.text.contains(r#"SELECT COUNT(*) FROM "nums""#),
        "got: {}",
        c.text
    );
}

#[test]
fn recursive_cte_mysql_backticks() {
    let out = sql(&numbers(), Dialect::MySql);
    assert!(out.starts_with("WITH RECURSIVE `nums` AS ("), "got: {out}");
    assert!(out.contains("`nums`.`n` + ?"), "got: {out}");
}

#[test]
fn step_with_order_by_warns() {
    // 递归段内 ORDER BY 在 SQLite/MySQL 都不允许，应告警（UNION 段丢弃逻辑复用）
    let base = SelectStmt::no_table().column_expr(Expr::lit(1_i64).as_("n"));
    let step = SelectStmt::from_cte("nums")
        .column_expr(Expr::col("nums", "n").as_("n"))
        .order_by(Expr::col("nums", "n"), Order::Asc);
    let q = SelectStmt::new("nums")
        .column("n")
        .recursive_cte("nums", &base, &step);
    let s = q.build(Dialect::Sqlite);
    assert!(
        s.warnings.iter().any(|w| w.contains("ORDER BY / LIMIT")),
        "got: {:?}",
        s.warnings
    );
}

#[test]
fn from_cte_qualifies_even_without_join() {
    // 单表但来自 CTE 引用：列带限定名，便于与 WHERE 条件一致
    let s = SelectStmt::from_cte("t").column("x");
    assert_eq!(r#"SELECT "t"."x" FROM "t""#, sql(&s, Dialect::Sqlite));
}
