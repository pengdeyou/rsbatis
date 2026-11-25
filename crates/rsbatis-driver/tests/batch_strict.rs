//! 批处理必须严格：中间语句失败要报错，且切分不能被引号/注释里的分号骗到。
//!
//! 这个文件来自一个真实缺陷：MySQL 的 `query_drop` 整段执行多语句时，
//! 中间失败返回 `Ok(())`，迁移工具会把半成品记成"已应用"。

use rsbatis_driver::sql_split::split_statements;

#[test]
fn splitter_respects_quotes_and_comments() {
    let sql = "INSERT INTO t VALUES ('a;b'); \
               SELECT 1; -- 注释里的分号 ; 不算\n \
               /* 块注释 ; */ CREATE TABLE `x;y` (id INT);";
    let parts = split_statements(sql);
    assert_eq!(parts.len(), 3, "实际：{parts:#?}");
    assert_eq!(parts[0], "INSERT INTO t VALUES ('a;b')");
    assert_eq!(parts[1], "SELECT 1");
    assert!(
        parts[2].contains("CREATE TABLE `x;y`"),
        "实际：{}",
        parts[2]
    );
}

#[test]
fn splitter_handles_escaped_quote_and_trailing_semicolon() {
    let parts = split_statements("INSERT INTO t VALUES ('it''s; fine');\n\n");
    assert_eq!(parts, vec!["INSERT INTO t VALUES ('it''s; fine')"]);
    assert!(split_statements("  ;;  ").is_empty());
}

#[cfg(feature = "sqlite")]
mod sqlite_side {
    use rsbatis_driver::sqlite::SqliteDriver;
    use rsbatis_driver::{Driver, ErrorKind};

    #[test]
    fn sqlite_mid_statement_error_is_reported() {
        let d = SqliteDriver::open_in_memory().unwrap();
        let err = d
            .execute_batch("CREATE TABLE a (id INTEGER); INSERT INTO nope VALUES (1)")
            .expect_err("第二条必然失败");
        assert_eq!(err.kind, ErrorKind::Sql, "实际：{err:?}");
    }

    #[test]
    fn sqlite_semicolon_inside_string_is_not_a_boundary() {
        let mut d = SqliteDriver::open_in_memory().unwrap();
        d.execute_batch("CREATE TABLE t (id INTEGER, v TEXT); INSERT INTO t VALUES (1, 'a;b');")
            .expect("引号里的分号不能断开");
        let rows = d
            .fetch(&rsbatis_core::Sql::raw("SELECT v FROM t"))
            .expect("查");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get_by_name::<String>("v").unwrap(), "a;b");
    }
}

#[cfg(feature = "mysql")]
mod mysql_side {
    use rsbatis_driver::mysql_driver::MySqlDriver;
    use rsbatis_driver::{Driver, ErrorKind};

    fn connect() -> Option<MySqlDriver> {
        let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
        Some(MySqlDriver::connect(&url).expect("连接"))
    }

    #[test]
    fn mysql_mid_statement_error_is_reported() {
        let Some(mut d) = connect() else {
            eprintln!("跳过：未设置 RSBATIS_MYSQL_URL");
            return;
        };
        let _ = d.execute_batch("DROP TABLE IF EXISTS v_batch_strict");
        let err = d
            .execute_batch(
                "CREATE TABLE v_batch_strict (id BIGINT); \
                 INSERT INTO v_batch_nope VALUES (1); \
                 CREATE TABLE v_batch_after (id BIGINT)",
            )
            .expect_err("中间那条必然失败");
        assert_eq!(err.kind, ErrorKind::Sql, "实际：{err:?}");
        assert!(
            err.message.contains("第 2 条语句失败"),
            "实际：{}",
            err.message
        );
        // 失败后不应继续执行第 3 条——这正是旧实现做不到的事。
        let rows = d
            .fetch(&rsbatis_core::Sql::raw(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = DATABASE() AND table_name = 'v_batch_after'",
            ))
            .expect("查");
        assert!(rows.is_empty(), "第 2 条失败后第 3 条不该被执行");
        let _ = d.execute_batch("DROP TABLE IF EXISTS v_batch_strict");
    }
}
