use rsbatis::{select, Entity};

#[derive(Entity)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    age: i32,
}

fn main() {
    let n = 10usize;
    // limit/offset 只接受字面量：变量传进去会让 SQL 结构不可预测
    let _ = select! { User where User.age >= 18 limit n };
}
