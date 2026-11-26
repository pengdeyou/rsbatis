//! 结果行与类型回读。
//!
//! 数据库返回的值统一是 [`rsbatis_core::Value`]，`Row` 负责按列名/下标取出来并
//! 转成业务想要的 Rust 类型。转换是**严格**的：`i64` 读进 `i32` 会检查范围，
//! 类型不符直接报错，绝不静默截断。

use std::sync::Arc;

use rsbatis_core::Value;

use crate::error::DriverError;

/// 一行结果。
///
/// 列名是**整个结果集共享**的一份（`Arc<[String]>`）：1000 行 × 8 列如果每行各存一份，
/// 就是 8000 次 `String` 分配与复制，而这部分与数据内容完全无关。共享之后每行只多一次
/// 原子计数。
#[derive(Clone, Debug)]
pub struct Row {
    columns: Arc<[String]>,
    values: Vec<Value>,
}

impl Row {
    pub fn new(columns: Vec<String>, values: Vec<Value>) -> Self {
        debug_assert_eq!(columns.len(), values.len(), "列数与值数不一致");
        Self {
            columns: Arc::from(columns),
            values,
        }
    }

    /// 用已共享的列名构造一行。驱动在取结果时应当走这条路，而不是每行 `new` 一次。
    pub fn from_shared(columns: Arc<[String]>, values: Vec<Value>) -> Self {
        debug_assert_eq!(columns.len(), values.len(), "列数与值数不一致");
        Self { columns, values }
    }

    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    pub fn values(&self) -> &[Value] {
        &self.values
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// 列名的下标。列名大小写敏感（不同库对大小写的处理不同，这里不猜）。
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c == name)
    }

    pub fn get<T: FromValue>(&self, idx: usize) -> Result<T, DriverError> {
        let v = self.values.get(idx).ok_or_else(|| {
            DriverError::new(
                crate::ErrorKind::ColumnNotFound,
                format!("列下标 {idx} 越界（共 {} 列）", self.values.len()),
            )
        })?;
        T::from_value(v)
    }

    pub fn get_by_name<T: FromValue>(&self, name: &str) -> Result<T, DriverError> {
        let idx = self
            .index_of(name)
            .ok_or_else(|| DriverError::column_not_found(name))?;
        self.get(idx)
    }
}

/// 从 [`Value`] 构造 Rust 值。
pub trait FromValue: Sized {
    fn from_value(v: &Value) -> Result<Self, DriverError>;
}

impl FromValue for Value {
    fn from_value(v: &Value) -> Result<Self, DriverError> {
        Ok(v.clone())
    }
}

#[cfg(feature = "chrono")]
impl FromValue for chrono::NaiveDateTime {
    fn from_value(v: &Value) -> Result<Self, DriverError> {
        match v {
            Value::DateTime(dt) => Ok(*dt),
            // SQLite 没有日期类型，日期全躺在 TEXT 里；MySQL 的字符列也可能是日期串
            Value::Text(s) => parse_naive(s).ok_or_else(|| {
                DriverError::type_mismatch(
                    "NaiveDateTime",
                    &format!("text(\"{s}\" 无法解析为日期)"),
                )
            }),
            // 另一种常见存法：unix 秒
            Value::I64(secs) => chrono::DateTime::from_timestamp(*secs, 0)
                .map(|dt| dt.naive_utc())
                .ok_or_else(|| {
                    DriverError::new(
                        crate::ErrorKind::OutOfRange,
                        format!("{secs} 不是合法的 unix 时间戳"),
                    )
                }),
            other => Err(DriverError::type_mismatch(
                "NaiveDateTime",
                other.type_name(),
            )),
        }
    }
}

/// 按"我们写出去的格式优先"的顺序尝试解析。
///
/// 只认这几种是刻意的：日期解析一旦宽松（比如允许 `2026/1/5`），
/// 就会把真正的脏数据当成合法值吞掉，问题延后到统计口径上才爆。
#[cfg(feature = "chrono")]
fn parse_naive(s: &str) -> Option<chrono::NaiveDateTime> {
    const FORMATS: &[&str] = &[
        "%Y-%m-%d %H:%M:%S%.f", // 我们写入的格式
        "%Y-%m-%dT%H:%M:%S%.f", // ISO-8601 / sqlite datetime()
        "%Y-%m-%d %H:%M:%S",
    ];
    for f in FORMATS {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, f) {
            return Some(dt);
        }
    }
    // 纯日期列（DATE）
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
}

