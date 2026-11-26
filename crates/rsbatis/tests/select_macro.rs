//! `select!` 的行为锁定：把整条查询收进一次宏调用。

#![allow(dead_code)]

use rsbatis::{select, filter, Dialect, Entity};

#[derive(Entity)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    name: String,
    age: i32,
}

fn text(stmt: rsbatis::SelectStmt, d: Dialect) -> String {
    stmt.build(d).text
}

#[test]
fn bare_entity() {
    let s = select! { User };
    assert_eq!(r#"SELECT * FROM "users""#, text(s, Dialect::Sqlite));
}

#[test]
fn filteronly() {
    let s = select! { User where User.age >= 18 };
    assert_eq!(
        r#"SELECT * FROM "users" WHERE "age" >= ?"#,
        text(s, Dialect::Sqlite)
    );
}

#[test]
fn with_filterorder_limit_offset() {
    let s = select! { User where User.age >= 18 order_by User.age desc limit 10 offset 5 };
    assert_eq!(
        r#"SELECT * FROM "users" WHERE "age" >= ? ORDER BY "age" DESC LIMIT 10 OFFSET 5"#,
        text(s, Dialect::Sqlite)
    );
    let sql = select! { User where User.age >= 18 order_by User.age desc limit 10 offset 5 }
        .build(Dialect::Sqlite);
    assert_eq!(1, sql.params.len(), "只有 where 段产生参数");
}

#[test]
fn multi_order_and_distinct() {
    let s = select! { User distinct order_by User.name asc, User.age desc };
    assert_eq!(
        r#"SELECT DISTINCT * FROM "users" ORDER BY "name" ASC, "age" DESC"#,
        text(s, Dialect::Sqlite)
    );
}

#[test]
fn filterinside_select_supports_full_expression() {
    // select! 内部的 where 段复用同一套 rewrite，所以括号/IS NULL/算术都可用
    let s = select! { User where (User.age >= 18 && User.name.like("张%")) || User.id == 1i64 };
    assert_eq!(
        r#"SELECT * FROM "users" WHERE ("age" >= ? AND "name" LIKE ?) OR "id" = ?"#,
        text(s, Dialect::Sqlite)
    );

    let s = select! { User where User.age == null };
    assert_eq!(
        r#"SELECT * FROM "users" WHERE "age" IS NULL"#,
        text(s, Dialect::Sqlite)
    );
}

#[test]
fn equivalent_to_builder_chain() {
    // 宏的输出必须与手写 builder 链完全一致（否则两条写法会漂移）
    let macro_sql =
        select! { User where User.age >= 18 order_by User.age desc limit 10 }.build(Dialect::MySql);
    let builder_sql = select::<User>()
        .filter(filter!(User.age >= 18))
        .order_by(User::age().expr(), rsbatis::Order::Desc)
        .limit(10)
        .build(Dialect::MySql);
    assert_eq!(builder_sql.text, macro_sql.text);
    assert_eq!(builder_sql.params, macro_sql.params);
}
