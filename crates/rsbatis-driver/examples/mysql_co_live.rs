//! 真机验证：连真实 MySQL，走 `mysql+co://` 跑完 CRUD + 事务 + utf8mb4。
//!
//! 跑法：
//!
//! ```text
//! MYSQL_CO_URL='mysql+co://root:root@127.0.0.1:3306/rsbatis_co_test' \
//!   cargo run -p rsbatis-driver --example mysql_co_live --features mysql
//! ```
//!
//! 结束时会打印"协程 socket 走了多少次读写"。计数 > 0 就证明这条链路
//! 没停在"配了但没用"的状态。
//!
//! 已验证环境：MySQL 8.0.33（`caching_sha2_password`），macOS x86_64 本机。

use rsbatis_core::Sql;
use rsbatis_driver::mysql_driver::MySqlDriver;
use rsbatis_driver::Driver;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("MYSQL_CO_URL")
        .unwrap_or_else(|_| "mysql+co://root:root@127.0.0.1:3306/rsbatis_co_test".to_string());
    println!("连接：{}", url.replace("root:root@", "root:***@"));

    let mut d = MySqlDriver::connect(&url)?;
    println!("握手完成，dialect = {:?}", d.dialect());

    d.execute_batch(
        "DROP TABLE IF EXISTS co_probe;\n\
         CREATE TABLE co_probe (id INT PRIMARY KEY, name VARCHAR(32) NOT NULL)",
    )?;
    println!("建表完成");

    for i in 1..=3 {
        d.execute(&Sql::raw(format!(
            "INSERT INTO co_probe (id, name) VALUES ({i}, 'row{i}')"
        )))?;
    }
    println!("插入 3 行");

    let rows = d.fetch(&Sql::raw("SELECT id, name FROM co_probe ORDER BY id"))?;
    println!("查询到 {} 行：", rows.len());
    for r in &rows {
        let id: i64 = r.get(0)?;
        let name: String = r.get(1)?;
        println!("  id={id} name={name}");
    }
    assert_eq!(rows.len(), 3, "插入 3 行却查回 {} 行", rows.len());

    // 事务：插一行再回滚，必须查不到
    d.begin()?;
    d.execute(&Sql::raw(
        "INSERT INTO co_probe (id, name) VALUES (99, 'should-vanish')",
    ))?;
    d.rollback()?;
    let after = d.fetch(&Sql::raw("SELECT id FROM co_probe WHERE id = 99"))?;
    assert!(after.is_empty(), "回滚失败：id=99 还在");
    println!("事务回滚生效");

    // 中文 + emoji：字符集与长度编码这一路的边界
    d.execute(&Sql::raw(
        "INSERT INTO co_probe (id, name) VALUES (100, '中文数据✅')",
    ))?;
    let cn = d.fetch(&Sql::raw("SELECT name FROM co_probe WHERE id = 100"))?;
    let got: String = cn[0].get(0)?;
    assert_eq!(got, "中文数据✅", "utf8mb4 往返不一致：{got:?}");
    println!("utf8mb4 往返一致");

    let stats = mysql::co_stats();
    println!(
        "协程 socket：连接 {} 条 / 读 {} 次 / 写 {} 次",
        stats.0, stats.1, stats.2
    );
    assert!(stats.0 >= 1, "没有走 Co socket");
    assert!(stats.1 > 0 && stats.2 > 0, "Co socket 没真正读写");

    d.execute_batch("DROP TABLE co_probe")?;
    println!("全部通过");
    Ok(())
}
