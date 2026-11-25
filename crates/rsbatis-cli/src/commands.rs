//! 各子命令的实现。
//!
//! 约定：**所有会写库的操作都先拿迁移锁**（MySQL 上 `GET_LOCK`）。两个 `rsbatis migrate up`
//! 同时跑时，第二个会等第一个放锁，而不是两边都读到"版本 X 未应用"然后都去执行一遍。
//! SQLite 不需要：文件锁会让第二个直接报 `database is locked`，那已经够明确了。

use std::path::{Path, PathBuf};

use rsbatis::{
    create_engine, dialect_of, Dialect, DriverError, Engine, MigrateError, Migration, Migrator,
    Session, Sql,
};

use crate::args::Cli;
use crate::text::{pad, pad_left, width};

/// CLI 的失败。分成两类是为了让退出码有意义：用错了是 1，跑挂了是 2。
#[derive(Debug)]
pub enum Failure {
    /// 参数、环境、目录不对——用户改一下就好
    Usage(String),
    /// 连接、SQL、迁移执行失败
    Runtime(String),
}

impl Failure {
    pub fn code(&self) -> i32 {
        match self {
            Failure::Usage(_) => 1,
            Failure::Runtime(_) => 2,
        }
    }
    pub fn message(&self) -> &str {
        match self {
            Failure::Usage(m) | Failure::Runtime(m) => m,
        }
    }
}

impl From<MigrateError> for Failure {
    fn from(e: MigrateError) -> Self {
        Failure::Runtime(format!("迁移失败：{e}"))
    }
}

impl From<DriverError> for Failure {
    fn from(e: DriverError) -> Self {
        Failure::Runtime(format!(
            "数据库错误（{:?}）：{}",
            e.kind, e.message
        ))
    }
}

fn usage(msg: impl Into<String>) -> Failure {
    Failure::Usage(msg.into())
}

/// 打印用：把「命令做了什么都返回给调用方」而不是直接写 stdout，
/// 测试才能断言输出，不需要去捕获进程的管道。
pub type CmdResult = Result<String, Failure>;

pub fn dispatch(cli: &Cli, env_url: Option<String>) -> CmdResult {
    match &cli.cmd {
        crate::args::Command::MigrateStatus => status(cli, env_url),
        crate::args::Command::MigrateUp { to, dry_run } => {
            if *dry_run {
                up_dry_run(cli, *to)
            } else {
                up(cli, env_url, *to)
            }
        }
        crate::args::Command::MigrateDown { to } => down(cli, env_url, *to),
        crate::args::Command::MigrateNew { name } => migrate_new(cli, name),
        crate::args::Command::New { name } => new_project(cli, name),
        crate::args::Command::Help => Ok(crate::args::HELP.to_string()),
        crate::args::Command::Version => Ok(format!("rsbatis-cli {}", env!("CARGO_PKG_VERSION"))),
    }
}

/// 解析 URL：命令行优先，其次 `DATABASE_URL`。
fn resolve_url(cli: &Cli, env_url: Option<String>) -> Result<String, Failure> {
    cli.url
        .clone()
        .or(env_url)
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| {
            usage(
                "没有数据库 URL。用 `--url sqlite:///app.db`，或设置环境变量 DATABASE_URL",
            )
        })
}

fn load_migrations(dir: &str) -> Result<(PathBuf, Vec<Migration>), Failure> {
    let p = PathBuf::from(dir);
    if !p.is_dir() {
        return Err(usage(format!(
            "迁移目录不存在：{}（用 `-d` 指定，或 `rsbatis migrate new <名字>` 建第一支）",
            p.display()
        )));
    }
    let ms = rsbatis::from_dir(&p)?;
    if ms.is_empty() {
        return Err(usage(format!(
            "迁移目录 {} 里没有 `<版本号>_<名字>.sql`：用 `rsbatis migrate new <名字>` 建一支",
            p.display()
        )));
    }
    Ok((p, ms))
}

// ---------------------------------------------------------------- status

