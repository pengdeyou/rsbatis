//! 分页端到端（SQLite 内存库）：总数、页边界、越界、参数校验、带条件与去重。
//!
//! 这里用一个计数驱动包装，把"越界页不发第二次查询"也断言下来——
//! 否则这句话就只是文档里的说法。

#![cfg(feature = "sqlite")]
#![allow(dead_code)]

use rsbatis::{
    insert, select, Dialect, Driver, DriverError, Entity, FromRow, PageRequest, Session, Sql,
    SqliteDriver,
};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "p_users")]
struct User {
    id: i64,
    name: String,
    age: i32,
}

/// 包一层，数 `fetch` 被调用了几次。
struct Counting<D> {
    inner: D,
    fetches: usize,
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
    fn supports_returning(&self) -> bool {
        self.inner.supports_returning()
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

fn seeded() -> Session<Counting<SqliteDriver>> {
    let driver = Counting {
        inner: SqliteDriver::open_in_memory().expect("内存库"),
        fetches: 0,
    };
    let mut db = Session::new(driver);
    db.execute_batch(
        r#"CREATE TABLE "p_users" (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER NOT NULL);"#,
    )
    .expect("建表");

    // 10 行，年龄 20..=29；其中 "张三" 出现 3 次（用于 distinct 分页）
    for i in 0..10 {
        let name = if i < 3 {
            "张三".to_string()
        } else {
            format!("用户{i}")
        };
        db.execute(
            insert::<User>()
                .set(User::name(), name)
                .set(User::age(), 20 + i)
                .build(Dialect::Sqlite),
        )
        .expect("插入");
    }
    db
}

#[test]
fn first_page_reports_total_and_pages() {
    let mut db = seeded();
    let page = db
        .paginate::<User>(&select::<User>(), PageRequest::new(1, 3))
        .expect("第一页");
    assert_eq!(page.total, 10);
    assert_eq!(page.items.len(), 3);
    assert_eq!(page.pages(), 4);
    assert!(page.has_next());
    assert_eq!(page.first_index(), Some(1));
}

#[test]
fn last_page_has_remainder_and_no_next() {
    let mut db = seeded();
    let page = db
        .paginate::<User>(&select::<User>(), PageRequest::new(4, 3))
        .expect("末页");
    assert_eq!(page.items.len(), 1);
    assert!(!page.has_next());
    assert_eq!(page.first_index(), Some(10));
}

#[test]
fn page_beyond_end_returns_empty_without_second_query() {
    let mut db = seeded();
    // 第一页：count + 取页 = 2 次 fetch
    db.paginate::<User>(&select::<User>(), PageRequest::new(1, 3))
        .expect("第一页");
    assert_eq!(db.driver().fetches, 2);

    // 越界页：只发 count 一次
    let page = db
        .paginate::<User>(&select::<User>(), PageRequest::new(5, 3))
        .expect("越界页");
    assert!(page.is_empty());
    assert_eq!(page.total, 10);
    assert!(!page.has_next());
    assert_eq!(db.driver().fetches, 3, "越界页不应该再发取页查询");
}

#[test]
fn invalid_page_request_is_an_error_not_a_silent_fix() {
    let mut db = seeded();
    let page0 = db.paginate::<User>(&select::<User>(), PageRequest::new(0, 3));
    assert!(page0.is_err(), "page = 0 应当报错而不是悄悄当成第一页");

    let size0 = db.paginate::<User>(&select::<User>(), PageRequest::new(1, 0));
    assert!(size0.is_err(), "size = 0 应当报错");
}

#[test]
fn filtered_count_only_counts_matching_rows() {
    let mut db = seeded();
    let page = db
        .paginate::<User>(
            &select! { User where User.age >= 25 },
            PageRequest::new(1, 2),
        )
        .expect("带条件分页");
    assert_eq!(page.total, 5);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.pages(), 3);
    // 条件必须进 count，否则 total 会是 10
    assert!(page.items.iter().all(|u| u.age >= 25));
}

#[test]
fn ordering_is_respected_across_pages() {
    let mut db = seeded();
    let mut seen: Vec<i32> = Vec::new();
    for p in 1..=4 {
        let page = db
            .paginate::<User>(
                &select! { User order_by User.age desc },
                PageRequest::new(p, 3),
            )
            .expect("按年龄降序分页");
        seen.extend(page.items.iter().map(|u| u.age));
    }
    // 10 行按降序：29..=20
    assert_eq!(seen, (20..=29).rev().collect::<Vec<i32>>());
}

#[test]
fn distinct_pagination_counts_distinct_rows() {
    let mut db = seeded();
    // 注意：`DISTINCT *` 去重的是整行，而 id 每行都不同，所以那等于不去重。
    // 要观察"去重后变少"，必须投影到真有重复的列上（这里只投影 name）。
    let stmt = select::<User>().column("name").distinct();
    let page = db
        .paginate::<rsbatis::Row>(&stmt, PageRequest::new(1, 3))
        .expect("去重分页");
    // 10 行里 "张三" 出现 3 次，其余 7 个名字各不相同 → 去重后 8 个
    assert_eq!(page.total, 8, "count 必须走子查询才能数出去重后的行数");
    assert_eq!(page.items.len(), 3);
    assert_eq!(page.pages(), 3);
}
