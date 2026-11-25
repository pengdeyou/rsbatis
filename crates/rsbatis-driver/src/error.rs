//! 驱动错误。
//!
//! 设计原则：错误必须**可分类**。上层（`Session`）要据此决定"重试 / 转换 /
//! 向上抛"，而不是靠字符串匹配。这也是 03 文档里"错误码归一化"任务的入口：
//! 唯一键冲突在 PG 是 `23505`、MySQL 是 `1062`、SQLite 是
//! `SQLITE_CONSTRAINT_UNIQUE`，归一到 [`ErrorKind::UniqueViolation`]。

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// 违反唯一约束。
    UniqueViolation,
    /// 违反外键约束。
    ForeignKeyViolation,
    /// 其他约束（NOT NULL / CHECK）。
    ConstraintViolation,
    /// SQL 语法或语义错误（多半是我们自己生成的 SQL 有问题）。
    Sql,
    /// 连接不可用、被关闭。
    Connection,
    /// 类型不匹配：读回来的值和期望的 Rust 类型不符。
    TypeMismatch,
    /// 该驱动不支持所请求的能力（例如某些驱动的显式事务）。
    Unsupported,
    /// 数值超出目标类型范围（例如把 `i64` 读进 `i32`）。
    OutOfRange,
    /// 列名 / 下标不存在。
    ColumnNotFound,
    /// 其他未分类错误，含原始消息。
    Other,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::UniqueViolation => "unique violation",
            ErrorKind::ForeignKeyViolation => "foreign key violation",
            ErrorKind::ConstraintViolation => "constraint violation",
            ErrorKind::Sql => "sql error",
            ErrorKind::Connection => "connection error",
            ErrorKind::TypeMismatch => "type mismatch",
            ErrorKind::OutOfRange => "out of range",
            ErrorKind::ColumnNotFound => "column not found",
            ErrorKind::Unsupported => "unsupported by driver",
            ErrorKind::Other => "driver error",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DriverError {
    pub kind: ErrorKind,
    pub message: String,
}

impl DriverError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn sql(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Sql, message)
    }

    pub fn type_mismatch(expected: &str, got: &str) -> Self {
        Self::new(
            ErrorKind::TypeMismatch,
            format!("期望 {expected}，实际读到 {got}"),
        )
    }

    pub fn out_of_range(value: impl fmt::Display, target: &str) -> Self {
        Self::new(
            ErrorKind::OutOfRange,
            format!("值 {value} 超出 {target} 的表示范围"),
        )
    }

    pub fn column_not_found(name: &str) -> Self {
        Self::new(
            ErrorKind::ColumnNotFound,
            format!("结果集中没有列 `{name}`"),
        )
    }

    pub fn is_unique_violation(&self) -> bool {
        self.kind == ErrorKind::UniqueViolation
    }
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

impl std::error::Error for DriverError {}
