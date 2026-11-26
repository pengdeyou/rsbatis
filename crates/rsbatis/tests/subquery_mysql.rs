//! 子查询在真实 MySQL / MariaDB 上：`IN` / `NOT IN` / `EXISTS`。
//!
//! MySQL 上有两件 SQLite 不会暴露的事，这里专门守：
//! 1. `NOT IN` 遇 NULL 会整条变 NULL（MySQL 语义更"严格"，不返回任何行）——
//!    这里断言的是**当前实现的行为**，同时说明这个坑存在，不假装它不存在。
//! 2. `EXISTS` 相关子查询的列解析必须落到正确的表。
//!
//! 没有 `RSBATIS_MYSQL_URL` 时直接跳过。

#![cfg(feature = "mysql")]
#![allow(dead_code)]

use rsbatis::{Column, Entity, Expr, FromRow, InsertStmt, MySqlDriver, Order, SelectStmt, Session};

#[derive(Entity, FromRow, Clone, Debug, PartialEq)]
#[rsbatis(table = "v_sq_users")]
struct User {
    id: i64,
    name: String,
    age: i32,
}

fn url() -> Option<String> {
    std::env::var("RSBATIS_MYSQL_URL").ok()
}

fn seeded() -> Option<Session<MySqlDriver>> {
    let url = url()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接 MySQL"));
    db.execute_batch(
        "DROP TABLE IF EXISTS v_sq_users;
         DROP TABLE IF EXISTS v_sq_orders;
         CREATE TABLE v_sq_users (id BIGINT PRIMARY KEY, name VARCHAR(32) NOT NULL, age INT NOT NULL) ENGINE=InnoDB;
         CREATE TABLE v_sq_orders (id BIGINT PRIMARY KEY, user_id BIGINT NOT NULL, amount BIGINT NOT NULL) ENGINE=InnoDB;",
    )
    .expect("建表");

    for (id, name, age) in [(1_i64, "a", 20_i32), (2, "b", 30), (3, "c", 40)] {
        db.execute(
            InsertStmt::new("v_sq_users")
                .set(Column::<i64>::new("v_sq_users", "id"), id)
                .set(
                    Column::<String>::new("v_sq_users", "name"),
                    name.to_string(),
                )
                .set(Column::<i32>::new("v_sq_users", "age"), age)
                .build(db.dialect()),
        )
        .expect("插用户");
    }
    // 用户 1：100/200；用户 3：50；用户 2：无订单
    for (id, user_id, amount) in [(1_i64, 1_i64, 100_i64), (2, 1, 200), (3, 3, 50)] {
        db.execute(
            InsertStmt::new("v_sq_orders")
                .set(Column::<i64>::new("v_sq_orders", "id"), id)
                .set(Column::<i64>::new("v_sq_orders", "user_id"), user_id)
                .set(Column::<i64>::new("v_sq_orders", "amount"), amount)
                .build(db.dialect()),
        )
        .expect("插订单");
    }
    Some(db)
}

fn names(db: &mut Session<MySqlDriver>, stmt: SelectStmt) -> Vec<String> {
    db.fetch_all::<User>(&stmt.build(db.dialect()))
        .expect("查用户")
        .into_iter()
        .map(|u| u.name)
        .collect()
}

#[test]
fn in_subquery_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let sub = SelectStmt::new("v_sq_orders")
        .column("user_id")
        .filter(Column::<i64>::new("v_sq_orders", "amount").ge(100_i64));
    let stmt = SelectStmt::for_entity::<User>().filter(User::id().in_subquery(&sub));
    assert_eq!(vec!["a".to_string()], names(&mut db, stmt));
}

#[test]
fn not_in_subquery_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let sub = SelectStmt::new("v_sq_orders")
        .column("user_id")
        .filter(Column::<i64>::new("v_sq_orders", "amount").ge(100_i64));
    let stmt = SelectStmt::for_entity::<User>().filter(User::id().not_in_subquery(&sub));
    let mut got = names(&mut db, stmt);
    got.sort();
    assert_eq!(vec!["b".to_string(), "c".to_string()], got);
}

#[test]
fn exists_correlated_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let sub = SelectStmt::new("v_sq_orders")
        .column_expr(Expr::lit(1_i64))
        .filter(Expr::cmp(
            rsbatis::BinOp::Eq,
            Expr::Col("v_sq_orders", "user_id"),
            Expr::Col("v_sq_users", "id"),
        ));
    let stmt = SelectStmt::for_entity::<User>().filter(Expr::exists(&sub));
    let mut got = names(&mut db, stmt);
    got.sort();
    assert_eq!(vec!["a".to_string(), "c".to_string()], got);
}

