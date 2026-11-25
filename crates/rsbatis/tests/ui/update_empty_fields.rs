//! `update!` 没给任何字段：宏必须自己报错，而不是生成一条 `SET` 为空的 SQL。
use rsbatis::{update, Entity};

#[derive(Entity)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
}

fn main() {
    let _ = update!(User {});
}
