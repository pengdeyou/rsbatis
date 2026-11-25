//! CTE（`WITH 名 AS (子查询)`）的渲染契约。
//!
//! `WITH` 出现在 `SELECT` 之前，所以 CTE 的参数必须排在最前面——
//! 与派生表、JOIN 子查询一起构成"三条前置位置"，参数顺序各有讲究。

use rsbatis_core::{BinOp, Dialect, Expr, SelectStmt, Value};

fn young() -> SelectStmt {
    SelectStmt::new("users")
        .column("id")
        .column("name")
        .column("age")
        .where_(Expr::cmp(
            BinOp::Ge,
            Expr::Col("users", "age"),
            Expr::lit(18_i64),
        ))
}

#[test]
fn single_cte_renders_prefixed() {
    let sql = SelectStmt::new("young")
        .cte("young", &young())
        .build(Dialect::Sqlite);
    assert_eq!(
        "WITH \"young\" AS (SELECT \"users\".\"id\", \"users\".\"name\", \"users\".\"age\" \
         FROM \"users\" WHERE \"users\".\"age\" >= ?) SELECT * FROM \"young\"",
        sql.text
    );
    assert_eq!(1, sql.params.len());
}

#[test]
fn multiple_ctes_render_comma_separated() {
    let old = SelectStmt::new("users").column("id").where_(Expr::cmp(
        BinOp::Ge,
        Expr::Col("users", "age"),
        Expr::lit(60_i64),
    ));
    let sql = SelectStmt::new("young")
        .cte("young", &young())
        .cte("old", &old)
        .build(Dialect::Sqlite);
    assert!(
        sql.text.starts_with("WITH \"young\" AS (SELECT"),
        "got: {}",
        sql.text
    );
    assert!(
        sql.text.contains("), \"old\" AS (SELECT"),
        "got: {}",
        sql.text
    );
    assert_eq!(2, sql.params.len());
    assert_eq!(Value::I64(18), sql.params[0]);
    assert_eq!(Value::I64(60), sql.params[1]);
}

#[test]
fn cte_params_precede_where_params() {
    // CTE 18 → 外层 age>=30 的顺序必须是 18、30
    let sql = SelectStmt::new("young")
        .cte("young", &young())
        .where_(Expr::cmp(
            BinOp::Ge,
            Expr::Col("young", "age"),
            Expr::lit(30_i64),
        ))
        .build(Dialect::Postgres);
    assert_eq!(2, sql.params.len());
    assert_eq!(Value::I64(18), sql.params[0], "CTE 参数在最前");
    assert_eq!(Value::I64(30), sql.params[1], "外层 WHERE 参数在后");
    assert!(
        sql.text.contains("$1") && sql.text.contains("$2"),
        "got: {}",
        sql.text
    );
}

#[test]
fn cte_survives_build_count() {
    // COUNT 也要带上 WITH，否则语法错误（COUNT 引用的表在 CTE 里）
    let sql = SelectStmt::new("young")
        .cte("young", &young())
        .build_count(Dialect::Sqlite);
    assert!(
        sql.text.starts_with("WITH \"young\" AS (SELECT"),
        "got: {}",
        sql.text
    );
    assert!(
        sql.text.contains("SELECT COUNT(*) FROM \"young\""),
        "got: {}",
        sql.text
    );
}

#[test]
fn cte_columns_are_qualified() {
    // 与派生表一致：引用 CTE 时列要带表名（多 CTE / 混物理表时才有歧义保护）
    let sql = SelectStmt::new("young")
        .cte("young", &young())
        .column("name")
        .where_(Expr::cmp(
            BinOp::Ge,
            Expr::Col("young", "age"),
            Expr::lit(30_i64),
        ))
        .build(Dialect::Sqlite);
    assert!(
        sql.text.contains("SELECT \"young\".\"name\""),
        "got: {}",
        sql.text
    );
    assert!(
        sql.text.contains("WHERE \"young\".\"age\""),
        "got: {}",
        sql.text
    );
}
