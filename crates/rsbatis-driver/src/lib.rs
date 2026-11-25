//! 驱动层：把 `rsbatis_core::Sql` 真正执行掉。
//!
//! 这一层的存在理由只有一个：**参数个数在运行时才知道**。
//!
//! `where_!` 展开的条件可能是 0 个参数，也可能是 20 个。任何"泛型逐次累加
//! 绑定"的方案（如 `sqlx` 的 `bind()` 每调一次就换一个类型）都无法表达这种情况，
//! 因为它要求编译期知道参数个数。所以这里必须用"动态参数切片"：
//!
//! - SQLite：`&[&dyn ToSql]`
//! - MySQL：`Vec<mysql_async::Value>`
//! - PostgreSQL：`&[&(dyn ToSql + Sync)]`
//!
//! 三者都接受运行时决定的长度，这就是选直连驱动而不是 sqlx 的**唯一**原因
//! （E2 已实测验证）。
//!
//! # 当前状态（0.1.0-alpha.2）
//!
//! 已实现：SQLite（`feature = "sqlite"`，同步，基于 `rusqlite`）。
//! 未实现：MySQL、PostgreSQL、异步执行、连接池。

#![forbid(unsafe_code)]

pub mod error;
pub mod row;
pub mod sql_split;
pub mod value_conv;

#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(feature = "mysql")]
pub mod mysql_driver;

pub use error::{DriverError, ErrorKind};

/// 装 MySQL socket 的读写钩子。
///
/// 由应用框架在 worker 启动时调用：传进来的函数替代 `TcpStream` 的阻塞读写，
/// 于是数据库等待也能让出 worker。**不装就是上游的阻塞行为**，功能不受影响。
#[cfg(feature = "mysql")]
pub fn set_mysql_socket_hooks(
    read: ::mysql::ReadHook,
    write: ::mysql::WriteHook,
) {
    ::mysql::set_io_hooks(read, write);
}
pub use row::Row;

use rsbatis_core::Sql;

/// 执行器的最小契约。
///
/// 用 `&mut self` 而不是 `&self`：SQLite 的 `Connection` 内部有可变语句缓存，
/// MySQL 的异步连接同理。连接池由上层（`Session`）管理，驱动本身不做池化。
pub trait Driver {
    /// 执行不返回结果集的语句，返回受影响行数。
    fn execute(&mut self, sql: &Sql) -> Result<usize, DriverError>;

    /// 执行查询，把所有行读进内存。
    ///
    /// 故意不提供流式迭代器：MVP 阶段"一次性读回"最简单，也避开了
    /// 语句生命周期与借用检查的纠缠。流式接口留给 v1.0（`Cursor`）。
    fn fetch(&mut self, sql: &Sql) -> Result<Vec<Row>, DriverError>;

    /// 方言标识，用于上层决定 `RETURNING` / `ON CONFLICT` 是否可用。
    fn dialect(&self) -> rsbatis_core::Dialect;

    /// 该驱动是否支持 `RETURNING`（等价于 `dialect().supports_returning()`，
    /// 但允许驱动层覆盖，例如 SQLite 版本低于 3.35 时）。
    fn supports_returning(&self) -> bool {
        self.dialect().supports_returning()
    }

    /// 最近一次 `INSERT` 生成的主键值（自增表用）。
    ///
    /// 语义与两个引擎各自的实现一致：
    /// - SQLite：`last_insert_rowid()`，即内部 rowid
    /// - MySQL/MariaDB：`LAST_INSERT_ID()`，连接级
    ///
    /// **不是"业务主键"的可靠来源**：SQLite 的 rowid 与 `TEXT PRIMARY KEY` 之类无关；
    /// MySQL 在没有自增列的表上插入后不会更新这个值（仍返回上一次的）。
    /// 所以上层只在没有 `RETURNING` 可用时才走这条路，并做"前后必须变化"的检测。
    fn last_insert_id(&mut self) -> Result<i64, DriverError> {
        Err(unsupported("last_insert_id"))
    }

    /// 把"最近一次自增 id"重置为 0。
    ///
    /// **为什么必须有这个**：`LAST_INSERT_ID()` 是连接级的，父表与子表的自增序列会互相
    /// 覆盖。父 1 拿到 1、子表插完拿到 2，父 2 插进去恰好也是 2 —— 前后一比对"没变化"，
    /// 于是把一次合法插入误判成失败。重置之后读到的值才必然是"本次插入"的。
    ///
    /// SQLite 没有对应的重置 API，所以那边只能走 `RETURNING` 拿精确值。
    fn reset_last_insert_id(&mut self) -> Result<(), DriverError> {
        Err(unsupported("reset_last_insert_id"))
    }

