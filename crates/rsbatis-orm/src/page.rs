//! 分页：`PageRequest` / `Page<T>` 与两次查询的语义。
//!
//! 这里刻意**不**把 count 和取页包成一个事务。理由：分页是读操作，包事务只会让
//! 长事务持有快照、影响写入；而"总数与当页数据来自同一瞬间"这件事，SQLite 的读事务和
//! MySQL 的一致性读都不是靠"包一层"就能顺手得到的允诺。
//!
//! 调用方需要强一致时，自己显式开事务（`session.transaction()`）再调 `paginate`，
//! 由调用方决定隔离级别与代价——而不是由库偷偷决定。

use rsbatis_core::SelectStmt;
use rsbatis_driver::{Driver, DriverError, ErrorKind};

use crate::{FromRow, Session};

/// 分页请求。`page` 从 1 开始（不是 0），`size` 必须大于 0。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRequest {
    pub page: u64,
    pub size: u64,
}

impl PageRequest {
    pub fn new(page: u64, size: u64) -> Self {
        Self { page, size }
    }

    /// 第一页，每页 `size` 条。
    pub fn first(size: u64) -> Self {
        Self { page: 1, size }
    }

    /// 本页对应的 `OFFSET`，同时校验参数。
    ///
    /// `page = 0` 明确当错误处理而不是当成第一页：静默纠正会让"前端传 0"这种 bug
    /// 一直存在，而且用户看到的永远是第一页，没人会发现。
    pub fn offset(&self) -> Result<u64, DriverError> {
        if self.size == 0 {
            return Err(DriverError::new(
                ErrorKind::Other,
                "分页 size 必须大于 0（size = 0 的 count 查询没有意义）",
            ));
        }
        if self.page == 0 {
            return Err(DriverError::new(
                ErrorKind::Other,
                "分页 page 从 1 开始（page = 0 不是合法页号）",
            ));
        }
        self.page
            .checked_sub(1)
            .and_then(|p| p.checked_mul(self.size))
            .ok_or_else(|| {
                DriverError::new(
                    ErrorKind::OutOfRange,
                    "分页 offset 溢出 u64（page × size 太大）",
                )
            })
    }
}

/// 一页数据 + 总数。
#[derive(Clone, Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u64,
    pub size: u64,
}

impl<T> Page<T> {
    /// 总页数（`total = 0` 时为 0，而不是 1）。
    pub fn pages(&self) -> u64 {
        if self.size == 0 {
            0
        } else {
            self.total.div_ceil(self.size)
        }
    }

    pub fn has_next(&self) -> bool {
        self.page < self.pages()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 本页第一条在全表中的序号（从 1 开始）；空页返回 `None`。
    pub fn first_index(&self) -> Option<u64> {
        if self.items.is_empty() {
            None
        } else {
            Some((self.page - 1) * self.size + 1)
        }
    }
}

impl<D: Driver> Session<D> {
    /// 按页取数据：先 `COUNT(*)`，再取当页。
    ///
    /// 两点值得知道：
    /// - `offset >= total`（页号越界）时**不再发第二次查询**，直接给空 `items` 和正确的 `total`。
    ///   前端翻到末页之后时这是常态，多发一次查询纯属浪费。
    /// - `total` 用 `i64` 读回来再转 `u64`；负数不可能出现（`COUNT` 不会返回负），
    ///   但万一是驱动层 bug，这里退化成 0 而不是 panic。
    pub fn paginate<T: FromRow>(
        &mut self,
        stmt: &SelectStmt,
        req: PageRequest,
    ) -> Result<Page<T>, DriverError> {
        let offset = req.offset()?;

        let count_sql = stmt.build_count(self.dialect());
        let rows = self.fetch(count_sql)?;
        let total = match rows.first() {
            Some(r) => u64::try_from(r.get::<i64>(0)?).unwrap_or(0),
            None => 0,
        };

        let items = if offset >= total {
            Vec::new()
        } else {
            let page_stmt = stmt.clone().limit(req.size).offset(offset);
            self.fetch_all::<T>(page_stmt)?
        };

        Ok(Page {
            items,
            total,
            page: req.page,
            size: req.size,
        })
    }
}
