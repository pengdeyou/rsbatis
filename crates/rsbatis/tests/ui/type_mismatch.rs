use rsbatis::{where_, Entity};

#[derive(Entity)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    age: i32,
}

fn main() {
    // 列类型是 i32，右值是 &str：类型检查归 rustc，应该报 E0277 而不是运行时出错
    let _ = where_!(User.age >= "18");
}
