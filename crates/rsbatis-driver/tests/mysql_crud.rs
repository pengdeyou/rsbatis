//! MySQL 端到端测试：core 生成 SQL → `MySqlDriver` 在真实 MariaDB/MySQL 上执行。
//!
//! 需要真实库，通过环境变量启用：
//!
//! ```bash
//! export RSBATIS_MYSQL_URL='mysql://rsbatis:rsbatis@127.0.0.1:3306/rsbatis_test'
//! cargo test -p rsbatis-driver --features mysql -- --test-threads=1
//! ```
//!
//! 没有设置环境变量时测试直接返回（不算失败），这样 CI 不需要 MySQL 也能绿。
//! `--test-threads=1` 是必须的：多个测试共用同一张表，并行会互相清表。

use rsbatis_core::{delete, insert, select, update, Column, Dialect, Entity, Expr, Sql, Value};
use rsbatis_driver::mysql_driver::MySqlDriver;
use rsbatis_driver::{Driver, ErrorKind};

struct RsbUser;

impl Entity for RsbUser {
    // 用 rsb_user 而不是 user：MySQL 自带 mysql.user 表，避免歧义
    const TABLE: &'static str = "rsb_user";
    const COLUMNS: &'static [&'static str] = &["id", "name", "age"];
}

fn id() -> Column<i64> {
    Column::new("rsb_user", "id")
}
fn name() -> Column<String> {
    Column::new("rsb_user", "name")
}
fn age() -> Column<i32> {
    Column::new("rsb_user", "age")
}

/// 没有 RSBATIS_MYSQL_URL 就跳过。
fn url() -> Option<String> {
    match std::env::var("RSBATIS_MYSQL_URL") {
        Ok(u) if !u.trim().is_empty() => Some(u),
        _ => {
            eprintln!("跳过：未设置 RSBATIS_MYSQL_URL");
            None
        }
    }
}

