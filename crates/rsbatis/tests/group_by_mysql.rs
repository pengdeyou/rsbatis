//! `GROUP BY` / `HAVING` / 聚合 在真实 MySQL / MariaDB 上，含分页总数。
//!
//! 没有 `RSBATIS_MYSQL_URL` 时直接跳过。

#![cfg(feature = "mysql")]

use rsbatis::{BinOp, Entity, Expr, FromRow, MySqlDriver, PageRequest, SelectStmt, Session};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "v_gb_users")]
struct User {
    id: i64,
    name: String,
}

fn seeded() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接 MySQL"));
    db.execute_batch(
        "DROP TABLE IF EXISTS v_gb_users;
         DROP TABLE IF EXISTS v_gb_orders;
         CREATE TABLE v_gb_users (id BIGINT PRIMARY KEY, name VARCHAR(32) NOT NULL) ENGINE=InnoDB;
         CREATE TABLE v_gb_orders (id BIGINT PRIMARY KEY, user_id BIGINT NOT NULL, amount BIGINT NOT NULL) ENGINE=InnoDB;
         INSERT INTO v_gb_users (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c');
         INSERT INTO v_gb_orders (id, user_id, amount) VALUES (1, 1, 100), (2, 1, 200), (3, 2, 50);",
    )
    .expect("建表与灌数据");
    Some(db)
}

#[derive(FromRow, Debug, PartialEq)]
struct UserCount {
    user_id: i64,
    cnt: i64,
}

#[test]
fn group_by_with_having_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let stmt = SelectStmt::new("v_gb_orders")
        .column("user_id")
        .column_expr(Expr::count_star().as_("cnt"))
        .group_by(Expr::Col("v_gb_orders", "user_id"))
        .having(Expr::cmp(BinOp::Gt, Expr::count_star(), Expr::lit(1_i64)));
    let rows: Vec<UserCount> = db.fetch_all(stmt.build(db.dialect())).expect("分组查询");
    assert_eq!(vec![UserCount { user_id: 1, cnt: 2 }], rows);
}

#[test]
fn sum_group_by_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    // MySQL 的 SUM(BIGINT) 返回 **DECIMAL**（字符形式上是个字符串），严格回读成 i64 会
    // TypeMismatch。这里显式 CAST 成整数（"INTEGER" 在 MySQL 侧映射为 SIGNED）。
    let stmt = SelectStmt::new("v_gb_orders")
        .column("user_id")
        .column_expr(
            Expr::cast(Expr::sum(Expr::Col("v_gb_orders", "amount")), "INTEGER").as_("total"),
        )
        .group_by(Expr::Col("v_gb_orders", "user_id"))
        .order_by(Expr::Col("v_gb_orders", "user_id"), rsbatis::Order::Asc);

    #[derive(FromRow, Debug, PartialEq)]
    struct UserSum {
        user_id: i64,
        total: i64,
    }
    let rows: Vec<UserSum> = db.fetch_all(stmt.build(db.dialect())).expect("SUM 查询");
    assert_eq!(
        vec![
            UserSum {
                user_id: 1,
                total: 300
            },
            UserSum {
                user_id: 2,
                total: 50
            },
        ],
        rows
    );
}

#[test]
fn paginate_counts_groups_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let stmt = SelectStmt::new("v_gb_orders")
        .column("user_id")
        .column_expr(Expr::count_star().as_("cnt"))
        .group_by(Expr::Col("v_gb_orders", "user_id"))
        .order_by(Expr::Col("v_gb_orders", "user_id"), rsbatis::Order::Asc);

    let page = db
        .paginate::<UserCount>(&stmt, PageRequest::new(1, 10))
        .expect("分页查询");
    assert_eq!(2, page.total, "总数应为组数 2");
    assert_eq!(2, page.items.len());
}

#[test]
fn left_join_group_by_on_mysql() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let stmt = SelectStmt::new("v_gb_users")
        .column("name")
        .column_expr(Expr::count_star().as_("cnt"))
        .join_table(
            rsbatis::JoinKind::Left,
            "v_gb_orders",
            Expr::cmp(
                BinOp::Eq,
                Expr::Col("v_gb_users", "id"),
                Expr::Col("v_gb_orders", "user_id"),
            ),
        )
        .group_by(Expr::Col("v_gb_users", "name"))
        .order_by(Expr::Col("v_gb_users", "name"), rsbatis::Order::Asc);

    #[derive(FromRow, Debug, PartialEq)]
    struct NameCount {
        name: String,
        cnt: i64,
    }
    let rows: Vec<NameCount> = db
        .fetch_all(stmt.build(db.dialect()))
        .expect("LEFT JOIN 分组");
    assert_eq!(
        rows.iter()
            .map(|r| (r.name.as_str(), r.cnt))
            .collect::<Vec<_>>(),
        vec![("a", 2), ("b", 1), ("c", 1)]
    );
}

/// 未 CAST 的 `SUM` 在 MySQL 上是 DECIMAL：读成 i64 会明确报类型不匹配，
/// 不会静默给一个错值。这条用例把这个真实行为钉住，同时说明为什么需要 CAST。
#[test]
fn raw_sum_is_decimal_and_rejected_as_i64() {
    let mut db = match seeded() {
        Some(db) => db,
        None => return,
    };
    let stmt = SelectStmt::new("v_gb_orders")
        .column("user_id")
        .column_expr(Expr::sum(Expr::Col("v_gb_orders", "amount")).as_("total"))
        .group_by(Expr::Col("v_gb_orders", "user_id"));

    #[derive(FromRow, Debug)]
    #[allow(dead_code)]
    struct UserSum {
        user_id: i64,
        total: i64,
    }
    let err = db
        .fetch_all::<UserSum>(&stmt.build(db.dialect()))
        .expect_err("未 CAST 的 SUM 读 i64 必须失败");
    assert!(
        err.to_string().contains("text") || err.to_string().contains("DECIMAL"),
        "错误信息里应体现实际类型，got: {err}"
    );
}

/// `CAST(... AS INTEGER)` 的方言映射：MySQL 侧必须是 `SIGNED`（`INTEGER` 不是合法目标类型）。
#[test]
fn cast_integer_maps_to_signed_on_mysql() {
    let sql = SelectStmt::new("v_gb_orders")
        .column_expr(Expr::cast(Expr::Col("v_gb_orders", "amount"), "INTEGER"))
        .build(rsbatis::Dialect::MySql);
    assert!(
        sql.text.contains("CAST(") && sql.text.contains("AS SIGNED"),
        "got: {}",
        sql.text
    );
    assert!(
        !sql.text.contains("AS INTEGER"),
        "MySQL 不能用 INTEGER 作 CAST 目标：{}",
        sql.text
    );
}
