//! 字段名写错：应指向 `col_xxx` 不存在，而不是运行时报"列不存在"。
use rsbatis::{update, Entity};

#[derive(Entity)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    age: i32,
}

fn main() {
    let _ = update!(User { aeg: 1 } where User.id == 1);
}
