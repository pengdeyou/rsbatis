use rsbatis::{where_, Entity};

#[derive(Entity)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    age: i32,
}

fn main() {
    // 列名拼错：宏会把它重写成 `User::col_nmae()`，由 rustc 报 E0599
    let _ = where_!(User.nmae == "x");
}
