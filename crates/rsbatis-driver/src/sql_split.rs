//! SQL 语句切分。
//!
//! 为什么需要它：MySQL 驱动用 `query_drop` 一次发多条语句时，**中间语句的错误会被
//! 吞掉**（实测 `CREATE TABLE a; INSERT INTO 不存在的表; ...` 返回 `Ok(())`，
//! 且 `a` 确实被建了出来）。对迁移工具来说这是致命的：它会记成"已应用"，
//! 下次不再重试，而数据库里其实是半成品。所以批处理必须逐条执行、逐条检查。
//!
//! 切分器只做一件事：找到**真正的语句边界**。引号里的 `;`、注释里的 `;` 都不算。
//! 已知边界：MySQL 的存储过程 / 触发器（`BEGIN ... END;` 内部的分号）会被切错，
//! 这类 DDL 不通过 `execute_batch` 执行。

/// 把一段 SQL 脚本切成单条语句，已去掉首尾空白与空语句。
pub fn split_statements(sql: &str) -> Vec<&str> {
    let b = sql.as_bytes();
    let len = b.len();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;

    while i < len {
        match b[i] {
            // 单引号字符串，'' 是转义的单引号。
            b'\'' => {
                i += 1;
                while i < len {
                    if b[i] == b'\'' {
                        if i + 1 < len && b[i + 1] == b'\'' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            // 双引号 / 反引号：字符串或标识符，成对出现。
            q @ (b'"' | b'`') => {
                i += 1;
                while i < len {
                    if b[i] == q {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            // `--` 行注释
            b'-' if i + 1 < len && b[i + 1] == b'-' => {
                i += 2;
                while i < len && b[i] != b'\n' {
                    i += 1;
                }
            }
            // `#` 行注释（MySQL）
            b'#' => {
                i += 1;
                while i < len && b[i] != b'\n' {
                    i += 1;
                }
            }
            // `/* ... */` 块注释
            b'/' if i + 1 < len && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < len && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(len);
            }
            b';' => {
                let seg = sql[start..i].trim();
                if !seg.is_empty() {
                    out.push(seg);
                }
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }

    let tail = sql[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}