fn status(cli: &Cli, env_url: Option<String>) -> CmdResult {
    let (dir, migrations) = load_migrations(&cli.dir)?;
    let url = resolve_url(cli, env_url)?;
    let mut db = create_engine(&url)?;
    let mut migrator = Migrator::new(&mut db, migrations)?;
    let list = migrator.status()?;

    let w_ver = list
        .iter()
        .map(|s| width(&s.version.to_string()))
        .max()
        .unwrap_or(0)
        .max(width("版本"));
    let w_state = width("状态").max(width("未应用"));

    let mut out = String::new();
    out.push_str(&format!(
        "{}（{} 支迁移）  {}\n",
        dir.display(),
        list.len(),
        describe_url(&url)
    ));
    out.push_str(&format!(
        "  {}  {}  {}\n",
        pad_left("版本", w_ver),
        pad("状态", w_state),
        "名称"
    ));
    let mut pending = 0;
    for s in &list {
        let state = if s.applied { "已应用" } else { "未应用" };
        if !s.applied {
            pending += 1;
        }
        out.push_str(&format!(
            "  {}  {}  {}\n",
            pad_left(&s.version.to_string(), w_ver),
            pad(state, w_state),
            s.name
        ));
    }
    if pending > 0 {
        out.push_str(&format!("\n还有 {pending} 支未应用：`rsbatis migrate up`\n"));
    } else {
        out.push_str("\n全部已应用。\n");
    }
    Ok(out)
}

/// 打印 URL 时**去掉密码**：CLI 输出经常被贴进 issue 和聊天记录。
fn describe_url(url: &str) -> String {
    let Some(rest) = url.split_once("://") else {
        return url.to_string();
    };
    let (scheme, after) = rest;
    // user:pass@host/db → user:***@host/db
    if let Some((userinfo, host)) = after.rsplit_once('@') {
        let user = userinfo.split(':').next().unwrap_or(userinfo);
        return format!("{scheme}://{user}:***@{host}");
    }
    url.to_string()
}

// ---------------------------------------------------------------- up

fn up_dry_run(cli: &Cli, to: Option<i64>) -> CmdResult {
    // 不连库：只读目录，把"将会应用哪些"和 SQL 打出来。
    // 这是唯一能安全在 CI 前置步骤跑的模式（无需数据库凭据）。
    let (_dir, migrations) = load_migrations(&cli.dir)?;
    let target = to.unwrap_or(i64::MAX);
    let mut out = String::new();
    let mut n = 0;
    for m in &migrations {
        if m.version > target {
            continue;
        }
        n += 1;
        out.push_str(&format!("-- {} {}\n{}\n", m.version, m.name, m.up.trim_end()));
    }
    if n == 0 {
        return Ok("（没有待执行的迁移）\n".to_string());
    }
    out.push_str(&format!(
        "-- 共 {n} 支。使用 `rsbatis migrate up` 实际执行（需要数据库 URL）\n"
    ));
    Ok(out)
}

fn up(cli: &Cli, env_url: Option<String>, to: Option<i64>) -> CmdResult {
    let (_dir, migrations) = load_migrations(&cli.dir)?;
    let url = resolve_url(cli, env_url)?;
    let mut db = create_engine(&url)?;
    let target = to.unwrap_or(i64::MAX);

    with_lock(&mut db, cli.no_lock, |db| {
        let is_mysql = db_dialect_is_mysql(db);
        let mut migrator = Migrator::new(db, migrations)?;
        let before = migrator.status()?;
        let pending: Vec<(i64, String)> = before
            .iter()
            .filter(|s| !s.applied && s.version <= target)
            .map(|s| (s.version, s.name.clone()))
            .collect();
        if pending.is_empty() {
            let head = before
                .iter()
                .filter(|s| s.applied)
                .map(|s| s.version)
                .max()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "（空）".to_string());
            return Ok(format!("没有待应用的迁移（当前版本 {head}）\n"));
        }
        let done = migrator.up_to(target)?;
        let mut out = String::new();
        for ((v, name), _applied) in pending.iter().zip(done.iter()) {
            out.push_str(&format!("应用 {v} {name} ... ok\n"));
        }
        if let Some(v) = done.last() {
            out.push_str(&format!("\n已应用 {} 支，当前版本 {v}\n", done.len()));
        }
        if is_mysql && !migrator.supports_transactional_ddl() {
            out.push_str(
                "注意：MySQL/MariaDB 的 DDL 会隐式提交，中途失败时已执行语句不回滚。\n",
            );
        }
        Ok(out)
    })
}

