//! `UNION` / `UNION ALL` 在真实 SQLite 上。
//!
//! 重点不是 SQL 文本，而是"去重 vs 不去重"和"整条复合查询的 count / limit"这三件会被
//! 静默做错的事。

#![cfg(feature = "sqlite")]

use rsbatis::{BinOp, Entity, Expr, FromRow, SelectStmt, Session, SortOrder, SqliteDriver};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "u_a")]
struct A {
    v: i64,
}

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "u_b")]
struct B {
    v: i64,
}

fn seeded() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE u_a (v INTEGER NOT NULL);
         CREATE TABLE u_b (v INTEGER NOT NULL);
         INSERT INTO u_a (v) VALUES (1), (2), (3);
         INSERT INTO u_b (v) VALUES (3), (4), (5);",
    )
    .unwrap();
    db
}

fn vals(db: &mut Session<SqliteDriver>, stmt: &SelectStmt) -> Vec<i64> {
    let rows: Vec<A> = db.all(stmt.build(db.dialect())).unwrap();
    rows.into_iter().map(|r| r.v).collect()
}

#[test]
fn union_all_keeps_duplicates() {
    let mut db = seeded();
    let stmt = SelectStmt::new("u_a")
        .column("v")
        .union_all(&SelectStmt::new("u_b").column("v"));
    let mut got = vals(&mut db, &stmt);
    got.sort();
    // 3 重复值都保留 → 6 行
    assert_eq!(vec![1, 2, 3, 3, 4, 5], got);
}

#[test]
fn union_dedups() {
    let mut db = seeded();
    let stmt = SelectStmt::new("u_a")
        .column("v")
        .union(&SelectStmt::new("u_b").column("v"));
    let mut got = vals(&mut db, &stmt);
    got.sort();
    // 3 被去掉一份 → 5 行
    assert_eq!(vec![1, 2, 3, 4, 5], got);
}

#[test]
fn count_of_union_is_total_rows() {
    let mut db = seeded();
    let stmt = SelectStmt::new("u_a")
        .column("v")
        .union_all(&SelectStmt::new("u_b").column("v"));
    let c = stmt.build_count(db.dialect());
    let total: i64 = db.first(c).unwrap().unwrap().get::<i64>(0).unwrap();
    assert_eq!(6, total, "UNION ALL 的总行数必须是两段之和");
}

#[test]
fn count_of_union_distinct_counts_once() {
    let mut db = seeded();
    let stmt = SelectStmt::new("u_a")
        .column("v")
        .union(&SelectStmt::new("u_b").column("v"));
    let c = stmt.build_count(db.dialect());
    let total: i64 = db.first(c).unwrap().unwrap().get::<i64>(0).unwrap();
    assert_eq!(5, total, "UNION 去重后是 5 行");
}

#[test]
fn order_and_limit_apply_to_whole_union() {
    let mut db = seeded();
    let stmt = SelectStmt::new("u_a")
        .column("v")
        .union_all(&SelectStmt::new("u_b").column("v"))
        .order_by(Expr::col("u_a", "v"), SortOrder::Asc)
        .limit(3);
    let got = vals(&mut db, &stmt);
    // 全局升序取前三：1,2,3（不是"每段各取前三"）
    assert_eq!(vec![1, 2, 3], got);
    assert_eq!(3, got.len());
}

#[test]
fn filteron_each_segment_filters_independently() {
    let mut db = seeded();
    let left = SelectStmt::new("u_a").column("v").and_filter(Expr::cmp(
        BinOp::Lt,
        Expr::col("u_a", "v"),
        Expr::lit(3_i64),
    ));
    let right = SelectStmt::new("u_b").column("v").and_filter(Expr::cmp(
        BinOp::Gt,
        Expr::col("u_b", "v"),
        Expr::lit(3_i64),
    ));
    let stmt = left.union_all(&right);
    let mut got = vals(&mut db, &stmt);
    got.sort();
    assert_eq!(vec![1, 2, 4, 5], got);
}

#[test]
fn union_used_as_derived_table_source() {
    // UNION 结果作为派生表再聚合：这是"合并两来源后统一统计"的常见写法
    let mut db = seeded();
    let union = SelectStmt::new("u_a")
        .column("v")
        .union_all(&SelectStmt::new("u_b").column("v"));
    let stmt = SelectStmt::from_subquery(&union, "t")
        .column("v")
        .column_expr(Expr::count_star().as_("cnt"))
        .group_by(Expr::col("t", "v"));
    #[derive(FromRow, Debug)]
    struct Row {
        v: i64,
        cnt: i64,
    }
    let mut rows: Vec<Row> = db.all(stmt.build(db.dialect())).unwrap();
    rows.sort_by_key(|r| r.v);
    let got: Vec<(i64, i64)> = rows.into_iter().map(|r| (r.v, r.cnt)).collect();
    // 3 出现两次
    assert_eq!(vec![(1, 1), (2, 1), (3, 2), (4, 1), (5, 1)], got);
}
