//! 关系预加载在真实 MySQL/MariaDB 上：selectin 与 joined 都要跑通。
#![cfg(feature = "mysql")]

use rsbatis::{select, BelongsTo, Entity, FromRow, HasMany, MySqlDriver, Session};

#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "v_rel_user")]
#[allow(dead_code)]
struct RU {
    id: i64,
    name: String,
}

#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "v_rel_post")]
#[allow(dead_code)]
struct RP {
    id: i64,
    user_id: i64,
    title: String,
}

fn posts_of() -> HasMany<RP> {
    HasMany::new(RP::col_user_id(), |p: &RP| p.user_id)
}

fn author_of() -> BelongsTo<RU> {
    BelongsTo::new(RP::col_user_id(), RU::col_id(), |u: &RU| u.id)
}

fn db() -> Option<Session<MySqlDriver>> {
    let url = std::env::var("RSBATIS_MYSQL_URL").ok()?;
    let mut s = Session::new(MySqlDriver::connect(&url).expect("连接"));
    s.execute_batch(
        "DROP TABLE IF EXISTS v_rel_post; DROP TABLE IF EXISTS v_rel_user;
         CREATE TABLE v_rel_user (id BIGINT PRIMARY KEY, name VARCHAR(32) NOT NULL) ENGINE=InnoDB;
         CREATE TABLE v_rel_post (id BIGINT PRIMARY KEY, user_id BIGINT NOT NULL, title VARCHAR(32) NOT NULL) ENGINE=InnoDB;
         INSERT INTO v_rel_user VALUES (1, '张三'), (2, '李四');
         INSERT INTO v_rel_post VALUES (11, 1, 'A1'), (12, 1, 'A2');",
    )
    .expect("建表");
    Some(s)
}

#[test]
fn mysql_selectin_and_belongs_to() {
    let Some(mut db) = db() else {
        eprintln!("跳过：未设置 RSBATIS_MYSQL_URL");
        return;
    };
    let users: Vec<RU> = db.fetch_all(select! { RU }).expect("查父");
    let posts = db.load_has_many(&posts_of(), &users, |u| u.id).expect("子");
    assert_eq!(2, posts[0].len());
    assert!(posts[1].is_empty(), "李四没有文章");

    let all: Vec<RP> = db.fetch_all(select! { RP }).expect("查子");
    let authors = db
        .load_belongs_to(&author_of(), &all, |p| p.user_id)
        .expect("父");
    assert_eq!("张三", authors[0].as_ref().expect("作者").name);
    assert_eq!("张三", authors[1].as_ref().expect("同一作者的第二篇").name);
}

#[test]
fn mysql_joined_load() {
    let Some(mut db) = db() else {
        eprintln!("跳过：未设置 RSBATIS_MYSQL_URL");
        return;
    };
    let stmt = select! { RU join RP as p on p.user_id == RU.id order_by RU.id asc };
    let pairs: Vec<(RU, Vec<RP>)> = db
        .load_joined_has_many(stmt, RU::TABLE, "p", |u: &RU| u.id)
        .expect("joined");
    assert_eq!(1, pairs.len());
    assert_eq!("张三", pairs[0].0.name);
    assert_eq!(
        pairs[0]
            .1
            .iter()
            .map(|p| p.title.as_str())
            .collect::<Vec<_>>(),
        vec!["A1", "A2"]
    );
}