#[test]
fn mixed_outer_and_inner_params_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    // 外层 age>=20 + 子查询内部 amount>=200：两个参数，任何错位都会让结果偏离
    let sub = SelectStmt::new("v_sq_orders")
        .column_expr(Expr::lit(1_i64))
        .filter(Expr::cmp(
            rsbatis::BinOp::Eq,
            Expr::Col("v_sq_orders", "user_id"),
            Expr::Col("v_sq_users", "id"),
        ))
        .and_filter(Column::<i64>::new("v_sq_orders", "amount").ge(200_i64));
    let stmt = SelectStmt::for_entity::<User>()
        .filter(Column::<i32>::new("v_sq_users", "age").ge(20_i32))
        .and_filter(Expr::exists(&sub));
    assert_eq!(vec!["a".to_string()], names(&mut db, stmt));
}

/// `IN (子查询 ... LIMIT)`：MariaDB 真实拒绝（错误 1235）。
///
/// 这条用例断言的是"我们提前给了告警"，不是"这条 SQL 能跑"——
/// 数据库限制绕不过去，能做的是不让用户去撞。
#[test]
fn in_subquery_with_limit_is_warned_not_silently_broken() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let sub = SelectStmt::new("v_sq_orders")
        .column("user_id")
        .order_by(Expr::Col("v_sq_orders", "amount"), Order::Desc)
        .limit(1);
    let stmt = SelectStmt::for_entity::<User>().filter(User::id().in_subquery(&sub));
    let sql = stmt.build(db.dialect());
    assert_eq!(1, sql.warnings.len());
    assert!(sql.warnings[0].contains("1235"));

    // 真去执行会拿到数据库错误——把这一点也断言下来，防止有人以为告警是多余的
    let err = db.fetch_all::<User>(&sql).unwrap_err();
    assert!(
        err.to_string().contains("1235"),
        "expected 1235, got: {err}"
    );
}

/// 同样的意图改用 EXISTS：MariaDB 支持，且能拿到正确答案。
#[test]
fn exists_replaces_in_limit_workaround() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    // EXISTS (SELECT 1 FROM orders o WHERE o.user_id = users.id HAVING ...) 太绕；
    // 直接用相关 EXISTS 挑"存在金额 200 订单的用户"，这是 LIMIT 场景的推荐替代写法。
    let sub = SelectStmt::new("v_sq_orders")
        .column_expr(Expr::lit(1_i64))
        .filter(Expr::cmp(
            rsbatis::BinOp::Eq,
            Expr::Col("v_sq_orders", "user_id"),
            Expr::Col("v_sq_users", "id"),
        ))
        .and_filter(Column::<i64>::new("v_sq_orders", "amount").ge(200_i64));
    let stmt = SelectStmt::for_entity::<User>().filter(Expr::exists(&sub));
    assert_eq!(vec!["a".to_string()], names(&mut db, stmt));
}

/// `NOT IN` + 子查询结果含 NULL 时的行为记录。
///
/// SQL 标准语义：`x NOT IN (…, NULL)` 恒为 NULL（不返回行）。
/// 这条用例断言的就是"MySQL 上会一行都不返回"，用来在文档里说清这个坑，
/// 而不是把它当 bug 去改。
#[test]
fn not_in_with_null_in_subquery_returns_nothing() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    db.execute_batch(
        "DROP TABLE IF EXISTS v_sq_nullable;
         CREATE TABLE v_sq_nullable (id BIGINT PRIMARY KEY, user_id BIGINT NULL) ENGINE=InnoDB;
         INSERT INTO v_sq_nullable VALUES (1, NULL);",
    )
    .expect("建 nullable 表");
    let sub = SelectStmt::new("v_sq_nullable").column("user_id");
    let stmt = SelectStmt::for_entity::<User>().filter(User::id().in_subquery(&sub));
    // IN 侧：NULL 不会匹配任何 id，所以空
    assert!(names(&mut db, stmt).is_empty());

    let stmt = SelectStmt::for_entity::<User>().filter(User::id().not_in_subquery(&sub));
    // NOT IN 侧：集合含 NULL → 恒 NULL → 也是空。这就是那个坑。
    assert!(names(&mut db, stmt).is_empty());
}
