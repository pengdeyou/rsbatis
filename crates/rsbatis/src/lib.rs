//! # rsbatis
//!
//! A SQLAlchemy-style ORM for Rust. Write queries that read like business code:
//!
//! ```ignore
//! let stmt = select::<User>().where_!(User.age >= 18 && User.name.like("zhang%"));
//! ```
//!
//! ## Status: 0.1.0-alpha.4
//!
//! This is an **alpha**. What actually works today:
//!
//! - expression layer: comparison, `AND` / `OR` / `NOT`, explicit parentheses preserved,
//!   `LIKE`, `IS NULL`, `IN`, `BETWEEN`, arithmetic, functions, `CASE`, `CAST`, aliases
//! - three dialects: SQLite, MySQL, PostgreSQL (quoting, placeholders, `RETURNING` support)
//! - statement builders: `SELECT` (distinct / order / limit / offset), `INSERT`, `UPDATE`, `DELETE`
//! - dialect downgrades are reported in `Sql::warnings` instead of being silently dropped
//! - compile-time type checking of column names and value types
//!
//! Not yet implemented: drivers (no execution yet), `Session`, transactions, JOIN,
//! subqueries, CTE, window functions, relations, migrations. Do not use in production.
//!
//! ## Design notes
//!
//! Rust cannot overload `>=` to return an expression (`PartialOrd::ge` returns `bool`),
//! so the SQLAlchemy-like syntax is produced by a procedural macro that rewrites tokens
//! and then lets `rustc` type-check the result. Column typos fail at compile time
//! (`E0599`), wrong value types fail at compile time (`E0277`).
//!
//! The crate name `rbatis` on crates.io belongs to an unrelated project; this project
//! publishes as `rsbatis`.

extern crate self as rsbatis;

pub use rsbatis_core::*;
pub use rsbatis_macros::where_;
// select 同样与 core 的 `select` 函数同名，但宏在 macro 命名空间，不冲突。
pub use rsbatis_macros::select;
// `insert!`（字面量式）与 core 的 `insert::<E>()` 函数同名，同样靠命名空间区分。
pub use rsbatis_macros::insert;
// `update!` / `delete!` 与 core 的 `update::<E>()` / `delete::<E>()` 同名，
// 宏在 macro 命名空间，不冲突。
pub use rsbatis_macros::{delete, update};
// derive(Entity) 与 trait Entity 同名但位于不同命名空间（macro vs type），
// Rust 允许；serde 的 `Serialize` 也是这个结构。
pub use rsbatis_macros::Entity;
pub use rsbatis_macros::FromRow;

// 执行层：只有开了对应 feature 才出现，避免不用数据库的人被迫编译 rusqlite
#[cfg(feature = "mysql")]
pub use rsbatis_driver::mysql_driver::MySqlDriver;
#[cfg(feature = "sqlite")]
pub use rsbatis_driver::sqlite::SqliteDriver;
#[cfg(any(feature = "sqlite", feature = "mysql"))]
pub use rsbatis_driver::{Driver, DriverError, ErrorKind, Row};
#[cfg(feature = "mysql")]
pub use rsbatis_driver::set_mysql_socket_hooks;

// Session / 事务 / FromRow：开了任意驱动就能用
#[cfg(feature = "orm")]
pub use rsbatis_orm::{
    BelongsTo, FromRow, HasMany, HasOne, ManyToMany, Page, PageRequest, Pool, PooledConn, Session,
    Transaction,
};

// 迁移工具：需要 orm（它要借 Session 执行），所以下面两条 cfg 缺一不可
#[cfg(all(feature = "orm", feature = "migrate"))]
pub use rsbatis_migrate::{from_dir, MigrateError, Migration, Migrator, Status, MIGRATIONS_TABLE};

// 统一入口：按 URL 打开数据库（SQLAlchemy 的 `create_engine`）
#[cfg(any(feature = "sqlite", feature = "mysql"))]
mod engine;
#[cfg(any(feature = "sqlite", feature = "mysql"))]
pub use engine::{create_engine, dialect_of, Engine};

/// Implementation detail used by the `where_!` macro expansion. Not public API.
#[doc(hidden)]
pub mod __private {
    pub use rsbatis_core::*;
}
