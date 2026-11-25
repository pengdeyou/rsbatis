//! 方言差异的集中定义。
//!
//! 所有"同一个语义、不同写法"的地方都必须经过这里，禁止在渲染器里写
//! `if dialect == MySql { ... }` 散落各处。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Sqlite,
    MySql,
    Postgres,
}

impl Dialect {
    pub fn name(self) -> &'static str {
        match self {
            Dialect::Sqlite => "sqlite",
            Dialect::MySql => "mysql",
            Dialect::Postgres => "postgres",
        }
    }

    pub fn all() -> [Dialect; 3] {
        [Dialect::Sqlite, Dialect::MySql, Dialect::Postgres]
    }

    /// 标识符引用。MySQL 用反引号，SQLite / PostgreSQL 用双引号。
    /// 内部做转义：标识符里出现引号时按各自方言翻倍。
    pub fn quote(self, ident: &str) -> String {
        match self {
            Dialect::MySql => format!("`{}`", ident.replace('`', "``")),
            _ => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }

    /// 参数占位符。PostgreSQL 用编号（`$1`, `$2`...），其余用 `?`。
    /// `idx` 从 1 开始。
    pub fn placeholder(self, idx: usize) -> String {
        match self {
            Dialect::Postgres => format!("${idx}"),
            _ => "?".to_string(),
        }
    }

    /// 布尔字面量。MySQL 没有 TRUE/FALSE 关键字（是保留字但历史上用 1/0），
    /// 用数值最稳。
    pub fn bool_literal(self, v: bool) -> &'static str {
        match (self, v) {
            (Dialect::MySql, true) => "1",
            (Dialect::MySql, false) => "0",
            (_, true) => "TRUE",
            (_, false) => "FALSE",
        }
    }

    /// 是否支持 `INSERT ... RETURNING`。
    /// SQLite 3.35+ 支持；MySQL 不支持（要用 `LAST_INSERT_ID()`）。
    pub fn supports_returning(self) -> bool {
        !matches!(self, Dialect::MySql)
    }

    /// 字符串拼接函数名：MySQL 要 `CONCAT(a, b)`，其余可以用 `a || b`。
    pub fn concat_fn(self) -> Option<&'static str> {
        match self {
            Dialect::MySql => Some("CONCAT"),
            _ => None,
        }
    }

    /// `ON CONFLICT`（SQLite / PostgreSQL）vs `ON DUPLICATE KEY UPDATE`（MySQL）。
    pub fn uses_on_conflict(self) -> bool {
        !matches!(self, Dialect::MySql)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_differs() {
        assert_eq!(r#""user""#, Dialect::Sqlite.quote("user"));
        assert_eq!("`user`", Dialect::MySql.quote("user"));
        assert_eq!(r#""user""#, Dialect::Postgres.quote("user"));
    }

    #[test]
    fn quote_escapes_embedded_quotes() {
        assert_eq!(r#""a""b""#, Dialect::Sqlite.quote(r#"a"b"#));
        assert_eq!("`a``b`", Dialect::MySql.quote("a`b"));
    }

    #[test]
    fn postgres_numbers_placeholders() {
        assert_eq!("$3", Dialect::Postgres.placeholder(3));
        assert_eq!("?", Dialect::Sqlite.placeholder(3));
    }

    #[test]
    fn returning_support_flags() {
        assert!(Dialect::Sqlite.supports_returning());
        assert!(Dialect::Postgres.supports_returning());
        assert!(!Dialect::MySql.supports_returning());
    }
}
