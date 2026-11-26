//! 写入宏 `update!` / `delete!` 的最小可运行示例（SQLite 内存库）。
//!
//! 跑：`cargo run -p rsbatis --features sqlite --example dml_macro`

use rsbatis::{delete, update, Entity, FromRow, SelectStmt, Session, SqliteDriver};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    name: String,
    age: i32,
}

fn dump(db: &mut Session<SqliteDriver>) -> Vec<String> {
    let mut rows: Vec<User> = db
        .all(&SelectStmt::for_entity::<User>())
        .unwrap();
    rows.sort_by_key(|u| u.id);
    rows.into_iter()
        .map(|u| format!("  #{} {:<4} age={}", u.id, u.name, u.age))
        .collect()
}

fn main() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER NOT NULL);
         INSERT INTO users (id, name, age) VALUES (1, '张三', 17), (2, '李四', 25), (3, '王五', 17);",
    )
    .unwrap();

    println!("初始：\n{}", dump(&mut db).join("\n"));

    // 把李四的年龄改成 30（单行更新），其余人不动
    let stmt = update!(User { age: 30 } where User.name == "李四");
    let sql = db.explain(&stmt);
    println!("\nSQL: {}\n参数: {:?}", sql.text, sql.params);
    db.execute(sql).unwrap();
    println!("\nupdate! 之后：\n{}", dump(&mut db).join("\n"));

    // 删掉未成年人
    let stmt = delete!(User where User.age < 18);
    println!("\nSQL: {}", db.explain(&stmt).text);
    db.execute(&stmt).unwrap();
    println!("\ndelete! 之后：\n{}", dump(&mut db).join("\n"));
}
