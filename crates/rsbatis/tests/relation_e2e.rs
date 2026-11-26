//! 关系预加载：selectin 与 joined 两种策略，含"查询次数"证明。
#![cfg(feature = "sqlite")]

use rsbatis::{
    BelongsTo, Dialect, Driver, DriverError, Entity, FromRow, HasMany, Session, Sql, SqliteDriver,
};

#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "v_user")]
#[allow(dead_code)]
struct U {
    id: i64,
    name: String,
}

#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "v_post")]
#[allow(dead_code)]
struct Pst {
    id: i64,
    user_id: i64,
    title: String,
}

/// 一对多描述符：子表 `v_post.user_id` 指向父，父键是 `U.id`
fn posts_of() -> HasMany<Pst> {
    HasMany::new(Pst::user_id(), |p: &Pst| p.user_id)
}

/// 多对一描述符：父键是 `U.id`
fn author_of() -> BelongsTo<U> {
    BelongsTo::new(Pst::user_id(), U::id(), |u: &U| u.id)
}

/// 统计 `fetch` 次数的驱动包装。
///
/// 没有它，"selectin 不是 N+1"就只是文档里的说法；有了它，这句话是被断言锁住的。
struct Counting<D> {
    inner: D,
    fetches: usize,
}

impl<D: Driver> Counting<D> {
    fn new(inner: D) -> Self {
        Self { inner, fetches: 0 }
    }
}

impl<D: Driver> Driver for Counting<D> {
    fn execute(&mut self, sql: &Sql) -> Result<usize, DriverError> {
        self.inner.execute(sql)
    }
    fn fetch(&mut self, sql: &Sql) -> Result<Vec<rsbatis::Row>, DriverError> {
        self.fetches += 1;
        self.inner.fetch(sql)
    }
    fn dialect(&self) -> Dialect {
        self.inner.dialect()
    }
    fn begin(&mut self) -> Result<(), DriverError> {
        self.inner.begin()
    }
    fn commit(&mut self) -> Result<(), DriverError> {
        self.inner.commit()
    }
    fn rollback(&mut self) -> Result<(), DriverError> {
        self.inner.rollback()
    }
    fn execute_batch(&mut self, sql: &str) -> Result<(), DriverError> {
        self.inner.execute_batch(sql)
    }
    fn in_transaction(&self) -> bool {
        self.inner.in_transaction()
    }
}

fn setup() -> Session<Counting<SqliteDriver>> {
    let mut db = Session::new(Counting::new(SqliteDriver::open_in_memory().unwrap()));
    db.execute_batch(
        r#"CREATE TABLE "v_user" (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
           CREATE TABLE "v_post" (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, title TEXT NOT NULL);
           INSERT INTO "v_user" VALUES (1, '张三'), (2, '李四'), (3, '王五');
           INSERT INTO "v_post" VALUES
             (11, 1, 'A1'), (12, 1, 'A2'), (13, 3, 'C1'), (14, 99, '孤儿');"#,
    )
    .unwrap();
    db
}

#[test]
fn selectin_loads_all_children_in_one_query() {
    let mut db = setup();
    let users: Vec<U> = db.all(rsbatis::select! { U }).expect("查父");
    let before = db.driver().fetches;

    let posts = db
        .has_many(&posts_of(), &users, |u| u.id)
        .expect("预加载子");

    assert_eq!(
        db.driver().fetches - before,
        1,
        "3 个父也只需要 1 次子查询——这就是 selectin 的全部意义"
    );

    assert_eq!(posts.len(), 3, "与父一一对应");
    assert_eq!(
        posts[0]
            .iter()
            .map(|p| p.title.as_str())
            .collect::<Vec<_>>(),
        vec!["A1", "A2"]
    );
    assert!(posts[1].is_empty(), "李四没有文章，给空 Vec 而不是 None");
    assert_eq!(posts[2].len(), 1);
    assert_eq!("C1", posts[2][0].title);
}

#[test]
fn selectin_skips_query_when_no_parents() {
    let mut db = setup();
    let before = db.driver().fetches;
    let posts = db
        .has_many(&posts_of(), &Vec::<U>::new(), |u| u.id)
        .expect("空父集合");
    assert!(posts.is_empty());
    assert_eq!(db.driver().fetches, before, "空集合不该发查询");
}

#[test]
fn belongs_to_maps_parent_or_none() {
    let mut db = setup();
    let posts: Vec<Pst> = db.all(rsbatis::select! { Pst }).expect("查子");
    let before = db.driver().fetches;

    let authors = db
        .belongs_to(&author_of(), &posts, |p| p.user_id)
        .expect("预加载父");

    assert_eq!(db.driver().fetches - before, 1, "4 个子也只查 1 次父");

    assert_eq!(4, authors.len());
    assert_eq!("张三", authors[0].as_ref().unwrap().name);
    assert_eq!(
        "张三",
        authors[1].as_ref().unwrap().name,
        "两篇文章同一个作者"
    );
    assert_eq!("王五", authors[2].as_ref().unwrap().name);
    assert!(
        authors[3].is_none(),
        "外键指向不存在的父时给 None，不 panic"
    );
}

#[test]
fn joined_load_groups_and_dedups_parent() {
    let mut db = setup();
    // 子表必须带别名，否则两表同名列无法区分
    let stmt = rsbatis::select! {
        U join Pst as p on p.user_id == U.id where U.id == 1
    };
    let pairs: Vec<(U, Vec<Pst>)> = db
        .joined_has_many(stmt, U::TABLE, "p", |u: &U| u.id)
        .expect("joined 预加载");

    assert_eq!(1, pairs.len(), "张三只有一份父实例，即使 JOIN 出了两行");
    assert_eq!("张三", pairs[0].0.name);
    assert_eq!(
        pairs[0]
            .1
            .iter()
            .map(|p| p.title.as_str())
            .collect::<Vec<_>>(),
        vec!["A1", "A2"],
        "子按 JOIN 顺序落入同一个父"
    );
}

#[test]
fn joined_load_keeps_parent_order() {
    let mut db = setup();
    let stmt = rsbatis::select! {
        U join Pst as p on p.user_id == U.id order_by U.id asc
    };
    let pairs: Vec<(U, Vec<Pst>)> = db
        .joined_has_many(stmt, U::TABLE, "p", |u: &U| u.id)
        .expect("joined 预加载");
    // 李四没有文章，JOIN 里根本不出现——这正是 joined 的代价：
    // 没有子的父会从结果里消失，而 selectin 会给空 Vec。
    let ids: Vec<i64> = pairs.iter().map(|(u, _)| u.id).collect();
    assert_eq!(ids, vec![1, 3], "实际：{ids:?}");
}

#[test]
fn duplicate_parents_get_same_children() {
    // 这条来自一个真实 bug：用 `HashMap::remove` 取子集合，同一个父传入两次时
    // 第二次会拿到空 Vec——看起来像"这个用户没有文章"。
    let mut db = setup();
    let users: Vec<U> = db
        .all(rsbatis::select! { U where U.id == 1 })
        .expect("查父");
    let twice = vec![users[0].clone(), users[0].clone()];

    let posts = db
        .has_many(&posts_of(), &twice, |u| u.id)
        .expect("预加载");
    assert_eq!(2, posts[0].len());
    assert_eq!(2, posts[1].len(), "同一个父出现两次，两次都要拿到子");
}
