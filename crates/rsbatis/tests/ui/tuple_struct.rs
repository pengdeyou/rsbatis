use rsbatis::Entity;

// 元组结构体 / 单元结构体没有列名，派生必须拒绝
#[derive(Entity)]
#[rsbatis(table = "t")]
struct Tuple(i64, String);

fn main() {}