fn down(cli: &Cli, env_url: Option<String>, to: Option<i64>) -> CmdResult {
    let (_dir, migrations) = load_migrations(&cli.dir)?;
    let url = resolve_url(cli, env_url)?;
    let mut db = create_engine(&url)?;

    with_lock(&mut db, cli.no_lock, |db| {
        let mut migrator = Migrator::new(db, migrations)?;
        let mut out = String::new();
        let rolled = match to {
            Some(t) => migrator.down_to(t)?,
            None => migrator.down()?.into_iter().collect(),
        };
        if rolled.is_empty() {
            let head = migrator
                .applied_versions()?
                .into_iter()
                .max()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "（空）".to_string());
            return Ok(format!("没有可回滚的迁移（当前版本 {head}）\n"));
        }
        for v in &rolled {
            out.push_str(&format!("回滚 {v} ... ok\n"));
        }
        let head = migrator
            .applied_versions()?
            .into_iter()
            .max()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "（空）".to_string());
        out.push_str(&format!("\n已回滚 {} 支，当前版本 {head}\n", rolled.len()));
        Ok(out)
    })
}

fn db_dialect_is_mysql(db: &Session<Engine>) -> bool {
    db.dialect() == Dialect::MySql
}

/// 迁移锁。MySQL 上用一个固定名字的 advisory lock 把并发迁移串行化。
///
/// 锁是**连接级**的：进程崩了连接断，锁自动释放，不会留下"锁钥匙被带走"的死局。
/// 所以这里没有做"残留锁清理"——那才是真正容易出错的地方。
fn with_lock<T>(
    db: &mut Session<Engine>,
    no_lock: bool,
    f: impl FnOnce(&mut Session<Engine>) -> Result<T, Failure>,
) -> Result<T, Failure> {
    if no_lock || !db_dialect_is_mysql(db) {
        return f(db);
    }
    acquire_lock(db, LOCK_WAIT_SECS)?;
    let r = f(db);
    // 无论成功失败都放锁；放锁失败本身不该覆盖真正的错误
    if let Err(e) = release_lock(db) {
        return match r {
            Ok(_) => Err(Failure::Runtime(format!("迁移完成但放锁失败：{}", e.message()))),
            Err(orig) => Err(orig),
        };
    }
    r
}

const LOCK_NAME: &str = "rsbatis_migrate";
const LOCK_WAIT_SECS: u32 = 30;

fn acquire_lock(db: &mut Session<Engine>, wait: u32) -> Result<(), Failure> {
    let sql = Sql::raw(format!("SELECT GET_LOCK('{LOCK_NAME}', {wait})"));
    let row = db.fetch_one(&sql)?.ok_or_else(|| {
        Failure::Runtime("GET_LOCK 没有返回结果".to_string())
    })?;
    match row.get::<i64>(0) {
        Ok(1) => Ok(()),
        Ok(0) => Err(Failure::Runtime(format!(
            "等待 {} 秒仍未拿到迁移锁：另一个 `rsbatis migrate` 正在跑，\
             或者有个已崩溃的会话还没释放（正常情况下连接断开即释放）。\
             确认没有其他迁移在跑，可以加 `--no-lock` 跳过加锁",
            wait
        ))),
        Ok(other) => Err(Failure::Runtime(format!(
            "GET_LOCK 返回 {other}（不是 1/0），通常是锁名冲突或权限问题"
        ))),
        Err(e) => Err(Failure::Runtime(format!("读 GET_LOCK 结果失败：{e}"))),
    }
}

fn release_lock(db: &mut Session<Engine>) -> Result<(), Failure> {
    let sql = Sql::raw(format!("SELECT RELEASE_LOCK('{LOCK_NAME}')"));
    db.fetch(&sql)?;
    Ok(())
}

// ---------------------------------------------------------------- new（迁移骨架）

