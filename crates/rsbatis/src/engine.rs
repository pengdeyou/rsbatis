//! `create_engine(url)` —— 按 URL 打开数据库，返回一个统一类型的 `Session`。
//!
//! SQLAlchemy 的入口是 `engine = create_engine("sqlite:///app.db")`，之后所有操作都从
//! engine 派生。Rust 里驱动是泛型参数（`Session<D>`），直接把两种驱动塞进一个类型需要
//! 一层**枚举分发**：`Engine` 只做 `impl Driver` 的转发，编译期类型是单一的，
//! 上层代码不会因为"支持两种数据库"而被泛型传染成两份。
//!
//! ```ignore
//! let db = create_engine("sqlite:///app.db")?;      // 相对路径
//! let db = create_engine("sqlite:////tmp/app.db")?; // 绝对路径
//! let db = create_engine("sqlite://:memory:")?;
//! let db = create_engine("mysql://user:pass@127.0.0.1:3306/app")?;
//! ```

use rsbatis_core::{Dialect, Sql};
use rsbatis_driver::{Driver, DriverError, Row};

/// 统一引擎句柄：类型上是单一的，内部分发到具体驱动。
pub struct Engine {
    inner: Backend,
}

/// 一个后端。只在这个枚举里存在"具体驱动"，外面看到的永远是 `Engine`。
enum Backend {
    #[cfg(feature = "sqlite")]
    Sqlite(rsbatis_driver::sqlite::SqliteDriver),
    #[cfg(feature = "mysql")]
    MySql(rsbatis_driver::mysql_driver::MySqlDriver),
}

/// 判断 URL 属于哪个方言。**不连接数据库**，纯字符串判断。
///
/// 单独暴露是因为 CLI 需要在连库前就报"这个 scheme 不支持"，
/// 而不是先花几秒尝试连接再失败。
pub fn dialect_of(url: &str) -> Result<Dialect, DriverError> {
    if url.starts_with("sqlite:") {
        Ok(Dialect::Sqlite)
    } else if url.starts_with("mysql:") || url.starts_with("mariadb:") {
        Ok(Dialect::MySql)
    } else if url.starts_with("postgres:") || url.starts_with("postgresql:") {
        Err(DriverError::new(
            rsbatis_driver::ErrorKind::Unsupported,
            "PostgreSQL 驱动尚未实现：rsbatis 目前只提供 SQLite 与 MySQL。\
             方言层（引用、占位符、RETURNING）已支持 pg，但驱动需要 `tokio-postgres`，\
             v0.2 计划内。",
        ))
    } else {
        Err(DriverError::new(
            rsbatis_driver::ErrorKind::Sql,
            format!(
                "无法识别的数据库 URL：`{url}`。\
                 支持 `sqlite://...`、`mysql://...`（以及 `mariadb://`）"
            ),
        ))
    }
}

/// 解析 `sqlite:` URL 到具体路径，`None` 表示内存库。
///
/// 斜杠数量是 SQLAlchemy 的既有约定，容易搞错，所以写死在这里并逐条测：
/// - `sqlite:///app.db` → 三段斜杠 → **相对路径** `app.db`
/// - `sqlite:////tmp/app.db` → 四段斜杠 → **绝对路径** `/tmp/app.db`
/// - `sqlite://:memory:` / `sqlite://` → 内存
///
/// 为什么不是"看到 `/` 就当绝对"：`sqlite:///app.db` 里那个 `/` 只是 URL 的路径起点，
/// 不是文件系统的根。按 URL 直觉去理解会得到 `/app.db`（一个需要写权限的根目录文件），
/// 而 SQLAlchemy 用户预期的是当前目录。
fn sqlite_target(url: &str) -> Result<Option<String>, DriverError> {
    let rest = url.strip_prefix("sqlite:").ok_or_else(|| {
        DriverError::new(
            rsbatis_driver::ErrorKind::Sql,
            format!("不是 sqlite URL：`{url}`"),
        )
    })?;
    // `sqlite::memory:` 是 libsqlite 自己的写法，顺手认掉
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    let rest = rest.strip_prefix(":memory:").map(|_| ":memory:").unwrap_or(rest);

    if rest.is_empty() || rest == ":memory:" {
        return Ok(None);
    }
    let path = if let Some(abs) = rest.strip_prefix("//") {
        // 四段斜杠：`sqlite:////tmp/app.db`
        format!("/{abs}")
    } else if let Some(rel) = rest.strip_prefix('/') {
        // 三段斜杠：`sqlite:///app.db`
        rel.to_string()
    } else {
        rest.to_string()
    };
    if path.is_empty() {
        return Err(DriverError::new(
            rsbatis_driver::ErrorKind::Sql,
            format!("sqlite URL 缺少文件路径：`{url}`"),
        ));
    }
    Ok(Some(path))
}

