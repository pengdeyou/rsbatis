//! `rsbatis_core::Value` 与各驱动原生值类型之间的双向转换。
//!
//! 每个驱动一个 `#[cfg]` 段。转换函数只做"一一映射"，不做类型推断——
//! 推断属于 `Column<T>` 的职责，在编译期就定死了。

#[cfg(any(feature = "sqlite", feature = "mysql"))]
use rsbatis_core::Value;

#[cfg(feature = "sqlite")]
mod sqlite_conv {
    use super::Value;
    use rusqlite::types::{ToSql, ValueRef};

    /// `Value` → `rusqlite` 参数。
    ///
    /// 返回 `Box<dyn ToSql>` 而不是 `&dyn ToSql`，因为 `Value::Text(String)` 需要
    /// 被克隆出去才能活到 `query()` 调用结束。这是 E2 验证过的做法：
    /// 先物化成 `Vec<Box<dyn ToSql>>`，再用 `params_from_iter` 传引用。
    pub fn to_param(v: &Value) -> Box<dyn ToSql> {
        match v {
            Value::Null => Box::new(rusqlite::types::Null),
            // SQLite 没有布尔类型，存 0/1
            Value::Bool(b) => Box::new(i64::from(*b)),
            Value::I32(x) => Box::new(i64::from(*x)),
            Value::I64(x) => Box::new(*x),
            Value::F64(x) => Box::new(*x),
            Value::Text(s) => Box::new(s.clone()),
            Value::Bytes(b) => Box::new(b.clone()),
            // SQLite 没有 DATETIME 类型，写进 TEXT；定长到微秒，字符串比较即时间比较
            #[cfg(feature = "chrono")]
            Value::DateTime(dt) => Box::new(dt.format("%Y-%m-%d %H:%M:%S%.6f").to_string()),
            // `_` 而不是 `Value::DateTime(_)`：后者在本 crate 没开 chrono 时**不存在**，
            // 写出来会报 E0599。通配分支在两种 feature 组合下都合法，且同时兜住
            // "core 新增了本驱动还没处理的值类型"这类未来情况。
            // 绝不能静默写成 NULL——那会把"配置错了"变成"数据悄悄丢了"。
            #[allow(unreachable_patterns)]
            _ => panic!(
                "rsbatis-driver 收到本驱动未处理的值类型：如果这是 DateTime，\
                 说明 core 开了 chrono 而 driver 没开；请在依赖里打开 rsbatis 的 chrono feature"
            ),
        }
    }

