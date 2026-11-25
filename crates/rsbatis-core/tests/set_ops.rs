
//! `INTERSECT` / `EXCEPT` 渲染，以及它们与 `UNION` 混用时的优先级告警。

use rsbatis_core::{BinOp, Dialect, Expr, SelectStmt};

fn sql(s: &SelectStmt, d: Dialect) -> String {
    s.build(d).text
}

fn seg(table: &'static str, col: &'static str) -> SelectStmt {
    SelectStmt::new(table).column(col)
}

#[test]
fn intersect_keyword() {
    assert_eq!(
        r#"SELECT "x" FROM "a" INTERSECT SELECT "x" FROM "b""#,
        sql(&seg("a", "x").intersect(&seg("b", "x")), Dialect::Sqlite)
    );
}

#[test]
fn except_keyword() {
    assert_eq!(
        r#"SELECT "x" FROM "a" EXCEPT SELECT "x" FROM "b""#,
        sql(&seg("a", "x").except(&seg("b", "x")), Dialect::Sqlite)
    );
}

#[test]
fn chained_except_is_left_associative_no_warning() {
    // (A EXCEPT B) EXCEPT C 与引擎的左结合一致，不该告警
    let stmt = seg("a", "x")
        .except(&seg("b", "x"))
        .except(&seg("c", "x"));
    let s = stmt.build(Dialect::Sqlite);
    assert_eq!(
        r#"SELECT "x" FROM "a" EXCEPT SELECT "x" FROM "b" EXCEPT SELECT "x" FROM "c""#,
        s.text
    );
    assert!(s.warnings.is_empty(), "got: {:?}", s.warnings);
}

#[test]
fn intersect_first_then_union_no_warning() {
    // (A INTERSECT B) UNION C：INTERSECT 优先级高，引擎也是这个意思，不告警
    let stmt = seg("a", "x")
        .intersect(&seg("b", "x"))
        .union_all(&seg("c", "x"));
    let s = stmt.build(Dialect::Sqlite);
    assert!(s.warnings.is_empty(), "got: {:?}", s.warnings);
}

#[test]
fn union_then_intersect_warns_about_precedence() {
    // A UNION B INTERSECT C 会被解析成 A UNION (B INTERSECT C)，与左结合不同 → 必须告警
    let stmt = seg("a", "x")
        .union_all(&seg("b", "x"))
        .intersect(&seg("c", "x"));
    let s = stmt.build(Dialect::Sqlite);
    assert!(
        s.warnings.iter().any(|w| w.contains("优先级**因引擎而异**")),
        "got: {:?}",
        s.warnings
    );
}

#[test]
fn except_then_union_no_warning() {
    // EXCEPT 与 UNION 同级、左结合，与引擎一致
    let stmt = seg("a", "x")
        .except(&seg("b", "x"))
        .union_all(&seg("c", "x"));
    assert!(stmt.build(Dialect::Sqlite).warnings.is_empty());
}

#[test]
fn set_op_column_mismatch_warns() {
    let two = SelectStmt::new("b").column("x").column("y");
    let s = seg("a", "x").intersect(&two).build(Dialect::Sqlite);
    assert!(
        s.warnings.iter().any(|w| w.contains("列数必须一致")),
        "got: {:?}",
        s.warnings
    );
}

#[test]
fn build_count_wraps_set_op() {
    let stmt = seg("a", "x").except(&seg("b", "x"));
    assert_eq!(
        r#"SELECT COUNT(*) FROM (SELECT "x" FROM "a" EXCEPT SELECT "x" FROM "b") AS "__rsbatis_count""#,
        stmt.build_count(Dialect::Sqlite).text
    );
}

#[test]
fn where_on_set_op_still_warns_about_scope() {
    // 与 UNION 同一个陷阱：WHERE 只作用于第一段
    let stmt = seg("a", "x")
        .and_where(Expr::cmp(BinOp::Gt, Expr::col("a", "x"), Expr::lit(1_i64)))
        .except(&seg("b", "x"));
    assert!(
        stmt.build(Dialect::Sqlite)
            .warnings
            .iter()
            .any(|w| w.contains("只作用于")),
        "EXCEPT 的前置 WHERE 同样只在第一段生效"
    );
}

#[test]
fn mysql_backticks_for_intersect() {
    assert_eq!(
        "SELECT `x` FROM `a` INTERSECT SELECT `x` FROM `b`",
        sql(&seg("a", "x").intersect(&seg("b", "x")), Dialect::MySql)
    );
}