fn fresh_db(conn: &mut MySqlDriver) {
    conn.execute_batch("DROP TABLE IF EXISTS rsb_user")
        .expect("drop");
    conn.execute_batch(
        "CREATE TABLE rsb_user (
            id   BIGINT AUTO_INCREMENT PRIMARY KEY,
            name VARCHAR(64) NOT NULL,
            age  INT NULL
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .expect("create");
}

fn seed(conn: &mut MySqlDriver) {
    for (n, a) in [("张三", 20), ("李四", 17), ("王五", 30)] {
        let sql = insert::<RsbUser>()
            .set(name(), n)
            .set(age(), a)
            .build(Dialect::MySql);
        assert!(sql.warnings.is_empty());
        conn.execute(&sql).expect("insert");
    }
}

#[test]
fn connect_and_select_database() {
    let Some(u) = url() else { return };
    let mut conn = MySqlDriver::connect(&u).expect("connect");
    assert!(!conn.database().unwrap().is_empty());
}

#[test]
fn full_crud_on_mysql() {
    let Some(u) = url() else { return };
    let mut conn = MySqlDriver::connect(&u).expect("connect");
    fresh_db(&mut conn);
    seed(&mut conn);

    // ---- 读 ----
    let all = conn
        .fetch(&select::<RsbUser>().build(Dialect::MySql))
        .expect("select all");
    assert_eq!(3, all.len(), "应读回 3 行");
    assert_eq!(
        vec!["id", "name", "age"],
        all[0]
            .columns()
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
    );

    // ---- 条件 + 类型回读（MySQL 返回的整数是 Int，字符串是 Bytes→Text） ----
    let adults = conn
        .fetch(
            &select::<RsbUser>()
                .filter(age().ge(18))
                .build(Dialect::MySql),
        )
        .expect("select adults");
    assert_eq!(2, adults.len());
    let ages = adults
        .iter()
        .map(|r| r.get_by_name::<i32>("age").unwrap())
        .collect::<Vec<_>>();
    assert!(ages.iter().all(|a| *a >= 18));
    assert_eq!(
        "王五".to_string(),
        adults[1].get_by_name::<String>("name").unwrap()
    );

    // ---- UPDATE ----
    let upd = update::<RsbUser>()
        .set(age(), 21)
        .filter(name().eq("张三"))
        .build(Dialect::MySql);
    assert_eq!(1, conn.execute(&upd).unwrap(), "应更新 1 行");

    // ---- DELETE ----
    let del = delete::<RsbUser>()
        .filter(age().lt(18))
        .build(Dialect::MySql);
    assert_eq!(1, conn.execute(&del).unwrap(), "应删除 1 行");
    assert_eq!(
        2,
        conn.fetch(&select::<RsbUser>().build(Dialect::MySql))
            .unwrap()
            .len()
    );
}

#[test]
fn last_insert_id_replaces_returning() {
    let Some(u) = url() else { return };
    let mut conn = MySqlDriver::connect(&u).expect("connect");
    fresh_db(&mut conn);

    let ins = insert::<RsbUser>()
        .set(name(), "带回填")
        .build(Dialect::MySql);
    // builder 本身会告警：MySQL 不支持 RETURNING
    let with_ret = insert::<RsbUser>()
        .set(name(), "带回填2")
        .returning(["id"])
        .build(Dialect::MySql);
    assert!(
        with_ret.has_warning(),
        "MySQL 必须告警而非静默省略 RETURNING"
    );

    conn.execute(&ins).unwrap();
    let new_id = conn.last_insert_id().unwrap();
    assert!(new_id >= 1, "自增 id 应 >= 1");
}

#[test]
fn unique_violation_is_classified_on_mysql() {
    let Some(u) = url() else { return };
    let mut conn = MySqlDriver::connect(&u).expect("connect");
    conn.execute_batch("DROP TABLE IF EXISTS rsb_uniq").unwrap();
    conn.execute_batch(
        "CREATE TABLE rsb_uniq (id BIGINT AUTO_INCREMENT PRIMARY KEY, v VARCHAR(32) UNIQUE) ENGINE=InnoDB",
    )
    .unwrap();
    conn.execute_batch("INSERT INTO rsb_uniq (v) VALUES ('a')")
        .unwrap();

    let raw = Sql {
        text: "INSERT INTO rsb_uniq (v) VALUES ('a')".into(),
        params: vec![],
        warnings: vec![],
    };
    let err = conn.execute(&raw).unwrap_err();
    assert_eq!(
        ErrorKind::UniqueViolation,
        err.kind,
        "1062 应归一到 UniqueViolation"
    );
}

#[test]
fn foreign_key_violation_is_classified_on_mysql() {
    let Some(u) = url() else { return };
    let mut conn = MySqlDriver::connect(&u).expect("connect");
    conn.execute_batch("DROP TABLE IF EXISTS rsb_child")
        .unwrap();
    conn.execute_batch("DROP TABLE IF EXISTS rsb_parent")
        .unwrap();
    conn.execute_batch("CREATE TABLE rsb_parent (id BIGINT PRIMARY KEY) ENGINE=InnoDB")
        .unwrap();
    conn.execute_batch(
        "CREATE TABLE rsb_child (
            id BIGINT PRIMARY KEY,
            pid BIGINT,
            CONSTRAINT fk_p FOREIGN KEY (pid) REFERENCES rsb_parent(id)
         ) ENGINE=InnoDB",
    )
    .unwrap();

    let raw = Sql {
        text: "INSERT INTO rsb_child (id, pid) VALUES (1, 999)".into(),
        params: vec![],
        warnings: vec![],
    };
    let err = conn.execute(&raw).unwrap_err();
    assert_eq!(
        ErrorKind::ForeignKeyViolation,
        err.kind,
        "1452 应归一到 ForeignKeyViolation"
    );
}

