//! 迁移错误。

use std::fmt;

use rsbatis_driver::DriverError;

#[derive(Debug)]
pub enum MigrateError {
    /// 底层数据库错误。
    Driver(DriverError),
    /// 读写迁移目录失败。
    Io(std::io::Error),
    /// 迁移定义本身有问题（版本重复、文件名不合规、缺少 down 等）。
    Invalid(String),
}

impl fmt::Display for MigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrateError::Driver(e) => write!(f, "数据库错误：{e}"),
            MigrateError::Io(e) => write!(f, "读写迁移目录失败：{e}"),
            MigrateError::Invalid(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for MigrateError {}

impl From<DriverError> for MigrateError {
    fn from(e: DriverError) -> Self {
        MigrateError::Driver(e)
    }
}

impl From<std::io::Error> for MigrateError {
    fn from(e: std::io::Error) -> Self {
        MigrateError::Io(e)
    }
}
