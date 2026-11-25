
//! 多对多预加载在真实 MariaDB 上（含"关联表先命中再查子表"的两次往返）。

#![cfg(feature = "mysql")]

use rsbatis::{Entity, FromRow, ManyToMany, MySqlDriver, SelectStmt, Session, SortOrder};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "m2m_s")]
#[allow(dead_code)]
struct S {
    id: i64,
    name: String,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "m2m_c")]
#[allow(dead_code)]
struct C {
    id: i64,
    title: String,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "m2m_e")]
#[allow(dead_code)]
struct E {
    s_id: i64,
    c_id: i64,
}

fn seeded() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接 MySQL"));
    db.execute_batch(
        "DROP TABLE IF EXISTS m2m_e;
         DROP TABLE IF EXISTS m2m_s;
         DROP TABLE IF EXISTS m2m_c;
         CREATE TABLE m2m_s (id BIGINT NOT NULL, name VARCHAR(32) NOT NULL) ENGINE=InnoDB;
         CREATE TABLE m2m_c (id BIGINT NOT NULL, title VARCHAR(32) NOT NULL) ENGINE=InnoDB;
         CREATE TABLE m2m_e (s_id BIGINT NOT NULL, c_id BIGINT NOT NULL) ENGINE=InnoDB;
         INSERT INTO m2m_s (id, name) VALUES (1, 'A'), (2, 'B'), (3, 'C');
         INSERT INTO m2m_c (id, title) VALUES (10, 'Rust'), (11, 'SQL'), (12, 'Go');
         INSERT INTO m2m_e (s_id, c_id) VALUES (1, 10), (1, 11), (2, 11);",
    )
    .expect("建表灌数据");
    Some(db)
}

#[test]
fn many_to_many_on_mariadb() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    let rel = ManyToMany::<C>::new("m2m_e", E::col_s_id(), E::col_c_id(), C::col_id(), |c| c.id);
    let ss: Vec<S> = db
        .fetch_all(
            SelectStmt::new("m2m_s")
                .column("id")
                .column("name")
                .order_by(rsbatis::Expr::bare_col("id"), SortOrder::Asc)
                .build(db.dialect()),
        )
        .unwrap();
    let got = db.load_many_to_many(&rel, &ss, |s| s.id).unwrap();
    assert_eq!(3, got.len());
    assert_eq!(vec![10, 11], got[0].iter().map(|c| c.id).collect::<Vec<_>>());
    assert_eq!(vec![11], got[1].iter().map(|c| c.id).collect::<Vec<_>>());
    assert!(got[2].is_empty());
}

#[test]
fn orphan_link_skipped_on_mariadb() {
    let mut db = match seeded() {
        Some(d) => d,
        None => return,
    };
    db.execute_batch("INSERT INTO m2m_e (s_id, c_id) VALUES (1, 99);")
        .unwrap();
    let rel = ManyToMany::<C>::new("m2m_e", E::col_s_id(), E::col_c_id(), C::col_id(), |c| c.id);
    let ss: Vec<S> = db
        .fetch_all(SelectStmt::new("m2m_s").column("id").column("name").build(db.dialect()))
        .unwrap();
    let got = db.load_many_to_many(&rel, &ss, |s| s.id).unwrap();
    // 子集合里按 id 找 99 不存在 → 跳过，不 panic
    assert!(got.iter().flatten().all(|c| c.id != 99));
}
