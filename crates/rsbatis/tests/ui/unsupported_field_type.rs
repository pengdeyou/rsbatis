use rsbatis::Entity;

#[derive(Entity)]
#[rsbatis(table = "t")]
struct Bad {
    id: i64,
    // 没有 SQL 列映射的类型必须当场编译失败，而不是落库时才 TypeMismatch
    when: std::time::Duration,
}

fn main() {}
