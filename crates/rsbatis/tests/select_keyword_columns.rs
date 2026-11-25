//! 列名与 `select!` 段关键字同名时的解析（回归测试）。
//!
//! 曾经的缺陷：遇到 Ident 就无条件当作段边界，导致表里若有列叫 `limit` / `offset` /
//! `order_by` / `where`，`Event.limit` 会在 `.` 之后被切成新段，查询被截断，
//! 而错误信息指向的位置跟用户写错的地方毫无关系。
#![cfg(feature = "sqlite")]
// 测试里的实体字段只用来声明「列」，不会被业务代码读取；
// 派生宏拿的是字段名，clippy 看不到「读」，因此在这里关掉 dead_code。
#![allow(dead_code)]

use rsbatis::{select, Dialect, Entity, Session, SqliteDriver};

#[derive(Entity)]
#[rsbatis(table = "kw")]
struct Kw {
    id: i64,
    limit: i32,
    offset: i32,
    order_by: String,
}

#[test]
fn column_names_colliding_with_segment_keywords() {
    let sql = select! { Kw where Kw.limit > 5 order_by Kw.order_by asc }.build(Dialect::Sqlite);
    assert!(
        sql.text.contains(r#""limit" > ?"#),
        "列名 limit 必须当列名：{}",
        sql.text
    );
    assert!(
        sql.text.contains(r#"ORDER BY "order_by" ASC"#),
        "列名 order_by 必须当列名：{}",
        sql.text
    );
}

#[test]
fn keyword_columns_work_end_to_end() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        r#"CREATE TABLE "kw" (id INTEGER PRIMARY KEY AUTOINCREMENT,
                             "limit" INTEGER NOT NULL,
                             "offset" INTEGER NOT NULL,
                             order_by TEXT NOT NULL)"#,
    )
    .unwrap();
    for (l, o, k) in [(1, 0, "a"), (9, 2, "b")] {
        db.execute_batch(&format!(
            r#"INSERT INTO "kw" ("limit", "offset", order_by) VALUES ({l}, {o}, '{k}')"#
        ))
        .unwrap();
    }

    let rows = db
        .fetch(select! { Kw where Kw.limit > 5 order_by Kw.order_by asc })
        .unwrap();
    assert_eq!(1, rows.len());
    let k: String = rows[0].get_by_name("order_by").unwrap();
    assert_eq!("b", k);
}
