//! 端到端：core 生成 SQL → driver 在真实 SQLite 上执行 → 回读 Rust 值。
//!
//! 这是 M3 的验收测试：如果这里绿了，说明"能生成 SQL"已经变成"能真正跑通 CRUD"。
//!
//! 运行方式：`cargo test -p rsbatis-driver --features sqlite`

use rsbatis_core::{delete, insert, select, update, Column, Dialect, Entity, Expr, Order};
use rsbatis_driver::sqlite::SqliteDriver;
use rsbatis_driver::{Driver, ErrorKind};

struct User;

impl Entity for User {
    const TABLE: &'static str = "user";
    const COLUMNS: &'static [&'static str] = &["id", "name", "age"];
}

fn id() -> Column<i64> {
    Column::new("user", "id")
}
fn name() -> Column<String> {
    Column::new("user", "name")
}
fn age() -> Column<i32> {
    Column::new("user", "age")
}

fn fresh_db() -> SqliteDriver {
    let d = SqliteDriver::open_in_memory().unwrap();
    d.execute_batch(
        r#"CREATE TABLE "user" (
             id   INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             age  INTEGER
           );"#,
    )
    .unwrap();
    d
}

fn seed(d: &mut SqliteDriver) {
    for (n, a) in [("张三", 20), ("李四", 17), ("王五", 30)] {
        let ins = insert::<User>()
            .set(name(), n)
            .set(age(), a)
            .build(Dialect::Sqlite);
        d.execute(&ins).unwrap();
    }
}

#[test]
fn full_crud_cycle() {
    let mut d = fresh_db();
    seed(&mut d);

    // ---- 读全表 ----
    let all = d.fetch(&select::<User>().build(Dialect::Sqlite)).unwrap();
    assert_eq!(3, all.len(), "应读回 3 行");
    assert_eq!(
        vec!["id", "name", "age"],
        all[0]
            .columns()
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
    );

    // ---- 条件查询 + 类型回读 ----
    let adults = d
        .fetch(&select::<User>().filter(age().ge(18)).build(Dialect::Sqlite))
        .unwrap();
    assert_eq!(2, adults.len());
    assert_eq!(
        "张三".to_string(),
        adults[0].get_by_name::<String>("name").unwrap()
    );
    assert_eq!(20i32, adults[0].get_by_name::<i32>("age").unwrap());
    assert_eq!(1i64, adults[0].get_by_name::<i64>("id").unwrap());

    // ---- UPDATE ----
    let upd = update::<User>()
        .set(age(), 21)
        .filter(name().eq("张三"))
        .build(Dialect::Sqlite);
    assert_eq!(1, d.execute(&upd).unwrap());
    let zhang = d
        .fetch(
            &select::<User>()
                .filter(name().eq("张三"))
                .build(Dialect::Sqlite),
        )
        .unwrap();
    assert_eq!(21i32, zhang[0].get_by_name::<i32>("age").unwrap());

    // ---- DELETE ----
    let del = delete::<User>().filter(age().lt(18)).build(Dialect::Sqlite);
    assert_eq!(1, d.execute(&del).unwrap());
    let left = d.fetch(&select::<User>().build(Dialect::Sqlite)).unwrap();
    assert_eq!(2, left.len(), "删掉李四后应剩 2 行");
}

#[test]
fn null_roundtrip_needs_option() {
    let mut d = fresh_db();
    let ins = insert::<User>()
        .set(name(), "无年龄")
        .build(Dialect::Sqlite);
    d.execute(&ins).unwrap();

    let rows = d
        .fetch(
            &select::<User>()
                .filter(name().eq("无年龄"))
                .build(Dialect::Sqlite),
        )
        .unwrap();

    // 强类型读取：age 是 NULL，读 i32 必须失败
    let err = rows[0].get_by_name::<i32>("age").unwrap_err();
    assert_eq!(ErrorKind::TypeMismatch, err.kind);

    // 可空读取：得到 None
    assert_eq!(None, rows[0].get_by_name::<Option<i32>>("age").unwrap());
    assert_eq!(
        Some("无年龄".to_string()),
        rows[0].get_by_name::<Option<String>>("name").unwrap()
    );
}

