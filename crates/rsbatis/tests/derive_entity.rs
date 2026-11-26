//! `#[derive(Entity)]` 的行为锁定。
//!
//! 这里断言的都是"用户看不见但依赖着"的东西：表名、列顺序、可空标记、
//! `col_xxx()` 的函数名与列名，以及它能否和 `filter!` 宏串起来。

#![allow(dead_code)] // 这些结构体只用来验证派生结果，字段本身不会被读取

use rsbatis::{select, filter, Dialect, Entity};

#[derive(Entity)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    name: String,
    age: Option<i32>,
}

#[derive(Entity)]
#[rsbatis(table = "sys_config")]
struct SysConfig {
    #[rsbatis(column = "cfg_key")]
    key: String,
    value: String,
}

#[derive(Entity)]
struct NoAttr {} // 空字段具名结构体，应走 snake_case 默认

#[test]
fn table_columns_and_nullable() {
    assert_eq!("users", <User as Entity>::TABLE);
    assert_eq!(&["id", "name", "age"], <User as Entity>::COLUMNS);
    assert_eq!(&["age"], <User as Entity>::NULLABLE, "只有 Option 字段可空");

    // 同一张表、同一列，从两个地方取到的 Column 必须相等
    assert_eq!("age", User::age().name());
    assert_eq!("users", User::age().table());
}

#[test]
fn column_attribute_overrides_field_name() {
    assert_eq!("sys_config", <SysConfig as Entity>::TABLE);
    assert_eq!(&["cfg_key", "value"], <SysConfig as Entity>::COLUMNS);
    assert_eq!("cfg_key", SysConfig::key().name());
    assert!(<SysConfig as Entity>::NULLABLE.is_empty());
}

#[test]
fn default_table_name_is_snake_case() {
    assert_eq!("no_attr", <NoAttr as Entity>::TABLE);
    assert!(
        <NoAttr as Entity>::COLUMNS.is_empty(),
        "无字段的结构体列集为空"
    );
}

#[test]
fn derived_columns_work_with_filtermacro() {
    let stmt = select::<User>()
        .filter(filter!(User.age >= 18))
        .build(Dialect::Sqlite);
    assert_eq!(r#"SELECT * FROM "users" WHERE "age" >= ?"#, stmt.text);
    assert_eq!(1, stmt.params.len());

    let mysql = select::<User>()
        .filter(filter!(User.name.like("zhang%") && User.age >= 18))
        .build(Dialect::MySql);
    assert_eq!(
        "SELECT * FROM `users` WHERE `name` LIKE ? AND `age` >= ?",
        mysql.text
    );
    assert_eq!(2, mysql.params.len());
}

#[test]
fn derived_columns_usable_in_dml() {
    let ins = rsbatis::insert::<User>()
        .set(User::name(), "张三")
        .set_null(User::age())
        .build(Dialect::MySql);
    assert_eq!(
        "INSERT INTO `users` (`name`, `age`) VALUES (?, ?)",
        ins.text
    );
    // set_null 写入的是真正的 NULL，不是空字符串
    assert!(matches!(ins.params[1], rsbatis::Value::Null));

    let upd = rsbatis::update::<User>()
        .set(User::age(), 21)
        .filter(filter!(User.id == 1))
        .build(Dialect::Sqlite);
    assert_eq!(r#"UPDATE "users" SET "age" = ? WHERE "id" = ?"#, upd.text);
}
