//! syn 版 `where_!` 的新能力锁定。
//!
//! 旧版是手写 `TokenTree` 扫描器，只能按"识别 `Ident . Ident` 当列"的启发式往前走，
//! 下面这些用例它要么生成错 SQL，要么直接 panic。换成 syn 的完整语法树之后，
//! 括号、一元运算、函数调用、算术都能按语法结构正确处理。

#![allow(dead_code)]

use rsbatis::{select, where_, Dialect, Entity};

#[derive(Entity)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    name: String,
    age: i32,
}

fn sql(expr: rsbatis::Expr, d: Dialect) -> String {
    select::<User>().where_(expr).build(d).text
}

#[test]
fn dump_actual_sql() {
    // 这个用例只负责打印真实 SQL，方便人工核对；断言在下面的具名用例里。
    let cases: Vec<(&str, String)> = vec![
        (
            "(a>=18 && b==\"a\") || id==1",
            sql(
                where_!((User.age >= 18 && User.name == "a") || User.id == 1i64),
                Dialect::Sqlite,
            ),
        ),
        (
            "age == null",
            sql(where_!(User.age == null), Dialect::Sqlite),
        ),
        (
            "age != null",
            sql(where_!(User.age != null), Dialect::Sqlite),
        ),
        (
            "age + 1 >= 18",
            sql(where_!(User.age + 1 >= 18), Dialect::Sqlite),
        ),
        (
            "!(age >= 18)",
            sql(where_!(!(User.age >= 18)), Dialect::Sqlite),
        ),
        (
            "coalesce(name, \"x\") == \"x\"",
            sql(where_!(coalesce(User.name, "x") == "x"), Dialect::Sqlite),
        ),
        (
            "lower(name) == \"abc\"",
            sql(where_!(lower(User.name) == "abc"), Dialect::Sqlite),
        ),
        (
            "age >= 18 && (name==\"a\" || id==1)",
            sql(
                where_!(User.age >= 18 && (User.name == "a" || User.id == 1i64)),
                Dialect::Sqlite,
            ),
        ),
    ];
    for (label, text) in cases {
        println!("{label:>40} => {text}");
    }
}

#[test]
fn explicit_parentheses_are_preserved() {
    // 用户手写括号代表他的意图；优先级渲染不能替他决定结合方式
    let s = sql(
        where_!((User.age >= 18 && User.name == "a") || User.id == 1i64),
        Dialect::Sqlite,
    );
    assert!(
        s.contains("((") || s.contains("(\""),
        "左分支必须被括号包住：{s}"
    );
    assert!(s.contains(" OR "), "外层是 OR：{s}");
}

#[test]
fn null_comparison_becomes_is_null() {
    let s = sql(where_!(User.age == null), Dialect::Sqlite);
    assert_eq!(r#"SELECT * FROM "users" WHERE "age" IS NULL"#, s);
    assert_eq!(
        0,
        select::<User>()
            .where_(where_!(User.age == null))
            .build(Dialect::Sqlite)
            .params
            .len()
    );

    let s = sql(where_!(User.age != null), Dialect::MySql);
    assert_eq!("SELECT * FROM `users` WHERE `age` IS NOT NULL", s);
}

#[test]
fn arithmetic_and_unary_not() {
    let s = sql(where_!(User.age + 1 >= 18), Dialect::Sqlite);
    assert!(s.contains(r#""age" + ? >= ?"#), "算术右侧应带两个参数：{s}");

    let s = sql(where_!(!(User.age >= 18)), Dialect::Sqlite);
    assert!(
        s.starts_with("SELECT * FROM \"users\" WHERE NOT"),
        "应为 NOT：{s}"
    );
}

#[test]
fn sql_functions_from_whitelist() {
    let s = sql(where_!(lower(User.name) == "abc"), Dialect::Sqlite);
    assert!(s.contains("LOWER(\"name\")"), "函数名应大写：{s}");

    let s = sql(where_!(coalesce(User.name, "x") == "x"), Dialect::MySql);
    assert!(s.contains("COALESCE(`name`, ?)"), "多参函数：{s}");
}

#[test]
fn and_chain_is_flattened() {
    // a && b && c 应生成一个 AND 列表，而不是 AND(AND(a,b),c) 这种多余的嵌套
    let s = sql(
        where_!(User.age >= 18 && User.name == "a" && User.id == 1i64),
        Dialect::Sqlite,
    );
    assert_eq!(
        2,
        s.matches(" AND ").count(),
        "应展平成 3 个叶子的 AND：{s}"
    );
    assert!(!s.contains("(("), "同层 AND 不该额外加括号：{s}");
}
