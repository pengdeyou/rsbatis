//! CTE 在真实 MySQL / MariaDB 上。MariaDB 10.2+ / MySQL 8.0+ 支持 `WITH`；
//! 若数据库更老会直接报语法错——这里就是那条分界线的实证。
//!
//! 没有 `RSBATIS_MYSQL_URL` 时直接跳过。

#![cfg(feature = "mysql")]

use rsbatis::{BinOp, Column, Entity, Expr, FromRow, MySqlDriver, SelectStmt, Session};

#[derive(Entity, FromRow, Clone, Debug, PartialEq)]
#[rsbatis(table = "v_ct_users")]
struct User {
    id: i64,
    name: String,
    age: i32,
}

fn seeded() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接 MySQL"));
    db.execute_batch(
        "DROP TABLE IF EXISTS v_ct_users;
         CREATE TABLE v_ct_users (id BIGINT PRIMARY KEY, name VARCHAR(32) NOT NULL, age INT NOT NULL) ENGINE=InnoDB;
         INSERT INTO v_ct_users (id, name, age) VALUES (1, 'a', 20), (2, 'b', 30), (3, 'c', 40);",
    )
    .expect("建表与灌数据");
    Some(db)
}

fn names(db: &mut Session<MySqlDriver>, stmt: SelectStmt) -> Vec<String> {
    db.fetch_all::<User>(&stmt.build(db.dialect()))
        .expect("CTE 查询失败")
        .into_iter()
        .map(|u| u.name)
        .collect()
}

#[test]
fn cte_filters_twice_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let sub = SelectStmt::new("v_ct_users")
        .column("id")
        .column("name")
        .column("age")
        .where_(Column::<i32>::new("v_ct_users", "age").ge(20_i32));
    let stmt = SelectStmt::new("t").cte("t", &sub).where_(Expr::cmp(
        BinOp::Ge,
        Expr::Col("t", "age"),
        Expr::lit(30_i32),
    ));
    let mut got = names(&mut db, stmt);
    got.sort();
    assert_eq!(vec!["b".to_string(), "c".to_string()], got);
}

#[test]
fn cte_count_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let sub = SelectStmt::new("v_ct_users")
        .column("id")
        .where_(Column::<i32>::new("v_ct_users", "age").ge(20_i32));
    let count_sql = SelectStmt::new("t")
        .cte("t", &sub)
        .build_count(db.dialect());
    let row = db
        .fetch_one(&count_sql)
        .expect("COUNT 查询失败")
        .expect("必须返回一行");
    let n: i64 = row.get(0).expect("取计数");
    assert_eq!(3, n);
}

#[test]
fn cte_param_order_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let sub = SelectStmt::new("v_ct_users")
        .column("id")
        .column("name")
        .column("age")
        .where_(Column::<i32>::new("v_ct_users", "age").ge(20_i32));
    let stmt = SelectStmt::new("t").cte("t", &sub).where_(Expr::cmp(
        BinOp::Ge,
        Expr::Col("t", "age"),
        Expr::lit(40_i32),
    ));
    assert_eq!(vec!["c".to_string()], names(&mut db, stmt));
}
