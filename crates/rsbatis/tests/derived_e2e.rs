//! 派生表端到端（SQLite 内存库）：`FROM (子查询) AS t` 在真实数据上是否给出正确答案。
//!
//! 渲染测试断言的是 SQL 文本；这里断言的是**结果**。参数顺序错了的话，
//! SQL 往往仍然"能跑"，只是把 20 传给了外层的 30，结果错得很安静。

#![cfg(feature = "sqlite")]

use rsbatis::{BinOp, Column, Entity, Expr, FromRow, SelectStmt, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug, PartialEq)]
#[rsbatis(table = "dv_users")]
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
            DROP TABLE IF EXISTS dv_users;
            DROP TABLE IF EXISTS dv_orders;
            CREATE TABLE dv_users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER NOT NULL);
            CREATE TABLE dv_orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount INTEGER NOT NULL);
            INSERT INTO dv_users (id, name, age) VALUES (1, 'a', 20), (2, 'b', 30), (3, 'c', 40);
            INSERT INTO dv_orders (id, user_id, amount) VALUES (1, 1, 100), (2, 1, 200), (3, 3, 50);
            "#,
        )
        .unwrap();
    session
}

fn names(session: &mut Session<SqliteDriver>, stmt: SelectStmt) -> Vec<String> {
    session
        .fetch_all::<User>(&stmt.build(session.dialect()))
        .unwrap()
        .into_iter()
        .map(|u| u.name)
        .collect()
}

#[test]
fn derived_table_filters_twice() {
    let mut db = setup();
    // 内层 age >= 20（全部），外层 t.age >= 30（b、c）
    let inner = SelectStmt::new("dv_users")
        .column("id")
        .column("name")
        .column("age")
        .where_(Column::<i32>::new("dv_users", "age").ge(20_i32));
    let outer = SelectStmt::from_subquery(&inner, "t").where_(Expr::cmp(
        BinOp::Ge,
        Expr::Col("t", "age"),
        Expr::lit(30_i32),
    ));
    let mut got = names(&mut db, outer);
    got.sort();
    assert_eq!(vec!["b".to_string(), "c".to_string()], got);
}

#[test]
fn derived_table_param_order_is_observable() {
    let mut db = setup();
    // 内层 age >= 20、外层 t.age >= 40：只有 c。
    // 若两个参数顺序颠倒（40 传给内层、20 给外层），结果会变成 b+c —— 这个断言能抓到它。
    let inner = SelectStmt::new("dv_users")
        .column("id")
        .column("name")
        .column("age")
        .where_(Column::<i32>::new("dv_users", "age").ge(20_i32));
    let outer = SelectStmt::from_subquery(&inner, "t").where_(Expr::cmp(
        BinOp::Ge,
        Expr::Col("t", "age"),
        Expr::lit(40_i32),
    ));
    assert_eq!(vec!["c".to_string()], names(&mut db, outer));
}

#[test]
fn derived_table_with_computed_column() {
    let mut db = setup();
    // 派生表里算 amount * 2 命名为 double，外层按 double >= 250 过滤：
    // 用户1 有 100、200 → double 200、400；用户3 有 50 → 100。满足 >=250 的只有用户1的那笔。
    let inner = SelectStmt::new("dv_orders")
        .column("user_id")
        .column_expr(Expr::alias(
            Expr::arith(
                rsbatis::ArithOp::Mul,
                Expr::Col("dv_orders", "amount"),
                Expr::lit(2_i64),
            ),
            "double",
        ));
    let outer = SelectStmt::from_subquery(&inner, "o").where_(Expr::cmp(
        BinOp::Ge,
        Expr::Col("o", "double"),
        Expr::lit(250_i64),
    ));

    // 这里映射的不是 User，所以单独取一列
    let rows = db
        .fetch_all::<UserIdOnly>(&outer.build(db.dialect()))
        .unwrap();
    assert_eq!(
        vec![1_i64],
        rows.into_iter().map(|r| r.user_id).collect::<Vec<_>>()
    );
}

#[derive(FromRow, Debug)]
struct UserIdOnly {
    user_id: i64,
}

#[test]
fn derived_table_count_matches() {
    let mut db = setup();
    let inner = SelectStmt::new("dv_users")
        .column("id")
        .where_(Column::<i32>::new("dv_users", "age").ge(20_i32));
    let count_sql = SelectStmt::from_subquery(&inner, "t").build_count(db.dialect());
    let row = db
        .fetch_one(&count_sql)
        .unwrap()
        .expect("COUNT 必须返回一行");
    let n: i64 = row.get(0).unwrap();
    assert_eq!(3, n);
}

#[test]
fn derived_table_projects_star_and_maps() {
    let mut db = setup();
    // 外层不给列 → `SELECT *`，FromRow 按列名映射仍然正确
    let inner = SelectStmt::new("dv_users")
        .column("id")
        .column("name")
        .column("age")
        .where_(Column::<i32>::new("dv_users", "age").ge(30_i32));
    let mut got = names(&mut db, SelectStmt::from_subquery(&inner, "t"));
    got.sort();
    assert_eq!(vec!["b".to_string(), "c".to_string()], got);
}

#[derive(FromRow, Debug)]
struct NameOnly {
    name: String,
}

#[test]
fn join_derived_table_returns_joined_rows() {
    let mut db = setup();
    // 订单里金额 >= 150 的那笔（user 1 的 200），再连回用户表取名字
    let sub = SelectStmt::new("dv_orders")
        .column("user_id")
        .column("amount")
        .where_(Column::<i64>::new("dv_orders", "amount").ge(150_i64));
    let stmt = SelectStmt::new("dv_users").column("name").join_subquery(
        rsbatis::JoinKind::Inner,
        &sub,
        "t",
        Expr::cmp(
            BinOp::Eq,
            Expr::Col("dv_users", "id"),
            Expr::Col("t", "user_id"),
        ),
    );
    let rows = db.fetch_all::<NameOnly>(&stmt.build(db.dialect())).unwrap();
    assert_eq!(
        vec!["a".to_string()],
        rows.into_iter().map(|r| r.name).collect::<Vec<_>>()
    );
}

#[test]
fn join_derived_table_param_order_is_observable() {
    let mut db = setup();
    // JOIN 里的 150 + WHERE 里的 30。顺序颠倒会把 30 用作金额下限、150 用作年龄下限 →
    // 结果从"a"变成"b"或空。这个断言专门抓参数顺序。
    let sub = SelectStmt::new("dv_orders")
        .column("user_id")
        .column("amount")
        .where_(Column::<i64>::new("dv_orders", "amount").ge(150_i64));
    let stmt = SelectStmt::new("dv_users")
        .column("name")
        .join_subquery(
            rsbatis::JoinKind::Inner,
            &sub,
            "t",
            Expr::cmp(
                BinOp::Eq,
                Expr::Col("dv_users", "id"),
                Expr::Col("t", "user_id"),
            ),
        )
        .where_(Column::<i32>::new("dv_users", "age").ge(30_i32));
    // 金额 >=150 只有 user1（age=20），再要求 age>=30 → 空集
    let rows = db.fetch_all::<NameOnly>(&stmt.build(db.dialect())).unwrap();
    assert!(rows.is_empty(), "参数错位会在这里露馅");
}
