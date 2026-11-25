
//! 级联写入：父与子的 id 自动接上。
//!
//! 运行：`cargo run -p rsbatis --features sqlite --example cascade_save`

use rsbatis::{Entity, FromRow, SelectStmt, Session, SortOrder, SqliteDriver};

#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "dept")]
#[allow(dead_code)]
struct Dept {
    #[rsbatis(primary_key)]
    id: Option<i64>,   // None 时跳过列 → 数据库自增
    name: String,
}

#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "staff")]
#[allow(dead_code)]
struct Staff {
    #[rsbatis(primary_key)]
    id: Option<i64>,
    dept_id: i64,
    name: String,
}

fn main() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE dept (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
         CREATE TABLE staff (id INTEGER PRIMARY KEY, dept_id INTEGER NOT NULL,
                             name TEXT NOT NULL);",
    )
    .unwrap();

    // 从"还没入库的对象图"开始：部门只给了名字，员工挂在部门名下
    let drafts = [
        ("研发", vec!["张三", "李四"]),
        ("销售", vec!["王五"]),
    ];
    let mut depts: Vec<Dept> = drafts
        .iter()
        .map(|(name, _)| Dept { id: None, name: (*name).to_string() })
        .collect();

    // 一条调用：插部门 → 拿回 id 填进 dept.id → 用它造员工 → 批量插员工
    let (parents, children) = db
        .insert_tree(
            &mut depts,
            |d, id| d.id = Some(id),
            |d| {
                let names = drafts
                    .iter()
                    .find(|(n, _)| n == &d.name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                names
                    .into_iter()
                    .map(|n| Staff { id: None, dept_id: d.id.unwrap_or(0), name: n.to_string() })
                    .collect()
            },
        )
        .unwrap();
    println!("插入 {parents} 个部门、{children} 名员工\n");

    let staff: Vec<Staff> = db
        .fetch_all(
            SelectStmt::new("staff")
                .column("id")
                .column("dept_id")
                .column("name")
                .order_by(rsbatis::Expr::bare_col("id"), SortOrder::Asc)
                .build(db.dialect()),
        )
        .unwrap();
    for s in &staff {
        let dept = depts.iter().find(|d| d.id == Some(s.dept_id)).unwrap();
        println!("{} → {} 部（dept_id = {}）", s.name, dept.name, s.dept_id);
    }

    // 整批事务：任何一条失败，父也不会留下来
    let mut bad = vec![Dept { id: None, name: "会失败".into() }];
    let res = db.insert_tree(&mut bad, |d, id| d.id = Some(id), |d| {
        vec![
            Staff { id: Some(7), dept_id: d.id.unwrap_or(0), name: "a".into() },
            Staff { id: Some(7), dept_id: d.id.unwrap_or(0), name: "b".into() },
        ]
    });
    println!("\n子表主键冲突 → {:?}", res.err().map(|e| e.message));
    let dept_count: i64 = db
        .fetch_one(
            SelectStmt::new("dept")
                .column_expr(rsbatis::Expr::count_star())
                .build(db.dialect()),
        )
        .unwrap()
        .unwrap()
        .get::<i64>(0)
        .unwrap();
    println!("回滚后 dept 仍有 {dept_count} 行（失败那批没留下父）");
}
