//! `Value` 模型的全分支覆盖：调试字面量、类型标签、`From` 转换、数值提升、`Option` 委派。
//!
//! 这些能力不依赖数据库，纯粹是值表示层的契约。它们此前只有 3 个用例，
//! 是 `core/value.rs` 覆盖率只有 51% 的直接原因。

use rsbatis_core::{IntoValue, Value};

#[test]
fn debug_literal_renders_every_variant_as_sql_ish_text() {
    assert_eq!("NULL", Value::Null.debug_literal());
    assert_eq!("true", Value::Bool(true).debug_literal());
    assert_eq!("false", Value::Bool(false).debug_literal());
    assert_eq!("-7", Value::I32(-7).debug_literal());
    assert_eq!("42", Value::I64(42).debug_literal());
    assert_eq!("1.5", Value::F64(1.5).debug_literal());
    // 文本带单引号，便于肉眼判断"这是字符串不是数字"
    assert_eq!("'abc'", Value::Text("abc".into()).debug_literal());
    // 二进制不展开，避免日志里出现巨量字节
    assert_eq!("<bytes>", Value::Bytes(vec![1, 2, 3]).debug_literal());
}

#[test]
fn type_name_is_the_stable_label_used_in_errors() {
    assert_eq!("null", Value::Null.type_name());
    assert_eq!("bool", Value::Bool(true).type_name());
    assert_eq!("i32", Value::I32(1).type_name());
    assert_eq!("i64", Value::I64(1).type_name());
    assert_eq!("f64", Value::F64(1.0).type_name());
    assert_eq!("text", Value::Text("a".into()).type_name());
    assert_eq!("bytes", Value::Bytes(vec![]).type_name());
}

#[test]
fn display_is_debug_literal() {
    // Display 与 debug_literal 必须一致，否则日志和断言会给出两套说法
    let samples = vec![
        Value::Null,
        Value::Bool(true),
        Value::I32(3),
        Value::Text("x".into()),
    ];
    for v in samples {
        assert_eq!(v.debug_literal(), v.to_string());
    }
}

#[test]
fn is_null_only_true_for_null() {
    assert!(Value::Null.is_null());
    assert!(!Value::I32(0).is_null());
    assert!(!Value::Text(String::new()).is_null());
}

#[test]
fn from_conversions_keep_the_narrow_type() {
    // 关键契约：From<i32> 产出 I32（不是 I64），否则列类型的宽窄信息会丢
    assert_eq!(Value::Bool(true), Value::from(true));
    assert_eq!(Value::I32(9), Value::from(9_i32));
    assert_eq!(Value::I64(9), Value::from(9_i64));
    assert_eq!(Value::F64(9.5), Value::from(9.5_f64));
    assert_eq!(Value::Text("hi".into()), Value::from(String::from("hi")));
    assert_eq!(Value::Text("hi".into()), Value::from("hi"));
    assert_eq!(Value::Bytes(vec![9]), Value::from(vec![9_u8]));
}

#[test]
fn numeric_widening_only_grows() {
    // i32 → i64 是无损的，允许；反方向不允许（编译期就挡住，这里只验证正向）
    assert_eq!(Value::I64(7), <i32 as IntoValue<i64>>::into_value(7));
    // f32 → f64 同样是无损提升，但结果统一装在 F64 里
    let wide: Value = 0.5_f32.into_value();
    assert_eq!(Value::F64(0.5), wide);
}

#[test]
fn text_accepts_str_owned_and_borrowed() {
    assert_eq!(
        <&str as IntoValue<String>>::into_value("same"),
        <String as IntoValue<String>>::into_value(String::from("same"))
    );
    let owned = String::from("same");
    assert_eq!(
        <&String as IntoValue<String>>::into_value(&owned),
        Value::Text("same".into())
    );
}

#[test]
fn bytes_accept_vec_and_slice() {
    // 拥有所有权的 Vec 与借用切片必须产出同一个 Value，否则调用方要关心"传哪一种"
    let from_vec: Value = vec![1_u8, 2].into_value();
    let arr: [u8; 2] = [1, 2];
    let slice: &[u8] = &arr;
    let from_slice: Value = slice.into_value();
    assert_eq!(from_vec, from_slice);
}

#[test]
fn option_some_delegates_and_none_falls_back_to_null() {
    // Some 必须走内层转换，而不是被包装成别的东西
    assert_eq!(Value::I32(5), Some(5_i32).into_value());
    assert_eq!(
        Value::Text("t".into()),
        Some(String::from("t")).into_value()
    );
    let none: Option<i32> = None;
    assert_eq!(Value::Null, none.into_value());
}

#[cfg(feature = "chrono")]
#[test]
fn datetime_variant_has_quoted_microsecond_literal() {
    use chrono::NaiveDateTime;
    let dt = NaiveDateTime::parse_from_str("2024-01-02 03:04:05.000006", "%Y-%m-%d %H:%M:%S%.6f")
        .expect("解析固定串不会失败");
    assert_eq!("datetime", Value::DateTime(dt).type_name());
    // 微秒必须保留：SQLite 侧就是靠定长微秒文本落库的
    assert_eq!(
        "'2024-01-02 03:04:05.000006'",
        Value::DateTime(dt).debug_literal()
    );
}