impl Engine {
    /// 按 URL 打开数据库，返回可直接使用的 `Session`。
    pub fn open(url: &str) -> Result<rsbatis_orm::Session<Engine>, DriverError> {
        Ok(rsbatis_orm::Session::new(Self::connect(url)?))
    }

    /// 只建驱动，不建 Session（需要自己控制 Session 生命周期时用）。
    pub fn connect(url: &str) -> Result<Self, DriverError> {
        dialect_of(url)?;
        #[cfg(feature = "sqlite")]
        if url.starts_with("sqlite:") {
            let backend = match sqlite_target(url)? {
                None => rsbatis_driver::sqlite::SqliteDriver::open_in_memory()?,
                Some(path) => rsbatis_driver::sqlite::SqliteDriver::open(path)?,
            };
            return Ok(Self {
                inner: Backend::Sqlite(backend),
            });
        }
        #[cfg(feature = "mysql")]
        if url.starts_with("mysql:") || url.starts_with("mariadb:") {
            // mariadb:// 是同一协议，mysql crate 只认 mysql://，做一次前缀替换
            let normalized = if let Some(rest) = url.strip_prefix("mariadb:") {
                format!("mysql:{rest}")
            } else {
                url.to_string()
            };
            return Ok(Self {
                inner: Backend::MySql(rsbatis_driver::mysql_driver::MySqlDriver::connect(
                    &normalized,
                )?),
            });
        }
        // 走到这里说明 scheme 认得出、但对应 feature 没开
        Err(DriverError::new(
            rsbatis_driver::ErrorKind::Unsupported,
            format!(
                "URL `{url}` 对应的驱动未启用：请在 Cargo.toml 里打开 rsbatis 的 \
                 `sqlite` 或 `mysql` feature"
            ),
        ))
    }

    pub fn dialect(&self) -> Dialect {
        <Self as Driver>::dialect(self)
    }
}

impl Driver for Engine {
    fn execute(&mut self, sql: &Sql) -> Result<usize, DriverError> {
        match &mut self.inner {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(d) => d.execute(sql),
            #[cfg(feature = "mysql")]
            Backend::MySql(d) => d.execute(sql),
        }
    }

    fn fetch(&mut self, sql: &Sql) -> Result<Vec<Row>, DriverError> {
        match &mut self.inner {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(d) => d.fetch(sql),
            #[cfg(feature = "mysql")]
            Backend::MySql(d) => d.fetch(sql),
        }
    }

    fn dialect(&self) -> Dialect {
        match &self.inner {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(d) => Driver::dialect(d),
            #[cfg(feature = "mysql")]
            Backend::MySql(d) => Driver::dialect(d),
        }
    }

    fn supports_returning(&self) -> bool {
        match &self.inner {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(d) => Driver::supports_returning(d),
            #[cfg(feature = "mysql")]
            Backend::MySql(d) => Driver::supports_returning(d),
        }
    }

    fn last_insert_id(&mut self) -> Result<i64, DriverError> {
        match &mut self.inner {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(d) => Driver::last_insert_id(d),
            #[cfg(feature = "mysql")]
            Backend::MySql(d) => Driver::last_insert_id(d),
        }
    }

    fn reset_last_insert_id(&mut self) -> Result<(), DriverError> {
        match &mut self.inner {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(d) => Driver::reset_last_insert_id(d),
            #[cfg(feature = "mysql")]
            Backend::MySql(d) => Driver::reset_last_insert_id(d),
        }
    }

    fn begin(&mut self) -> Result<(), DriverError> {
        match &mut self.inner {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(d) => Driver::begin(d),
            #[cfg(feature = "mysql")]
            Backend::MySql(d) => Driver::begin(d),
        }
    }

    fn commit(&mut self) -> Result<(), DriverError> {
        match &mut self.inner {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(d) => Driver::commit(d),
            #[cfg(feature = "mysql")]
            Backend::MySql(d) => Driver::commit(d),
        }
    }

    fn rollback(&mut self) -> Result<(), DriverError> {
        match &mut self.inner {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(d) => Driver::rollback(d),
            #[cfg(feature = "mysql")]
            Backend::MySql(d) => Driver::rollback(d),
        }
    }

