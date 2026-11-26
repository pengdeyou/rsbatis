//! rsbatis-core —— SQLAlchemy Core 的 Rust 等价层。
//!
//! 只做三件事：构造 `Expr`、渲染成 SQL、收集参数。**不**依赖任何数据库驱动，
//! 因此可以单独发布、单独测试、单独被别的项目复用。
//!
//! # 用法
//!
//! ```
//! use rsbatis_core::{select, Column, Dialect, Entity, Expr};
//!
//! struct User;
//! impl Entity for User {
//!     const TABLE: &'static str = "user";
//!     const COLUMNS: &'static [&'static str] = &["id", "name", "age"];
//! }
//!
//! let age = Column::<i32>::new(User::TABLE, "age");
//! let name = Column::<String>::new(User::TABLE, "name");
//!
//! let sql = select::<User>()
//!     .where_(Expr::and(vec![age.ge(18), name.like("zhang%")]))
//!     .build(Dialect::Sqlite);
//!
//! assert_eq!(sql.text, r#"SELECT * FROM "user" WHERE "age" >= ? AND "name" LIKE ?"#);
//! assert_eq!(sql.params.len(), 2);
//! ```
//!
//! # 当前版本状态（0.1.0-alpha.3）
//!
//! 已实现：比较 / AND / OR / NOT / 括号保留 / LIKE / IS NULL / IN / BETWEEN /
//! 算术 / 函数 / CASE / CAST / 别名 / 三方言渲染 / SELECT 的 DISTINCT 与排序分页。
//!
//! 未实现：INSERT / UPDATE / DELETE、JOIN、子查询、CTE、窗口函数、驱动层。

#![forbid(unsafe_code)]

mod column;
mod dialect;
mod expr;
mod id;
mod render;
mod stmt;
mod value;

pub use column::Column;
pub use dialect::Dialect;
pub use id::{IdError, Snowflake};
pub use expr::{ArithOp, BinOp, Expr, Frame, FrameBound, Window};
pub use render::render_expr;
pub use stmt::{
    ConflictSet, DeleteStmt, InsertStmt, Join, JoinKind, Order, SelectStmt, SortOrder, Sql,
    UnionKind, UpdateStmt,
};
pub use value::{IntoValue, Value};

/// 实体元数据。真实实现里由 `#[derive(Entity)]` 生成。
///
/// `COLUMNS` 是"可用列"的唯一权威来源：迁移 diff、文档生成、`IN` 校验、
/// 以及 IDE 补全都靠它（编译器没法把它的值打进错误信息里，见 E4 报告）。
pub trait Entity {
    const TABLE: &'static str;
    /// 表的全部列名，顺序与结构体字段声明顺序一致。
    const COLUMNS: &'static [&'static str] = &[];
    /// 可空列（对应 `Option<T>` 字段）。`#[derive(Entity)]` 自动生成，
    /// 迁移工具和 `FromRow` 用它判断某列是否允许 NULL。
    const NULLABLE: &'static [&'static str] = &[];

    /// 主键列（复合主键按顺序给出）。由字段上的 `#[rsbatis(primary_key)]` 声明。
    ///
    /// 默认空：没有主键声明的实体仍然可以 `add` / `insert`，只是 `save`（upsert）
    /// 会明确拒绝——"不知道拿哪一列判冲突"时猜一个，等于把数据写错却不报错。
    const PRIMARY_KEY: &'static [&'static str] = &[];

    /// 把模型字段收集成 `(列名, 值)`，供"对象式插入"（`Session::add`）使用。
    ///
    /// 默认实现什么都不做：手写 `impl Entity` 时不需要实现它，
    /// 只是 `add` 会因为没有可写列而拒绝执行（而不是静默插入一行空值）。
    ///
    /// `Option<T>` 字段为 `None` 时**不收集该列**——语义是"这一列交给数据库默认值/可空性"，
    /// 而不是写一个 NULL。显式写 NULL 走 `InsertStmt::set_null`。
    fn column_values(&self, _out: &mut Vec<(&'static str, crate::Value)>) {}
}

/// 任何能产出一条 `Sql` 的东西。
///
/// 存在的理由：`Session` 的方法不应该强迫用户先手写 `.build(dialect())`。
/// 有了这个契约，`session.fetch(select! { User where .. })` 可以直接用，
/// 也可以继续传已经 build 好的 `&Sql`（`Sql` 自己也实现了本 trait）。
pub trait ToSql {
    fn to_sql(&self, dialect: Dialect) -> Sql;
}

impl ToSql for Sql {
    fn to_sql(&self, _dialect: Dialect) -> Sql {
        self.clone()
    }
}

impl<T: ToSql + ?Sized> ToSql for &T {
    fn to_sql(&self, dialect: Dialect) -> Sql {
        (*self).to_sql(dialect)
    }
}

impl ToSql for SelectStmt {
    fn to_sql(&self, dialect: Dialect) -> Sql {
        self.build(dialect)
    }
}

impl ToSql for InsertStmt {
    fn to_sql(&self, dialect: Dialect) -> Sql {
        self.build(dialect)
    }
}

impl ToSql for UpdateStmt {
    fn to_sql(&self, dialect: Dialect) -> Sql {
        self.build(dialect)
    }
}

impl ToSql for DeleteStmt {
    fn to_sql(&self, dialect: Dialect) -> Sql {
        self.build(dialect)
    }
}

/// 开始一个 `SELECT`。表名由实体提供。
pub fn select<T: Entity>() -> SelectStmt {
    SelectStmt::for_entity::<T>()
}

/// 开始一个 `INSERT`。
pub fn insert<T: Entity>() -> InsertStmt {
    InsertStmt::new(T::TABLE)
}

/// 开始一个 `UPDATE`。
pub fn update<T: Entity>() -> UpdateStmt {
    UpdateStmt::new(T::TABLE)
}

/// 开始一个 `DELETE`。
pub fn delete<T: Entity>() -> DeleteStmt {
    DeleteStmt::new(T::TABLE)
}
