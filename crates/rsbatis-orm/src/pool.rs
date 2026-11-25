//! 同步连接池。
//!
//! 为什么自研而不直接上 r2d2 / deadpool：这两个生态都围绕「池里的东西 = 一个数据库连接」
//! 做了固定抽象，而 rsbatis 的 `Driver` 是自己的 trait（SQLite 用 rusqlite、MySQL 用 mysql
//! crate），硬套要为每个驱动再写一个 manager、把错误类型再折一层。
//! MVP 真正需要的只有三件事：复用连接、上限、**归还前保证事务已结束**——
//! 一个 `Mutex<Vec<D>>` 加一次 `in_transaction()` 检查就够了。

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};

use rsbatis_core::{Dialect, Sql};
use rsbatis_driver::{Driver, DriverError, ErrorKind, Row};

type Factory<D> = Arc<dyn Fn() -> Result<D, DriverError> + Send + Sync>;

/// 固定上限的同步连接池。
pub struct Pool<D: Driver + Send> {
    factory: Factory<D>,
    idle: Mutex<Vec<D>>,
    live: Mutex<usize>,
    max_size: usize,
}

impl<D: Driver + Send> Pool<D> {
    /// `max_size` 是「同时存活的连接数上限」（借出 + 空闲），不是空闲队列长度。
    pub fn new<F>(factory: F, max_size: usize) -> Self
    where
        F: Fn() -> Result<D, DriverError> + Send + Sync + 'static,
    {
        Self {
            factory: Arc::new(factory),
            idle: Mutex::new(Vec::new()),
            live: Mutex::new(0),
            max_size: max_size.max(1),
        }
    }

    /// 借一个连接。池空且已达上限时**报错而不是阻塞**：
    /// 同步 API 里阻塞等待很容易变成死锁，MVP 阶段把选择权交回给调用方。
    pub fn acquire(&self) -> Result<PooledConn<'_, D>, DriverError> {
        let reused = self.idle.lock().map_err(|_| poisoned())?.pop();

        let conn = match reused {
            Some(c) => c,
            None => {
                let mut live = self.live.lock().map_err(|_| poisoned())?;
                if *live >= self.max_size {
                    return Err(DriverError::new(
                        ErrorKind::Connection,
                        format!("连接池已满（max_size={}）", self.max_size),
                    ));
                }
                let c = (self.factory)()?;
                *live += 1;
                c
            }
        };

        Ok(PooledConn {
            pool: self,
            conn: Some(conn),
        })
    }

    pub fn idle_count(&self) -> usize {
        self.idle.lock().map(|v| v.len()).unwrap_or(0)
    }

    /// 已创建的连接总数（借出 + 空闲）。
    pub fn live_count(&self) -> usize {
        self.live.lock().map(|v| *v).unwrap_or(0)
    }

    pub fn max_size(&self) -> usize {
        self.max_size
    }
}

/// 借出中的连接。离开作用域自动归还，归还前会把未结束的事务回滚掉。
pub struct PooledConn<'p, D: Driver + Send> {
    pool: &'p Pool<D>,
    conn: Option<D>,
}

impl<D: Driver + Send> PooledConn<'_, D> {
    fn guard(&self) -> &D {
        self.conn.as_ref().expect("PooledConn 已归还，不应再访问")
    }

    fn guard_mut(&mut self) -> &mut D {
        self.conn.as_mut().expect("PooledConn 已归还，不应再访问")
    }
}

impl<D: Driver + Send> Deref for PooledConn<'_, D> {
    type Target = D;
    fn deref(&self) -> &D {
        self.guard()
    }
}

impl<D: Driver + Send> DerefMut for PooledConn<'_, D> {
    fn deref_mut(&mut self) -> &mut D {
        self.guard_mut()
    }
}

impl<D: Driver + Send> Driver for PooledConn<'_, D> {
    fn execute(&mut self, sql: &Sql) -> Result<usize, DriverError> {
        self.guard_mut().execute(sql)
    }

    fn fetch(&mut self, sql: &Sql) -> Result<Vec<Row>, DriverError> {
        self.guard_mut().fetch(sql)
    }

    fn dialect(&self) -> Dialect {
        self.guard().dialect()
    }

    fn supports_returning(&self) -> bool {
        self.guard().supports_returning()
    }

    fn begin(&mut self) -> Result<(), DriverError> {
        self.guard_mut().begin()
    }

    fn commit(&mut self) -> Result<(), DriverError> {
        self.guard_mut().commit()
    }

    fn rollback(&mut self) -> Result<(), DriverError> {
        self.guard_mut().rollback()
    }

    fn execute_batch(&mut self, sql: &str) -> Result<(), DriverError> {
        self.guard_mut().execute_batch(sql)
    }

    fn in_transaction(&self) -> bool {
        self.guard().in_transaction()
    }
}

impl<D: Driver + Send> Drop for PooledConn<'_, D> {
    fn drop(&mut self) {
        let Some(mut conn) = self.conn.take() else {
            return;
        };
        // 归还的连接必须是干净的：带事务回池，下一个借到它的调用方会莫名继承。
        // 这里的回滚失败无处上报（Drop 里 panic 会 abort），只能忽略；
        // 调用方该显式 rollback 才能拿到错误。
        if conn.in_transaction() {
            let _ = conn.rollback();
        }
        if let Ok(mut idle) = self.pool.idle.lock() {
            idle.push(conn);
        }
    }
}

fn poisoned() -> DriverError {
    DriverError::new(
        ErrorKind::Other,
        "连接池内部锁已中毒（另一个线程在持锁时 panic）；这属于内部状态错误",
    )
}
