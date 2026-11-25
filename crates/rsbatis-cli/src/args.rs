//! 参数解析。手写而不引 `clap`：只有 6 个选项、两种命令族，引一个大依赖不划算，
//! 而且**手写才能给出这种错误**："`--url` 需要一个值，你给的是 `--dry-run`"。
//!
//! 选项与位置参数可以任意交错（`--url x migrate up --to 1` 与
//! `migrate up --to 1 --url x` 等价），因为真实命令行里没人记得住顺序。

use std::fmt;

pub const HELP: &str = "\
rsbatis —— rsbatis 工程工具

用法：
  rsbatis [选项] migrate <status|up|down|new> [参数]
  rsbatis [选项] new <项目名>

选项：
  -u, --url <URL>    数据库 URL（默认读环境变量 DATABASE_URL）
                     sqlite:///app.db      相对路径（三段斜杠）
                     sqlite:////tmp/app.db 绝对路径（四段斜杠）
                     sqlite://:memory:
                     mysql://user:pass@host:3306/db
  -d, --dir <DIR>    迁移目录（默认 ./migrations）
      --into <DIR>   `new` 生成项目的父目录（默认当前目录）
      --to <V>       目标版本（`up`/`down` 用）
      --dry-run      只打印将执行的 SQL，不连库、不执行
      --no-lock      不加迁移锁（默认 MySQL 上用 GET_LOCK 串行化并发迁移）
  -h, --help         显示本帮助
  -V, --version      显示版本

退出码：
  0  成功
  1  用法错误（参数不对、缺 URL）
  2  执行失败（连接、SQL、迁移冲突）";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    MigrateStatus,
    MigrateUp { to: Option<i64>, dry_run: bool },
    MigrateDown { to: Option<i64> },
    MigrateNew { name: String },
    New { name: String },
    Help,
    Version,
}

#[derive(Debug)]
pub struct Cli {
    pub url: Option<String>,
    pub dir: String,
    pub into: String,
    pub no_lock: bool,
    pub cmd: Command,
}

/// 用法错误：只需要一句话说清哪里不对。
#[derive(Debug)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn err(msg: impl Into<String>) -> UsageError {
    UsageError(msg.into())
}