    /// `rusqlite` 读出的值 → `Value`。
    pub fn from_ref(v: ValueRef<'_>) -> Value {
        match v {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(i) => Value::I64(i),
            ValueRef::Real(f) => Value::F64(f),
            ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Blob(b) => Value::Bytes(b.to_vec()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn bool_binds_as_integer_in_a_real_db() {
            // 不用格式化断言（dyn ToSql 没有 Debug），直接落到内存库看结果
            let conn = rusqlite::Connection::open_in_memory().unwrap();
            conn.execute_batch("CREATE TABLE t (b INTEGER)").unwrap();
            let params = [to_param(&Value::Bool(true))];
            conn.execute(
                "INSERT INTO t (b) VALUES (?)",
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
            )
            .unwrap();
            let v: i64 = conn.query_row("SELECT b FROM t", [], |r| r.get(0)).unwrap();
            assert_eq!(1, v);
        }

        #[test]
        fn text_roundtrip() {
            assert_eq!(Value::Text("x".into()), from_ref(ValueRef::Text(b"x")));
        }
    }
}

#[cfg(feature = "sqlite")]
pub use sqlite_conv::{from_ref as from_sqlite_ref, to_param as to_sqlite_param};

#[cfg(feature = "mysql")]
mod mysql_conv {
    use super::Value;
    use ::mysql::{Params, Value as MyValue};

    /// `Value` → MySQL 参数。
    ///
    /// 文本走 `Bytes`（MySQL 协议里 TEXT 和 BLOB 是同一个类型，见 `mysql_driver.rs` 的说明）。
    pub fn to_param(v: &Value) -> MyValue {
        match v {
            Value::Null => MyValue::NULL,
            // MySQL 没有独立布尔类型，TINYINT(1) 存 0/1
            Value::Bool(b) => MyValue::Int(i64::from(*b)),
            Value::I32(x) => MyValue::Int(i64::from(*x)),
            Value::I64(x) => MyValue::Int(*x),
            Value::F64(x) => MyValue::Double(*x),
            Value::Text(s) => MyValue::Bytes(s.as_bytes().to_vec()),
            Value::Bytes(b) => MyValue::Bytes(b.clone()),
            #[cfg(feature = "chrono")]
            Value::DateTime(dt) => {
                use chrono::{Datelike, Timelike};
                MyValue::Date(
                    dt.year() as u16,
                    dt.month() as u8,
                    dt.day() as u8,
                    dt.hour() as u8,
                    dt.minute() as u8,
                    dt.second() as u8,
                    dt.nanosecond() / 1000,
                )
            }
            // 同 sqlite 分支：用 `_` 而非具体变体，兼容任意 feature 组合。
            #[allow(unreachable_patterns)]
            _ => panic!(
                "rsbatis-driver 收到本驱动未处理的值类型：如果这是 DateTime，\
                 说明 core 开了 chrono 而 driver 没开；请在依赖里打开 rsbatis 的 chrono feature"
            ),
        }
    }

    /// MySQL 读出的值 → `Value`。
    ///
    /// `Bytes` 先尝试 UTF-8：能解码就当文本，否则当二进制。这是协议层的硬限制。
    pub fn from_value(v: MyValue) -> Value {
        match v {
            MyValue::NULL => Value::Null,
            MyValue::Int(i) => Value::I64(i),
            MyValue::UInt(u) => {
                if u <= i64::MAX as u64 {
                    Value::I64(u as i64)
                } else {
                    // 超出 i64 的 UNSIGNED BIGINT：保留为文本，避免静默错值
                    Value::Text(u.to_string())
                }
            }
            MyValue::Float(f) => Value::F64(f64::from(f)),
            MyValue::Double(f) => Value::F64(f),
            MyValue::Bytes(b) => match String::from_utf8(b) {
                Ok(s) => Value::Text(s),
                Err(e) => Value::Bytes(e.into_bytes()),
            },
            MyValue::Date(y, mo, d, h, mi, s, us) => {
                // MySQL 允许 0000-00-00，这种值构造不出合法日期，只能按原样退回文本
                #[cfg(feature = "chrono")]
                if let Some(dt) = chrono::NaiveDate::from_ymd_opt(y as i32, mo as u32, d as u32)
                    .and_then(|d| d.and_hms_micro_opt(h as u32, mi as u32, s as u32, us))
                {
                    return Value::DateTime(dt);
                }
                Value::Text(format!(
                    "{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{us:06}"
                ))
            }
            MyValue::Time(neg, days, h, mi, s, us) => {
                let total_h = days * 24 + u32::from(h);
                Value::Text(format!(
                    "{}{:02}:{:02}:{:02}.{:06}",
                    if neg { "-" } else { "" },
                    total_h,
                    mi,
                    s,
                    us
                ))
            }
        }
    }

    /// 把参数列表包成 `Params::Positional`——长度在运行时决定，这是选它而不是 sqlx 的原因。
    pub fn params(list: &[Value]) -> Params {
        Params::Positional(list.iter().map(to_param).collect())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn text_roundtrip_through_bytes() {
            let v = Value::Text("张三".into());
            let my = to_param(&v);
            assert_eq!(Value::Text("张三".into()), from_value(my));
        }

        #[test]
        fn bool_becomes_tinyint() {
            assert_eq!(MyValue::Int(1), to_param(&Value::Bool(true)));
        }

        #[test]
        fn invalid_utf8_stays_binary() {
            let raw = vec![0xffu8, 0xfe];
            assert_eq!(Value::Bytes(raw.clone()), from_value(MyValue::Bytes(raw)));
        }

        #[test]
        fn unsigned_bigint_overflow_becomes_text_not_wrapped() {
            // 静默回绕是最危险的错，这里必须保留原值
            assert_eq!(
                Value::Text(u64::MAX.to_string()),
                from_value(MyValue::UInt(u64::MAX))
            );
        }
    }
}

#[cfg(feature = "mysql")]
pub use mysql_conv::{
    from_value as from_mysql_value, params as mysql_params, to_param as to_mysql_param,
};

/// 把整个参数列表物化成 `Vec<Box<dyn T>>`，供驱动一次性绑定。
///
/// 单独抽出来是因为三个驱动都要这一步，只是 trait 不同。
#[cfg(feature = "sqlite")]
pub fn sqlite_params(params: &[Value]) -> Vec<Box<dyn rusqlite::types::ToSql>> {
    params.iter().map(to_sqlite_param).collect()
}
