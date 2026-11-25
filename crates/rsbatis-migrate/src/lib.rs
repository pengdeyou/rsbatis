//! # rsbatis-migrate
//!
//! 版本化 SQL 迁移：`up` / `down` / `status` / `rollback`。
//!
//! 设计上只做三件确定的事：
//! 1. **迁移就是 SQL 文本 + 版本号**，不做 DDL 的语义解析（那需要完整的 DDL 语法树，
//!    而且解析错了比不解析更危险）；
//! 2. **元数据落库**（`_rsbatis_migrations` 表），而不是靠 `PRAGMA user_version`：
//!    后者只能存一个整数，无法回答"哪一支迁移在哪天应用的"、也无法回滚到中间版本；
//! 3. **每个迁移包在事务里执行**。SQLite / PostgreSQL 的 DDL 可回滚，MySQL 的 DDL
//!    隐式提交（这是数据库本身的限制，不是我们的取舍），这一点会在
//!    `Sql::warnings` 与文档里明确写出来，不假装原子。

#![forbid(unsafe_code)]

mod error;
mod migration;
mod migrator;
mod store;

pub use error::MigrateError;
pub use migration::{from_dir, Migration};
pub use migrator::{Migrator, Status};
pub use store::MIGRATIONS_TABLE;
