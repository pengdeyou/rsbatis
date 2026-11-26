//! 分页在真实 MySQL / MariaDB 上：`LIMIT n OFFSET m` 语法、COUNT 返回 BIGINT、
//! 带条件与越界页。
//!
//! 没有 `RSBATIS_MYSQL_URL` 时直接跳过（CI 里没有数据库的 job 也能过）。

#![cfg(feature = "mysql")]
#![allow(dead_code)]

use rsbatis::{insert, select, Dialect, Entity, FromRow, MySqlDriver, PageRequest, Session};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "v_p_users")]
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
        "DROP TABLE IF EXISTS v_p_users;
         CREATE TABLE v_p_users (id BIGINT PRIMARY KEY AUTO_INCREMENT, name VARCHAR(32) NOT NULL, age INT NOT NULL) ENGINE=InnoDB;",
    )
    .expect("建表");

    for i in 0..10 {
        let name = if i < 3 {
            "张三".to_string()
        } else {
            format!("用户{i}")
        };
        db.execute(
            insert::<User>()
                .set(User::name(), name)
                .set(User::age(), 20 + i)
                .build(Dialect::MySql),
        )
        .expect("插入");
    }
    Some(db)
}

#[test]
fn mysql_pagination_end_to_end() {
    let Some(mut db) = seeded() else { return };

    let page = db
        .paginate::<User>(&select::<User>(), PageRequest::new(2, 4))
        .expect("第二页");
    assert_eq!(page.total, 10);
    assert_eq!(page.items.len(), 4);
    assert_eq!(page.pages(), 3);
    assert!(page.has_next());
    assert_eq!(page.first_index(), Some(5));
}

#[test]
fn mysql_filtered_pagination() {
    let Some(mut db) = seeded() else { return };
    let page = db
        .paginate::<User>(
            &select! { User where User.age >= 25 order_by User.age asc },
            PageRequest::new(1, 2),
        )
        .expect("带条件分页");
    assert_eq!(page.total, 5, "COUNT 必须带上 WHERE");
    assert_eq!(
        page.items.iter().map(|u| u.age).collect::<Vec<_>>(),
        vec![25, 26]
    );
}

#[test]
fn mysql_page_beyond_end_is_empty() {
    let Some(mut db) = seeded() else { return };
    let page = db
        .paginate::<User>(&select::<User>(), PageRequest::new(99, 5))
        .expect("越界页");
    assert!(page.is_empty());
    assert_eq!(page.total, 10);
    assert_eq!(page.pages(), 2);
}

#[test]
fn mysql_distinct_count_uses_subquery() {
    let Some(mut db) = seeded() else { return };
    // 只投影 name 再 DISTINCT：MySQL 上子查询形式同样要能跑通
    let stmt = select::<User>().column("name").distinct();
    let page = db
        .paginate::<rsbatis::Row>(&stmt, PageRequest::new(1, 3))
        .expect("去重分页");
    assert_eq!(page.total, 8);
    assert_eq!(page.items.len(), 3);
}