/// 生成 `<版本号>_<名字>.sql` 与 `<版本号>_<名字>.down.sql`。
///
/// 版本号 = `YYYYMMDD` + 当日 3 位序号。用日期前缀而不是纯自增，是为了让人一眼看出
/// "这支迁移是什么时候加的"——迁移顺序出问题时这是第一手线索。
fn migrate_new(cli: &Cli, name: &str) -> CmdResult {
    validate_name(name)?;
    let dir = PathBuf::from(&cli.dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| Failure::Runtime(format!("建目录 {} 失败：{e}", dir.display())))?;

    let today = chrono::Local::now().format("%Y%m%d").to_string();
    let next = next_seq(&dir, &today)?;
    let version = format!("{today}{next:03}");

    let up_path = dir.join(format!("{version}_{name}.sql"));
    let down_path = dir.join(format!("{version}_{name}.down.sql"));
    let up = format!(
        "-- {version} {name}\n\
         --\n\
         -- 这里的 SQL 由 `rsbatis migrate up` 按版本号顺序执行。\n\
         -- SQLite / PostgreSQL 的 DDL 可回滚；MySQL 的 DDL 隐式提交（数据库限制）。\n\
         -- 多条语句用 `;` 分隔，不要用 `BEGIN`/`COMMIT` 包起来——执行器已经开了事务。\n\
         \n\
         -- 在这里写 SQL\n"
    );
    let down = format!(
        "-- {version} {name} 的回滚\n\
         --\n\
         -- 只有注释、没有语句 = 不可回滚：`rsbatis migrate down` 会明确报错，\n\
         -- 而不是打印 \"回滚成功\" 却什么都没做。\n"
    );
    write_new(&up_path, &up)?;
    write_new(&down_path, &down)?;

    Ok(format!(
        "已生成：\n  {}\n  {}\n\n下一步：写 SQL，然后 `rsbatis migrate up`\n",
        up_path.display(),
        down_path.display()
    ))
}

fn write_new(path: &Path, body: &str) -> Result<(), Failure> {
    if path.exists() {
        // 覆盖一个已有的迁移文件会造成"两份不同的 SQL 共享一个版本号"，
        // 在不同机器上表现不一致，所以宁可失败
        return Err(usage(format!(
            "文件已存在，拒绝覆盖：{}（换个名字或换个目录）",
            path.display()
        )));
    }
    std::fs::write(path, body)
        .map_err(|e| Failure::Runtime(format!("写 {} 失败：{e}", path.display())))
}

fn next_seq(dir: &Path, today: &str) -> Result<u32, Failure> {
    let mut max = 0u32;
    for entry in std::fs::read_dir(dir)
        .map_err(|e| Failure::Runtime(format!("读目录 {} 失败：{e}", dir.display())))?
    {
        let entry = entry.map_err(|e| Failure::Runtime(e.to_string()))?;
        let Some(stem) = entry.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };
        if !stem.ends_with(".sql") {
            continue;
        }
        let base = stem.trim_end_matches(".sql").trim_end_matches(".down");
        // 期望 `{today}{seq:03}_name`
        if let Some(rest) = base.strip_prefix(today) {
            if let Some((seq, _)) = rest.split_once('_') {
                if let Ok(n) = seq.parse::<u32>() {
                    max = max.max(n);
                }
            }
        }
    }
    Ok(max + 1)
}

