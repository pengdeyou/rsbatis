//! 参数值模型与字面量转换。
//!
//! `Value` 是**运行时**统一的值表示：宏和表达式产出它，驱动层再把它转成各自的
//! 原生参数类型（`rusqlite::ToSql` / `mysql_async::Value` / `tokio_postgres::types::ToSql`）。
//! 这一层不依赖任何驱动，因此可以单独发布、单独测试。

use std::fmt;

/// 运行时的参数值。
///
/// 不含 `Eq`：`F64` 存在 NaN，派生 `Eq` 会给出错误语义。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    I32(i32),
    I64(i64),
    F64(f64),
    Text(String),
    Bytes(Vec<u8>),
    /// 无时区的日期时间（`chrono::NaiveDateTime`）。
    ///
    /// 为什么是 `NaiveDateTime` 而不是带时区的 `DateTime<Tz>`：数据库列本身
    /// 不携带时区，带时区的类型一旦落库就必然发生一次隐式转换，而那次转换
    /// 用的是谁的时区是个说不清的问题。要时区请在应用层自己加。
    #[cfg(feature = "chrono")]
    DateTime(chrono::NaiveDateTime),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// SQL 字面量形式，仅用于调试输出与 golden 测试对比，**不要**用它拼 SQL。
    pub fn debug_literal(&self) -> String {
        match self {
            Value::Null => "NULL".into(),
            Value::Bool(b) => b.to_string(),
            Value::I32(v) => v.to_string(),
            Value::I64(v) => v.to_string(),
            Value::F64(v) => v.to_string(),
            Value::Text(s) => format!("'{s}'"),
            Value::Bytes(_) => "<bytes>".into(),
            #[cfg(feature = "chrono")]
            // 带引号，确保拼出来的调试字符串看起来就是 SQL 字面量
            Value::DateTime(dt) => format!("'{}'", dt.format("%Y-%m-%d %H:%M:%S%.6f")),
        }
    }

    /// 稳定的类型标签，用于错误信息与跨库类型映射。
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::I32(_) => "i32",
            Value::I64(_) => "i64",
            Value::F64(_) => "f64",
            Value::Text(_) => "text",
            Value::Bytes(_) => "bytes",
            #[cfg(feature = "chrono")]
            Value::DateTime(_) => "datetime",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.debug_literal())
    }
}

/// 从 Rust 值到 `Value` 的转换。
///
/// 参数位置是 `impl IntoValue<T>`，`T` 是列的值类型。这样 `Column<i64>` 可以接受
/// `i32` 字面量（无损提升），而 `Column<i32>` 不接受 `i64`（可能截断）——把
/// "类型不匹配"变成编译期错误而不是运行期截断。
pub trait IntoValue<T> {
    fn into_value(self) -> Value;
}

impl IntoValue<bool> for bool {
    fn into_value(self) -> Value {
        Value::Bool(self)
    }
}

impl IntoValue<i32> for i32 {
    fn into_value(self) -> Value {
        Value::I32(self)
    }
}

impl IntoValue<i64> for i64 {
    fn into_value(self) -> Value {
        Value::I64(self)
    }
}

#[cfg(feature = "chrono")]
impl IntoValue<chrono::NaiveDateTime> for chrono::NaiveDateTime {
    fn into_value(self) -> Value {
        Value::DateTime(self)
    }
}

impl IntoValue<i64> for i32 {
    fn into_value(self) -> Value {
        Value::I64(self as i64)
    }
}

impl IntoValue<f64> for f64 {
    fn into_value(self) -> Value {
        Value::F64(self)
    }
}

impl IntoValue<f32> for f32 {
    fn into_value(self) -> Value {
        Value::F64(self as f64)
    }
}

impl IntoValue<String> for String {
    fn into_value(self) -> Value {
        Value::Text(self)
    }
}

impl IntoValue<String> for &str {
    fn into_value(self) -> Value {
        Value::Text(self.to_owned())
    }
}

impl IntoValue<String> for &String {
    fn into_value(self) -> Value {
        Value::Text(self.clone())
    }
}

impl IntoValue<Vec<u8>> for Vec<u8> {
    fn into_value(self) -> Value {
        Value::Bytes(self)
    }
}

impl IntoValue<Vec<u8>> for &[u8] {
    fn into_value(self) -> Value {
        Value::Bytes(self.to_vec())
    }
}

impl<T: IntoValue<T>> IntoValue<T> for Option<T> {
    fn into_value(self) -> Value {
        self.map(IntoValue::into_value).unwrap_or(Value::Null)
    }
}

// `From` 系列给 `Expr::lit` 用（表达式里写常量）。注意不要同时手写 `Into`，
// 否则会和标准库的 `impl<T, U> Into<U> for T where U: From<T>` 产生歧义。
impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Bool(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Value {
        Value::I32(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Value {
        Value::I64(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::F64(v)
    }
}

impl From<String> for Value {
    fn from(v: String) -> Value {
        Value::Text(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::Text(v.to_owned())
    }
}

impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Value {
        Value::Bytes(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i32_can_fill_both_i32_and_i64_columns() {
        // 同一个 i32 字面量既能填 INT 列，也能无损填 BIGINT 列
        assert_eq!(Value::I32(7), <i32 as IntoValue<i32>>::into_value(7));
        assert_eq!(Value::I64(7), <i32 as IntoValue<i64>>::into_value(7));
    }

    #[test]
    fn option_none_becomes_null() {
        let v: Option<i32> = None;
        assert_eq!(Value::Null, v.into_value());
    }

    #[test]
    fn str_and_owned_string_are_same_value() {
        let a: Value = "x".into_value();
        let b: Value = String::from("x").into_value();
        assert_eq!(a, b);
    }
}
