//! 迁移元数据的持久化。
//!
//! 表名带下划线前缀 `_rsbatis_migrations`，与业务表区分开，也不容易撞名。

use rsbatis_core::{Dialect, Sql, Value};
use rsbatis_driver::{Driver, Row};
use rsbatis_orm::Session;

use crate::error::MigrateError;

pub const MIGRATIONS_TABLE: &str = "_rsbatis_migrations";

/// 一条已应用的记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub version: i64,
    pub name: String,
}

fn create_table_sql(d: Dialect) -> String {
    let q = d.quote(MIGRATIONS_TABLE);
    match d {
        Dialect::MySql => format!(
            "CREATE TABLE IF NOT EXISTS {q} (
               version BIGINT PRIMARY KEY,
               name VARCHAR(255) NOT NULL,
               applied_at DATETIME NOT NULL
             ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"
        ),
        Dialect::Sqlite => format!(
            "CREATE TABLE IF NOT EXISTS {q} (
               version INTEGER PRIMARY KEY,
               name TEXT NOT NULL,
               applied_at TEXT NOT NULL
             )"
        ),
        Dialect::Postgres => format!(
            "CREATE TABLE IF NOT EXISTS {q} (
               version BIGINT PRIMARY KEY,
               name TEXT NOT NULL,
               applied_at TIMESTAMPTZ NOT NULL
             )"
        ),
    }
}

fn now_expr(d: Dialect) -> &'static str {
    match d {
        Dialect::MySql => "NOW()",
        Dialect::Sqlite => "datetime('now')",
        Dialect::Postgres => "now()",
    }
}

pub fn ensure<D: Driver>(session: &mut Session<D>) -> Result<(), MigrateError> {
    let ddl = create_table_sql(session.dialect());
    session.execute_batch(&ddl)?;
    Ok(())
}

pub fn applied<D: Driver>(session: &mut Session<D>) -> Result<Vec<Applied>, MigrateError> {
    let d = session.dialect();
    let sql = format!(
        "SELECT version, name FROM {} ORDER BY version ASC",
        d.quote(MIGRATIONS_TABLE)
    );
    let rows: Vec<Row> = session.fetch(Sql::raw(sql))?;
    rows.iter()
        .map(|r| {
            Ok(Applied {
                version: r.get_by_name("version")?,
                name: r.get_by_name("name")?,
            })
        })
        .collect::<Result<Vec<_>, rsbatis_driver::DriverError>>()
        .map_err(MigrateError::from)
}

/// 记录一支迁移已应用。返回的是 `Sql`，由调用方在**同一个事务**里执行——
/// 迁移 SQL 与元数据写入必须同生共死，否则会出现"表建好了但没记账"。
pub fn record_up_sql(d: Dialect, version: i64, name: &str) -> Sql {
    Sql {
        text: format!(
            "INSERT INTO {} (version, name, applied_at) VALUES ({}, {}, {})",
            d.quote(MIGRATIONS_TABLE),
            d.placeholder(1),
            d.placeholder(2),
            now_expr(d)
        ),
        params: vec![Value::I64(version), Value::Text(name.to_string())],
        warnings: Vec::new(),
    }
}

pub fn record_down_sql(d: Dialect, version: i64) -> Sql {
    Sql {
        text: format!(
            "DELETE FROM {} WHERE version = {}",
            d.quote(MIGRATIONS_TABLE),
            d.placeholder(1)
        ),
        params: vec![Value::I64(version)],
        warnings: Vec::new(),
    }
}
