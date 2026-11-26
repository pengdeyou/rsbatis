use rsbatis::{filter, Entity};

#[derive(Entity)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    age: i32,
}

fn main() {
    // 近似拼写：rustc 的 E0599 在名字足够接近时会主动给出候选，这条用来量化"候选覆盖率"
    let _ = filter!(User.ag == 1);
}
