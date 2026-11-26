//! `UNION` / `UNION ALL` 渲染。
//!
//! 钉三件事：段怎么拼、顶层 `ORDER BY` 只用裸列、`build_count` 走子查询。

use rsbatis_core::{BinOp, Dialect, Expr, Order, SelectStmt};

fn sql(s: &SelectStmt, d: Dialect) -> String {
    s.build(d).text
}

fn seg(table: &'static str, col: &'static str) -> SelectStmt {
    SelectStmt::new(table).column(col)
}

#[test]
fn union_all_joins_two_segments() {
    let stmt = seg("a", "x").union_all(&seg("b", "x"));
    assert_eq!(
        r#"SELECT "x" FROM "a" UNION ALL SELECT "x" FROM "b""#,
        sql(&stmt, Dialect::Sqlite)
    );
}

#[test]
fn union_distinct_keyword() {
    let stmt = seg("a", "x").union(&seg("b", "x"));
    assert_eq!(
        r#"SELECT "x" FROM "a" UNION SELECT "x" FROM "b""#,
        sql(&stmt, Dialect::Sqlite)
    );
}

#[test]
fn chained_three_segments() {
    let stmt = seg("a", "x")
        .union_all(&seg("b", "x"))
        .union_all(&seg("c", "x"));
    assert_eq!(
        r#"SELECT "x" FROM "a" UNION ALL SELECT "x" FROM "b" UNION ALL SELECT "x" FROM "c""#,
        sql(&stmt, Dialect::Sqlite)
    );
}

#[test]
fn order_by_applies_to_whole_union_without_table_prefix() {
    // 顶层 ORDER BY 在复合查询上不能用带表名的列引用，
    // 所以即使写 `Expr::col("a","x")` 也应渲染成裸 `"x"`
    let stmt = seg("a", "x")
        .union_all(&seg("b", "x"))
        .order_by(Expr::col("a", "x"), Order::Desc);
    assert_eq!(
        r#"SELECT "x" FROM "a" UNION ALL SELECT "x" FROM "b" ORDER BY "x" DESC"#,
        sql(&stmt, Dialect::Sqlite)
    );
}

#[test]
fn limit_after_union() {
    let stmt = seg("a", "x").union_all(&seg("b", "x")).limit(5);
    assert!(
        sql(&stmt, Dialect::Sqlite).ends_with("UNION ALL SELECT \"x\" FROM \"b\" LIMIT 5"),
        "LIMIT 必须落在整条复合查询末尾"
    );
}

#[test]
fn segment_order_by_is_dropped_with_warning() {
    // 段内排序在 SQLite 上是语法错误，所以丢弃 + 告警（不生成必然被拒的 SQL）
    let bad = SelectStmt::new("b")
        .column("x")
        .order_by(Expr::col("b", "x"), Order::Asc);
    let stmt = seg("a", "x").union_all(&bad);
    let s = stmt.build(Dialect::Sqlite);
    assert_eq!(
        r#"SELECT "x" FROM "a" UNION ALL SELECT "x" FROM "b""#,
        s.text
    );
    assert!(
        s.warnings.iter().any(|w| w.contains("ORDER BY / LIMIT")),
        "应告警段内排序被丢弃，got: {:?}",
        s.warnings
    );
}

#[test]
fn column_count_mismatch_warns() {
    let two = SelectStmt::new("b").column("x").column("y");
    let stmt = seg("a", "x").union_all(&two);
    let s = stmt.build(Dialect::Sqlite);
    assert!(
        s.warnings.iter().any(|w| w.contains("列数必须一致")),
        "got: {:?}",
        s.warnings
    );
}

#[test]
fn build_count_wraps_union_in_subquery() {
    // 直接 `SELECT COUNT(*) FROM a UNION SELECT COUNT(*) FROM b` 会返回两行，
    // 必须把整条复合查询包一层
    let stmt = seg("a", "x").union_all(&seg("b", "x"));
    let c = stmt.build_count(Dialect::Sqlite);
    assert_eq!(
        r#"SELECT COUNT(*) FROM (SELECT "x" FROM "a" UNION ALL SELECT "x" FROM "b") AS "__rsbatis_count""#,
        c.text
    );
}

#[test]
fn union_with_filteron_each_segment() {
    let left = SelectStmt::new("a").column("x").and_filter(Expr::cmp(
        BinOp::Lt,
        Expr::col("a", "x"),
        Expr::lit(10_i64),
    ));
    let right = SelectStmt::new("b").column("x").and_filter(Expr::cmp(
        BinOp::Gt,
        Expr::col("b", "x"),
        Expr::lit(100_i64),
    ));
    let stmt = left.union_all(&right);
    assert_eq!(
        r#"SELECT "x" FROM "a" WHERE "x" < ? UNION ALL SELECT "x" FROM "b" WHERE "x" > ?"#,
        sql(&stmt, Dialect::Sqlite)
    );
    assert_eq!(
        2,
        stmt.build(Dialect::Sqlite).params.len(),
        "两段各一个参数"
    );
}

