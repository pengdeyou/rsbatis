
//! `INTERSECT` / `EXCEPT` 在真实 MariaDB 上。
//!
//! 关键：MariaDB 遵循 SQL 标准，INTERSECT 优先级**高于** UNION，
//! 所以 SQLite 与 MariaDB 对同一条混合查询会给出不同结果（见 e2e 对照）。

#![cfg(feature = "mysql")]

use rsbatis::{Entity, FromRow, MySqlDriver, SelectStmt, Session, SortOrder};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "v_s_a")]
#[allow(dead_code)]
struct A {
    v: i64,
}

fn seeded() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接 MySQL"));
    db.execute_batch(
        "DROP TABLE IF EXISTS v_s_a;
         DROP TABLE IF EXISTS v_s_b;
         DROP TABLE IF EXISTS v_s_c;
         CREATE TABLE v_s_a (v BIGINT NOT NULL) ENGINE=InnoDB;
         CREATE TABLE v_s_b (v BIGINT NOT NULL) ENGINE=InnoDB;
         CREATE TABLE v_s_c (v BIGINT NOT NULL) ENGINE=InnoDB;
         INSERT INTO v_s_a (v) VALUES (1), (2), (3), (4);
         INSERT INTO v_s_b (v) VALUES (2), (3), (5);
         INSERT INTO v_s_c (v) VALUES (3), (6);",
    )
    .expect("建表与灌数据");
    Some(db)
}

#[derive(FromRow, Debug)]
struct V {
    v: i64,
}

fn vals(db: &mut Session<MySqlDriver>, stmt: &SelectStmt) -> Vec<i64> {
    let mut rows: Vec<V> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    rows.sort_by_key(|r| r.v);
    rows.into_iter().map(|r| r.v).collect()
}

#[test]
fn intersect_and_except_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let inter = SelectStmt::new("v_s_a")
        .column("v")
        .intersect(&SelectStmt::new("v_s_b").column("v"));
    assert_eq!(vec![2, 3], vals(&mut db, &inter));

    let diff = SelectStmt::new("v_s_a")
        .column("v")
        .except(&SelectStmt::new("v_s_b").column("v"));
    assert_eq!(vec![1, 4], vals(&mut db, &diff));
}

#[test]
fn mixed_union_intersect_differs_from_sqlite_on_mariadb() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    // MariaDB 遵循标准：A UNION ALL (B INTERSECT C)
    //   B ∩ C = 3；A UNION ALL {3} 交付 1,2,3,4 再加上额外的 3 → 1,2,3,3,4
    // SQLite 上同一条 SQL 是 (A ∪ B) ∩ C = 3（INTERSECT 去重）—— 两边结果天差地别
    let stmt = SelectStmt::new("v_s_a")
        .column("v")
        .union_all(&SelectStmt::new("v_s_b").column("v"))
        .intersect(&SelectStmt::new("v_s_c").column("v"));
    let got = vals(&mut db, &stmt);
    assert_eq!(
        vec![1, 2, 3, 3, 4],
        got,
        "MariaDB 是 INTERSECT 优先且 UNION ALL 保留重复；同一 SQL 在 SQLite 上只返回 3"
    );
    assert!(
        stmt.build(db.dialect())
            .warnings
            .iter()
            .any(|w| w.contains("因引擎而异")),
        "跨引擎差异必须告警"
    );
}

#[test]
fn intersect_first_matches_on_both_engines() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    // INTERSECT 在前时两家结果一致：(A ∩ B) ∪ C = {2,3} ∪ {3,6}（UNION ALL）
    let stmt = SelectStmt::new("v_s_a")
        .column("v")
        .intersect(&SelectStmt::new("v_s_b").column("v"))
        .union_all(&SelectStmt::new("v_s_c").column("v"))
        .order_by(rsbatis::Expr::bare_col("v"), SortOrder::Asc);
    assert_eq!(vec![2, 3, 3, 6], vals(&mut db, &stmt));
}

#[test]
fn count_except_mysql() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let stmt = SelectStmt::new("v_s_a")
        .column("v")
        .except(&SelectStmt::new("v_s_b").column("v"));
    let total: i64 = db
        .fetch_one(stmt.build_count(db.dialect()))
        .unwrap()
        .unwrap()
        .get::<i64>(0)
        .unwrap();
    assert_eq!(2, total);
}
