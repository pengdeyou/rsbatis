//! 关系预加载的可编译示例（对应 README 的 "Relations" 一节）。
//!
//! 这里刻意不写"懒加载"：Rust 无法拦截字段访问，`user.posts` 不可能变成一次查询。
//! 能保证的是"不产生 N+1"——所以 selectin 的查询次数是被测试断言过的，不是口头承诺。

// 示例里的实体字段只用于声明「列」，不会被业务代码读；派生宏读字段名，clippy 看不到「读」。
#![allow(dead_code)]

use rsbatis::{BelongsTo, Driver, Entity, FromRow, HasMany, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    name: String,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "posts")]
struct Post {
    id: i64,
    user_id: i64,
    title: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Session::new(SqliteDriver::open_in_memory()?);
    db.driver_mut().execute_batch(
        r#"
        CREATE TABLE "users" (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
        CREATE TABLE "posts" (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, title TEXT NOT NULL);
        INSERT INTO "users" (id, name) VALUES (1, '张三'), (2, '李四'), (3, '王五');
        INSERT INTO "posts" (id, user_id, title) VALUES (1, 1, 'A'), (2, 1, 'B'), (3, 2, 'C');
        "#,
    )?;

    let users: Vec<User> = db.fetch_all(&rsbatis::select! { User order_by User.id asc })?;

    // 一对多：无论父有多少个，子表只查一次（IN 查询）
    let rel = HasMany::new(Post::col_user_id(), |p: &Post| p.user_id);
    let posts: Vec<Vec<Post>> = db.load_has_many(&rel, &users, |u| u.id)?;
    assert_eq!(posts.len(), 3);
    assert_eq!(posts[0].len(), 2); // 张三
    assert_eq!(posts[2].len(), 0); // 王五没有文章，给空 Vec 而不是报错

    // 多对一：缺失的父给 None（而不是丢弃这个子，也不是 panic）
    let all_posts: Vec<Post> = db.fetch_all(&rsbatis::select! { Post })?;
    let rel = BelongsTo::new(Post::col_user_id(), User::col_id(), |u: &User| u.id);
    let authors: Vec<Option<User>> = db.load_belongs_to(&rel, &all_posts, |p| p.user_id)?;
    assert_eq!(authors.iter().filter(|a| a.is_some()).count(), 3);

    // 同一个父被多个子引用时必须都拿到它——这条曾经因为用了 remove 而让第二个子拿到 None
    assert!(authors[0].as_ref().is_some_and(|u| u.name == "张三"));
    assert!(authors[1].as_ref().is_some_and(|u| u.name == "张三"));

    println!("OK");
    Ok(())
}
