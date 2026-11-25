use rsbatis::{where_, Entity};

#[derive(Entity)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
}

fn main() {
    // 实体名必须首字母大写（宏靠这个区分实体与普通表达式）
    let _ = where_!(user.id == 1);
}
