//! # rsbatis-orm
//!
//! Session、事务与模型映射。
//!
//! ## 当前能力（0.1.0-alpha.2）
//!
//! - `Session<D>`：连接 + 事务状态，提供 `execute` / `fetch` / `all::<T>`；
//! - `Transaction`：RAII，作用域结束未 `commit` 即回滚；
//! - `FromRow`：把 `Row` 转成模型结构体（由 `#[derive(FromRow)]` 生成）；
//! - 关系预加载：`HasMany` / `BelongsTo` + selectin 与 joined 两种策略；
//! - 分页：`PageRequest` / `Page<T>`，count 与取页分两次查询（不隐式开事务）。
//!
//! ## 未实现
//!
//! `async`、Unit of Work / Identity Map、懒加载（字段访问不可拦截，做不了透明版）、
//! Changeset 脏追踪、多对多关系。
//! 这些都在 v0.5 / v1.0 的范围内，MVP 先保证"能安全地用完一条连接"。

#![forbid(unsafe_code)]

pub mod from_row;
pub mod page;
pub mod pool;
mod relation;
mod session;

pub use from_row::FromRow;
pub use page::{Page, PageRequest};
pub use pool::{Pool, PooledConn};
pub use relation::{BelongsTo, HasMany, HasOne, ManyToMany};
pub use session::{Session, Transaction};
