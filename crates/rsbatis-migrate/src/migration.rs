//! 迁移定义与目录加载。

use std::path::Path;

use crate::error::MigrateError;

/// 一支迁移。
#[derive(Clone, Debug)]
pub struct Migration {
    /// 版本号。约定用"日期 + 序号"（如 `20260105001`），保证天然有序且不冲突。
    pub version: i64,
    pub name: String,
    /// 正向 SQL（可含多条语句）。
    pub up: String,
    /// 回滚 SQL。为空表示不可回滚——`down` 会明确报错，而不是假装回滚成功。
    pub down: Option<String>,
}

impl Migration {
    pub fn new(version: i64, name: impl Into<String>, up: impl Into<String>) -> Self {
        Self {
            version,
            name: name.into(),
            up: up.into(),
            down: None,
        }
    }

    pub fn with_down(mut self, down: impl Into<String>) -> Self {
        self.down = Some(down.into());
        self
    }

    /// `up` 里是否有真语句（剥掉注释后非空）。
    ///
    /// 校验放在**执行时**而不是加载时：`rsboot`/`rsbatis-cli migrate new` 生成的骨架
    /// 只有注释占位，用户先建文件、后填 SQL。加载时就拒绝会让 `status` 直接报错，
    /// 用户连"我有哪些迁移"都看不到。执行时才拒绝，既能让文件先存在，
    /// 又不会出现"记成已应用、其实什么都没做"。
    pub fn has_up_statements(&self) -> bool {
        has_statements(&self.up)
    }
}

/// 从目录加载迁移。
///
/// 文件命名：
/// - `20260105001_create_users.sql`
/// - `20260105001_create_users.down.sql`（可选，同名 `+ .down`）
///
/// 解析规则故意严格：版本必须是下划线前的纯数字。格式不对就报错并指出文件名，
/// 因为"猜一个版本号"会让迁移顺序在不知不觉中错乱。
pub fn from_dir(dir: impl AsRef<Path>) -> Result<Vec<Migration>, MigrateError> {
    let dir = dir.as_ref();
    let mut ups: Vec<(i64, String, String)> = Vec::new();
    let mut downs: std::collections::HashMap<i64, String> = std::collections::HashMap::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sql") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| MigrateError::Invalid(format!("文件名不是合法 UTF-8：{path:?}")))?;

        let is_down = stem.ends_with(".down");
        let base = stem.strip_suffix(".down").unwrap_or(stem);

        let (ver_str, name) = base.split_once('_').ok_or_else(|| {
            MigrateError::Invalid(format!(
                "迁移文件名必须是 `<版本号>_<名字>.sql`，实际是 `{}`",
                path.file_name().unwrap_or_default().to_string_lossy()
            ))
        })?;
        let version: i64 = ver_str.parse().map_err(|_| {
            MigrateError::Invalid(format!("迁移文件 `{stem}` 的版本号 `{ver_str}` 不是整数"))
        })?;
        let sql = std::fs::read_to_string(&path)?;

        // 只有注释、没有语句的 down 文件等价于"不提供回滚"。
        // 否则 `down` 会执行一段空 SQL —— 那会打印"回滚成功"却什么都没做，
        // 而这正是最该避免的一类谎报。脚手架生成的 down 模板就是这种形态。
        let sql = if is_down && !has_statements(&sql) {
            String::new()
        } else {
            sql
        };

        if is_down {
            if downs.insert(version, sql).is_some() {
                return Err(MigrateError::Invalid(format!(
                    "版本 {version} 有多个 down 脚本"
                )));
            }
        } else {
            ups.push((version, name.to_string(), sql));
        }
    }

    ups.sort_by_key(|(v, _, _)| *v);
    for w in ups.windows(2) {
        if w[0].0 == w[1].0 {
            return Err(MigrateError::Invalid(format!(
                "版本 {} 重复（`{}` 与 `{}`）",
                w[0].0, w[0].1, w[1].1
            )));
        }
    }

    Ok(ups
        .into_iter()
        .map(|(version, name, up)| Migration {
            version,
            name,
            up,
            // 空串一律当"没有 down"：空 SQL 执行起来没有意义，留着只会让人以为可回滚
            down: downs.remove(&version).filter(|d| !d.trim().is_empty()),
        })
        .collect())
}

/// 除去注释与空白后是否还有语句。
///
/// **不能用 `split_statements` 判断**：切分器为了找边界会跳过注释，但返回的片段
/// 仍是原始子串（注释留在里面）。于是"全是注释"的文本会得到一段非空片段，
/// 看起来"有语句"——我第一版就是这么误判的，结果一个只有注释的 down 被执行，
/// 打印"回滚成功"却什么都没做。
fn has_statements(sql: &str) -> bool {
    !strip_comments(sql).trim().is_empty()
}

/// 去掉 `--` / `#` 行注释与 `/* */` 块注释，字符串与标识符原样保留。
///
/// 字符串里的 `--` 是数据不是注释，所以必须先认引号。
fn strip_comments(sql: &str) -> String {
    let b = sql.as_bytes();
    let len = b.len();
    let mut out = String::with_capacity(len);
    let mut i = 0usize;
    while i < len {
        match b[i] {
            q @ (b'\'' | b'"' | b'`') => {
                out.push(b[i] as char);
                i += 1;
                while i < len {
                    out.push(b[i] as char);
                    if b[i] == q {
                        // '' 是转义的单引号，继续留在字符串里
                        if q == b'\'' && i + 1 < len && b[i + 1] == q {
                            out.push(b[i + 1] as char);
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'-' if i + 1 < len && b[i + 1] == b'-' => {
                while i < len && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'#' => {
                while i < len && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < len && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < len && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(len);
            }
            _ => {
                out.push(b[i] as char);
                i += 1;
            }
        }
    }
    out
}
