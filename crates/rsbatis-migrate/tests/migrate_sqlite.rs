//! M6：迁移工具在真实 SQLite 上的行为验证。

use rsbatis_driver::sqlite::SqliteDriver;
use rsbatis_migrate::{MigrateError, Migration, Migrator, Status, MIGRATIONS_TABLE};
use rsbatis_orm::Session;

fn db() -> Session<SqliteDriver> {
    Session::new(SqliteDriver::open_in_memory().expect("内存库"))
}

fn table_exists(s: &mut Session<SqliteDriver>, name: &str) -> bool {
    let sql = format!("SELECT name FROM sqlite_master WHERE type = 'table' AND name = '{name}'");
    s.fetch(rsbatis_core::Sql::raw(sql)).expect("查表").len() == 1
}

fn three() -> Vec<Migration> {
    vec![
        Migration::new(
            1,
            "create_users",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
        )
        .with_down("DROP TABLE users"),
        Migration::new(2, "add_email", "ALTER TABLE users ADD COLUMN email TEXT")
            .with_down("ALTER TABLE users DROP COLUMN email"),
        Migration::new(
            3,
            "create_orders",
            "CREATE TABLE orders (id INTEGER PRIMARY KEY, amount INTEGER NOT NULL)",
        )
        .with_down("DROP TABLE orders"),
    ]
}

#[test]
fn up_applies_all_and_records_versions() {
    let mut s = db();
    let mut m = Migrator::new(&mut s, three()).expect("构造");
    assert_eq!(m.up().expect("up"), vec![1, 2, 3]);

    assert!(table_exists(m.session_mut(), "users"));
    assert!(table_exists(m.session_mut(), "orders"));
    assert!(table_exists(m.session_mut(), MIGRATIONS_TABLE));
    assert_eq!(m.applied_versions().expect("applied"), vec![1, 2, 3]);

    // 幂等：再跑一次什么都不做。
    assert!(m.up().expect("up again").is_empty());
    assert_eq!(m.applied_versions().expect("applied"), vec![1, 2, 3]);
}

#[test]
fn status_marks_each_migration() {
    let mut s = db();
    {
        let mut m = Migrator::new(&mut s, three()).expect("构造");
        m.up_to(2).expect("up_to");
    }
    // 重新构造一次，模拟"第二次打开应用"。
    let mut m = Migrator::new(&mut s, three()).expect("构造");
    let st = m.status().expect("status");
    assert_eq!(
        st,
        vec![
            Status {
                version: 1,
                name: "create_users".into(),
                applied: true
            },
            Status {
                version: 2,
                name: "add_email".into(),
                applied: true
            },
            Status {
                version: 3,
                name: "create_orders".into(),
                applied: false
            },
        ]
    );
}

#[test]
fn down_rolls_back_last_only() {
    let mut s = db();
    let mut m = Migrator::new(&mut s, three()).expect("构造");
    m.up().expect("up");

    assert_eq!(m.down().expect("down"), Some(3));
    assert!(!table_exists(m.session_mut(), "orders"), "orders 应被 DROP");
    assert!(table_exists(m.session_mut(), "users"), "users 不该动");
    assert_eq!(m.applied_versions().expect("applied"), vec![1, 2]);

    assert_eq!(m.down().expect("down"), Some(2));
    assert_eq!(m.down().expect("down"), Some(1));
    assert_eq!(m.down().expect("down"), None, "没有可回滚的了");
    assert!(!table_exists(m.session_mut(), "users"));
}

#[test]
fn down_to_keeps_target() {
    let mut s = db();
    let mut m = Migrator::new(&mut s, three()).expect("构造");
    m.up().expect("up");

    assert_eq!(m.down_to(1).expect("down_to"), vec![3, 2], "从新到旧");
    assert_eq!(m.applied_versions().expect("applied"), vec![1]);
    assert!(table_exists(m.session_mut(), "users"));
    assert!(!table_exists(m.session_mut(), "orders"));
}

#[test]
fn missing_down_script_is_an_error() {
    let mut s = db();
    let mut m = Migrator::new(
        &mut s,
        vec![Migration::new(1, "create_t", "CREATE TABLE t (id INTEGER)")],
    )
    .expect("构造");
    m.up().expect("up");
    let err = m.down().expect_err("应当报错");
    assert!(
        matches!(&err, MigrateError::Invalid(msg) if msg.contains("没有 down 脚本")),
        "实际：{err}"
    );
}

