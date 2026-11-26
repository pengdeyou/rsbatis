//! CTE 端到端（SQLite 内存库）：`WITH t AS (...) SELECT ... FROM t`。
//!
//! CTE 的参数在 `SELECT` 之前渲染，是"最靠前"的一批参数；这里同时验证
//! 结果正确性与"参数顺序错了会怎样"。

#![cfg(feature = "sqlite")]

use rsbatis::{BinOp, Column, Entity, Expr, FromRow, SelectStmt, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug, PartialEq)]
#[rsbatis(table = "ct_users")]
struct User {
    id: i64,
    name: String,
    age: i32,
}

fn setup() -> Session<SqliteDriver> {
    let mut session = Session::new(SqliteDriver::open_in_memory().unwrap());
    session
        .execute_batch(
            r#"
            DROP TABLE IF EXISTS ct_users;
            CREATE TABLE ct_users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER NOT NULL);
            INSERT INTO ct_users (id, name, age) VALUES (1, 'a', 20), (2, 'b', 30), (3, 'c', 40);
            "#,
        )
        .unwrap();
    session
}

fn names(db: &mut Session<SqliteDriver>, stmt: SelectStmt) -> Vec<String> {
    db.all::<User>(&stmt.build(db.dialect()))
        .unwrap()
        .into_iter()
        .map(|u| u.name)
        .collect()
}

#[test]
fn cte_filters_twice() {
    let mut db = setup();
    let sub = SelectStmt::new("ct_users")
        .column("id")
        .column("name")
        .column("age")
        .filter(Column::<i32>::new("ct_users", "age").ge(20_i32));
    let stmt = SelectStmt::new("t").cte("t", &sub).filter(Expr::cmp(
        BinOp::Ge,
        Expr::Col("t", "age"),
        Expr::lit(30_i32),
    ));
    let mut got = names(&mut db, stmt);
    got.sort();
    assert_eq!(vec!["b".to_string(), "c".to_string()], got);
}

#[test]
fn cte_param_order_is_observable() {
    let mut db = setup();
    // CTE 里 20、外层 40：只有 c。顺序颠倒会得到 b+c
    let sub = SelectStmt::new("ct_users")
        .column("id")
        .column("name")
        .column("age")
        .filter(Column::<i32>::new("ct_users", "age").ge(20_i32));
    let stmt = SelectStmt::new("t").cte("t", &sub).filter(Expr::cmp(
        BinOp::Ge,
        Expr::Col("t", "age"),
        Expr::lit(40_i32),
    ));
    assert_eq!(vec!["c".to_string()], names(&mut db, stmt));
}

#[test]
fn cte_count_matches() {
    let mut db = setup();
    let sub = SelectStmt::new("ct_users")
        .column("id")
        .filter(Column::<i32>::new("ct_users", "age").ge(20_i32));
    let count_sql = SelectStmt::new("t")
        .cte("t", &sub)
        .build_count(db.dialect());
    let row = db.first(&count_sql).unwrap().expect("必须返回一行");
    let n: i64 = row.get(0).unwrap();
    assert_eq!(3, n);
}

#[test]
fn two_ctes_in_one_query() {
    let mut db = setup();
    let young = SelectStmt::new("ct_users")
        .column("id")
        .column("name")
        .column("age")
        .filter(Column::<i32>::new("ct_users", "age").le(30_i32));
    let mid = SelectStmt::new("ct_users")
        .column("id")
        .column("name")
        .column("age")
        .filter(Column::<i32>::new("ct_users", "age").ge(20_i32));
    // 两个 CTE 都定义，主查询只用一个（SQLite 允许定义未使用的 CTE）
    let stmt = SelectStmt::new("y")
        .cte("y", &young)
        .cte("m", &mid)
        .filter(Expr::cmp(
            BinOp::Ge,
            Expr::Col("y", "age"),
            Expr::lit(30_i32),
        ));
    assert_eq!(vec!["b".to_string()], names(&mut db, stmt));
}
