//! rsbatis-cli 的端到端测试：真的建库、真的跑迁移、真的回滚。
//!
//! 全部用 SQLite 临时文件，不依赖外部服务；MySQL 侧的加锁逻辑在
//! `cli_mysql_lock` 里单独测（需要 `RSBATIS_MYSQL_URL`）。

use std::fs;
use std::path::PathBuf;

fn tmp(prefix: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "rscli_e2e_{prefix}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn run(v: &[&str]) -> rsbatis_cli::Output {
    rsbatis_cli::run(&args(v), None)
}

/// 用 sqlite 绝对路径拼 URL。**四个斜杠**：`sqlite://` + `/tmp/...`
fn sqlite_url(p: &std::path::Path) -> String {
    format!("sqlite:///{}", p.display())
}

struct Fix {
    dir: PathBuf,
    db: PathBuf,
}

impl Fix {
    fn new(name: &str) -> Self {
        let dir = tmp(&format!("{name}_mig"));
        let dbf = std::env::temp_dir().join(format!(
            "rscli_e2e_{name}_{}.db",
            std::process::id()
        ));
        let _ = fs::remove_file(&dbf);
        Self { dir, db: dbf }
    }
    fn mig(&self, file: &str, sql: &str) {
        fs::write(self.dir.join(file), sql).unwrap();
    }
    fn url(&self) -> String {
        sqlite_url(&self.db)
    }
    /// 直接读库，验证 DDL 真的生效（不信 CLI 自己的输出）
    fn table_exists(&self, table: &str) -> bool {
        let mut db = rsbatis::create_engine(&self.url()).unwrap();
        let sql = rsbatis::Sql::raw(format!(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{table}'"
        ));
        let row = db.first(&sql).unwrap().unwrap();
        row.get::<i64>(0).unwrap() > 0
    }
    fn applied_count(&self) -> i64 {
        let mut db = rsbatis::create_engine(&self.url()).unwrap();
        let sql = rsbatis::Sql::raw(format!(
            "SELECT COUNT(*) FROM {}",
            rsbatis::MIGRATIONS_TABLE
        ));
        match db.first(&sql) {
            Ok(Some(r)) => r.get::<i64>(0).unwrap(),
            _ => 0,
        }
    }
}

impl Drop for Fix {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
        let _ = fs::remove_file(&self.db);
        let _ = fs::remove_file(self.db.with_extension("db-journal"));
    }
}

#[test]
fn full_cycle_up_status_down() {
    let f = Fix::new("cycle");
    f.mig(
        "20260101001_users.sql",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);",
    );
    f.mig(
        "20260101001_users.down.sql",
        "DROP TABLE users;",
    );
    f.mig(
        "20260101002_posts.sql",
        "CREATE TABLE posts (id INTEGER PRIMARY KEY, title TEXT);",
    );
    f.mig("20260101002_posts.down.sql", "DROP TABLE posts;");

    // 迁移前：两张表都不存在
    assert!(!f.table_exists("users"));
    assert_eq!(f.applied_count(), 0);

    // status：两支都未应用
    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "status", "-u", &f.url()]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(o.stdout.matches("  未应用  ").count(), 2, "{}", o.stdout);
    assert!(o.stdout.contains("20260101001"), "{}", o.stdout);

    // up：两支都应用
    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "up", "-u", &f.url()]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stdout.contains("应用 20260101001"), "{}", o.stdout);
    assert!(o.stdout.contains("当前版本 20260101002"), "{}", o.stdout);
    assert!(f.table_exists("users"));
    assert!(f.table_exists("posts"));
    assert_eq!(f.applied_count(), 2);

    // 再跑一次：幂等，什么都不做
    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "up", "-u", &f.url()]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stdout.contains("没有待应用的迁移"), "{}", o.stdout);
    assert_eq!(f.applied_count(), 2);

    // status：全部已应用
    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "status", "-u", &f.url()]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stdout.contains("全部已应用"), "{}", o.stdout);

    // down：回滚最后一支，posts 没了，users 还在
    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "down", "-u", &f.url()]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stdout.contains("回滚 20260101002"), "{}", o.stdout);
    assert!(!f.table_exists("posts"));
    assert!(f.table_exists("users"));
    assert_eq!(f.applied_count(), 1);

    // down --to 0：全回滚
    let o = run(&[
        "-d",
        &f.dir.to_string_lossy(),
        "migrate",
        "down",
        "--to",
        "0",
        "-u",
        &f.url(),
    ]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(!f.table_exists("users"));
    assert_eq!(f.applied_count(), 0);
}

