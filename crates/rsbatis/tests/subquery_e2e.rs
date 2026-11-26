//! 子查询端到端（SQLite 内存库）：`IN` / `NOT IN` / `EXISTS` 在真实数据上是否给出正确答案。
//!
//! 只断言 SQL 文本是不够的：参数错位这类缺陷在 SQLite 上通常表现为"查不到数据"，
//! 文本看起来却完全正常。所以这里每条用例都对比真实结果集。

#![cfg(feature = "sqlite")]

use rsbatis::{Entity, FromRow, SelectStmt, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug, PartialEq)]
#[rsbatis(table = "sq_users")]
struct User {
    id: i64,
    name: String,
    age: i32,
}

#[derive(Entity, FromRow, Clone, Debug, PartialEq)]
#[rsbatis(table = "sq_orders")]
struct Order {
    id: i64,
    user_id: i64,
    amount: i64,
}

fn setup() -> Session<SqliteDriver> {
    let mut session = Session::new(SqliteDriver::open_in_memory().unwrap());
    session
        .execute_batch(
            r#"
            DROP TABLE IF EXISTS sq_users;
            DROP TABLE IF EXISTS sq_orders;
            CREATE TABLE sq_users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER NOT NULL);
            CREATE TABLE sq_orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount INTEGER NOT NULL);
            "#,
        )
        .unwrap();
    for (id, name, age) in [(1_i64, "a", 20), (2, "b", 30), (3, "c", 40)] {
        session
            .execute(
                rsbatis::InsertStmt::new("sq_users")
                    .set(rsbatis::Column::<i64>::new("sq_users", "id"), id)
                    .set(rsbatis::Column::<String>::new("sq_users", "name"), name)
                    .set(rsbatis::Column::<i32>::new("sq_users", "age"), age)
                    .build(session.dialect()),
            )
            .unwrap();
    }
    // 用户 1 有两笔（100/200），用户 3 有一笔（50），用户 2 没有订单
    for (id, user_id, amount) in [(1_i64, 1_i64, 100_i64), (2, 1, 200), (3, 3, 50)] {
        session
            .execute(
                rsbatis::InsertStmt::new("sq_orders")
                    .set(rsbatis::Column::<i64>::new("sq_orders", "id"), id)
                    .set(rsbatis::Column::<i64>::new("sq_orders", "user_id"), user_id)
                    .set(rsbatis::Column::<i64>::new("sq_orders", "amount"), amount)
                    .build(session.dialect()),
            )
            .unwrap();
    }
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
fn in_subquery_filters_by_related_table() {
    let mut session = setup();
    // 有金额 >= 100 订单的用户
    let sub = SelectStmt::new("sq_orders")
        .column("user_id")
        .where_(rsbatis::Column::<i64>::new("sq_orders", "amount").ge(100_i64));
    let stmt = SelectStmt::for_entity::<User>().where_(User::id().in_subquery(&sub));
    assert_eq!(vec!["a".to_string()], names(&mut session, stmt));
}

#[test]
fn not_in_subquery_excludes_and_includes_users_without_orders() {
    let mut session = setup();
    // 没有金额 >= 100 订单的用户：b（无订单）、c（50）
    // 注意 NULL 陷阱不存在于此（NOT IN 的集合非空且无 NULL），所以 c 会被正确包含
    let sub = SelectStmt::new("sq_orders")
        .column("user_id")
        .where_(rsbatis::Column::<i64>::new("sq_orders", "amount").ge(100_i64));
    let stmt = SelectStmt::for_entity::<User>().where_(User::id().not_in_subquery(&sub));
    let mut got = names(&mut session, stmt);
    got.sort();
    assert_eq!(vec!["b".to_string(), "c".to_string()], got);
}

#[test]
fn exists_correlated_subquery_sees_outer_column() {
    let mut session = setup();
    // 相关子查询：EXISTS (SELECT 1 FROM sq_orders WHERE sq_orders.user_id = sq_users.id)
    // 这里外层的 "sq_users"."id" 必须被解析成外层表——若子查询没带表名限定，
    // 会变成 sq_orders 自己的 id 列，结果完全不同（这是本用例真正在守的东西）。
    let sub = SelectStmt::new("sq_orders")
        .column_expr(rsbatis::Expr::lit(1_i64))
        .where_(rsbatis::Expr::cmp(
            rsbatis::BinOp::Eq,
            rsbatis::Expr::Col("sq_orders", "user_id"),
            rsbatis::Expr::Col("sq_users", "id"),
        ));
    let stmt = SelectStmt::for_entity::<User>().where_(rsbatis::Expr::exists(&sub));
    let mut got = names(&mut session, stmt);
    got.sort();
    assert_eq!(vec!["a".to_string(), "c".to_string()], got);
}

#[test]
fn exists_with_extra_inner_condition() {
    let mut session = setup();
    // EXISTS + 子查询内部还有参数：验证外层参数与内层参数没有错位
    let sub = SelectStmt::new("sq_orders")
        .column_expr(rsbatis::Expr::lit(1_i64))
        .where_(rsbatis::Expr::cmp(
            rsbatis::BinOp::Eq,
            rsbatis::Expr::Col("sq_orders", "user_id"),
            rsbatis::Expr::Col("sq_users", "id"),
        ))
        .and_where(rsbatis::Column::<i64>::new("sq_orders", "amount").ge(200_i64));
    let stmt = SelectStmt::for_entity::<User>()
        .where_(rsbatis::Column::<i32>::new("sq_users", "age").ge(20_i32))
        .and_where(rsbatis::Expr::exists(&sub));
    // 只有用户 a：订单金额 200 的那笔，且 age>=20
    assert_eq!(vec!["a".to_string()], names(&mut session, stmt));
}

#[test]
fn subquery_with_order_and_limit() {
    let mut session = setup();
    // 子查询里带 ORDER BY + LIMIT：取金额最大的订单所属用户
    let sub = SelectStmt::new("sq_orders")
        .column("user_id")
        .order_by(
            rsbatis::Expr::Col("sq_orders", "amount"),
            rsbatis::Order::Desc,
        )
        .limit(1);
    let stmt = SelectStmt::for_entity::<User>().where_(User::id().in_subquery(&sub));
    assert_eq!(vec!["a".to_string()], names(&mut session, stmt));
}