#[test]
fn union_params_numbered_in_order_pg() {
    let left = SelectStmt::new("a").column("x").and_filter(Expr::cmp(
        BinOp::Lt,
        Expr::col("a", "x"),
        Expr::lit(10_i64),
    ));
    let right = SelectStmt::new("b").column("x").and_filter(Expr::cmp(
        BinOp::Gt,
        Expr::col("b", "x"),
        Expr::lit(100_i64),
    ));
    let stmt = left.union_all(&right);
    let out = sql(&stmt, Dialect::Postgres);
    assert!(out.contains("\"x\" < $1"), "第一段参数是 $1，got: {out}");
    assert!(out.contains("\"x\" > $2"), "第二段参数接续 $2，got: {out}");
}

#[test]
fn mysql_backticks_in_union() {
    let stmt = seg("a", "x").union_all(&seg("b", "x"));
    assert_eq!(
        "SELECT `x` FROM `a` UNION ALL SELECT `x` FROM `b`",
        sql(&stmt, Dialect::MySql)
    );
}

#[test]
fn union_segment_can_be_derived_table() {
    let inner = SelectStmt::new("a").column("x");
    let derived = SelectStmt::from_subquery(&inner, "t").column("x");
    let stmt = seg("b", "x").union_all(&derived);
    let out = sql(&stmt, Dialect::Sqlite);
    assert!(
        // 派生表内部的子查询强制带表名（这是既有约定，与 UNION 无关）
        out.contains(r#"UNION ALL SELECT "t"."x" FROM (SELECT "a"."x" FROM "a") AS "t""#),
        "got: {out}"
    );
}

#[test]
fn filteron_union_warns_about_scope() {
    // `.union_all(&b).and_filter(…)` 的 WHERE 只作用于第一段，用户几乎一定误解，
    // 所以必须告警（否则结果是"多出来的行"，不报错）
    let stmt = SelectStmt::new("a")
        .column("x")
        .and_filter(Expr::cmp(BinOp::Gt, Expr::col("a", "x"), Expr::lit(1_i64)))
        .union_all(&seg("b", "x"));
    let s = stmt.build(Dialect::Sqlite);
    assert!(
        s.warnings
            .iter()
            .any(|w| w.contains("只作用于 UNION 的第一段")),
        "got: {:?}",
        s.warnings
    );
}

#[test]
fn union_inside_derived_table_keeps_segment_style_uniform() {
    // 外层派生表会把 force_qualify 传进来；UNION 各段不能因此只给第一段加前缀
    let union = seg("a", "x").union_all(&seg("b", "x"));
    let outer = SelectStmt::from_subquery(&union, "t").column("x");
    let out = sql(&outer, Dialect::Sqlite);
    assert_eq!(
        r#"SELECT "t"."x" FROM (SELECT "x" FROM "a" UNION ALL SELECT "x" FROM "b") AS "t""#,
        out
    );
}

#[test]
fn bare_col_has_no_table_prefix_even_when_qualified() {
    // `bare_col` 在 JOIN（qualify = true）下也必须渲染成裸列名
    let stmt = SelectStmt::new("a")
        .join_table(
            rsbatis_core::JoinKind::Inner,
            "b",
            Expr::cmp(BinOp::Eq, Expr::col("a", "id"), Expr::col("b", "a_id")),
        )
        .column_expr(Expr::bare_col("x").as_("x"));
    let out = sql(&stmt, Dialect::Sqlite);
    assert!(out.starts_with(r#"SELECT "x" AS "x""#), "got: {out}");
}

#[test]
fn union_of_aggregates_counts_once_per_segment() {
    // 每段各自聚合后合并：两段都不该互相影响
    let left = SelectStmt::new("a")
        .column_expr(Expr::count_star().as_("c"))
        .group_by(Expr::col("a", "g"));
    let right = SelectStmt::new("b")
        .column_expr(Expr::count_star().as_("c"))
        .group_by(Expr::col("b", "g"));
    let stmt = left.union_all(&right);
    let out = sql(&stmt, Dialect::Sqlite);
    assert_eq!(
        r#"SELECT COUNT(*) AS "c" FROM "a" GROUP BY "g" UNION ALL SELECT COUNT(*) AS "c" FROM "b" GROUP BY "g""#,
        out
    );
    assert!(
        stmt.build(Dialect::Sqlite).warnings.is_empty(),
        "段内 GROUP BY 不是告警项"
    );
}
