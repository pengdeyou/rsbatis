
//! `INTERSECT` 与 `EXCEPT`，以及跨引擎优先级陷阱的现场演示。
//!
//! 运行：`cargo run -p rsbatis --features sqlite --example set_ops`

use rsbatis::{BinOp, Entity, Expr, FromRow, SelectStmt, Session, SortOrder, SqliteDriver};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "emp")]
#[allow(dead_code)]
struct Emp {
    id: i64,
    team: i64,
}

#[derive(FromRow, Debug)]
struct IdRow {
    id: i64,
}

fn main() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE emp (id INTEGER NOT NULL, team INTEGER NOT NULL);
         CREATE TABLE oncall (id INTEGER NOT NULL);
         INSERT INTO emp (id, team) VALUES (1, 1), (2, 1), (3, 2), (4, 2), (5, 3);
         INSERT INTO oncall (id) VALUES (2), (3), (5);",
    )
    .unwrap();

    let emp_ids = SelectStmt::new("emp").column("id");
    let oncall_ids = SelectStmt::new("oncall").column("id");

    // 1) 交集：既在员工里又在值班名单里
    let both = emp_ids.intersect(&oncall_ids);
    println!("SQL: {}", db.explain(&both).text);
    let rows: Vec<IdRow> = db.fetch_all(&both).unwrap();
    println!("既在名册又在值班：{:?}\n", rows.iter().map(|r| r.id).collect::<Vec<_>>());

    // 2) 差集：所有员工减去值班的人
    let not_oncall = SelectStmt::new("emp")
        .column("id")
        .except(&oncall_ids)
        .order_by(Expr::bare_col("id"), SortOrder::Asc);
    println!("SQL: {}", db.explain(&not_oncall).text);
    let rows: Vec<IdRow> = db.fetch_all(&not_oncall).unwrap();
    println!("没排值班的：{:?}\n", rows.iter().map(|r| r.id).collect::<Vec<_>>());

    // 3) 链式差集：(全部 - 名册A) - 名册B
    let mut others = SelectStmt::new("emp")
        .column("id")
        .except(&oncall_ids);
    let late = SelectStmt::new("emp").column("id").and_filter(Expr::cmp(
        BinOp::Gt,
        Expr::col("emp", "team"),
        Expr::lit(1_i64),
    ));
    others = others.except(&late);
    println!("SQL: {}", db.explain(&others).text);
    let rows: Vec<IdRow> = db.fetch_all(&others).unwrap();
    println!("名册外且不属于 team 2 之后：{:?}\n", rows.iter().map(|r| r.id).collect::<Vec<_>>());

    // 4) ⚠️ 混用 UNION 与 INTERSECT：两个引擎结果不同，build() 会告警
    let mixed = SelectStmt::new("emp")
        .column("id")
        .union_all(&SelectStmt::new("oncall").column("id"))
        .intersect(&late);
    let sql = db.explain(&mixed);
    println!("SQL: {}", sql.text);
    for w in &sql.warnings {
        println!("⚠️  {}", w);
    }
    let rows: Vec<IdRow> = db.fetch_all(&mixed).unwrap();
    println!("SQLite 上是左结合 (A∪B)∩C：{:?}", rows.iter().map(|r| r.id).collect::<Vec<_>>());
    println!("同一条 SQL 在 MySQL/MariaDB 上会按 A∪(B∩C) 求值，结果不同");
}
