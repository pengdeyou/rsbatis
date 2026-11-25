//! 编译期诊断基线。
//!
//! 这些用例**故意编译不过**。trybuild 会把 rustc 的实际输出与 `tests/ui/*.stderr`
//! 逐字比较：宏改坏了、span 漂了、错误退化成宏内部 panic，都会在这里变红。
//!
//! 更新基线：`TRYBUILD=overwrite cargo test -p rsbatis --test ui_diagnostics`
//! 注意 `.stderr` 绑定 rustc 版本，CI 里必须固定 toolchain，否则换版本就会全红。
#![cfg(feature = "sqlite")]

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
