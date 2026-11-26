//! 窗口函数渲染（纯字符串断言，三个方言）。
//!
//! 这里钉的是"SQL 文本与参数顺序"，不是查询结果 —— 结果语义由 `rsbatis` 的
//! `window_e2e` / `window_mysql` 用真库验证。

use rsbatis_core::{Dialect, Expr, FrameBound, Order, SelectStmt, SortOrder, Window};

fn sql(stmt: &SelectStmt, d: Dialect) -> String {
    stmt.build(d).text
}

fn params(stmt: &SelectStmt, d: Dialect) -> Vec<rsbatis_core::Value> {
    stmt.build(d).params
}

#[test]
fn row_number_with_partition_and_order() {
    let stmt = SelectStmt::new("orders")
        .column_expr(
            Expr::row_number()
                .over(
                    Window::new()
                        .partition_by(Expr::col("orders", "user_id"))
                        .order_by(Expr::col("orders", "amount"), Order::Desc),
                )
                .as_("rn"),
        )
        .column("user_id");
    assert_eq!(
        r#"SELECT ROW_NUMBER() OVER (PARTITION BY "user_id" ORDER BY "amount" DESC) AS "rn", "user_id" FROM "orders""#,
        sql(&stmt, Dialect::Sqlite)
    );
}

#[test]
fn empty_over_is_legal() {
    let stmt = SelectStmt::new("t").column_expr(Expr::row_number().as_("rn"));
    assert_eq!(
        r#"SELECT ROW_NUMBER() OVER () AS "rn" FROM "t""#,
        sql(&stmt, Dialect::Sqlite)
    );
}

#[test]
fn aggregate_as_window_with_cumulative_frame() {
    let stmt = SelectStmt::new("orders").column_expr(
        Expr::sum(Expr::col("orders", "amount"))
            .over(
                Window::new()
                    .order_by(Expr::col("orders", "id"), SortOrder::Asc)
                    .rows_cumulative(),
            )
            .as_("running"),
    );
    assert_eq!(
        r#"SELECT SUM("amount") OVER (ORDER BY "id" ASC ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS "running" FROM "orders""#,
        sql(&stmt, Dialect::Sqlite)
    );
}

#[test]
fn frame_with_numeric_bounds() {
    let w = Window::new()
        .order_by(Expr::col("t", "id"), Order::Asc)
        .rows_between(FrameBound::Preceding(2), FrameBound::Following(1));
    let stmt = SelectStmt::new("t").column_expr(Expr::avg(Expr::col("t", "v")).over(w).as_("m"));
    let out = sql(&stmt, Dialect::Sqlite);
    assert!(
        out.contains("ROWS BETWEEN 2 PRECEDING AND 1 FOLLOWING"),
        "got: {out}"
    );
}

#[test]
fn unbounded_following_for_whole_partition() {
    let w = Window::new()
        .partition_by(Expr::col("t", "g"))
        .order_by(Expr::col("t", "id"), Order::Asc)
        .rows_between(
            FrameBound::UnboundedPreceding,
            FrameBound::UnboundedFollowing,
        );
    let stmt = SelectStmt::new("t").column_expr(
        Expr::last_value(Expr::col("t", "v"))
            .over(w)
            .as_("last_all"),
    );
    assert!(
        sql(&stmt, Dialect::Sqlite)
            .contains("ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING"),
        "整分区取 last_value 必须显式给帧"
    );
}

#[test]
fn over_replaces_instead_of_nesting() {
    // `row_number()` 自带 `OVER ()`，再 `.over(spec)` 应当替换 scpec，
    // 不能渲染成 `ROW_NUMBER() OVER () OVER (…)`。
    let stmt = SelectStmt::new("t").column_expr(
        Expr::row_number()
            .over(Window::new().order_by(Expr::col("t", "id"), Order::Asc))
            .as_("rn"),
    );
    let out = sql(&stmt, Dialect::Sqlite);
    assert_eq!(1, out.matches("OVER").count(), "got: {out}");
}

#[test]
fn ntile_parameter_uses_placeholder() {
    let stmt = SelectStmt::new("t").column_expr(Expr::ntile(4).as_("bucket"));
    let out = sql(&stmt, Dialect::Postgres);
    assert!(out.contains("NTILE($1) OVER ()"), "got: {out}");
    assert_eq!(1, params(&stmt, Dialect::Postgres).len());
}

#[test]
fn lag_and_lead_offsets_are_params() {
    let stmt = SelectStmt::new("t")
        .column_expr(Expr::lag(Expr::col("t", "v"), 1).as_("prev"))
        .column_expr(Expr::lead(Expr::col("t", "v"), 2).as_("next"));
    // PG 的 $N 必须按渲染顺序递增：LAG 的 offset 是 $1，LEAD 的是 $2
    let out = sql(&stmt, Dialect::Postgres);
    assert!(out.contains("LAG(\"v\", $1) OVER ()"), "got: {out}");
    assert!(out.contains("LEAD(\"v\", $2) OVER ()"), "got: {out}");
}

#[test]
fn window_params_come_after_projection_order() {
    // 投影里带参数 → 参数先入列；然后 WHERE 的参数。
    // 这条钉的是"窗口不是特殊路径，参数据就按 SELECT 文本顺序走"。
    let stmt = SelectStmt::new("t")
        .column_expr(Expr::ntile(3).as_("b"))
        .and_filter(Expr::cmp(
            rsbatis_core::BinOp::Gt,
            Expr::col("t", "id"),
            Expr::lit(10_i64),
        ));
    let out = sql(&stmt, Dialect::Postgres);
    assert!(out.contains("NTILE($1)"), "got: {out}");
    assert!(out.contains("\"id\" > $2"), "got: {out}");
}

