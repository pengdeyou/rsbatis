# 占位夹具

上游把这些测试夹具放在仓库根，但**没有随 crate 发布**（`cargo package` 只带 `src/`）。
它们的唯一使用者是上游自己的单元测试，而这个 fork 的单元测试是关掉的
（`[lib] test = false`，见 `Cargo.toml`）。

这里放占位文件只是为了 `cargo clippy --all-targets` 能编译通过——
内容不会被任何运行的测试使用。

如果将来要跑上游测试，请从
<https://github.com/blackbeam/rust-mysql-simple/tree/master/tests/fixtures>
取回真实文件。
