//! upsert 的 SQL 渲染：三方言对照。
//!
//! 这里断言的是**字符串级**契约。三种方言的写法差异很大（`excluded` 伪表 vs
//! `VALUES()` 函数、冲突目标列要不要写），而这类差异如果只靠真实库跑通来验证，
//! 出现"MySQL 上静默少了 SET 子句"这种问题时很难定位。

use rsbatis_core::{ConflictSet, Dialect, InsertStmt, Value};

fn insert() -> InsertStmt {
    InsertStmt::new("users")
        .set_raw("id", Value::I64(1))
        .set_raw("name", Value::Text("张三".into()))
}

#[test]
fn sqlite_upsert_uses_excluded_pseudo_table() {
    let sql = insert()
        .on_conflict_do_update(["id"], ["name"])
        .build(Dialect::Sqlite);
    assert_eq!(
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES (?, ?) \
         ON CONFLICT (\"id\") DO UPDATE SET \"name\" = excluded.\"name\"",
        sql.text
    );
    // 冲突段不引入新参数：excluded 引用的是同一行
    assert_eq!(2, sql.params.len());
    assert!(sql.warnings.is_empty(), "warnings: {:?}", sql.warnings);
}

#[test]
fn postgres_matches_sqlite_shape() {
    let sql = insert()
        .on_conflict_do_update(["id"], ["name"])
        .build(Dialect::Postgres);
    assert!(sql
        .text
        .contains("ON CONFLICT (\"id\") DO UPDATE SET \"name\" = excluded.\"name\""));
    assert!(sql.warnings.is_empty());
}

#[test]
fn mysql_upsert_uses_values_function_and_warns_about_target() {
    let sql = insert()
        .on_conflict_do_update(["id"], ["name"])
        .build(Dialect::MySql);
    assert_eq!(
        "INSERT INTO `users` (`id`, `name`) VALUES (?, ?) \
         ON DUPLICATE KEY UPDATE `name` = VALUES(`name`)",
        sql.text
    );
    // MySQL 无法指定冲突目标，必须把"忽略了这个参数"说出来，不能假装用了
    assert_eq!(1, sql.warnings.len());
    assert!(sql.warnings[0].contains("忽略冲突目标列"));
}

#[test]
fn constant_set_adds_one_parameter_per_column() {
    let sql = InsertStmt::new("users")
        .set_raw("id", Value::I64(1))
        .on_conflict_do_update_with(
            ["id"],
            [("name", ConflictSet::Value(Value::Text("固定".into())))],
        )
        .build(Dialect::Sqlite);
    assert!(sql.text.ends_with("DO UPDATE SET \"name\" = ?"));
    // VALUES 里 1 个 + SET 里 1 个
    assert_eq!(2, sql.params.len());
    assert_eq!(Value::Text("固定".into()), sql.params[1]);
}

#[test]
fn mysql_do_nothing_becomes_self_assignment() {
    let sql = insert().on_conflict_do_nothing().build(Dialect::MySql);
    // 关键：不是 INSERT IGNORE（那会连带吞掉类型截断等错误），而是真的空操作
    assert_eq!(
        "INSERT INTO `users` (`id`, `name`) VALUES (?, ?) \
         ON DUPLICATE KEY UPDATE `id` = `id`",
        sql.text
    );
    assert!(sql.warnings.is_empty());
}

#[test]
fn sqlite_do_nothing_stays_as_is() {
    let sql = insert().on_conflict_do_nothing().build(Dialect::Sqlite);
    assert!(sql.text.ends_with("ON CONFLICT DO NOTHING"));
    assert!(sql.warnings.is_empty());
}

#[test]
fn postgres_do_update_without_target_is_warned() {
    let sql = insert()
        .on_conflict_do_update_with([], [("name", ConflictSet::KeepInserted)])
        .build(Dialect::Postgres);
    // SQLite 允许无目标，PG 不允许——这里只警告，不擅自替调用方补一个主键
    assert!(sql.text.contains("ON CONFLICT DO UPDATE SET"));
    assert_eq!(1, sql.warnings.len());
    assert!(sql.warnings[0].contains("必须指定冲突目标列"));
}

#[test]
fn upsert_wins_over_do_nothing_when_both_set() {
    let sql = insert()
        .on_conflict_do_nothing()
        .on_conflict_do_update(["id"], ["name"])
        .build(Dialect::Sqlite);
    assert!(sql.text.contains("DO UPDATE SET"));
    assert!(!sql.text.contains("DO NOTHING"));
}