#[test]
fn order_limit_offset() {
    let mut d = fresh_db();
    seed(&mut d);

    let sql = select::<User>()
        .column("name")
        .order_by(age().expr(), Order::Desc)
        .limit(2)
        .offset(0)
        .build(Dialect::Sqlite);
    let rows = d.fetch(&sql).unwrap();
    assert_eq!(2, rows.len());
    assert_eq!(
        "王五".to_string(),
        rows[0].get_by_name::<String>("name").unwrap()
    );
    assert_eq!(
        "张三".to_string(),
        rows[1].get_by_name::<String>("name").unwrap()
    );
}

#[test]
fn transaction_rollback_discards_rows() {
    let mut d = fresh_db();

    d.begin().unwrap();
    let ins = insert::<User>().set(name(), "临时").build(Dialect::Sqlite);
    d.execute(&ins).unwrap();
    d.rollback().unwrap();

    let rows = d.fetch(&select::<User>().build(Dialect::Sqlite)).unwrap();
    assert_eq!(0, rows.len(), "回滚后不应留下任何行");
}

#[test]
fn transaction_commit_keeps_rows() {
    let mut d = fresh_db();
    d.begin().unwrap();
    d.execute(&insert::<User>().set(name(), "保留").build(Dialect::Sqlite))
        .unwrap();
    d.commit().unwrap();

    let rows = d.fetch(&select::<User>().build(Dialect::Sqlite)).unwrap();
    assert_eq!(1, rows.len());
}

#[test]
fn unique_violation_is_classified_as_such() {
    let mut d = SqliteDriver::open_in_memory().unwrap();
    d.execute_batch(r#"CREATE TABLE "t" (id INTEGER PRIMARY KEY, v TEXT UNIQUE NOT NULL);"#)
        .unwrap();
    d.execute_batch("INSERT INTO t (v) VALUES ('a')").unwrap();

    // 直接用原生 SQL：表名与实体不一致，为了测约束分类不需要绕进 builder
    use rsbatis_core::Sql;
    let raw = Sql {
        text: "INSERT INTO t (v) VALUES ('a')".into(),
        params: vec![],
        warnings: vec![],
    };
    let err = d.execute(&raw).unwrap_err();
    assert_eq!(ErrorKind::UniqueViolation, err.kind);
    assert!(err.is_unique_violation());
}

#[test]
fn returning_gives_back_generated_id() {
    let mut d = fresh_db();
    let ins = insert::<User>()
        .set(name(), "带回填")
        .returning(["id"])
        .build(Dialect::Sqlite);
    assert!(!ins.has_warning(), "SQLite 应支持 RETURNING");

    // RETURNING 让 INSERT 变成查询，得用 fetch
    let rows = d.fetch(&ins).unwrap();
    assert_eq!(1, rows.len());
    let new_id: i64 = rows[0].get_by_name("id").unwrap();
    assert!(new_id >= 1);
}

#[test]
fn in_list_and_between_execute_correctly() {
    let mut d = fresh_db();
    seed(&mut d);

    let by_in = select::<User>()
        .filter(id().in_([1i64, 3]))
        .build(Dialect::Sqlite);
    assert_eq!(2, d.fetch(&by_in).unwrap().len());

    let by_between = select::<User>()
        .filter(age().between(18, 25))
        .build(Dialect::Sqlite);
    assert_eq!(1, d.fetch(&by_between).unwrap().len());

    // 空 IN 必须返回 0 行（恒假）
    let empty = select::<User>()
        .filter(id().in_(Vec::<i64>::new()))
        .build(Dialect::Sqlite);
    assert_eq!(0, d.fetch(&empty).unwrap().len());
}

#[test]
fn expr_and_not_execute_correctly() {
    let mut d = fresh_db();
    seed(&mut d);

    let sql = select::<User>()
        .filter(Expr::and(vec![age().ge(18), Expr::not(name().like("王%"))]))
        .build(Dialect::Sqlite);
    let rows = d.fetch(&sql).unwrap();
    assert_eq!(1, rows.len());
    assert_eq!(
        "张三".to_string(),
        rows[0].get_by_name::<String>("name").unwrap()
    );
}
