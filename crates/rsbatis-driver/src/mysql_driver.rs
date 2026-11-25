//! MySQL 驱动（`mysql` crate，同步，纯 Rust 协议实现）。
//!
//! # 为什么不用 `mysql_async`
//!
//! 设计文档最初写的是 `mysql_async`，理由是"网络 IO 应该异步"。但落地时发现
//! 两条约束：
//!
//! 1. `Driver` trait 是同步的（SQLite 必须同步，见 `sqlite.rs` 的理由），
//!    强行让两个驱动不同步会让 M4 的 `Session` 出现两套接口；
//! 2. `mysql` crate 的 `Params::Positional(Vec<Value>)` 天然支持**运行时长度**，
//!    这正是 E2 证明必须的能力，而它不需要 async 运行时就能验证。
//!
//! 所以 MVP 用同步 `mysql`。异步化（tokio + `spawn_blocking`，或换 `mysql_async`）
//! 是 M4 的 D1 设计点，届时统一处理——接口不会变，只换内部实现。
//!
//! # 类型映射的已知妥协
//!
//! MySQL 协议把字符串和二进制都放在 `Bytes` 里，**无法从协议层区分 `TEXT` 与 `BLOB`**。
//! 因此读回时先尝试 UTF-8 解码：成功当作 `Value::Text`，失败当作 `Value::Bytes`。
//! 这意味着"存了非法 UTF-8 的 TEXT 列"会被误判成 BLOB——这是协议限制，不是实现缺陷，
//! 已在文档中标注。

use ::mysql::prelude::Queryable;
use ::mysql::{Conn, Opts, Value as MyValue};
use rsbatis_core::{Dialect, Sql};

use crate::error::{DriverError, ErrorKind};
use crate::row::Row;
use crate::value_conv::{from_mysql_value, mysql_params};
use crate::Driver;

pub struct MySqlDriver {
    conn: Conn,
    /// mysql crate 没有 `is_in_transaction()`，只能自己维护。
    /// 只有 `begin` 成功才置 true，`commit` / `rollback` 成功才置 false——
    /// 失败时保持原值，避免把"事务其实还活着"误报成已结束。
    in_tx: bool,
}

impl MySqlDriver {
    /// 用 URL 连接：`mysql://user:pass@127.0.0.1:3306/db`
    ///
    /// `mysql+co://` 是同一条路径，但底层 socket 走装的钩子（rsboot 里就成了
    /// 协程：等数据库响应时让出 worker）。钩子没装时两者行为完全相同，
    /// 只是多一层间接——所以默认用 `mysql://`，要协程化再显式写 `mysql+co://`。
    pub fn connect(url: &str) -> Result<Self, DriverError> {
        let (url, co) = match url.strip_prefix("mysql+co://") {
            Some(rest) => (format!("mysql://{rest}"), true),
            None => (url.to_string(), false),
        };
        ::mysql::set_co_mode(co);
        let result = Opts::from_url(&url)
            .map_err(|e| DriverError::new(ErrorKind::Connection, format!("URL 解析失败：{e}")))
            .and_then(|opts| Conn::new(opts).map_err(map_err));
        // 无论成败都要还原：建连是一次性的，之后这个线程不该还带着标志
        ::mysql::set_co_mode(false);
        let conn = result?;
        Ok(Self { conn, in_tx: false })
    }

    pub fn from_conn(conn: Conn) -> Self {
        Self { conn, in_tx: false }
    }

    pub fn conn(&self) -> &Conn {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Conn {
        &mut self.conn
    }

    /// 执行 DDL / 原生 SQL（迁移工具用）。
    pub fn execute_batch(&mut self, sql: &str) -> Result<(), DriverError> {
        // 不能整段丢给 `query_drop`：多语句批处理会吞掉中间语句的错误并返回 Ok，
        // 迁移工具据此记账会记成"已应用"，而库里只是半成品。逐条执行、逐条检查。
        for (idx, stmt) in crate::sql_split::split_statements(sql)
            .into_iter()
            .enumerate()
        {
            self.conn.query_drop(stmt).map_err(|e| {
                let err = map_err(e);
                let n = idx + 1;
                DriverError::new(
                    err.kind,
                    format!("批处理第 {n} 条语句失败：{}", err.message),
                )
            })?;
        }
        Ok(())
    }

    /// 当前连接所选库名，仅用于诊断输出。
    pub fn database(&mut self) -> Result<String, DriverError> {
        self.conn
            .query_first::<String, _>("SELECT DATABASE()")
            .map_err(map_err)?
            .ok_or_else(|| DriverError::new(ErrorKind::Other, "DATABASE() 返回空"))
    }

    // ---- 显式事务（M4 会包成 RAII 的 Transaction） ----

    pub fn begin(&mut self) -> Result<(), DriverError> {
        self.conn.query_drop("START TRANSACTION").map_err(map_err)?;
        self.in_tx = true;
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), DriverError> {
        self.conn.query_drop("COMMIT").map_err(map_err)?;
        self.in_tx = false;
        Ok(())
    }

    pub fn rollback(&mut self) -> Result<(), DriverError> {
        self.conn.query_drop("ROLLBACK").map_err(map_err)?;
        self.in_tx = false;
        Ok(())
    }

    /// MySQL 没有 `RETURNING`，插入后要单独取：`SELECT LAST_INSERT_ID()`。
    pub fn last_insert_id(&mut self) -> Result<i64, DriverError> {
        self.conn
            .query_first::<i64, _>(LAST_INSERT_ID_SQL)
            .map_err(map_err)?
            .ok_or_else(|| DriverError::new(ErrorKind::Other, "LAST_INSERT_ID() 返回空"))
    }
}

impl Driver for MySqlDriver {
    fn last_insert_id(&mut self) -> Result<i64, DriverError> {
        MySqlDriver::last_insert_id(self)
    }

