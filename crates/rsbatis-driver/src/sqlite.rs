//! SQLite 驱动（`rusqlite`，同步）。
//!
//! 为什么是同步：`rusqlite` 的 `Connection` 不是线程安全的可变对象，硬塞进 async
//! 里要么 `spawn_blocking`、要么加锁，两者都会污染 API。SQLite 本身是嵌入式库、
//! 查询是毫秒级，同步调用反而是正确抽象。异步执行留给 MySQL / PostgreSQL。
//!
//! 想在上层统一，交给 `Session`（M4）：它会按驱动类型决定要不要 `spawn_blocking`。

use std::path::Path;

use rsbatis_core::{Dialect, Sql};
use rusqlite::{params_from_iter, Connection, ErrorCode, OpenFlags};

use crate::error::{DriverError, ErrorKind};
use crate::row::Row;
use crate::value_conv::{from_sqlite_ref, sqlite_params};
use crate::Driver;

pub struct SqliteDriver {
    conn: Connection,
}

impl SqliteDriver {
    /// 打开内存库（测试 / 演示用）。
    pub fn open_in_memory() -> Result<Self, DriverError> {
        let conn = Connection::open_in_memory().map_err(map_err)?;
        Ok(Self { conn })
    }

    /// 打开文件库，不存在则创建。
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, DriverError> {
        let conn = Connection::open(path).map_err(map_err)?;
        Ok(Self { conn })
    }

    /// 只读打开（用于生产环境"不小心写库"的防护）。
    pub fn open_read_only<P: AsRef<Path>>(path: P) -> Result<Self, DriverError> {
        let conn =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(map_err)?;
        Ok(Self { conn })
    }

    /// 执行 DDL / 多条语句（`CREATE TABLE`、`PRAGMA`）。
    ///
    /// 这是唯一吃裸字符串的入口：建表语句不在 `Expr` 的语义范围内，
    /// 迁移工具（`rsbatis-migrate`）会用它。
    pub fn execute_batch(&self, sql: &str) -> Result<(), DriverError> {
        self.conn.execute_batch(sql).map_err(map_err)
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// 显式事务：`BEGIN` / `COMMIT` / `ROLLBACK` 由调用方控制。
    /// M4 会把它包成 RAII 的 `Transaction`。
    pub fn begin(&self) -> Result<(), DriverError> {
        self.conn.execute_batch("BEGIN").map_err(map_err)
    }
    pub fn commit(&self) -> Result<(), DriverError> {
        self.conn.execute_batch("COMMIT").map_err(map_err)
    }
    pub fn rollback(&self) -> Result<(), DriverError> {
        self.conn.execute_batch("ROLLBACK").map_err(map_err)
    }

    /// 读 `PRAGMA user_version`，迁移工具用它记录版本。
    pub fn user_version(&self) -> Result<i64, DriverError> {
        self.conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .map_err(map_err)
    }

    pub fn set_user_version(&self, v: i64) -> Result<(), DriverError> {
        self.conn
            .execute_batch(&format!("PRAGMA user_version = {v}"))
            .map_err(map_err)
    }
}

impl Driver for SqliteDriver {
    // 固有方法接 `&self`（rusqlite 的 execute_batch 就是 &self），
    // 这里转发以满足 Driver 的 `&mut self` 契约。
    fn begin(&mut self) -> Result<(), DriverError> {
        SqliteDriver::begin(self)
    }
    fn commit(&mut self) -> Result<(), DriverError> {
        SqliteDriver::commit(self)
    }
    fn rollback(&mut self) -> Result<(), DriverError> {
        SqliteDriver::rollback(self)
    }

    fn execute_batch(&mut self, sql: &str) -> Result<(), DriverError> {
        self.conn.execute_batch(sql).map_err(map_err)
    }

    fn last_insert_id(&mut self) -> Result<i64, DriverError> {
        Ok(self.conn.last_insert_rowid())
    }

    /// rusqlite 自带权威判断：autocommit 关闭就说明在事务里。
    fn in_transaction(&self) -> bool {
        !self.conn.is_autocommit()
    }

