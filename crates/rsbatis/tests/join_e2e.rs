//! 联表端到端：宏语法、元组映射、LEFT JOIN 的 NULL 处理、自连接拒绝。
#![cfg(feature = "sqlite")]
// 测试里的实体字段只用来声明「列」，不会被业务代码读取；
// 派生宏拿的是字段名，clippy 看不到「读」，因此在这里关掉 dead_code。
#![allow(dead_code)]

use rsbatis::{select, Dialect, Entity, ErrorKind, FromRow, JoinKind, Session, SqliteDriver};

#[derive(Entity, FromRow)]
#[rsbatis(table = "users")]
struct User {
    id: i64,
    name: String,
}

#[derive(Entity, FromRow)]
#[rsbatis(table = "orders")]
struct Order {
    id: i64,
    user_id: i64,
    amount: i64,
}

/// LEFT JOIN 的右表列可能为 NULL，模型必须用 Option 才接得住
#[derive(Entity, FromRow)]
#[rsbatis(table = "orders")]
struct OrderOpt {
    id: Option<i64>,
    user_id: Option<i64>,
    amount: Option<i64>,
}

fn setup() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        r#"CREATE TABLE "users" (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
           CREATE TABLE "orders" (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount INTEGER NOT NULL);
           INSERT INTO "users" VALUES (1, '张三'), (2, '李四');
           INSERT INTO "orders" VALUES (11, 1, 150), (12, 1, 80), (13, 1, 200);"#,
    )
    .unwrap();
    db
}

#[test]
fn inner_join_with_macro_syntax() {
    let mut db = setup();
    let rows: Vec<(User, Order)> = db
        .fetch_all(select! {
            User join Order on Order.user_id == User.id
            where Order.amount >= 100
            order_by Order.id asc
        })
        .expect("inner join");

    assert_eq!(2, rows.len());
    assert_eq!("张三", rows[0].0.name);
    assert_eq!(11, rows[0].1.id);
    assert_eq!(150, rows[0].1.amount);
    assert_eq!(200, rows[1].1.amount);
}

#[test]
fn left_join_yields_none_for_missing_side() {
    let mut db = setup();
    let sql =
        select! { User left_join OrderOpt on OrderOpt.user_id == User.id order_by User.id asc }
            .build(Dialect::Sqlite);
    assert!(sql.text.contains("LEFT JOIN"), "{}", sql.text);

    let rows: Vec<(User, OrderOpt)> = db.fetch_all(&sql).expect("left join");
    // 张三有 3 个订单 -> 3 行；李四没有 -> 1 行（右表全 NULL）。共 4 行。
    assert_eq!(4, rows.len(), "一个用户多单会展开成多行，这是 SQL 语义本身");
    assert!(
        rows.iter().any(|(u, o)| u.id == 2 && o.id.is_none()),
        "李四的订单列应为 NULL"
    );
}

#[test]
fn three_table_join() {
    let mut db = setup();
    db.execute_batch(
        r#"CREATE TABLE "items" (id INTEGER PRIMARY KEY, order_id INTEGER NOT NULL, sku TEXT NOT NULL);
           INSERT INTO "items" VALUES (101, 11, 'A'), (102, 11, 'B');"#,
    )
    .unwrap();

    #[derive(Entity, FromRow)]
    #[rsbatis(table = "items")]
    struct Item {
        id: i64,
        order_id: i64,
        sku: String,
    }

    let rows: Vec<(User, Order, Item)> = db
        .fetch_all(select! {
            User join Order on Order.user_id == User.id
            join Item on Item.order_id == Order.id
            where Order.amount == 150
        })
        .unwrap();
    assert_eq!(2, rows.len(), "一笔 150 的订单有两件商品");
    assert_eq!("张三", rows[0].0.name);
    assert_eq!(150, rows[0].1.amount);
    assert_eq!("A", rows[0].2.sku);
}

#[test]
fn self_join_is_rejected_instead_of_returning_wrong_data() {
    let mut db = setup();
    match db.fetch_all::<(User, User)>(select::<User>()) {
        Ok(_) => panic!("自连接必须拒绝，否则两张表的列别名完全重复"),
        Err(e) => {
            assert_eq!(ErrorKind::Unsupported, e.kind);
            assert!(e.message.contains("自连接"), "{}", e.message);
        }
    }
}

#[test]
fn join_condition_can_compare_columns_directly() {
    // builder 路径与宏路径必须产出一致的 SQL
    let macro_sql = select! { User join Order on Order.user_id == User.id }.build(Dialect::Sqlite);
    let builder_sql = select::<User>()
        .join::<Order>(JoinKind::Inner, Order::user_id().eq_col(User::id()))
        .build(Dialect::Sqlite);
    assert_eq!(macro_sql.text, builder_sql.text);
    assert!(macro_sql.params.is_empty(), "列对列比较不产生参数");
}
