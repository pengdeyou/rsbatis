//! 派生表在真实 MySQL / MariaDB 上：`FROM (子查询) AS t`。
//!
//! MySQL 会对派生表做 merge 优化，某些写法下执行计划与 PG/SQLite 不同，
//! 但结果必须一致——这里断言的是结果。
//!
//! 没有 `RSBATIS_MYSQL_URL` 时直接跳过。

#![cfg(feature = "mysql")]
#![allow(dead_code)]

use rsbatis::{BinOp, Column, Entity, Expr, FromRow, MySqlDriver, SelectStmt, Session};

#[derive(Entity, FromRow, Clone, Debug, PartialEq)]
#[rsbatis(table = "v_dv_users")]
struct User {
    id: i64,
    name: String,
    age: i32,
}

fn seeded() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接 MySQL"));
    db.execute_batch(
        "DROP TABLE IF EXISTS v_dv_users;
         DROP TABLE IF EXISTS v_dv_orders;
         CREATE TABLE v_dv_users (id BIGINT PRIMARY KEY, name VARCHAR(32) NOT NULL, age INT NOT NULL) ENGINE=InnoDB;
         CREATE TABLE v_dv_orders (id BIGINT PRIMARY KEY, user_id BIGINT NOT NULL, amount BIGINT NOT NULL) ENGINE=InnoDB;
         INSERT INTO v_dv_users (id, name, age) VALUES (1, 'a', 20), (2, 'b', 30), (3, 'c', 40);
         INSERT INTO v_dv_orders (id, user_id, amount) VALUES (1, 1, 100), (2, 1, 200), (3, 3, 50);",
    )
    .expect("建表与灌数据");
    Some(db)
}

fn names(db: &mut Session<MySqlDriver>, stmt: SelectStmt) -> Vec<String> {
    db.all::<User>(&stmt.build(db.dialect()))
        .expect("查用户")
        .into_iter()
        .map(|u| u.name)
        .collect()
}

#[test]
fn derived_table_filters_twice_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let inner = SelectStmt::new("v_dv_users")
        .column("id")
        .column("name")
        .column("age")
        .filter(Column::<i32>::new("v_dv_users", "age").ge(20_i32));
    let outer = SelectStmt::from_subquery(&inner, "t").filter(Expr::cmp(
        BinOp::Ge,
        Expr::Col("t", "age"),
        Expr::lit(30_i32),
    ));
    let mut got = names(&mut db, outer);
    got.sort();
    assert_eq!(vec!["b".to_string(), "c".to_string()], got);
}

#[test]
fn derived_table_param_order_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    // 内层 20、外层 40：只有 c。参数颠倒会得到 b+c。
    let inner = SelectStmt::new("v_dv_users")
        .column("id")
        .column("name")
        .column("age")
        .filter(Column::<i32>::new("v_dv_users", "age").ge(20_i32));
    let outer = SelectStmt::from_subquery(&inner, "t").filter(Expr::cmp(
        BinOp::Ge,
        Expr::Col("t", "age"),
        Expr::lit(40_i32),
    ));
    assert_eq!(vec!["c".to_string()], names(&mut db, outer));
}

#[test]
fn derived_table_count_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let inner = SelectStmt::new("v_dv_users")
        .column("id")
        .filter(Column::<i32>::new("v_dv_users", "age").ge(20_i32));
    let count_sql = SelectStmt::from_subquery(&inner, "t").build_count(db.dialect());
    let row = db
        .first(&count_sql)
        .expect("MySQL 查询失败")
        .expect("COUNT 必须有一行");
    let n: i64 = row.get(0).expect("取计数");
    assert_eq!(3, n);
}

#[test]
fn derived_table_computed_column_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let inner = SelectStmt::new("v_dv_orders")
        .column("user_id")
        .column_expr(Expr::alias(
            Expr::arith(
                rsbatis::ArithOp::Mul,
                Expr::Col("v_dv_orders", "amount"),
                Expr::lit(2_i64),
            ),
            "double",
        ));
    let outer = SelectStmt::from_subquery(&inner, "o").filter(Expr::cmp(
        BinOp::Ge,
        Expr::Col("o", "double"),
        Expr::lit(250_i64),
    ));

    #[derive(FromRow, Debug)]
    struct UserIdOnly {
        user_id: i64,
    }
    let rows = db
        .all::<UserIdOnly>(&outer.build(db.dialect()))
        .expect("查派生表");
    assert_eq!(
        vec![1_i64],
        rows.into_iter().map(|r| r.user_id).collect::<Vec<_>>()
    );
}

#[test]
fn join_derived_table_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let sub = SelectStmt::new("v_dv_orders")
        .column("user_id")
        .column("amount")
        .filter(Column::<i64>::new("v_dv_orders", "amount").ge(150_i64));
    let stmt = SelectStmt::new("v_dv_users").column("name").join_subquery(
        rsbatis::JoinKind::Inner,
        &sub,
        "t",
        Expr::cmp(
            BinOp::Eq,
            Expr::Col("v_dv_users", "id"),
            Expr::Col("t", "user_id"),
        ),
    );

    #[derive(FromRow, Debug)]
    struct NameOnly {
        name: String,
    }
    let rows = db
        .all::<NameOnly>(&stmt.build(db.dialect()))
        .expect("JOIN 派生表查询失败");
    assert_eq!(
        vec!["a".to_string()],
        rows.into_iter().map(|r| r.name).collect::<Vec<_>>()
    );
}

#[test]
fn join_derived_table_param_order_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let sub = SelectStmt::new("v_dv_orders")
        .column("user_id")
        .column("amount")
        .filter(Column::<i64>::new("v_dv_orders", "amount").ge(150_i64));
    let stmt = SelectStmt::new("v_dv_users")
        .column("name")
        .join_subquery(
            rsbatis::JoinKind::Inner,
            &sub,
            "t",
            Expr::cmp(
                BinOp::Eq,
                Expr::Col("v_dv_users", "id"),
                Expr::Col("t", "user_id"),
            ),
        )
        .filter(Column::<i32>::new("v_dv_users", "age").ge(30_i32));

    #[derive(FromRow, Debug)]
    struct NameOnly {
        name: String,
    }
    let rows = db
        .all::<NameOnly>(&stmt.build(db.dialect()))
        .expect("查询失败");
    assert!(rows.is_empty(), "参数错位会在这里露馅");
}