pub fn parse(argv: &[String]) -> Result<Cli, UsageError> {
    let mut url = None;
    let mut dir = "migrations".to_string();
    let mut into = ".".to_string();
    let mut no_lock = false;
    let mut to: Option<i64> = None;
    let mut dry_run = false;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    let mut only_positional = false;
    while i < argv.len() {
        let a = argv[i].as_str();
        i += 1;

        if only_positional {
            positional.push(a.to_string());
            continue;
        }
        if a == "--" {
            only_positional = true;
            continue;
        }

        // `--key=value` 形式先拆开，再统一按"需要一个值"处理
        let (key, inline) = match a.split_once('=') {
            Some((k, v)) if k.starts_with('-') => (k, Some(v.to_string())),
            _ => (a, None),
        };

        let need_value = |i: &mut usize, inline: Option<String>, name: &str| -> Result<String, UsageError> {
            if let Some(v) = inline {
                return Ok(v);
            }
            match argv.get(*i) {
                // 下一个 token 是另一个选项 → 明确报错，而不是把它当成值吞掉
                Some(next) if next.starts_with('-') && next.len() > 1 => Err(err(format!(
                    "`{name}` 需要一个值，但下一个参数是 `{next}`"
                ))),
                Some(next) => {
                    *i += 1;
                    Ok(next.clone())
                }
                None => Err(err(format!("`{name}` 需要一个值，但参数已经用完了"))),
            }
        };

        match key {
            "-h" | "--help" => return Ok(Cli { url, dir, into, no_lock, cmd: Command::Help }),
            "-V" | "--version" => {
                return Ok(Cli { url, dir, into, no_lock, cmd: Command::Version })
            }
            "-u" | "--url" => url = Some(need_value(&mut i, inline, key)?),
            "-d" | "--dir" => dir = need_value(&mut i, inline, key)?,
            "--into" => into = need_value(&mut i, inline, key)?,
            "--to" => {
                let v = need_value(&mut i, inline, key)?;
                to = Some(v.parse().map_err(|_| {
                    err(format!("`--to` 需要一个版本号（整数），实际是 `{v}`"))
                })?);
            }
            "--dry-run" => dry_run = true,
            "--no-lock" => no_lock = true,
            _ if key.starts_with('-') => {
                return Err(err(format!(
                    "未知选项 `{key}`。可用选项见 `rsbatis --help`"
                )))
            }
            _ => positional.push(a.to_string()),
        }
    }

    let cmd = match positional.first().map(|s| s.as_str()) {
        None => return Err(err("没有给出命令。可用：`migrate`、`new`。见 `rsbatis --help`")),
        Some("migrate") => match positional.get(1).map(|s| s.as_str()) {
            None => return Err(err("`migrate` 需要一个子命令：status / up / down / new")),
            Some("status") => Command::MigrateStatus,
            Some("up") => Command::MigrateUp { to, dry_run },
            Some("down") => Command::MigrateDown { to },
            Some("new") => Command::MigrateNew {
                name: positional.get(2).cloned().ok_or_else(|| {
                    err("`migrate new` 需要一个名字，例如 `rsbatis migrate new add_email`")
                })?,
            },
            Some(other) => {
                return Err(err(format!(
                    "未知的 `migrate` 子命令 `{other}`：可用 status / up / down / new"
                )))
            }
        },
        Some("new") => Command::New {
            name: positional
                .get(1)
                .cloned()
                .ok_or_else(|| err("`new` 需要一个项目名，例如 `rsbatis new todo`"))?,
        },
        Some(other) => {
            return Err(err(format!(
                "未知命令 `{other}`：可用 `migrate`、`new`。见 `rsbatis --help`"
            )))
        }
    };

    // `--to` / `--dry-run` 只对特定子命令有意义，给了就报错而不是静默忽略
    let to_used = matches!(
        cmd,
        Command::MigrateUp { .. } | Command::MigrateDown { .. }
    );
    if to.is_some() && !to_used {
        return Err(err("`--to` 只能用于 `migrate up` / `migrate down`"));
    }
    if dry_run && !matches!(cmd, Command::MigrateUp { .. }) {
        return Err(err("`--dry-run` 目前只用于 `migrate up`"));
    }

    Ok(Cli { url, dir, into, no_lock, cmd })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Cli, UsageError> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn options_can_come_before_or_after() {
        let a = p(&["--url", "sqlite://x", "migrate", "status"]).unwrap();
        let b = p(&["migrate", "status", "--url", "sqlite://x"]).unwrap();
        assert_eq!(a.url, b.url);
        assert_eq!(a.cmd, Command::MigrateStatus);
    }

    #[test]
    fn equals_form_works() {
        let c = p(&["--url=sqlite://x", "--dir=db/mig", "migrate", "status"]).unwrap();
        assert_eq!(c.url.unwrap(), "sqlite://x");
        assert_eq!(c.dir, "db/mig");
    }

    #[test]
    fn short_flags() {
        let c = p(&["-u", "sqlite://x", "migrate", "up"]).unwrap();
        assert_eq!(c.url.unwrap(), "sqlite://x");
        assert!(matches!(c.cmd, Command::MigrateUp { to: None, dry_run: false }));
    }

    #[test]
    fn missing_value_is_reported_not_swallowed() {
        // 关键：不能把 `--dry-run` 当成 `--url` 的值
        let e = p(&["--url", "--dry-run", "migrate", "up"]).unwrap_err();
        assert!(e.0.contains("--dry-run"), "{}", e.0);
    }

    #[test]
    fn missing_value_at_end() {
        let e = p(&["migrate", "status", "--url"]).unwrap_err();
        assert!(e.0.contains("用完"), "{}", e.0);
    }

    #[test]
    fn unknown_option_lists_help() {
        let e = p(&["--nope", "migrate", "status"]).unwrap_err();
        assert!(e.0.contains("--help"), "{}", e.0);
    }

    #[test]
    fn to_parses_integer() {
        let c = p(&["migrate", "up", "--to", "42"]).unwrap();
        assert!(matches!(c.cmd, Command::MigrateUp { to: Some(42), .. }));
    }

    #[test]
    fn to_rejects_non_integer() {
        let e = p(&["migrate", "up", "--to", "abc"]).unwrap_err();
        assert!(e.0.contains("版本号"), "{}", e.0);
    }

    #[test]
    fn to_only_for_up_down() {
        let e = p(&["migrate", "status", "--to", "1"]).unwrap_err();
        assert!(e.0.contains("只能用于"), "{}", e.0);
    }

    #[test]
    fn dry_run_only_for_up() {
        let e = p(&["migrate", "down", "--dry-run"]).unwrap_err();
        assert!(e.0.contains("只用于"), "{}", e.0);
    }

    #[test]
    fn unknown_command_and_subcommand() {
        assert!(p(&["frobnicate"]).unwrap_err().0.contains("未知命令"));
        assert!(p(&["migrate", "frob"])
            .unwrap_err()
            .0
            .contains("未知的 `migrate` 子命令"));
    }

    #[test]
    fn new_needs_name() {
        assert!(p(&["new"]).unwrap_err().0.contains("项目名"));
        assert_eq!(
            p(&["new", "todo"]).unwrap().cmd,
            Command::New { name: "todo".into() }
        );
    }

    #[test]
    fn migrate_new_needs_name() {
        assert!(p(&["migrate", "new"]).unwrap_err().0.contains("名字"));
        assert_eq!(
            p(&["migrate", "new", "add_email"]).unwrap().cmd,
            Command::MigrateNew {
                name: "add_email".into()
            }
        );
    }

    #[test]
    fn double_dash_makes_everything_positional() {
        let e = p(&["--", "--url"]).unwrap_err();
        assert!(e.0.contains("未知命令"), "{}", e.0);
    }

    #[test]
    fn help_and_version_win() {
        assert_eq!(p(&["--help", "migrate", "up"]).unwrap().cmd, Command::Help);
        assert_eq!(p(&["-V"]).unwrap().cmd, Command::Version);
    }
}
