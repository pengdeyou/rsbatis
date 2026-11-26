//! README "Queries" / "Relations" 两节的唯一可编译对照物。
//!
//! 文档里的每一行代码都在这里跑过一遍——示例写错的代价是读者照抄之后撞编译错误，
//! 而 `cargo build --examples` 会替文档把关。
//!
//! 覆盖：实体定义、按主键查、列表、分页、条件、插入、更新（按主键/按条件）、删除、
//! 一对一、一对多、多对多、INNER JOIN、LEFT JOIN。
// 字段只用于声明列，示例本身不读它们；clippy 看不到「读」。
#![allow(dead_code)]

use rsbatis::{
    delete, insert, select, update, filter, BelongsTo, Dialect, Entity, FromRow, HasMany,
    HasOne, ManyToMany, PageRequest, Session, Snowflake, SqliteDriver,
};

// ---------- 实体定义 ----------

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "events")]
pub struct Event {
    #[rsbatis(primary_key, snowflake)]
    pub id: i64,
    pub name: String,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "users")]
struct User {
    #[rsbatis(primary_key)]
    id: i64,
    name: String,
    age: Option<i32>,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "orders")]
struct Order {
    #[rsbatis(primary_key)]
    id: i64,
    user_id: i64,
    amount: i64,
}

/// LEFT JOIN 的右表模型：右行可能不存在，所有列都必须是 `Option`。
#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "orders")]
struct OrderOpt {
    id: Option<i64>,
    user_id: Option<i64>,
    amount: Option<i64>,
}

/// 自增主键写成 `Option<i64>`：`None` = "交给数据库生成"，`Some(v)` = 显式主键。
/// 写成 `i64` 就只能显式给值——`add`/`insert` 会把 `0` 当成你要插的主键。
#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "logs")]
struct Log {
    #[rsbatis(primary_key)]
    id: Option<i64>,
    msg: String,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "students")]
struct Student {
    #[rsbatis(primary_key)]
    id: i64,
    name: String,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "courses")]
struct Course {
    #[rsbatis(primary_key)]
    id: i64,
    title: String,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "enrollments")]
