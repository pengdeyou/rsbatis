//! 级联写入在 MariaDB 上。MySQL 没有 `RETURNING`，这里走 `last_insert_id` 路径，
//! 所以这批测试真正验证的是"归零 + 自增值"这半条路。

#![cfg(feature = "mysql")]

use rsbatis::{Entity, FromRow, MySqlDriver, SelectStmt, Session, SortOrder};

#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "k_dept")]
#[allow(dead_code)]
struct Dept {
    #[rsbatis(primary_key)]
    id: Option<i64>,
    name: String,
}

#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "k_staff")]
#[allow(dead_code)]
struct Staff {
    #[rsbatis(primary_key)]
    id: Option<i64>,
    dept_id: i64,
    name: String,
}

/// 有 id 列但**不是自增**的表：插入成功后 LAST_INSERT_ID 仍为 0。
#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "k_noauto")]
#[allow(dead_code)]
struct NoAuto {
    #[rsbatis(primary_key)]
    id: Option<i64>,
    name: String,
}

fn fresh() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut db = Session::new(MySqlDriver::connect(&url).expect("连接 MySQL"));
    db.execute_batch(
        "DROP TABLE IF EXISTS k_staff;
         DROP TABLE IF EXISTS k_dept;
         DROP TABLE IF EXISTS k_noauto;
         CREATE TABLE k_dept (id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
                              name VARCHAR(32) NOT NULL) ENGINE=InnoDB;
         CREATE TABLE k_staff (id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
                               dept_id BIGINT NOT NULL,
                               name VARCHAR(32) NOT NULL) ENGINE=InnoDB;
         CREATE TABLE k_noauto (id BIGINT NOT NULL DEFAULT 0 PRIMARY KEY,
                                name VARCHAR(32) NOT NULL) ENGINE=InnoDB;",
    )
    .expect("建表");
    Some(db)
}

#[test]
fn insert_autoincrement() {
    let mut db = match fresh() {
        Some(d) => d,
        None => return,
    };
    let a = db
        .insert(&Dept { id: None, name: "A".into() })
        .unwrap();
    let b = db
        .insert(&Dept { id: None, name: "B".into() })
        .unwrap();
    assert_eq!((1, 2), (a, b));
}

#[test]
fn insert_explicit_pk_returns_that_value() {
    let mut db = match fresh() {
        Some(d) => d,
        None => return,
    };
    // 显式主键：自增没被触发，LAST_INSERT_ID 不会变；此时不能误判成失败
    let got = db
        .insert(&Dept {
            id: Some(50),
            name: "显式".into(),
        })
        .unwrap();
    assert_eq!(50, got);
}

#[test]
fn insert_tree_on_mariadb() {
    let mut db = match fresh() {
        Some(d) => d,
        None => return,
    };
    let mut depts = vec![
        Dept {
            id: None,
            name: "研发".into(),
        },
        Dept {
            id: None,
            name: "销售".into(),
        },
    ];
    let (pc, cc) = db
        .insert_tree(
            &mut depts,
            |d, id| d.id = Some(id),
            |d| {
                let who = if d.name == "研发" {
                    ("张三", "李四")
                } else {
                    ("王五", "赵六")
                };
                [who.0, who.1]
                    .iter()
                    .map(|n| Staff {
                        id: None,
                        dept_id: d.id.unwrap_or(0),
                        name: (*n).to_string(),
                    })
                    .collect()
            },
        )
        .unwrap();
    assert_eq!((2, 4), (pc, cc));

    let staff: Vec<Staff> = db
        .all(
            SelectStmt::new("k_staff")
                .column("id")
                .column("dept_id")
                .column("name")
                .order_by(rsbatis::Expr::bare_col("id"), SortOrder::Asc)
                .build(db.dialect()),
        )
        .unwrap();
    let d0 = depts[0].id.unwrap();
    let d1 = depts[1].id.unwrap();
    assert_eq!(2, staff.iter().filter(|s| s.dept_id == d0).count());
    assert_eq!(2, staff.iter().filter(|s| s.dept_id == d1).count());
    assert!(
        staff.iter().all(|s| s.dept_id == d0 || s.dept_id == d1),
        "所有子都必须挂在这两个父的 id 上，不能出现 0"
    );
}

#[test]
fn table_without_autoincrement_reports_zero_instead_of_lying() {
    let mut db = match fresh() {
        Some(d) => d,
        None => return,
    };
    // k_noauto 有 id 列但不是自增：归零后插入，读回来仍是 0 → 必须报错，
    // 不能把 0 当成主键回填给子表（那会让所有子都挂到 id=0 上）
    let err = db
        .insert(&NoAuto {
            id: None,
            name: "x".into(),
        })
        .unwrap_err();
    assert!(
        err.message.contains("没有拿到自增值"),
        "got: {}",
        err.message
    );
}