#[test]
fn null_and_in_and_between() {
    let Some(u) = url() else { return };
    let mut conn = MySqlDriver::connect(&u).expect("connect");
    fresh_db(&mut conn);
    seed(&mut conn);

    // NULL 写入 + 可空读取
    let ins_null = insert::<RsbUser>()
        .set(name(), "无年龄")
        .build(Dialect::MySql);
    conn.execute(&ins_null).unwrap();
    let rows = conn
        .fetch(
            &select::<RsbUser>()
                .filter(name().eq("无年龄"))
                .build(Dialect::MySql),
        )
        .unwrap();
    assert_eq!(None, rows[0].get_by_name::<Option<i32>>("age").unwrap());
    assert_eq!(
        ErrorKind::TypeMismatch,
        rows[0].get_by_name::<i32>("age").unwrap_err().kind
    );

    // IN
    let in_sql = select::<RsbUser>()
        .filter(id().in_([1i64, 2]))
        .build(Dialect::MySql);
    let got = conn.fetch(&in_sql).unwrap();
    assert_eq!(2, got.len());

    // 空 IN → 恒假
    let empty_in = select::<RsbUser>()
        .filter(id().in_(Vec::<i64>::new()))
        .build(Dialect::MySql);
    assert_eq!(0, conn.fetch(&empty_in).unwrap().len());

    // BETWEEN
    let between = select::<RsbUser>()
        .filter(age().between(18, 25))
        .build(Dialect::MySql);
    assert_eq!(1, conn.fetch(&between).unwrap().len());
}

#[test]
fn transaction_rollback_on_mysql() {
    let Some(u) = url() else { return };
    let mut conn = MySqlDriver::connect(&u).expect("connect");
    fresh_db(&mut conn);

    conn.begin().unwrap();
    conn.execute(
        &insert::<RsbUser>()
            .set(name(), "临时")
            .build(Dialect::MySql),
    )
    .unwrap();
    conn.rollback().unwrap();

    assert_eq!(
        0,
        conn.fetch(&select::<RsbUser>().build(Dialect::MySql))
            .unwrap()
            .len(),
        "回滚后应无数据"
    );

    conn.begin().unwrap();
    conn.execute(
        &insert::<RsbUser>()
            .set(name(), "保留")
            .build(Dialect::MySql),
    )
    .unwrap();
    conn.commit().unwrap();
    assert_eq!(
        1,
        conn.fetch(&select::<RsbUser>().build(Dialect::MySql))
            .unwrap()
            .len(),
        "提交后应保留 1 行"
    );
}

#[test]
fn expr_and_order_by_on_mysql() {
    let Some(u) = url() else { return };
    let mut conn = MySqlDriver::connect(&u).expect("connect");
    fresh_db(&mut conn);
    seed(&mut conn);

    let sql = select::<RsbUser>()
        .column("name")
        .filter(Expr::and(vec![age().ge(18), Expr::not(name().like("王%"))]))
        .order_by(age().expr(), rsbatis_core::Order::Desc)
        .build(Dialect::MySql);
    let rows = conn.fetch(&sql).unwrap();
    assert_eq!(1, rows.len());
    assert_eq!(
        "张三".to_string(),
        rows[0].get_by_name::<String>("name").unwrap()
    );

    // 编码检查：中文必须原样往返（utf8mb4）
    let all = conn
        .fetch(&select::<RsbUser>().build(Dialect::MySql))
        .unwrap();
    assert!(all
        .iter()
        .any(|r| r.get_by_name::<String>("name").unwrap() == "王五"));
}

#[test]
fn bool_and_float_roundtrip() {
    let Some(u) = url() else { return };
    let mut conn = MySqlDriver::connect(&u).expect("connect");
    conn.execute_batch("DROP TABLE IF EXISTS rsb_types")
        .unwrap();
    conn.execute_batch(
        "CREATE TABLE rsb_types (id INT PRIMARY KEY, flag TINYINT(1), score DOUBLE)",
    )
    .unwrap();

    conn.execute_batch("INSERT INTO rsb_types VALUES (1, 1, 3.5)")
        .unwrap();
    let raw = Sql {
        text: "SELECT flag, score FROM rsb_types WHERE id = 1".into(),
        params: vec![],
        warnings: vec![],
    };
    let rows = conn.fetch(&raw).unwrap();
    assert!(rows[0].get_by_name::<bool>("flag").unwrap());
    assert!((rows[0].get_by_name::<f64>("score").unwrap() - 3.5).abs() < 1e-9);
    let _ = Value::Null; // 保持 import 使用
}