    fn execute(&mut self, sql: &Sql) -> Result<usize, DriverError> {
        let params = sqlite_params(&sql.params);
        let n = self
            .conn
            .execute(
                &sql.text,
                params_from_iter(params.iter().map(|p| p.as_ref())),
            )
            .map_err(map_err)?;
        Ok(n)
    }

    fn fetch(&mut self, sql: &Sql) -> Result<Vec<Row>, DriverError> {
        let params = sqlite_params(&sql.params);
        let mut stmt = self.conn.prepare(&sql.text).map_err(map_err)?;

        let ncols = stmt.column_count();
        // rusqlite 0.43 的 column_names() 返回迭代器而不是 Vec
        let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();

        let mut rows = stmt
            .query(params_from_iter(params.iter().map(|p| p.as_ref())))
            .map_err(map_err)?;

        let mut out = Vec::new();
        while let Some(r) = rows.next().map_err(map_err)? {
            let mut vals = Vec::with_capacity(ncols);
            for i in 0..ncols {
                vals.push(from_sqlite_ref(r.get_ref(i).map_err(map_err)?));
            }
            out.push(Row::new(names.clone(), vals));
        }
        Ok(out)
    }

    fn dialect(&self) -> Dialect {
        Dialect::Sqlite
    }
}

/// `rusqlite::Error` → `DriverError`，把约束冲突按类别归出来。
///
/// 唯一键冲突在 SQLite 里的 extended code 是 2067（`SQLITE_CONSTRAINT_UNIQUE`），
/// 这是上层做"插入重复 → 返回已存在记录"逻辑的依据。
fn map_err(e: rusqlite::Error) -> DriverError {
    match &e {
        rusqlite::Error::SqliteFailure(ffi, msg) => {
            let msg = msg.clone().unwrap_or_else(|| ffi.to_string());
            let kind = match ffi.code {
                ErrorCode::ConstraintViolation => match ffi.extended_code {
                    2067 => ErrorKind::UniqueViolation,    // SQLITE_CONSTRAINT_UNIQUE
                    787 => ErrorKind::ForeignKeyViolation, // SQLITE_CONSTRAINT_FOREIGNKEY
                    _ => ErrorKind::ConstraintViolation,
                },
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => ErrorKind::Connection,
                _ => ErrorKind::Sql,
            };
            DriverError::new(kind, msg)
        }
        rusqlite::Error::InvalidColumnIndex(i) => {
            DriverError::new(ErrorKind::ColumnNotFound, format!("列下标 {i} 无效"))
        }
        rusqlite::Error::InvalidColumnName(n) => DriverError::column_not_found(n),
        other => DriverError::new(ErrorKind::Other, other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsbatis_core::{insert, select, Column, Entity};

    struct User;
    impl Entity for User {
        const TABLE: &'static str = "user";
    }

    fn conn() -> SqliteDriver {
        let d = SqliteDriver::open_in_memory().unwrap();
        d.execute_batch(
            r#"CREATE TABLE "user" (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER);"#,
        )
        .unwrap();
        d
    }

    #[test]
    fn insert_then_select() {
        let mut d = conn();
        let ins = insert::<User>()
            .set(Column::<String>::new("user", "name"), "a")
            .set(Column::<i32>::new("user", "age"), 20)
            .build(Dialect::Sqlite);
        assert_eq!(1, d.execute(&ins).unwrap());

        let q = select::<User>()
            .where_(Column::<i32>::new("user", "age").ge(18))
            .build(Dialect::Sqlite);
        let rows = d.fetch(&q).unwrap();
        assert_eq!(1, rows.len());
        assert_eq!(
            "a".to_string(),
            rows[0].get_by_name::<String>("name").unwrap()
        );
    }

    #[test]
    fn unique_violation_is_classified() {
        let mut d = SqliteDriver::open_in_memory().unwrap();
        d.execute_batch(r#"CREATE TABLE "t" (id INTEGER PRIMARY KEY, v TEXT UNIQUE);"#)
            .unwrap();
        d.execute_batch("INSERT INTO t (v) VALUES ('a')").unwrap();
        let dup = Sql {
            text: "INSERT INTO t (v) VALUES ('a')".into(),
            params: vec![],
            warnings: vec![],
        };
        let err = d.execute(&dup).unwrap_err();
        assert_eq!(ErrorKind::UniqueViolation, err.kind);
    }
}
