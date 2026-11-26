
//! 基于关系元数据的 `EXISTS` 过滤（`has()` 的 Rust 形态）。
//!
//! SQLAlchemy 里是 `User.has(Post)` / `~User.has(Post)`；这里描述符直接产出
//! `Expr`，塞进 `where_` 即可。

#![cfg(feature = "sqlite")]

use rsbatis::{
    BinOp, Entity, Expr, FromRow, HasMany, ManyToMany, SelectStmt, Session, SqliteDriver,
    SortOrder,
};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "h_user")]
#[allow(dead_code)]
struct User {
    id: i64,
    name: String,
}

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "h_post")]
#[allow(dead_code)]
struct Post {
    id: i64,
    user_id: i64,
    title: String,
    published: i64,
}

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "h_tag")]
#[allow(dead_code)]
struct Tag {
    id: i64,
    label: String,
}

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "h_post_tag")]
#[allow(dead_code)]
struct PostTag {
    post_id: i64,
    tag_id: i64,
}

fn seeded() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE h_user (id INTEGER NOT NULL, name TEXT NOT NULL);
         CREATE TABLE h_post (id INTEGER NOT NULL, user_id INTEGER NOT NULL,
                              title TEXT NOT NULL, published INTEGER NOT NULL);
         INSERT INTO h_user (id, name) VALUES (1, 'A'), (2, 'B'), (3, 'C');
         -- A 两篇（一篇未发），B 一篇已发，C 没写过
         INSERT INTO h_post (id, user_id, title, published) VALUES
             (10, 1, 'p1', 1), (11, 1, 'p2', 0), (12, 2, 'p3', 1);",
    )
    .unwrap();
    db
}

fn users_where(db: &mut Session<SqliteDriver>, filter: Expr) -> Vec<String> {
    let stmt = SelectStmt::new("h_user")
        .column("name")
        .and_where(filter)
        .order_by(Expr::bare_col("name"), SortOrder::Asc);
    #[derive(FromRow, Debug)]
    struct R {
        name: String,
    }
    let rows: Vec<R> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    rows.into_iter().map(|r| r.name).collect()
}

fn post_user() -> HasMany<Post> {
    HasMany::<Post>::new(Post::user_id(), |p| p.user_id)
}

#[test]
fn exists_filters_parents_with_children() {
    let mut db = seeded();
    let got = users_where(&mut db, post_user().exists_filter(User::id()));
    assert_eq!(vec!["A", "B"], got, "C 没写过文章，应被排除");
}

#[test]
fn not_exists_filters_parents_without_children() {
    let mut db = seeded();
    let got = users_where(&mut db, post_user().not_exists_filter(User::id()));
    assert_eq!(vec!["C"], got);
}

#[test]
fn exists_sql_shape_is_correlated() {
    let db = seeded();
    let stmt = SelectStmt::new("h_user")
        .column("name")
        .and_where(post_user().exists_filter(User::id()))
        .build(db.dialect());
    assert_eq!(
        r#"SELECT "name" FROM "h_user" WHERE EXISTS (SELECT ? AS "__exists" FROM "h_post" WHERE "h_post"."user_id" = "h_user"."id")"#,
        stmt.text
    );
}

#[test]
fn exists_sub_can_be_narrowed() {
    let mut db = seeded();
    // 只看已发布的文章：A 有一篇未发、一篇已发，所以 A 仍在；B 也仍在
    let mut sub = post_user().exists_sub(User::id());
    sub = sub.and_where(Expr::cmp(
        BinOp::Eq,
        Expr::col("h_post", "published"),
        Expr::lit(1_i64),
    ));
    let got = users_where(&mut db, Expr::exists(&sub));
    assert_eq!(vec!["A", "B"], got);

    // 反过来：只有未发布文章的用户 → 只剩 A
    let mut sub = post_user().exists_sub(User::id());
    sub = sub.and_where(Expr::cmp(
        BinOp::Eq,
        Expr::col("h_post", "published"),
        Expr::lit(0_i64),
    ));
    let got = users_where(&mut db, Expr::exists(&sub));
    assert_eq!(vec!["A"], got, "B 的 p3 已发布，不该命中");
}