struct Enrollment {
    #[rsbatis(primary_key)]
    id: i64,
    student_id: i64,
    course_id: i64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Session::new(SqliteDriver::open_in_memory()?);
    db.driver_mut().execute_batch(
        r#"
        CREATE TABLE "users" (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER);
        CREATE TABLE "orders" (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount INTEGER NOT NULL);
        CREATE TABLE "students" (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
        CREATE TABLE "courses" (id INTEGER PRIMARY KEY, title TEXT NOT NULL);
        CREATE TABLE "enrollments" (id INTEGER PRIMARY KEY, student_id INTEGER NOT NULL, course_id INTEGER NOT NULL);
        CREATE TABLE "logs" (id INTEGER PRIMARY KEY AUTOINCREMENT, msg TEXT NOT NULL);
        CREATE TABLE "events" (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
        INSERT INTO "users" VALUES (1,'张三',20),(2,'李四',17),(3,'王五',30);
        INSERT INTO "orders" VALUES (1,1,100),(2,1,300),(3,2,50);
        INSERT INTO "students" VALUES (1,'学生甲'),(2,'学生乙');
        INSERT INTO "courses" VALUES (1,'Rust'),(2,'SQL');
        INSERT INTO "enrollments" VALUES (1,1,1),(2,1,2),(3,2,1);
        "#,
    )?;

    // ---------- 1. 按主键查单条 ----------
    // 返回 `Option<User>`：没有这行就是 None，不是错误
    let one: Option<User> = db.fetch_one_as(&select! { User where User.id == 1 })?;
    assert!(one.is_some());

    // ---------- 2. 查列表 ----------
    let all: Vec<User> = db.fetch_all(&select! { User order_by User.id asc })?;
    assert_eq!(all.len(), 3);

    // ---------- 3. 分页 ----------
    // 先 COUNT(*) 再取当页；页号越界时不再发第二次查询，直接给空 items + 正确的 total
    let stmt = select::<User>().order_by(User::id().expr(), rsbatis::Order::Asc);
    let page: rsbatis::Page<User> = db.paginate(&stmt, PageRequest::new(1, 2))?;
    assert_eq!(page.total, 3);
    assert_eq!(page.items.len(), 2);
    assert!(page.has_next());

    // ---------- 4. 条件查询 ----------
    // `filter!` 只做语法重写，列名与值类型仍由 rustc 检查
    let adults: Vec<User> = db.fetch_all(&select! {
        User where User.age >= 18 && User.name.like("张%")
    })?;
    assert_eq!(adults.len(), 1);

    let builder = select::<User>().filter(filter!(
        User.age.between(18, 30) || User.name.in_(["李四"])
    ));
    assert_eq!(db.fetch_all::<User>(&builder)?.len(), 3);

    // ---------- 5. 插入 ----------
    // 方式 A：对象式（字段值来自模型，主键自增）
    let new_user = User { id: 4, name: "赵六".into(), age: Some(25) };
    db.add(&new_user)?;
    // 需要数据库生成的主键时用 insert（实体主键必须是 Option<i64>）
    let id: i64 = db.insert(&Log { id: None, msg: "started".into() })?;
    assert_eq!(id, 1);

    // 方式 B：语句式（可写 NULL、可加 ON CONFLICT）
    db.execute(
        &insert::<User>()
            .set(User::name(), "孙八")
            .set_null(User::age())
            ,
    )?;

    // ---------- 6. 更新（按主键） ----------
    db.execute(
        &update::<User>()
            .set(User::age(), 21)
            .filter(filter!(User.id == 1))
            ,
    )?;

    // ---------- 7. 更新（按条件，命中多行） ----------
    let affected = db.execute(
        &update::<User>()
            .set(User::age(), 18)
            .filter(filter!(User.age < 18))
            ,
    )?;
    assert!(affected >= 1);

    // ---------- 8. 删除 ----------
    db.execute(&delete::<User>().filter(filter!(User.id == 4)))?;
    // 按条件删除同理：delete::<User>().filter(filter!(User.age < 0))

    // ---------- 9. 一对一 ----------
    // 与一对多同一条 IN 查询，父侧只留一个：返回 Option<C>。
    // 前提是数据本身就唯一——同一外键多行时，"取哪一条"由数据库返回顺序决定。
    let rel = HasOne::new(Order::user_id(), |o: &Order| o.user_id);
    let one_per_user: Vec<Option<Order>> = db.load_has_one(&rel, &all, |u| u.id)?;
    assert!(one_per_user[2].is_none()); // 王五没有订单

    // ---------- 10. 一对多 ----------
    // 无论父有多少个，子表只查一次（IN 查询），不会 N+1
    let rel = HasMany::new(Order::user_id(), |o: &Order| o.user_id);
    let per_user: Vec<Vec<Order>> = db.load_has_many(&rel, &all, |u| u.id)?;
    assert_eq!(per_user[0].len(), 2); // 张三有两单

    // 反向：多对一
    let orders: Vec<Order> = db.fetch_all(&select! { Order })?;
    let rel = BelongsTo::new(Order::user_id(), User::id(), |u: &User| u.id);
    let authors: Vec<Option<User>> = db.load_belongs_to(&rel, &orders, |o| o.user_id)?;
    assert!(authors.iter().all(|a| a.is_some()));

    // ---------- 11. 多对多 ----------
    // 关联表、指向父的列、指向子的列、子表里被引用的列——四个都要给，写错只会静默查空
    let rel = ManyToMany::new(
        "enrollments",
        Enrollment::student_id(),
        Enrollment::course_id(),
        Course::id(),
        |c: &Course| c.id,
    );
    let students: Vec<Student> = db.fetch_all(&select! { Student order_by Student.id asc })?;
    let courses: Vec<Vec<Course>> = db.load_many_to_many(&rel, &students, |s| s.id)?;
    assert_eq!(courses[0].len(), 2);
    assert_eq!(courses[1].len(), 1);

    // ---------- 12. INNER JOIN ----------
    // 有 join 时每列投影成 "u"."id" AS "u__id"，元组按前缀映射回来
    let pairs: Vec<(User, Order)> = db.fetch_all(&select! {
        User as u join Order as o on o.user_id == u.id
        where u.id == 1 order_by o.amount desc
    })?;
    assert_eq!(pairs.len(), 2);

    // ---------- 13. LEFT JOIN ----------
    // 右表可能缺失 → 模型必须写成 Option；LEFT JOIN 本身不去重
    // 只取前三个父行，免得前面插入的行影响行数：2 + 1 条匹配，加王五 1 个空行
    let left: Vec<(User, OrderOpt)> = db.fetch_all(&select! {
        User as u left_join OrderOpt as o on o.user_id == u.id
        where u.id <= 3
    })?;
    assert_eq!(left.len(), 4);

    // ---------- 14. 雪花 id 主键 ----------
    // 实体上声明一次 `snowflake`：插入时看到 id = 0 就换成新生成的值，
    // 返回值就是落库的那个 id。0 不是合法雪花 id，所以"0 = 没给"是安全的。
    let id1 = db.insert(&Event { id: 0, name: "created".into() })?;
    let id2 = db.insert(&Event { id: 0, name: "updated".into() })?;
    assert!(id2 > id1);

    let back: Event = db
        .fetch_one_as(&select! { Event where Event.id == id2 })?
        .expect("刚插入的行应该查得到");
    assert_eq!(back.name, "updated");
    assert_eq!(Snowflake::parse(id2).1, Snowflake::global().node()); // 节点 id 就在 id 里

    println!("all queries OK");
    Ok(())
}
