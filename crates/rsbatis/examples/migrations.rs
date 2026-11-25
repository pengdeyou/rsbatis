//! 迁移工具的可编译示例（对应 README 的 "Migrations" 一节）。
//!
//! 用内存库演示，所以迁移从代码构造；真实项目用 `from_dir("migrations")` 从
//! `<版本>_<名字>.sql` + `<版本>_<名字>.down.sql` 文件加载。

use rsbatis::{Migration, Migrator, Session, SqliteDriver};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Session::new(SqliteDriver::open_in_memory()?);
    let migrations = vec![
        Migration::new(
            1,
            "create_users",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
        )
        .with_down("DROP TABLE users"),
        Migration::new(2, "add_email", "ALTER TABLE users ADD COLUMN email TEXT")
            .with_down("ALTER TABLE users DROP COLUMN email"),
    ];

    let mut m = Migrator::new(&mut db, migrations)?;
    let applied = m.up()?;
    println!("applied: {applied:?}");

    // status 报告已应用 / 待应用，而不是让调用方自己去猜
    println!("{:?}", m.status()?);

    // 回滚到指定版本：down_to 会逐个执行 down
    m.down_to(1)?;
    Ok(())
}