    // ---- 事务 ----
    //
    // 默认实现一律返回 `Unsupported`，而不是"假装成功"。事务语义一旦静默降级，
    // 上层 RAII 的回滚承诺就是假的——那比不支持更危险。
    // 具体驱动覆盖这三个方法，`Session::transaction` 才能构造出来。

    fn begin(&mut self) -> Result<(), DriverError> {
        Err(unsupported("BEGIN"))
    }
    fn commit(&mut self) -> Result<(), DriverError> {
        Err(unsupported("COMMIT"))
    }
    fn rollback(&mut self) -> Result<(), DriverError> {
        Err(unsupported("ROLLBACK"))
    }

    // ---- DDL / 诊断 ----

    /// 执行一批可能含多条语句的 SQL 文本（建表、迁移脚本）。
    ///
    /// 默认实现只支持单条语句；具体驱动如果自带多语句能力就覆盖它。
    fn execute_batch(&mut self, sql: &str) -> Result<(), DriverError> {
        self.execute(&Sql::raw(sql)).map(|_| ())
    }

    /// 当前连接是否处于显式事务中。
    ///
    /// 连接池在归还连接前用它判断是否需要回滚——带着未结束的事务把连接还回池里，
    /// 下一个借到它的人会莫名其妙地"继承"一个事务。
    fn in_transaction(&self) -> bool {
        false
    }
}

fn unsupported(what: &str) -> DriverError {
    DriverError::new(
        ErrorKind::Unsupported,
        format!("当前驱动未实现 {what}：该连接不支持显式事务"),
    )
}

/// 给上层用的便利方法：先 `build` 再执行。
pub fn execute_stmt(d: &mut dyn Driver, sql: &Sql) -> Result<usize, DriverError> {
    d.execute(sql)
}

#[cfg(test)]
mod tests {
    //! `Driver` trait 默认实现的契约测试。
    //!
    //! 这几段默认实现此前**一行都没被执行过**（`lib.rs` 覆盖率 0%）。它们恰恰
    //! 是安全网：如果哪天有人把"默认返回 Unsupported"改成"默认假装成功"，
    //! 上层的 RAII 回滚承诺就会静默失效。这些用例就是钉住这个承诺的钉子。

    use super::*;
    use rsbatis_core::Dialect;

    /// 只实现必需三个方法的最小驱动：其余能力全部落到 trait 默认实现上。
    struct Minimal;

    impl Driver for Minimal {
        fn execute(&mut self, _sql: &Sql) -> Result<usize, DriverError> {
            Ok(1)
        }
        fn fetch(&mut self, _sql: &Sql) -> Result<Vec<Row>, DriverError> {
            Ok(Vec::new())
        }
        fn dialect(&self) -> Dialect {
            Dialect::Sqlite
        }
    }

    #[test]
    fn default_tx_methods_refuse_instead_of_pretending() {
        let mut d = Minimal;
        for r in [d.begin(), d.commit(), d.rollback()] {
            let err = r.expect_err("默认实现必须拒绝，不能假装成功");
            assert_eq!(ErrorKind::Unsupported, err.kind);
            // 消息要带上被拒绝的具体语句，否则排障时不知道是谁没实现
            assert!(
                err.message.contains("BEGIN")
                    || err.message.contains("COMMIT")
                    || err.message.contains("ROLLBACK"),
                "错误消息缺少语句名：{}",
                err.message
            );
        }
    }

    #[test]
    fn default_supports_returning_follows_dialect() {
        // SQLite 3.35+ 支持 RETURNING，默认实现应当跟随方言回答 true
        assert!(Minimal.supports_returning());
    }

    #[test]
    fn default_execute_batch_delegates_to_execute() {
        let mut d = Minimal;
        // 默认实现把整段文本当一条语句执行——多语句能力由具体驱动自己覆盖
        // 默认实现把整段文本当一条语句执行——返回值是 `()`，不代表受影响行数
        d.execute_batch("SELECT 1").expect("单条语句应成功");
    }

    #[test]
    fn default_in_transaction_is_false() {
        // 不实现事务的驱动永远不在事务里，连接池据此判断是否需要回滚
        assert!(!Minimal.in_transaction());
    }

    #[test]
    fn execute_stmt_helper_forwards_to_driver() {
        let mut d = Minimal;
        assert_eq!(1, execute_stmt(&mut d, &Sql::raw("SELECT 1")).unwrap());
    }
}