impl FromValue for i64 {
    fn from_value(v: &Value) -> Result<Self, DriverError> {
        match v {
            Value::I64(x) => Ok(*x),
            Value::I32(x) => Ok(*x as i64),
            Value::Bool(b) => Ok(*b as i64),
            other => Err(DriverError::type_mismatch("i64", other.type_name())),
        }
    }
}

impl FromValue for i32 {
    fn from_value(v: &Value) -> Result<Self, DriverError> {
        match v {
            Value::I32(x) => Ok(*x),
            Value::I64(x) => i32::try_from(*x).map_err(|_| DriverError::out_of_range(*x, "i32")),
            Value::Bool(b) => Ok(*b as i32),
            other => Err(DriverError::type_mismatch("i32", other.type_name())),
        }
    }
}

impl FromValue for u64 {
    fn from_value(v: &Value) -> Result<Self, DriverError> {
        match v {
            Value::I64(x) => u64::try_from(*x).map_err(|_| DriverError::out_of_range(*x, "u64")),
            Value::I32(x) => u64::try_from(*x).map_err(|_| DriverError::out_of_range(*x, "u64")),
            other => Err(DriverError::type_mismatch("u64", other.type_name())),
        }
    }
}

impl FromValue for f64 {
    fn from_value(v: &Value) -> Result<Self, DriverError> {
        match v {
            Value::F64(x) => Ok(*x),
            Value::I64(x) => Ok(*x as f64),
            Value::I32(x) => Ok(*x as f64),
            other => Err(DriverError::type_mismatch("f64", other.type_name())),
        }
    }
}

impl FromValue for bool {
    fn from_value(v: &Value) -> Result<Self, DriverError> {
        match v {
            Value::Bool(b) => Ok(*b),
            // SQLite 没有布尔类型，存的是 0/1
            Value::I64(0) | Value::I32(0) => Ok(false),
            Value::I64(1) | Value::I32(1) => Ok(true),
            Value::I64(x) => Err(DriverError::new(
                crate::ErrorKind::TypeMismatch,
                format!("布尔列读到了 {x}，只接受 0/1"),
            )),
            other => Err(DriverError::type_mismatch("bool", other.type_name())),
        }
    }
}

impl FromValue for String {
    fn from_value(v: &Value) -> Result<Self, DriverError> {
        match v {
            Value::Text(s) => Ok(s.clone()),
            other => Err(DriverError::type_mismatch("String", other.type_name())),
        }
    }
}

impl FromValue for Vec<u8> {
    fn from_value(v: &Value) -> Result<Self, DriverError> {
        match v {
            Value::Bytes(b) => Ok(b.clone()),
            Value::Text(s) => Ok(s.as_bytes().to_vec()),
            other => Err(DriverError::type_mismatch("Vec<u8>", other.type_name())),
        }
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(v: &Value) -> Result<Self, DriverError> {
        match v {
            Value::Null => Ok(None),
            other => T::from_value(other).map(Some),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    fn row() -> Row {
        Row::new(
            vec!["id".into(), "name".into(), "age".into()],
            vec![Value::I64(1), Value::Text("a".into()), Value::Null],
        )
    }

    #[test]
    fn get_by_name_and_index() {
        let r = row();
        assert_eq!(1i64, r.get_by_name::<i64>("id").unwrap());
        assert_eq!("a".to_string(), r.get::<String>(1).unwrap());
    }

    #[test]
    fn i64_into_i32_checks_range() {
        let r = Row::new(vec!["v".into()], vec![Value::I64(i64::MAX)]);
        let err = r.get::<i32>(0).unwrap_err();
        assert_eq!(ErrorKind::OutOfRange, err.kind);
    }

    #[test]
    fn null_is_none_and_some() {
        let r = row();
        assert_eq!(None, r.get_by_name::<Option<i32>>("age").unwrap());
        assert_eq!(
            Some("a".to_string()),
            r.get_by_name::<Option<String>>("name").unwrap()
        );
    }

    #[test]
    fn type_mismatch_is_reported_not_coerced() {
        let r = row();
        let err = r.get_by_name::<i64>("name").unwrap_err();
        assert_eq!(ErrorKind::TypeMismatch, err.kind);
    }

    #[test]
    fn missing_column() {
        let r = row();
        let err = r.get_by_name::<i64>("nope").unwrap_err();
        assert_eq!(ErrorKind::ColumnNotFound, err.kind);
    }

    #[test]
    fn sqlite_bool_from_integer() {
        let r = Row::new(vec!["f".into()], vec![Value::I64(1)]);
        assert!(r.get::<bool>(0).unwrap());
    }
}
