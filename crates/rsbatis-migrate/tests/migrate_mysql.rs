//! M6：迁移工具在真实 MySQL/MariaDB 上的行为验证。
//!
//! 这里有一条**故意验证坏消息**的用例：MySQL 的 DDL 隐式提交，所以迁移中途失败
//! 会留下半成品。把它写成测试，是为了防止以后有人以为"包了事务就原子了"。

use rsbatis_driver::mysql_driver::MySqlDriver;
use rsbatis_migrate::{Migration, Migrator};
use rsbatis_orm::Session;

fn url() -> Option<String> {
    std::env::var("RSBATIS_MYSQL_URL").ok()
}

fn db() -> Option<Session<MySqlDriver>> {
    let url = url()?;
    let d = MySqlDriver::connect(&url).expect("连接 MySQL");
    Some(Session::new(d))
}

fn table_exists(s: &mut Session<MySqlDriver>, name: &str) -> bool {
    let sql = format!(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = '{name}'"
    );
    !s.fetch(rsbatis_core::Sql::raw(sql))
        .expect("查表")
        .is_empty()
}

fn cleanup(s: &mut Session<MySqlDriver>) {
    for t in ["v_mig_users", "v_mig_half", "_rsbatis_migrations"] {
        let _ = s.execute(rsbatis_core::Sql::raw(format!("DROP TABLE IF EXISTS {t}")));
    }
}

#[test]
fn mysql_up_down_and_metadata() {
    let Some(mut s) = db() else {
        eprintln!("跳过：未设置 RSBATIS_MYSQL_URL");
        return;
    };
    cleanup(&mut s);

    let ms = vec![
        Migration::new(
            1,
            "create_users",
            "CREATE TABLE v_mig_users (id BIGINT PRIMARY KEY, name VARCHAR(64) NOT NULL)",
        )
        .with_down("DROP TABLE v_mig_users"),
        Migration::new(
            2,
            "add_email",
            "ALTER TABLE v_mig_users ADD COLUMN email VARCHAR(128)",
        )
        .with_down("ALTER TABLE v_mig_users DROP COLUMN email"),
    ];
    let mut m = Migrator::new(&mut s, ms).expect("构造");
    assert_eq!(m.up().expect("up"), vec![1, 2]);
    assert!(table_exists(m.session_mut(), "v_mig_users"));
    assert!(table_exists(m.session_mut(), "_rsbatis_migrations"));
    assert!(!m.supports_transactional_ddl(), "MySQL 的 DDL 隐式提交");

    let st = m.status().expect("status");
    assert!(st.iter().all(|x| x.applied));

    assert_eq!(m.down().expect("down"), Some(2));
    assert_eq!(m.applied_versions().expect("applied"), vec![1]);
    assert_eq!(m.down_to(0).expect("down_to"), vec![1]);
    assert!(!table_exists(m.session_mut(), "v_mig_users"));
    assert_eq!(m.applied_versions().expect("applied"), Vec::<i64>::new());
}

#[test]
fn mysql_failed_migration_leaves_half_state() {
    let Some(mut s) = db() else {
        eprintln!("跳过：未设置 RSBATIS_MYSQL_URL");
        return;
    };
    cleanup(&mut s);

    let mut m = Migrator::new(
        &mut s,
        vec![Migration::new(
            1,
            "half_broken",
            "CREATE TABLE v_mig_half (id BIGINT); INSERT INTO v_mig_nope VALUES (1)",
        )],
    )
    .expect("构造");

    assert!(m.up().is_err());
    // 与 SQLite 相反：第一句 DDL 已经隐式提交，表留下了。
    assert!(
        table_exists(m.session_mut(), "v_mig_half"),
        "MySQL DDL 不可回滚——半成品表会留下，这是数据库限制"
    );
    // 但元数据没记上，所以下次 up 会重新尝试这支迁移。
    assert!(m.applied_versions().expect("applied").is_empty());
    cleanup(m.session_mut());
}
