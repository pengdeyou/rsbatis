//! DML 的 golden 测试：INSERT / UPDATE / DELETE 在三方言下的输出与降级说明。

use rsbatis_core::{delete, insert, update, Column, Dialect, Entity, Expr, Value};

struct User;

impl Entity for User {
    const TABLE: &'static str = "user";
    const COLUMNS: &'static [&'static str] = &["id", "name", "age"];
}

fn age() -> Column<i32> {
    Column::new(User::TABLE, "age")
}

fn name() -> Column<String> {
    Column::new(User::TABLE, "name")
}

fn id() -> Column<i64> {
    Column::new(User::TABLE, "id")
}

// ---------- INSERT ----------

#[test]
fn insert_single_row_sqlite() {
    let sql = insert::<User>()
        .set(name(), "zhangsan")
        .set(age(), 20)
        .build(Dialect::Sqlite);
    assert_eq!(
        sql.text,
        r#"INSERT INTO "user" ("name", "age") VALUES (?, ?)"#
    );
    assert_eq!(
        sql.params,
        vec![Value::Text("zhangsan".into()), Value::I32(20)]
    );
    assert!(!sql.has_warning());
}

#[test]
fn insert_with_returning_postgres() {
    let sql = insert::<User>()
        .set(name(), "a")
        .returning(["id"])
        .build(Dialect::Postgres);
    assert_eq!(
        sql.text,
        r#"INSERT INTO "user" ("name") VALUES ($1) RETURNING "id""#
    );
    assert!(!sql.has_warning());
}

#[test]
fn insert_with_returning_mysql_degrades_loudly() {
    let sql = insert::<User>()
        .set(name(), "a")
        .returning(["id"])
        .build(Dialect::MySql);
    // RETURNING 被省略，但必须有 warning，不能静默
    assert_eq!(sql.text, "INSERT INTO `user` (`name`) VALUES (?)");
    assert_eq!(sql.warnings.len(), 1);
    assert!(sql.warnings[0].contains("LAST_INSERT_ID"));
}

#[test]
fn insert_on_conflict_do_nothing() {
    let sql = insert::<User>()
        .set(name(), "a")
        .on_conflict_do_nothing()
        .build(Dialect::Sqlite);
    assert_eq!(
        sql.text,
        r#"INSERT INTO "user" ("name") VALUES (?) ON CONFLICT DO NOTHING"#
    );

    // MySQL 侧：没有 ON CONFLICT DO NOTHING，用"某列 = 它自己"达成真空操作。
    // 早期版本这里是"降级成普通 INSERT + 警告"，那等于没实现 do-nothing
    // （唯一键冲突照样抛错）；现已改为真正的空操作，且不吞其他错误。
    let my = insert::<User>()
        .set(name(), "a")
        .on_conflict_do_nothing()
        .build(Dialect::MySql);
    assert_eq!(
        my.text,
        "INSERT INTO `user` (`name`) VALUES (?) ON DUPLICATE KEY UPDATE `name` = `name`"
    );
    assert!(!my.has_warning(), "不能既执行了又报警告");
}

#[test]
fn insert_i32_into_bigint_column_promotes() {
    // id 是 i64 列，传 i32 字面量应无损提升
    let sql = insert::<User>().set(id(), 1).build(Dialect::Sqlite);
    assert_eq!(sql.params, vec![Value::I64(1)]);
}

// ---------- UPDATE ----------

#[test]
fn update_sets_then_filterin_order() {
    let sql = update::<User>()
        .set(age(), 25)
        .set(name(), "lisi")
        .filter(id().eq(1i64))
        .build(Dialect::Sqlite);
    assert_eq!(
        sql.text,
        r#"UPDATE "user" SET "age" = ?, "name" = ? WHERE "id" = ?"#
    );
    // 参数顺序必须与占位符出现顺序一致：SET 在前，WHERE 在后
    assert_eq!(
        sql.params,
        vec![Value::I32(25), Value::Text("lisi".into()), Value::I64(1)]
    );
}

#[test]
fn update_postgres_numbers_set_and_filter_continuously() {
    let sql = update::<User>()
        .set(age(), 25)
        .filter(Expr::and(vec![id().gt(0i64), name().like("a%")]))
        .build(Dialect::Postgres);
    assert_eq!(
        sql.text,
        r#"UPDATE "user" SET "age" = $1 WHERE "id" > $2 AND "name" LIKE $3"#
    );
}

#[test]
fn update_without_filterwarns() {
    let sql = update::<User>().set(age(), 25).build(Dialect::Sqlite);
    assert_eq!(sql.text, r#"UPDATE "user" SET "age" = ?"#);
    assert!(sql.has_warning());
    assert!(sql.warnings[0].contains("整张表"));
}

#[test]
fn update_without_set_warns() {
    let sql = update::<User>().build(Dialect::Sqlite);
    assert!(sql.has_warning());
}

// ---------- DELETE ----------

#[test]
fn delete_with_condition() {
    let sql = delete::<User>()
        .filter(Expr::or(vec![age().lt(18), name().eq("spam")]))
        .build(Dialect::MySql);
    assert_eq!(sql.text, "DELETE FROM `user` WHERE `age` < ? OR `name` = ?");
    assert_eq!(sql.params.len(), 2);
}

#[test]
fn delete_without_filterwarns_but_still_builds() {
    let sql = delete::<User>().build(Dialect::Sqlite);
    assert_eq!(sql.text, r#"DELETE FROM "user""#);
    assert!(sql.has_warning());
    assert!(sql.warnings[0].contains("整张表"));
}

#[test]
fn delete_returning_on_sqlite() {
    let sql = delete::<User>()
        .filter(id().eq(1i64))
        .returning(["id"])
        .build(Dialect::Sqlite);
    assert_eq!(
        sql.text,
        r#"DELETE FROM "user" WHERE "id" = ? RETURNING "id""#
    );
}
