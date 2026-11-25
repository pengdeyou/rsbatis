//! 连接池：复用、上限、归还前回滚、池化连接直接交给 Session。

use rsbatis::{Driver, Pool, Session, Sql, SqliteDriver};

#[test]
fn pool_reuses_and_caps() {
    let pool = Pool::new(SqliteDriver::open_in_memory, 2);

    {
        let _c = pool.acquire().unwrap();
    }
    assert_eq!(1, pool.live_count());
    assert_eq!(1, pool.idle_count(), "归还后应进空闲队列");

    let a = pool.acquire().unwrap();
    let b = pool.acquire().unwrap();
    assert_eq!(2, pool.live_count());
    assert_eq!(0, pool.idle_count(), "两个都在借出状态");
    assert!(
        pool.acquire().is_err(),
        "超过上限必须报错，不能静默超发连接"
    );
    drop(a);
    drop(b);
    assert_eq!(2, pool.idle_count());
}

#[test]
fn dangling_transaction_is_rolled_back_on_return() {
    // max_size = 1 且用内存库：第二次借到的必然是同一个连接，才能证明数据没落库
    let pool = Pool::new(SqliteDriver::open_in_memory, 1);
    {
        let mut c = pool.acquire().unwrap();
        c.execute_batch("CREATE TABLE t (id INTEGER)").unwrap();
        c.begin().unwrap();
        c.execute(&Sql::raw("INSERT INTO t VALUES (1)")).unwrap();
        assert!(c.in_transaction());
        // 既不 commit 也不 rollback，直接离开作用域
    }

    let mut c = pool.acquire().unwrap();
    assert!(!c.in_transaction(), "归还前必须先回滚");
    assert_eq!(
        0,
        c.fetch(&Sql::raw("SELECT * FROM t")).unwrap().len(),
        "未提交的插入不应留下痕迹"
    );
}

#[test]
fn session_over_pooled_conn() {
    let pool = Pool::new(SqliteDriver::open_in_memory, 1);
    let c = pool.acquire().unwrap();
    let mut db = Session::new(c);
    db.execute_batch("CREATE TABLE u (id INTEGER, name TEXT)")
        .unwrap();
    db.execute_batch("INSERT INTO u VALUES (1, 'a')").unwrap();

    assert_eq!(1, db.fetch(Sql::raw("SELECT * FROM u")).unwrap().len());

    // 事务在池化连接上同样成立，且 Session 自己也会在归还时清干净
    {
        let mut tx = db.transaction().unwrap();
        tx.execute(Sql::raw("INSERT INTO u VALUES (2, 'b')"))
            .unwrap();
    }
    assert_eq!(1, db.fetch(Sql::raw("SELECT * FROM u")).unwrap().len());
    assert!(!db.in_transaction());
}