    fn reset_last_insert_id(&mut self) -> Result<(), DriverError> {
        // `LAST_INSERT_ID(0)` 直接设置会话值；`DO` 语法避免把结果集当查询结果用
        self.conn
            .query_drop("DO LAST_INSERT_ID(0)")
            .map_err(map_err)
    }

    fn begin(&mut self) -> Result<(), DriverError> {
        MySqlDriver::begin(self)
    }
    fn commit(&mut self) -> Result<(), DriverError> {
        MySqlDriver::commit(self)
    }
    fn rollback(&mut self) -> Result<(), DriverError> {
        MySqlDriver::rollback(self)
    }

    fn execute_batch(&mut self, sql: &str) -> Result<(), DriverError> {
        MySqlDriver::execute_batch(self, sql)
    }

    fn in_transaction(&self) -> bool {
        self.in_tx
    }

    fn execute(&mut self, sql: &Sql) -> Result<usize, DriverError> {
        let params = mysql_params(&sql.params);
        // exec_drop 会把结果集彻底消费掉并留下 OK 包；
        // 行数必须从 Conn::affected_rows() 读——QueryResult 上的同名字段在
        // prepared statement 的 DML 路径上不会被填充（实测恒为 0）。
        self.conn
            .exec_drop(sql.text.as_str(), params)
            .map_err(map_err)?;
        Ok(self.conn.affected_rows() as usize)
    }

    fn fetch(&mut self, sql: &Sql) -> Result<Vec<Row>, DriverError> {
        let params = mysql_params(&sql.params);
        let result = self
            .conn
            .exec_iter(sql.text.as_str(), params)
            .map_err(map_err)?;

        let names: Vec<String> = result
            .columns()
            .as_ref()
            .iter()
            .map(|c| c.name_str().to_string())
            .collect();

        let mut out = Vec::new();
        for row in result {
            let r = row.map_err(map_err)?;
            // Row::unwrap() 给出 Vec<Value>，顺序与列一致
            let vals: Vec<rsbatis_core::Value> =
                r.unwrap().into_iter().map(from_mysql_value).collect();
            out.push(Row::new(names.clone(), vals));
        }
        Ok(out)
    }

    fn dialect(&self) -> Dialect {
        Dialect::MySql
    }
}

/// MySQL 错误码 → 分类。
///
/// `1062` 重复键、`1452` 外键约束失败、`1048` 非空约束、`3819` CHECK 约束，
/// 与 SQLite 的 extended code、PostgreSQL 的 SQLSTATE 一起归一到同一组 `ErrorKind`。
pub fn map_err(e: ::mysql::Error) -> DriverError {
    match &e {
        ::mysql::Error::MySqlError(me) => {
            let kind = match me.code {
                1062 => ErrorKind::UniqueViolation,
                1452 => ErrorKind::ForeignKeyViolation,
                1048 | 3819 | 1264 => ErrorKind::ConstraintViolation,
                _ => ErrorKind::Sql,
            };
            DriverError::new(kind, format!("[{}] {}", me.code, me.message))
        }
        other => DriverError::new(ErrorKind::Other, other.to_string()),
    }
}

/// 供上层判断的辅助值：MySQL 没有 `RETURNING`，要用 `LAST_INSERT_ID()`。
pub const LAST_INSERT_ID_SQL: &str = "SELECT LAST_INSERT_ID()";

/// 把一个 `Value` 手工转成 MySQL 参数（公开给测试用）。
pub fn param(v: &rsbatis_core::Value) -> MyValue {
    crate::value_conv::to_mysql_param(v)
}
