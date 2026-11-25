//! `UNION` / `UNION ALL` 在真实 MariaDB 上。
//!
//! 没有 `RSBATIS_MYSQL_URL` 时直接跳过。

#![cfg(feature = "mysql")]

use rsbatis::{Entity, Expr, FromRow, MySqlDriver, SelectStmt, Session, SortOrder};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "v_ua")]
#[allow(dead_code)] // 只用到 TABLE 元数据
struct A {
    v: i64,
}

fn seeded() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接 MySQL"));
    db.execute_batch(
        "DROP TABLE IF EXISTS v_ua;
         DROP TABLE IF EXISTS v_ub;
         CREATE TABLE v_ua (v BIGINT NOT NULL) ENGINE=InnoDB;
         CREATE TABLE v_ub (v BIGINT NOT NULL) ENGINE=InnoDB;
         INSERT INTO v_ua (v) VALUES (1), (2), (3);
         INSERT INTO v_ub (v) VALUES (3), (4), (5);",
    )
    .expect("建表与灌数据");
    Some(db)
}

fn vals(db: &mut Session<MySqlDriver>, stmt: &SelectStmt) -> Vec<i64> {
    let rows: Vec<A> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    rows.into_iter().map(|r| r.v).collect()
}

#[test]
fn union_all_then_distinct_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let all = SelectStmt::new("v_ua")
        .column("v")
        .union_all(&SelectStmt::new("v_ub").column("v"));
    let mut got = vals(&mut db, &all);
    got.sort();
    assert_eq!(vec![1, 2, 3, 3, 4, 5], got);

    let distinct = SelectStmt::new("v_ua")
        .column("v")
        .union(&SelectStmt::new("v_ub").column("v"));
    let mut got2 = vals(&mut db, &distinct);
    got2.sort();
    assert_eq!(vec![1, 2, 3, 4, 5], got2);
}

#[test]
fn count_and_limit_over_union_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let all = SelectStmt::new("v_ua")
        .column("v")
        .union_all(&SelectStmt::new("v_ub").column("v"));
    let total: i64 = db
        .fetch_one(all.build_count(db.dialect()))
        .unwrap()
        .unwrap()
        .get::<i64>(0)
        .unwrap();
    assert_eq!(6, total);

    let limited = SelectStmt::new("v_ua")
        .column("v")
        .union_all(&SelectStmt::new("v_ub").column("v"))
        .order_by(Expr::col("v_ua", "v"), SortOrder::Asc)
        .limit(3);
    let got = vals(&mut db, &limited);
    assert_eq!(vec![1, 2, 3], got);
}

#[test]
fn union_as_derived_table_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let union = SelectStmt::new("v_ua")
        .column("v")
        .union_all(&SelectStmt::new("v_ub").column("v"));
    let stmt = SelectStmt::from_subquery(&union, "t")
        .column("v")
        .column_expr(Expr::count_star().as_("cnt"))
        .group_by(Expr::col("t", "v"))
        .order_by(Expr::col("t", "v"), SortOrder::Asc);
    #[derive(FromRow, Debug)]
    struct Row {
        v: i64,
        cnt: i64,
    }
    let rows: Vec<Row> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    let got: Vec<(i64, i64)> = rows.into_iter().map(|r| (r.v, r.cnt)).collect();
    assert_eq!(vec![(1, 1), (2, 1), (3, 2), (4, 1), (5, 1)], got);
}