#[test]
fn broken_up_sql_rolls_back_entirely() {
    let mut s = db();
    let mut m = Migrator::new(
        &mut s,
        vec![Migration::new(
            1,
            "half_broken",
            "CREATE TABLE t1 (id INTEGER); INSERT INTO no_such_table VALUES (1)",
        )],
    )
    .expect("构造");

    assert!(m.up().is_err(), "第二条语句必然失败");
    // SQLite 的 DDL 参与事务，所以 t1 必须被回滚掉；
    // 元数据也必须没记上，否则下一次 up 会跳过这支迁移。
    assert!(
        !table_exists(m.session_mut(), "t1"),
        "失败的迁移不应留下半张表"
    );
    assert!(m.applied_versions().expect("applied").is_empty());
    assert!(m.supports_transactional_ddl(), "SQLite 的 DDL 可事务回滚");
}

#[test]
fn duplicate_version_rejected_at_construction() {
    let mut s = db();
    let err = Migrator::new(
        &mut s,
        vec![
            Migration::new(1, "a", "CREATE TABLE a (id INTEGER)"),
            Migration::new(1, "b", "CREATE TABLE b (id INTEGER)"),
        ],
    )
    .expect_err("版本重复必须当场拒绝");
    assert!(
        matches!(&err, MigrateError::Invalid(m) if m.contains("重复")),
        "实际：{err}"
    );
    // 构造失败时不该留下元数据表。
    assert!(!table_exists(&mut s, MIGRATIONS_TABLE));
}

#[test]
fn unknown_version_in_db_is_reported() {
    let mut s = db();
    {
        let mut m =
            Migrator::new(
                &mut s,
                vec![Migration::new(7, "only", "CREATE TABLE t (id INTEGER)")
                    .with_down("DROP TABLE t")],
            )
            .expect("构造");
        m.up().expect("up");
    }
    // 换成一套不含版本 7 的迁移代码（相当于切回了旧分支）。
    let mut m = Migrator::new(
        &mut s,
        vec![Migration::new(1, "other", "CREATE TABLE o (id INTEGER)")],
    )
    .expect("构造");
    let err = m.down().expect_err("应报找不到版本");
    assert!(
        matches!(&err, MigrateError::Invalid(m) if m.contains("不在当前迁移列表里")),
        "实际：{err}"
    );
}

#[test]
fn from_dir_loads_up_and_down_pairs() {
    let dir = std::env::temp_dir().join(format!("rsbatis-mig-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建目录");
    std::fs::write(
        dir.join("20260105001_create_users.sql"),
        "CREATE TABLE users (id INTEGER)",
    )
    .unwrap();
    std::fs::write(
        dir.join("20260105001_create_users.down.sql"),
        "DROP TABLE users",
    )
    .unwrap();
    std::fs::write(
        dir.join("20260106001_create_orders.sql"),
        "CREATE TABLE orders (id INTEGER)",
    )
    .unwrap();

    let ms = rsbatis_migrate::from_dir(&dir).expect("加载");
    assert_eq!(ms.len(), 2);
    assert_eq!(ms[0].version, 20260105001);
    assert_eq!(ms[0].name, "create_users");
    assert_eq!(ms[0].down.as_deref(), Some("DROP TABLE users"));
    assert_eq!(ms[1].version, 20260106001);
    assert!(ms[1].down.is_none(), "没写 down 文件就是不可回滚");

    // 真跑一遍。
    let mut s = db();
    let mut m = Migrator::new(&mut s, ms).expect("构造");
    assert_eq!(m.up().expect("up"), vec![20260105001, 20260106001]);
    assert!(table_exists(m.session_mut(), "users"));

    // 栈顶是 20260106001，它没有 down 文件 → 拒绝回滚，而不是"回滚了一个不存在的动作"。
    let err = m.down().expect_err("栈顶没有 down 脚本");
    assert!(
        matches!(&err, MigrateError::Invalid(x) if x.contains("没有 down 脚本")),
        "实际：{err}"
    );
    drop(m);

    // 补上 down 文件后重新加载，就能正常回滚。
    std::fs::write(
        dir.join("20260106001_create_orders.down.sql"),
        "DROP TABLE orders",
    )
    .unwrap();
    let ms = rsbatis_migrate::from_dir(&dir).expect("重新加载");
    let mut m = Migrator::new(&mut s, ms).expect("构造");
    assert_eq!(m.down().expect("down"), Some(20260106001));
    assert!(!table_exists(m.session_mut(), "orders"));

    // 坏文件名要报错并指出文件名。
    std::fs::write(dir.join("bad-name.sql"), "CREATE TABLE x (id INTEGER)").unwrap();
    let err = rsbatis_migrate::from_dir(&dir).expect_err("文件名不合规");
    assert!(
        matches!(&err, MigrateError::Invalid(m) if m.contains("bad-name.sql")),
        "实际：{err}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
