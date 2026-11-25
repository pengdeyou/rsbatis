//! # rsbatis-cli
//!
//! rsbatis 生态的工程工具：迁移管理与项目脚手架。
//!
//! ```text
//! rsbatis migrate status          # 看哪些迁移已应用
//! rsbatis migrate up              # 应用全部未应用的
//! rsbatis migrate up --dry-run    # 只打印 SQL，不连库
//! rsbatis migrate down            # 回滚最后一支
//! rsbatis migrate new add_email   # 生成一支迁移骨架
//! rsbatis new todo                # 建一个项目骨架
//! ```
//!
//! 数据库 URL 从 `--url` 或环境变量 `DATABASE_URL` 取（后者是 SQLAlchemy 的惯例）。
//!
//! ## 为什么是独立 crate 而不是 `rsbatis` 的子命令
//!
//! `rsbatis` 是库，不该为了一个命令行工具把 `clap` 之类的依赖传染给所有使用者。
//! 这里手写参数解析，依赖只有 `rsbatis` 本体与 `chrono`。
//!
//! ## 设计取向
//!
//! - **不假装成功**：不可回滚的迁移执行 `down` 会报错；`--to` 给错命令直接拒绝，
//!   而不是"静默忽略多余参数"。
//! - **不留半截状态**：迁移 SQL 与元数据写入在同一个事务里；并发迁移用 MySQL 的
//!   `GET_LOCK` 串行化。
#![forbid(unsafe_code)]

mod args;
mod commands;
mod text;

pub use args::{Cli, Command, UsageError, HELP};
pub use commands::{dispatch, Failure, RSBATIS_REQ};

/// 一次运行的产物。**不直接写 stdout**：把文本交回来，调用方（`main` 或测试）
/// 才知道该怎么处理，也省掉测试里去捕获管道的麻烦。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }
    pub fn fail(f: &Failure) -> Self {
        Self {
            code: f.code(),
            stdout: String::new(),
            stderr: format!("{}\n", f.message()),
        }
    }
}

/// 跑一次命令。`argv` 不含程序名。
pub fn run(argv: &[String], env_url: Option<String>) -> Output {
    let cli = match args::parse(argv) {
        Ok(c) => c,
        Err(e) => {
            return Output {
                code: 1,
                stdout: String::new(),
                stderr: format!("{}\n\n{}", e.0, args::HELP),
            }
        }
    };
    match commands::dispatch(&cli, env_url) {
        Ok(s) => Output::ok(s),
        Err(f) => Output::fail(&f),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn help_goes_to_stdout_with_code_zero() {
        let o = run(&v(&["--help"]), None);
        assert_eq!(o.code, 0);
        assert!(o.stdout.contains("rsbatis"), "{}", o.stdout);
        assert!(o.stdout.contains("migrate"));
    }

    #[test]
    fn bad_args_print_usage_and_a_hint() {
        let o = run(&v(&["--nope"]), None);
        assert_eq!(o.code, 1);
        assert!(o.stderr.contains("未知选项"));
        // 用错参数时把帮助一起给，而不是让人再跑一次 --help
        assert!(o.stderr.contains("用法"));
    }

    #[test]
    fn missing_url_is_usage_error() {
        let d = std::env::temp_dir().join(format!("rsboot_nourl_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        std::fs::write(d.join("20260101001_a.sql"), "SELECT 1;").unwrap();
        let o = run(&v(&["-d", &d.to_string_lossy(), "migrate", "status"]), None);
        assert_eq!(o.code, 1);
        assert!(o.stderr.contains("DATABASE_URL"), "{}", o.stderr);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_dir_is_usage_error() {
        let o = run(
            &v(&[
                "-d",
                "/nonexistent/xyz",
                "migrate",
                "status",
                "-u",
                "sqlite://:memory:",
            ]),
            None,
        );
        assert_eq!(o.code, 1);
        assert!(o.stderr.contains("迁移目录不存在"), "{}", o.stderr);
    }

    #[test]
    fn version_prints_crate_version() {
        let o = run(&v(&["-V"]), None);
        assert_eq!(o.code, 0);
        assert!(o.stdout.starts_with("rsbatis-cli "));
    }

    #[test]
    fn env_url_is_used_when_flag_absent() {
        let d = std::env::temp_dir().join(format!("rsboot_envurl_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        std::fs::write(d.join("20260101001_a.sql"), "CREATE TABLE t (a INTEGER);").unwrap();
        let o = run(
            &v(&["-d", &d.to_string_lossy(), "migrate", "status"]),
            Some("sqlite://:memory:".to_string()),
        );
        assert_eq!(o.code, 0, "{}", o.stderr);
        assert!(o.stdout.contains("未应用"), "{}", o.stdout);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn scaffold_req_matches_facade_version() {
        // 脚手架里硬编码的版本必须与工作区当前版本一致，否则生成的项目装不上。
        // 这条断言就是"bump 版本时别忘了改这里"的自动提醒。
        let facade = include_str!("../../rsbatis/Cargo.toml");
        let line = facade
            .lines()
            .find(|l| l.starts_with("version"))
            .expect("rsbatis/Cargo.toml 里没有 version");
        let ver = line.split('"').nth(1).unwrap();
        assert_eq!(RSBATIS_REQ, ver);
    }
}
