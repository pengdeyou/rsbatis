//! `GROUP BY` / `HAVING` / 聚合 的端到端（SQLite 内存库）。
//!
//! 断言真实聚合结果，以及分页时"总数 = 组数"而不是"行数"。

#![cfg(feature = "sqlite")]

use rsbatis::{BinOp, Entity, Expr, FromRow, PageRequest, SelectStmt, Session, SqliteDriver};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "gb_users")]
struct User {
    id: i64,
    name: String,
}

fn setup() -> Session<SqliteDriver> {
    let mut session = Session::new(SqliteDriver::open_in_memory().unwrap());
    session
        .execute_batch(
            r#"
            DROP TABLE IF EXISTS gb_users;
            DROP TABLE IF EXISTS gb_orders;
            CREATE TABLE gb_users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE gb_orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount INTEGER NOT NULL);
            INSERT INTO gb_users (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c');
            -- a 两单(300)、b 一单(50)、c 无单
            INSERT INTO gb_orders (id, user_id, amount) VALUES (1, 1, 100), (2, 1, 200), (3, 2, 50);
            "#,
        )
        .unwrap();
    session
}

#[derive(FromRow, Debug, PartialEq)]
struct UserCount {
    user_id: i64,
    cnt: i64,
}

#[test]
fn group_by_counts_rows_per_user() {
    let mut db = setup();
    let stmt = SelectStmt::new("gb_orders")
        .column("user_id")
        .column_expr(Expr::count_star().as_("cnt"))
        .group_by(Expr::Col("gb_orders", "user_id"))
        .order_by(Expr::Col("gb_orders", "user_id"), rsbatis::Order::Asc);
    let rows: Vec<UserCount> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    assert_eq!(
        vec![
            UserCount { user_id: 1, cnt: 2 },
            UserCount { user_id: 2, cnt: 1 },
        ],
        rows
    );
}

#[test]
fn having_filters_groups() {
    let mut db = setup();
    let stmt = SelectStmt::new("gb_orders")
        .column("user_id")
        .column_expr(Expr::count_star().as_("cnt"))
        .group_by(Expr::Col("gb_orders", "user_id"))
        .having(Expr::cmp(BinOp::Gt, Expr::count_star(), Expr::lit(1_i64)));
    let rows: Vec<UserCount> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    assert_eq!(vec![UserCount { user_id: 1, cnt: 2 }], rows);
}

#[derive(FromRow, Debug, PartialEq)]
struct UserSum {
    user_id: i64,
    total: i64,
}

#[test]
fn sum_with_having() {
    let mut db = setup();
    let stmt = SelectStmt::new("gb_orders")
        .column("user_id")
        .column_expr(Expr::sum(Expr::Col("gb_orders", "amount")).as_("total"))
        .group_by(Expr::Col("gb_orders", "user_id"))
        .having(Expr::cmp(
            BinOp::Ge,
            Expr::sum(Expr::Col("gb_orders", "amount")),
            Expr::lit(150_i64),
        ));
    let rows: Vec<UserSum> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    // a=300 满足，b=50 不满足
    assert_eq!(
        vec![UserSum {
            user_id: 1,
            total: 300
        }],
        rows
    );
}

#[test]
fn group_by_with_join() {
    let mut db = setup();
    // 用户表 LEFT JOIN 订单，按用户名分组统计订单数：c 无单也要出现（COUNT 非空列给 0）
    let stmt = SelectStmt::new("gb_users")
        .column("name")
        .column_expr(Expr::count_star().as_("cnt"))
        .join_table(
            rsbatis::JoinKind::Left,
            "gb_orders",
            Expr::cmp(
                BinOp::Eq,
                Expr::Col("gb_users", "id"),
                Expr::Col("gb_orders", "user_id"),
            ),
        )
        .group_by(Expr::Col("gb_users", "name"))
        .order_by(Expr::Col("gb_users", "name"), rsbatis::Order::Asc);

    #[derive(FromRow, Debug, PartialEq)]
    struct NameCount {
        name: String,
        cnt: i64,
    }
    let rows: Vec<NameCount> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    // LEFT JOIN 下 c 也会有一行（全 NULL 的订单侧），COUNT(*) 数是 1；
    // 想数"真实订单"应用 COUNT(订单列)。这里断言 COUNT(*) 的真实行为。
    assert_eq!(
        rows.iter()
            .map(|r| (r.name.as_str(), r.cnt))
            .collect::<Vec<_>>(),
        vec![("a", 2), ("b", 1), ("c", 1)]
    );
}

#[test]
fn paginate_counts_groups_not_rows() {
    let mut db = setup();
    // 3 行订单、2 个分组。分页总数必须是 2（组数），不是 3（行数）
    let stmt = SelectStmt::new("gb_orders")
        .column("user_id")
        .column_expr(Expr::count_star().as_("cnt"))
        .group_by(Expr::Col("gb_orders", "user_id"))
        .order_by(Expr::Col("gb_orders", "user_id"), rsbatis::Order::Asc);

    let page = db
        .paginate::<UserCount>(&stmt, PageRequest::new(1, 10))
        .unwrap();
    assert_eq!(2, page.total, "总数应为组数 2，而非行数 3");
    assert_eq!(2, page.items.len());

    // 第二页（每页 1 条）应只剩 1 个分组
    let page = db
        .paginate::<UserCount>(&stmt, PageRequest::new(2, 1))
        .unwrap();
    assert_eq!(2, page.total);
    assert_eq!(vec![UserCount { user_id: 2, cnt: 1 }], page.items);
}
