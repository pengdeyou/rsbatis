
//! 关系过滤：把"有没有子行"直接当 WHERE 条件。
//!
//! SQLAlchemy 是 `User.has(Post)`；这里是 `rel.exists_filter(User::id())`，
//! 产出一个 `Expr` 塞进 `filter`。
//!
//! 运行：`cargo run -p rsbatis --features sqlite --example has_filter`

use rsbatis::{BinOp, Entity, Expr, FromRow, HasMany, SelectStmt, Session, SortOrder, SqliteDriver};

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "author")]
#[allow(dead_code)]
struct Author {
    id: i64,
    name: String,
}

#[derive(Entity, FromRow, Debug)]
#[rsbatis(table = "article")]
#[allow(dead_code)]
struct Article {
    id: i64,
    author_id: i64,
    title: String,
    published: i64,
}

#[derive(FromRow, Debug)]
struct NameRow {
    name: String,
}

fn main() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE author (id INTEGER NOT NULL, name TEXT NOT NULL);
         CREATE TABLE article (id INTEGER NOT NULL, author_id INTEGER NOT NULL,
                               title TEXT NOT NULL, published INTEGER NOT NULL);
         INSERT INTO author (id, name) VALUES (1, '老王'), (2, '小李'), (3, '小张');
         INSERT INTO article (id, author_id, title, published) VALUES
             (10, 1, 'a1', 1), (11, 1, 'a2', 0), (12, 2, 'b1', 1);",
    )
    .unwrap();

    let rel = HasMany::<Article>::new(Article::author_id(), |a| a.author_id);

    // 1) 写过文章的作者
    let stmt = SelectStmt::new("author")
        .column("name")
        .and_filter(rel.exists_filter(Author::id()))
        .order_by(Expr::bare_col("name"), SortOrder::Asc);
    println!("SQL: {}", db.explain(&stmt).text);
    let rows: Vec<NameRow> = db.all(&stmt).unwrap();
    println!("有文章的作者: {:?}\n", rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>());

    // 2) 一篇都没写的作者
    let stmt = SelectStmt::new("author")
        .column("name")
        .and_filter(rel.not_exists_filter(Author::id()))
        .order_by(Expr::bare_col("name"), SortOrder::Asc);
    let rows: Vec<NameRow> = db.all(&stmt).unwrap();
    println!("一篇文章都没有: {:?}\n", rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>());

    // 3) 只算"已发布"：给子查询叠加条件
    let mut sub = rel.exists_sub(Author::id());
    sub = sub.and_filter(Expr::cmp(
        BinOp::Eq,
        Expr::col("article", "published"),
        Expr::lit(1_i64),
    ));
    let stmt = SelectStmt::new("author")
        .column("name")
        .and_filter(Expr::exists(&sub))
        .order_by(Expr::bare_col("name"), SortOrder::Asc);
    println!("SQL: {}", db.explain(&stmt).text);
    let rows: Vec<NameRow> = db.all(&stmt).unwrap();
    println!("有已发布文章的作者: {:?}", rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>());
    println!("（小李那篇 b1 已发布、老王的 a1 已发布，两人都命中）");
}