#[test]
fn up_to_stops_at_target_version() {
    let f = Fix::new("upto");
    f.mig("20260101001_a.sql", "CREATE TABLE a (x INTEGER);");
    f.mig("20260101002_b.sql", "CREATE TABLE b (x INTEGER);");
    f.mig("20260101003_c.sql", "CREATE TABLE c (x INTEGER);");

    let o = run(&[
        "-d",
        &f.dir.to_string_lossy(),
        "migrate",
        "up",
        "--to",
        "20260101002",
        "-u",
        &f.url(),
    ]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(f.table_exists("a"));
    assert!(f.table_exists("b"));
    assert!(!f.table_exists("c"), "--to 之后的版本不该被应用");
    assert_eq!(f.applied_count(), 2);
}

#[test]
fn dry_run_needs_no_database_and_touches_nothing() {
    let f = Fix::new("dry");
    f.mig(
        "20260101001_ddl.sql",
        "CREATE TABLE t (id INTEGER PRIMARY KEY);",
    );
    // 故意给一个打不开的 URL：dry-run 根本不该连库
    let o = run(&[
        "-d",
        &f.dir.to_string_lossy(),
        "migrate",
        "up",
        "--dry-run",
        "-u",
        "mysql://nobody@127.0.0.1:1/none",
    ]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stdout.contains("CREATE TABLE t"), "{}", o.stdout);
    assert!(!f.db.exists(), "dry-run 不该创建数据库文件");
}

#[test]
fn failed_migration_leaves_markers_untouched() {
    let f = Fix::new("fail");
    // 第二支故意语法错误
    f.mig("20260101001_ok.sql", "CREATE TABLE ok1 (x INTEGER);");
    f.mig("20260101002_bad.sql", "THIS IS NOT SQL;");

    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "up", "-u", &f.url()]);
    assert_eq!(o.code, 2, "执行失败应该是退出码 2");
    assert!(!o.stderr.is_empty());

    // 第一支成功、第二支失败：元数据里只该有 1 条，且 ok1 表存在
    assert!(f.table_exists("ok1"));
    assert_eq!(f.applied_count(), 1, "失败的那支不能记账");
}

#[test]
fn down_refuses_when_there_is_no_rollback_script() {
    let f = Fix::new("nodown");
    f.mig("20260101001_a.sql", "CREATE TABLE a (x INTEGER);");
    // 只给正向，不给 down
    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "up", "-u", &f.url()]);
    assert_eq!(o.code, 0, "{}", o.stderr);

    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "down", "-u", &f.url()]);
    assert_eq!(o.code, 2, "不可回滚必须失败");
    assert!(o.stderr.contains("没有 down 脚本"), "{}", o.stderr);
    // 表还在：失败就是失败，不能偷偷删
    assert!(f.table_exists("a"));
}

#[test]
fn comment_only_down_is_treated_as_not_reversible() {
    let f = Fix::new("commentdown");
    f.mig("20260101001_a.sql", "CREATE TABLE a (x INTEGER);");
    // 脚手架生成的就是这种形态：只有注释
    f.mig(
        "20260101001_a.down.sql",
        "-- 只有注释、没有语句 = 不可回滚\n",
    );
    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "up", "-u", &f.url()]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "down", "-u", &f.url()]);
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("没有 down 脚本"), "{}", o.stderr);
}

#[test]
fn missing_database_in_record_is_reported_not_guessed() {
    let f = Fix::new("stray");
    f.mig("20260101001_a.sql", "CREATE TABLE a (x INTEGER);");
    run(&["-d", &f.dir.to_string_lossy(), "migrate", "up", "-u", &f.url()]);

    // 把迁移文件删掉，模拟"切到了没有这支迁移的分支"
    fs::remove_file(f.dir.join("20260101001_a.sql")).unwrap();
    f.mig("20260101002_b.sql", "CREATE TABLE b (x INTEGER);");

    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "down", "-u", &f.url()]);
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("不在当前迁移列表"), "{}", o.stderr);
}

#[test]
fn migrate_new_generates_a_pair_and_refuses_overwrite() {
    let f = Fix::new("new");
    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "new", "add_email"]);
    assert_eq!(o.code, 0, "{}", o.stderr);

    let files: Vec<String> = fs::read_dir(&f.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let ups: Vec<&String> = files.iter().filter(|n| !n.contains(".down.")).collect();
    assert_eq!(ups.len(), 1, "{files:?}");
    let up_name = ups[0];
    assert!(up_name.ends_with("_add_email.sql"), "{up_name}");
    // 版本号是 日期+3位序号
    let ver = up_name.split('_').next().unwrap();
    assert_eq!(ver.len(), 11, "版本号应为 YYYYMMDDNNN：{ver}");
    assert!(ver.chars().all(|c| c.is_ascii_digit()));

    let down_name = files.iter().find(|n| n.contains(".down.")).unwrap();
    assert!(down_name.starts_with(ver), "{down_name} 应与 {up_name} 同版本");

    // 同一天再建一支：序号 +1
    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "new", "add_phone"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let second = fs::read_dir(&f.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with("_add_phone.sql"))
        .expect("第二支没生成");
    let seq2: u32 = second.split('_').next().unwrap()[8..].parse().unwrap();
    let seq1: u32 = ver[8..].parse().unwrap();
    assert_eq!(seq2, seq1 + 1, "同日序号应该递增");

    // 生成的 down 只有注释 → 加载器视为不可回滚，不是"能回滚但什么都不做"
    let ms = rsbatis::from_dir(&f.dir).unwrap();
    assert!(ms.iter().all(|m| m.down.is_none()));
}

