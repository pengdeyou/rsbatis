//! README 的 quick-start 示例，作为可编译的对照物存在。
//!
//! README 里的代码和这里必须一致：文档里的示例一旦写错（历史上出现过把宏当方法调用、
//! 或者 `select!(User)` 这种根本不成立的写法），读者会照着写进项目再撞编译错误。
//! 把这个示例放进 `examples/`，CI 的 `cargo build --examples` 就会替文档把关。

use rsbatis::{insert, select, where_, Dialect, Entity, FromRow, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "users")]
pub struct User {
    pub id: i64,
    pub name: String,
    pub age: Option<i32>, // 可空列；col_age() 仍是 Column<i32>
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Session::new(SqliteDriver::open_in_memory()?);
    db.driver_mut().execute_batch(
        r#"CREATE TABLE "users" (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER);"#,
    )?;

    for (name, age) in [("张三", 20), ("李四", 17), ("王五", 30)] {
        db.execute(
            insert::<User>()
                .set(User::col_name(), name)
                .set(User::col_age(), age)
                .build(Dialect::Sqlite),
        )?;
    }

    // 一个宏写完整个查询
    let adults: Vec<User> =
        db.fetch_all(&select! { User where User.age >= 18 order_by User.age desc })?;
    for u in &adults {
        println!("{} ({})", u.name, u.age.unwrap_or(-1));
    }

    // 或者 builder + 表达式宏
    let stmt = select::<User>().where_(where_!(User.age >= 18 && User.name.like("张%")));
    let rows = db.fetch(&stmt)?;
    println!("{} rows via builder", rows.len());

    Ok(())
}