#[test]
fn exists_with_outer_alias_requires_matching_of_table() {
    let mut db = seeded();
    // 外层用别名 u：父列必须也指向 u，否则渲染出 users 表名而 FROM 是 u
    let stmt = SelectStmt::new("h_user")
        .from_alias("u")
        .column("name")
        .and_where(post_user().exists_filter(User::id().of_table("u")));
    let sql = stmt.build(db.dialect());
    assert!(
        sql.text.contains(r#""h_user"."id""#) || sql.text.contains(r#""u"."id""#),
        "SQL: {}",
        sql.text
    );
    #[derive(FromRow, Debug)]
    #[allow(dead_code)]
    struct R {
        name: String,
    }
    let rows: Vec<R> = db.fetch_all(sql).unwrap();
    assert_eq!(2, rows.len());
}

#[test]
fn many_to_many_has_excludes_orphan_links() {
    let mut db = seeded();
    db.execute_batch(
        "CREATE TABLE h_tag (id INTEGER NOT NULL, label TEXT NOT NULL);
         CREATE TABLE h_post_tag (post_id INTEGER NOT NULL, tag_id INTEGER NOT NULL);
         INSERT INTO h_tag (id, label) VALUES (20, 'db'), (21, 'net');
         -- post 10 挂了两个有效标签；post 11 挂了一个已被删除的标签 99
         INSERT INTO h_post_tag (post_id, tag_id) VALUES (10, 20), (10, 21), (11, 99);",
    )
    .unwrap();

    let rel = ManyToMany::<Tag>::new(
        "h_post_tag",
        PostTag::post_id(),
        PostTag::tag_id(),
        Tag::id(),
        |t| t.id,
    );

    let sql = SelectStmt::new("h_post")
        .column("title")
        .and_where(rel.exists_filter(Post::id()))
        .order_by(Expr::bare_col("title"), SortOrder::Asc)
        .build(db.dialect());
    #[derive(FromRow, Debug)]
    struct R {
        title: String,
    }
    let rows: Vec<R> = db.fetch_all(sql).unwrap();
    assert_eq!(
        vec!["p1"],
        rows.into_iter().map(|r| r.title).collect::<Vec<_>>(),
        "p2 只连着一个已被删除的标签，不该算“有标签”"
    );
}

#[test]
fn many_to_many_not_exists() {
    let mut db = seeded();
    db.execute_batch(
        "CREATE TABLE h_tag (id INTEGER NOT NULL, label TEXT NOT NULL);
         CREATE TABLE h_post_tag (post_id INTEGER NOT NULL, tag_id INTEGER NOT NULL);
         INSERT INTO h_tag (id, label) VALUES (20, 'db'), (21, 'net');
         INSERT INTO h_post_tag (post_id, tag_id) VALUES (10, 20), (10, 21), (11, 99);",
    )
    .unwrap();
    let rel = ManyToMany::<Tag>::new(
        "h_post_tag",
        PostTag::post_id(),
        PostTag::tag_id(),
        Tag::id(),
        |t| t.id,
    );
    let sql = SelectStmt::new("h_post")
        .column("title")
        .and_where(rel.not_exists_filter(Post::id()))
        .order_by(Expr::bare_col("title"), SortOrder::Asc)
        .build(db.dialect());
    #[derive(FromRow, Debug)]
    struct R {
        title: String,
    }
    let rows: Vec<R> = db.fetch_all(sql).unwrap();
    assert_eq!(vec!["p2", "p3"], rows.into_iter().map(|r| r.title).collect::<Vec<_>>());
}

#[test]
fn exists_inside_join_query() {
    let mut db = seeded();
    // EXISTS 与其他条件叠加、并在联表查询里使用
    let stmt = SelectStmt::new("h_user")
        .column("name")
        .join_table(
            rsbatis::JoinKind::Inner,
            "h_post",
            Expr::cmp(
                BinOp::Eq,
                Expr::col("h_post", "user_id"),
                Expr::col("h_user", "id"),
            ),
        )
        .and_where(post_user().exists_filter(User::id()))
        .and_where(Expr::cmp(
            BinOp::Eq,
            Expr::col("h_post", "published"),
            Expr::lit(1_i64),
        ));
    #[derive(FromRow, Debug)]
    struct R {
        name: String,
    }
    let rows: Vec<R> = db.fetch_all(stmt.build(db.dialect())).unwrap();
    let mut names: Vec<String> = rows.into_iter().map(|r| r.name).collect();
    names.sort();
    assert_eq!(vec!["A", "B"], names);
}