/// 名字会进文件名，所以只允许"能安全出现在路径里"的字符。
fn validate_name(name: &str) -> Result<(), Failure> {
    if name.is_empty() {
        return Err(usage("名字不能为空"));
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !ok {
        return Err(usage(format!(
            "名字 `{name}` 含不允许的字符：只能用字母、数字、`_`、`-`（它会进文件名）"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------- new（项目脚手架）

/// 脚手架固定依赖这个版本。alpha 阶段的 `0.1.0-alpha.X` 是精确匹配，
/// 写 `0.1` 反而解析不到——所以这里必须写全，并在 bump 版本时一起改（见 scripts/bump-version.sh）。
pub const RSBATIS_REQ: &str = "0.1.0-alpha.29";

fn new_project(cli: &Cli, name: &str) -> CmdResult {
    validate_name(name)?;
    let root = PathBuf::from(&cli.into).join(name);
    if root.exists() {
        return Err(usage(format!("目录已存在，拒绝覆盖：{}", root.display())));
    }
    let src = root.join("src");
    let mig = root.join("migrations");
    for d in [&src, &mig] {
        std::fs::create_dir_all(d)
            .map_err(|e| Failure::Runtime(format!("建目录 {} 失败：{e}", d.display())))?;
    }

    let cargo = format!(
        "[package]\n\
         name = \"{name}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         \n\
         [dependencies]\n\
         # alpha 版本是精确匹配，升到稳定版后可以写成 \"0.1\"\n\
         rsbatis = {{ version = \"{RSBATIS_REQ}\", features = [\"sqlite\", \"mysql\", \"migrate\"] }}\n\
         \n\
         [[bin]]\n\
         name = \"{name}\"\n\
         path = \"src/main.rs\"\n"
    );
    let main_rs = format!(
        "use rsbatis::create_engine;\n\
         \n\
         fn main() -> Result<(), Box<dyn std::error::Error>> {{\n\
             let url = std::env::var(\"DATABASE_URL\")\n\
                 .unwrap_or_else(|_| \"sqlite:///app.db\".to_string());\n\
             let db = create_engine(&url)?;\n\
             println!(\"已连接 {{:?}}：{{url}}\", db.dialect());\n\
             Ok(())\n\
         }}\n"
    );
    let readme = format!(
        "# {name}\n\
         \n\
         用 rsbatis 建的项目骨架。\n\
         \n\
         ```bash\n\
         export DATABASE_URL=\"sqlite:///app.db\"\n\
         cargo run                      # 连接测试\n\
         rsbatis migrate status          # 看迁移状态\n\
         rsbatis migrate up              # 应用迁移\n\
         ```\n"
    );
    let gitignore = "target/\n*.db\n*.db-journal\n";
    let init_up = "-- 初始迁移\n-- 在这里写建表 SQL\n";
    let init_down = "-- 初始迁移的回滚\n-- 只有注释 = 不可回滚\n";

    for (p, body) in [
        (root.join("Cargo.toml"), cargo.as_str()),
        (src.join("main.rs"), main_rs.as_str()),
        (root.join("README.md"), readme.as_str()),
        (root.join(".gitignore"), gitignore),
        (mig.join("20260101001_init.sql"), init_up),
        (mig.join("20260101001_init.down.sql"), init_down),
    ] {
        std::fs::write(&p, body)
            .map_err(|e| Failure::Runtime(format!("写 {} 失败：{e}", p.display())))?;
    }

    Ok(format!(
        "已创建 {}：\n  Cargo.toml\n  src/main.rs\n  migrations/20260101001_init.sql\n  migrations/20260101001_init.down.sql\n\n下一步：cd {} && export DATABASE_URL=... && cargo run\n",
        root.display(),
        root.display()
    ))
}

// 让测试能拿到方言判定，不依赖具体驱动类型
#[allow(dead_code)]
fn dialect_from_url(url: &str) -> Result<Dialect, Failure> {
    Ok(dialect_of(url)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_masking_hides_password() {
        assert_eq!(
            describe_url("mysql://root:s3cret@10.0.0.1:3306/app"),
            "mysql://root:***@10.0.0.1:3306/app"
        );
        // 没有密码就不要凭空加 ***
        assert_eq!(describe_url("sqlite:///app.db"), "sqlite:///app.db");
    }

    #[test]
    fn names_with_path_separators_are_refused() {
        assert!(validate_name("../evil").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("创建用户").is_err());
        assert!(validate_name("add_email").is_ok());
        assert!(validate_name("add-email2").is_ok());
    }

    #[test]
    fn seq_increments_within_the_same_day() {
        let d = std::env::temp_dir().join(format!("rsboot_seq_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(next_seq(&d, "20260101").unwrap(), 1);
        std::fs::write(d.join("20260101001_a.sql"), "").unwrap();
        assert_eq!(next_seq(&d, "20260101").unwrap(), 2);
        std::fs::write(d.join("20260101002_a.down.sql"), "").unwrap();
        assert_eq!(next_seq(&d, "20260101").unwrap(), 3);
        // 其他日期不干扰
        std::fs::write(d.join("20251231009_b.sql"), "").unwrap();
        assert_eq!(next_seq(&d, "20260101").unwrap(), 3);
        let _ = std::fs::remove_dir_all(&d);
    }
}