    fn execute_batch(&mut self, sql: &str) -> Result<(), DriverError> {
        match &mut self.inner {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(d) => Driver::execute_batch(d, sql),
            #[cfg(feature = "mysql")]
            Backend::MySql(d) => Driver::execute_batch(d, sql),
        }
    }

    fn in_transaction(&self) -> bool {
        match &self.inner {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(d) => Driver::in_transaction(d),
            #[cfg(feature = "mysql")]
            Backend::MySql(d) => Driver::in_transaction(d),
        }
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Engine({:?})", self.dialect())
    }
}

/// SQLAlchemy 的入口函数，同义于 `Engine::open`。
pub fn create_engine(url: &str) -> Result<rsbatis_orm::Session<Engine>, DriverError> {
    Engine::open(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialect_detection_does_not_connect() {
        // 这些 URL 都不存在，但判断方言不需要连库
        assert_eq!(dialect_of("sqlite:///nope.db").unwrap(), Dialect::Sqlite);
        assert_eq!(dialect_of("mysql://x@h/db").unwrap(), Dialect::MySql);
        assert_eq!(dialect_of("mariadb://x@h/db").unwrap(), Dialect::MySql);
    }

    #[test]
    fn postgres_is_refused_with_a_reason() {
        let e = dialect_of("postgresql://u:p@h/db").unwrap_err();
        assert!(e.message.contains("尚未实现"), "{}", e.message);
    }

    #[test]
    fn unknown_scheme_lists_what_is_supported() {
        let e = dialect_of("redis://h").unwrap_err();
        assert!(e.message.contains("sqlite://"), "{}", e.message);
        assert!(e.message.contains("mysql://"), "{}", e.message);
    }

    #[test]
    fn relative_path_uses_sqlalchemy_slashes() {
        // 三段斜杠 = 相对路径（不是 /app.db）
        assert_eq!(sqlite_target("sqlite:///app.db").unwrap().unwrap(), "app.db");
        assert_eq!(
            sqlite_target("sqlite://data/a.db").unwrap().unwrap(),
            "data/a.db"
        );
    }

    #[test]
    fn absolute_path_needs_four_slashes() {
        assert_eq!(
            sqlite_target("sqlite:////tmp/app.db").unwrap().unwrap(),
            "/tmp/app.db"
        );
    }

    #[test]
    fn memory_forms() {
        assert_eq!(sqlite_target("sqlite://").unwrap(), None);
        assert_eq!(sqlite_target("sqlite://:memory:").unwrap(), None);
        assert_eq!(sqlite_target("sqlite::memory:").unwrap(), None);
    }

    #[test]
    fn empty_path_is_refused() {
        let e = sqlite_target("sqlite:///").unwrap_err();
        assert!(e.message.contains("缺少文件路径"), "{}", e.message);
    }

    #[test]
    fn open_in_memory_returns_usable_session() {
        let mut db = Engine::open("sqlite://:memory:").unwrap();
        db.execute_batch("CREATE TABLE t (a INTEGER)")
            .unwrap();
        let rows = db
            .fetch(rsbatis_core::SelectStmt::new("t").column("a"))
            .unwrap();
        assert_eq!(rows.len(), 0);
        assert_eq!(db.dialect(), Dialect::Sqlite);
    }

    #[test]
    fn open_file_creates_nothing_but_connects() {
        let dir = std::env::temp_dir().join(format!("rsbatis_engine_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("e.db");
        let _ = std::fs::remove_file(&path);
        // 注意这里的三段斜杠拼接：`sqlite://` + `/tmp/...` = 四段斜杠（绝对路径）。
        // 写成 `sqlite://{path}`（两段）会被解析成相对路径 `tmp/...` 而打不开——
        // 我第一版测试就是这么挂的，说明这条规则必须显式告诉用户。
        {
            let mut db = Engine::open(&format!("sqlite:///{}", path.display()))
                .unwrap();
            db.execute_batch("CREATE TABLE t (a INTEGER)").unwrap();
        }
        assert!(path.exists());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn engine_forwards_transactions() {
        let mut db = Engine::open("sqlite://:memory:").unwrap();
        db.execute_batch("CREATE TABLE t (a INTEGER)")
            .unwrap();
        let mut tx = db.transaction().unwrap();
        tx.execute(rsbatis_core::InsertStmt::new("t").set_raw("a", 1.into()))
            .unwrap();
        tx.commit().unwrap();
        let rows = db
            .fetch(rsbatis_core::SelectStmt::new("t").column("a"))
            .unwrap();
        assert_eq!(rows.len(), 1);
    }
}
