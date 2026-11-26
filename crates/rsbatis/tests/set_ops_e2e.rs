
//! `INTERSECT` / `EXCEPT` 在真实 SQLite 上，并验证混用优先级确实是引擎语义。

#![cfg(feature = "sqlite")]

use rsbatis::{BinOp, Entity, Expr, FromRow, SelectStmt, Session, SortOrder, SqliteDriver};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "s_a")]
#[allow(dead_code)]
struct A {
    v: i64,
}

fn seeded() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE s_a (v INTEGER NOT NULL);
         CREATE TABLE s_b (v INTEGER NOT NULL);
         CREATE TABLE s_c (v INTEGER NOT NULL);
         INSERT INTO s_a (v) VALUES (1), (2), (3), (4);
         INSERT INTO s_b (v) VALUES (2), (3), (5);
         INSERT INTO s_c (v) VALUES (3), (6);",
    )
    .unwrap();
    db
}

#[derive(FromRow, Debug)]
struct V {
    v: i64,
}

fn vals(db: &mut Session<SqliteDriver>, stmt: &SelectStmt) -> Vec<i64> {
    let mut rows: Vec<V> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    rows.sort_by_key(|r| r.v);
    rows.into_iter().map(|r| r.v).collect()
}

#[test]
fn intersect_returns_common_rows() {
    let mut db = seeded();
    // s_a = {1,2,3,4}，s_b = {2,3,5} → 交集 {2,3}
    let stmt = SelectStmt::new("s_a")
        .column("v")
        .intersect(&SelectStmt::new("s_b").column("v"));
    assert_eq!(vec![2, 3], vals(&mut db, &stmt));
}

#[test]
fn except_returns_left_minus_right() {
    let mut db = seeded();
    // s_a - s_b = {1,2,3,4} - {2,3,5} = {1,4}
    let stmt = SelectStmt::new("s_a")
        .column("v")
        .except(&SelectStmt::new("s_b").column("v"));
    assert_eq!(vec![1, 4], vals(&mut db, &stmt));
}

#[test]
fn chained_except_matches_left_associative() {
    let mut db = seeded();
    // (s_a - s_b) - s_c = {1,4} - {3,6} = {1,4}
    let stmt = SelectStmt::new("s_a")
        .column("v")
        .except(&SelectStmt::new("s_b").column("v"))
        .except(&SelectStmt::new("s_c").column("v"));
    assert_eq!(vec![1, 4], vals(&mut db, &stmt));
}

#[test]
fn union_then_intersect_differs_between_engines() {
    let mut db = seeded();
    // ⚠️ SQLite 与 MySQL/MariaDB 对这三个运算符的结合规则**不同**：
    //   SQLite：UNION / INTERSECT / EXCEPT 同级、左结合 → (A UNION B) INTERSECT C = {3}
    //   MariaDB：遵循标准，INTERSECT 更高             → A UNION (B INTERSECT C) = {1,2,3,4}
    // 同一条 SQL 在两个引擎上结果不同，所以 build() 必须告警。
    let stmt = SelectStmt::new("s_a")
        .column("v")
        .union_all(&SelectStmt::new("s_b").column("v"))
        .intersect(&SelectStmt::new("s_c").column("v"));
    let got = vals(&mut db, &stmt);
    assert_eq!(
        vec![3],
        got,
        "SQLite 是左结合：A∪B 有 1,2,3,4,5，再与 C 的 3,6 求交，只剩 3"
    );
    assert!(
        stmt.build(db.dialect())
            .warnings
            .iter()
            .any(|w| w.contains("优先级**因引擎而异**")),
        "跨引擎语义差异必须告警"
    );
}

#[test]
fn intersect_then_union_matches_left_associative() {
    let mut db = seeded();
    // (A ∩ B) ∪ C = {2,3} ∪ {3,6} = {2,3,6}
    let stmt = SelectStmt::new("s_a")
        .column("v")
        .intersect(&SelectStmt::new("s_b").column("v"))
        .union_all(&SelectStmt::new("s_c").column("v"));
    // 用的是 UNION ALL，所以 C 里的 3 与 A∩B 的 3 各保留一份
    assert_eq!(vec![2, 3, 3, 6], vals(&mut db, &stmt));
}

#[test]
fn except_with_filteron_first_segment() {
    let mut db = seeded();
    let left = SelectStmt::new("s_a")
        .column("v")
        .and_filter(Expr::cmp(BinOp::Lt, Expr::col("s_a", "v"), Expr::lit(4_i64)));
    let stmt = left
        .except(&SelectStmt::new("s_b").column("v"))
        .order_by(Expr::bare_col("v"), SortOrder::Asc);
    // s_a 里 < 4 的是 {1,2,3}，减去 s_b{2,3,5} → {1}
    assert_eq!(vec![1], vals(&mut db, &stmt));
}

#[test]
fn count_of_except_is_rows_after_difference() {
    let mut db = seeded();
    let stmt = SelectStmt::new("s_a")
        .column("v")
        .except(&SelectStmt::new("s_b").column("v"));
    let c = stmt.build_count(db.dialect());
    let total: i64 = db
        .fetch_one(c)
        .unwrap()
        .unwrap()
        .get::<i64>(0)
        .unwrap();
    assert_eq!(2, total, "差集是 1 与 4 两行");
}