#[test]
fn empty_scaffold_is_loadable_but_refused_at_up() {
    // 骨架里 up 只有注释：`status` 要能看到它（文件先存在），`up` 要拒绝执行。
    let f = Fix::new("emptyup");
    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "new", "placeholder"]);
    assert_eq!(o.code, 0, "{}", o.stderr);

    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "status", "-u", &f.url()]);
    assert_eq!(o.code, 0, "空骨架不该让 status 失败：{}", o.stderr);
    assert!(o.stdout.contains("未应用"), "{}", o.stdout);

    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "up", "-u", &f.url()]);
    assert_eq!(o.code, 2, "空迁移执行必须失败");
    assert!(o.stderr.contains("没有语句"), "{}", o.stderr);
    assert_eq!(f.applied_count(), 0, "失败的迁移不能记账");
}

#[test]
fn migrate_new_refuses_to_overwrite_existing_file() {
    let f = Fix::new("overwrite");
    fs::write(f.dir.join("20260101001_x.sql"), "SELECT 1;").unwrap();
    // 名字含非法字符
    let o = run(&["-d", &f.dir.to_string_lossy(), "migrate", "new", "a/b"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("不允许的字符"), "{}", o.stderr);
}

#[test]
fn new_project_scaffolds_and_migration_dir_is_loadable() {
    let root = tmp("proj");
    let o = run(&["new", "todo", "--into", &root.to_string_lossy()]);
    assert_eq!(o.code, 0, "{}", o.stderr);

    let p = root.join("todo");
    assert!(p.join("Cargo.toml").is_file());
    assert!(p.join("src/main.rs").is_file());
    assert!(p.join("README.md").is_file());

    // 生成的 Cargo.toml 要指向当前工作区版本（alpha 是精确匹配，写错就装不上）
    let cargo = fs::read_to_string(p.join("Cargo.toml")).unwrap();
    assert!(
        cargo.contains(&format!("version = \"{}\"", rsbatis_cli::RSBATIS_REQ)),
        "{cargo}"
    );
    assert!(cargo.contains("features = [\"sqlite\", \"mysql\", \"migrate\"]"));

    // 生成的迁移目录必须真的能被加载（否则脚手架产出的是废件）
    let ms = rsbatis::from_dir(p.join("migrations")).unwrap();
    assert_eq!(ms.len(), 1);
    assert_eq!(ms[0].version, 20260101001);
    assert!(ms[0].down.is_none(), "骨架的 down 只有注释 = 不可回滚");

    // 同名再建一次要拒绝
    let o = run(&["new", "todo", "--into", &root.to_string_lossy()]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("已存在"), "{}", o.stderr);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn scaffolded_project_migrations_apply_cleanly() {
    let root = tmp("projrun");
    let proj = root.join("shop");
    let o = run(&["new", "shop", "--into", &root.to_string_lossy()]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    // 往骨架的迁移里填一条真 SQL
    fs::write(
        proj.join("migrations/20260101001_init.sql"),
        "CREATE TABLE shop_item (id INTEGER PRIMARY KEY, name TEXT);",
    )
    .unwrap();

    let dbf = root.join("shop.db");
    let url = sqlite_url(&dbf);
    let o = run(&[
        "-d",
        &proj.join("migrations").to_string_lossy(),
        "migrate",
        "up",
        "-u",
        &url,
    ]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(dbf.exists());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn password_is_masked_in_status_output() {
    // 只是字符串断言：不连库，故 URL 指到不可能存在的主机
    let f = Fix::new("mask");
    f.mig("20260101001_a.sql", "SELECT 1;");
    let o = run(&[
        "-d",
        &f.dir.to_string_lossy(),
        "migrate",
        "status",
        "-u",
        "mysql://root:supersecret@127.0.0.1:1/app",
    ]);
    // 连不上是运行时错误（2），但报错里绝不能出现明文密码
    assert_eq!(o.code, 2);
    let all = format!("{}{}", o.stdout, o.stderr);
    assert!(
        !all.contains("supersecret"),
        "输出里泄露了密码：{all}"
    );
}

#[test]
fn unknown_scheme_is_rejected_before_connecting() {
    let f = Fix::new("scheme");
    f.mig("20260101001_a.sql", "SELECT 1;");
    let o = run(&[
        "-d",
        &f.dir.to_string_lossy(),
        "migrate",
        "status",
        "-u",
        "redis://localhost/0",
    ]);
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("无法识别的数据库 URL"), "{}", o.stderr);
}

#[test]
fn postgres_url_says_what_is_missing() {
    let f = Fix::new("pg");
    f.mig("20260101001_a.sql", "SELECT 1;");
    let o = run(&[
        "-d",
        &f.dir.to_string_lossy(),
        "migrate",
        "status",
        "-u",
        "postgresql://u:p@h/db",
    ]);
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("尚未实现"), "{}", o.stderr);
}
