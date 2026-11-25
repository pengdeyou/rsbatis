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
            down: downs.remove(&version),
        })
        .collect())
}