#[test]
fn mysql_uses_backticks_inside_window() {
    let stmt = SelectStmt::new("orders").column_expr(
        Expr::rank()
            .over(
                Window::new()
                    .partition_by(Expr::col("orders", "user_id"))
                    .order_by(Expr::col("orders", "amount"), Order::Desc),
            )
            .as_("rk"),
    );
    assert_eq!(
        "SELECT RANK() OVER (PARTITION BY `user_id` ORDER BY `amount` DESC) AS `rk` FROM `orders`",
        sql(&stmt, Dialect::MySql)
    );
}

#[test]
fn dense_rank_and_first_value() {
    let stmt = SelectStmt::new("t")
        .column_expr(Expr::dense_rank().as_("dr"))
        .column_expr(Expr::first_value(Expr::col("t", "v")).as_("fv"));
    let out = sql(&stmt, Dialect::Sqlite);
    assert!(out.contains("DENSE_RANK() OVER ()"), "got: {out}");
    assert!(out.contains("FIRST_VALUE(\"v\") OVER ()"), "got: {out}");
}

#[test]
fn window_inside_filteris_rendered_with_parens() {
    // 窗口表达式参与比较时，`OVER (…)` 自带括号，外层不需要再包一层
    let stmt = SelectStmt::new("t").and_filter(Expr::cmp(
        rsbatis_core::BinOp::Le,
        Expr::row_number().over(Window::new().order_by(Expr::col("t", "id"), Order::Asc)),
        Expr::lit(3_i64),
    ));
    let out = sql(&stmt, Dialect::Sqlite);
    assert!(
        out.contains(r#"ROW_NUMBER() OVER (ORDER BY "id" ASC) <= ?"#),
        "got: {out}"
    );
}

#[test]
fn multi_partition_columns_join_with_comma() {
    let stmt = SelectStmt::new("t").column_expr(
        Expr::row_number()
            .over(
                Window::new()
                    .partition_by(Expr::col("t", "a"))
                    .partition_by(Expr::col("t", "b")),
            )
            .as_("rn"),
    );
    assert!(
        sql(&stmt, Dialect::Sqlite).contains(r#"PARTITION BY "a", "b""#),
        "多个分区列必须逗号分隔"
    );
}

#[test]
fn joined_query_qualifies_window_columns() {
    // 两张表都有 `id` / `amount` 时，窗口里的列若不限定表名，ORDER BY 会歧义
    // （SQLite 会随便挑一个、MySQL 直接报错）。JOIN 场景 qualify 必须传到窗口内部。
    let on = Expr::cmp(
        rsbatis_core::BinOp::Eq,
        Expr::col("orders", "user_id"),
        Expr::col("users", "id"),
    );
    let stmt = SelectStmt::new("orders")
        .join_table(rsbatis_core::JoinKind::Left, "users", on)
        .column_expr(
            Expr::row_number()
                .over(
                    Window::new()
                        .partition_by(Expr::col("orders", "user_id"))
                        .order_by(Expr::col("orders", "amount"), Order::Desc),
                )
                .as_("rn"),
        );
    let out = sql(&stmt, Dialect::Sqlite);
    assert!(
        out.contains(r#"PARTITION BY "orders"."user_id""#),
        "JOIN 场景窗口列必须带表名，got: {out}"
    );
    assert!(
        out.contains(r#"ORDER BY "orders"."amount" DESC"#),
        "got: {out}"
    );
}

#[test]
fn window_in_filteremits_warning() {
    let stmt = SelectStmt::new("t").and_filter(Expr::cmp(
        rsbatis_core::BinOp::Le,
        Expr::row_number().over(Window::new()),
        Expr::lit(3_i64),
    ));
    let sql = stmt.build(Dialect::Sqlite);
    assert!(
        sql.warnings
            .iter()
            .any(|w| w.contains("窗口函数不能出现在 WHERE")),
        "应告警而不是让引擎报 misuse，got: {:?}",
        sql.warnings
    );
}

#[test]
fn window_in_having_emits_warning() {
    let stmt = SelectStmt::new("t")
        .group_by(Expr::col("t", "g"))
        .having(Expr::cmp(
            rsbatis_core::BinOp::Le,
            Expr::rank().over(Window::new()),
            Expr::lit(3_i64),
        ));
    let sql = stmt.build(Dialect::Sqlite);
    assert!(
        sql.warnings
            .iter()
            .any(|w| w.contains("窗口函数不能出现在 HAVING")),
        "got: {:?}",
        sql.warnings
    );
}

#[test]
fn window_in_projection_has_no_warning() {
    // 正常的窗口用法不能产生噪音告警
    let stmt = SelectStmt::new("t").column_expr(Expr::row_number().as_("rn"));
    assert!(stmt.build(Dialect::Sqlite).warnings.is_empty());
}

#[test]
fn window_in_subquery_filterwarns_at_subquery_level() {
    // 子查询里的窗口已经在派生表内部，外层过滤 `t.rn` 是列引用不是窗口，
    // 所以外层不该告警（推荐写法不能被自己的防护误伤）
    let inner = SelectStmt::new("t").column_expr(Expr::row_number().as_("rn"));
    let outer = SelectStmt::from_subquery(&inner, "t").and_filter(Expr::cmp(
        rsbatis_core::BinOp::Le,
        Expr::col("t", "rn"),
        Expr::lit(2_i64),
    ));
    assert!(outer.build(Dialect::Sqlite).warnings.is_empty());
}
