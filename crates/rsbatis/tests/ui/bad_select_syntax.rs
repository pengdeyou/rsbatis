use rsbatis::{select, Entity};

#[derive(Entity)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
}

fn main() {
    // 第二个 token 既不属于任何段、也不是段关键字：必须当场拒绝，
    // 而不是把它当实体名后面的垃圾静默丢掉
    let _ = select! { User User.id == 1 };
}
