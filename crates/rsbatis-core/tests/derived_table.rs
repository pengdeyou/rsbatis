//! 派生表（`FROM (子查询) AS 别名`）的渲染契约。
//!
//! 关键正确性点：派生表在 SQL 文本里出现在 `WHERE` **之前**，所以它的参数必须
//! 先入参数列表。顺序错了在 SQLite 上表现为"类型不匹配"或"查不到数据"。

use rsbatis_core::{BinOp, Dialect, Expr, SelectStmt, Value};

fn inner() -> SelectStmt {
    SelectStmt::new("users")
        .column("id")
        .column("name")
        .where_(Expr::cmp(
            BinOp::Ge,
            Expr::Col("users", "age"),
            Expr::lit(20_i64),
        ))
}

#[test]
fn derived_table_renders_with_alias() {
    let sql = SelectStmt::from_subquery(&inner(), "t")
        .column("name")
        .build(Dialect::Sqlite);
    assert_eq!(
        "SELECT \"t\".\"name\" FROM (SELECT \"users\".\"id\", \"users\".\"name\" \
         FROM \"users\" WHERE \"users\".\"age\" >= ?) AS \"t\"",
        sql.text
    );
}

#[test]
fn derived_table_params_come_before_where_params() {
    // 子查询 age>=20 在前、外层 t.age>=30 在后：参数顺序必须是 20, 30
    let sql = SelectStmt::from_subquery(&inner(), "t")
        .column("name")
        .where_(Expr::cmp(
            BinOp::Ge,
            Expr::Col("t", "age"),
            Expr::lit(30_i64),
        ))
        .build(Dialect::Postgres);
    assert!(sql.text.contains("$1"), "子查询参数应是 $1：{}", sql.text);
    assert!(sql.text.contains("$2"), "外层参数应是 $2：{}", sql.text);
    assert_eq!(2, sql.params.len());
    assert_eq!(Value::I64(20), sql.params[0], "第一个参数来自派生表内部");
    assert_eq!(Value::I64(30), sql.params[1], "第二个参数来自外层 WHERE");
}

#[test]
fn derived_table_projects_star_when_columns_empty() {
    // 派生表的列集合由子查询决定，这里没有元数据，必须给 `*` 而不是拼空列表
    let sql = SelectStmt::from_subquery(&inner(), "t").build(Dialect::Sqlite);
    assert!(
        sql.text.starts_with("SELECT * FROM (SELECT"),
        "got: {}",
        sql.text
    );
}

#[test]
fn derived_table_count() {
    let sql = SelectStmt::from_subquery(&inner(), "t").build_count(Dialect::Sqlite);
    assert_eq!(
        "SELECT COUNT(*) FROM (SELECT \"users\".\"id\", \"users\".\"name\" \
         FROM \"users\" WHERE \"users\".\"age\" >= ?) AS \"t\"",
        sql.text
    );
    assert_eq!(1, sql.params.len());
}

#[test]
fn derived_table_keeps_expression_columns() {
    // 派生表里的计算列：外层用别名引用它
    let sub = SelectStmt::new("orders")
        .column("user_id")
        .column_expr(Expr::alias(
            Expr::arith(
                rsbatis_core::ArithOp::Mul,
                Expr::Col("orders", "amount"),
                Expr::lit(2_i64),
            ),
            "double",
        ));
    let sql = SelectStmt::from_subquery(&sub, "o")
        .column("user_id")
        .column("double")
        .where_(Expr::cmp(
            BinOp::Ge,
            Expr::Col("o", "double"),
            Expr::lit(500_i64),
        ))
        .build(Dialect::Sqlite);
    assert_eq!(
        "SELECT \"o\".\"user_id\", \"o\".\"double\" FROM \
         (SELECT \"orders\".\"user_id\", (\"orders\".\"amount\" * ?) AS \"double\" FROM \"orders\") AS \"o\" \
         WHERE \"o\".\"double\" >= ?",
        sql.text
    );
    assert_eq!(2, sql.params.len());
    assert_eq!(Value::I64(2), sql.params[0]);
    assert_eq!(Value::I64(500), sql.params[1]);
}

// `JOIN 派生表`（`JOIN (子查询) AS x`）需要给 `join` 开一个"表名直给"的入口，
// 当前 `join<E: Entity>` 的表名来自实体元数据。这一项留到下一轮，与命名子查询一起做。

#[test]
fn join_subquery_renders_derived_table() {
    let sub = SelectStmt::new("orders")
        .column("user_id")
        .column("amount")
        .where_(Expr::cmp(
            BinOp::Ge,
            Expr::Col("orders", "amount"),
            Expr::lit(150_i64),
        ));
    let sql = SelectStmt::new("users")
        .column("name")
        .join_subquery(
            rsbatis_core::JoinKind::Inner,
            &sub,
            "t",
            Expr::cmp(
                BinOp::Eq,
                Expr::Col("users", "id"),
                Expr::Col("t", "user_id"),
            ),
        )
        .build(Dialect::Sqlite);
    assert_eq!(
        "SELECT \"users\".\"name\" FROM \"users\" INNER JOIN \
         (SELECT \"orders\".\"user_id\", \"orders\".\"amount\" FROM \"orders\" \
         WHERE \"orders\".\"amount\" >= ?) AS \"t\" ON \"users\".\"id\" = \"t\".\"user_id\"",
        sql.text
    );
    assert_eq!(1, sql.params.len());
}

#[test]
fn join_subquery_params_precede_where_params() {
    // SQL 文本顺序是 FROM → JOIN → WHERE，参数必须按这个顺序入列表
    let sub = SelectStmt::new("orders")
        .column("user_id")
        .where_(Expr::cmp(
            BinOp::Ge,
            Expr::Col("orders", "amount"),
            Expr::lit(150_i64),
        ));
    let sql = SelectStmt::new("users")
        .column("name")
        .join_subquery(
            rsbatis_core::JoinKind::Inner,
            &sub,
            "t",
            Expr::cmp(
                BinOp::Eq,
                Expr::Col("users", "id"),
                Expr::Col("t", "user_id"),
            ),
        )
        .where_(Expr::cmp(
            BinOp::Ge,
            Expr::Col("users", "age"),
            Expr::lit(20_i64),
        ))
        .build(Dialect::Postgres);
    assert_eq!(2, sql.params.len());
    assert_eq!(Value::I64(150), sql.params[0], "JOIN 里的参数在前");
    assert_eq!(Value::I64(20), sql.params[1], "WHERE 里的参数在后");
    assert!(sql.text.contains("$1") && sql.text.contains("$2"));
}
