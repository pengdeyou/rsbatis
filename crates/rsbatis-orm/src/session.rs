//! Session 与事务。
//!
//! 这一层的职责边界：
//!
//! - **Session 负责"连接 + 事务状态"**，不负责生成 SQL（那是 core 的事）；
//! - **Driver 负责"一条连接上执行一条语句"**，不负责池化、不负责事务状态机；
//! - **Transaction 负责 RAII**：不显式 `commit()` 就回滚。
//!
//! 为什么事务状态机放 Session 而不是 Driver：驱动可能被换成连接池取出的短连接，
//! "现在是否在事务中"是逻辑状态，不是物理连接属性。

use rsbatis_core::{Dialect, ToSql};
use rsbatis_driver::{Driver, DriverError, ErrorKind, Row};

use crate::FromRow;

/// 一次数据库会话。
///
/// 泛型参数是具体驱动（`SqliteDriver` / `MySqlDriver`），不是 `dyn Driver`：
/// 事务 RAII 需要在 `Drop` 里调 `rollback`，而 `dyn` 会让借用关系更难表达；
/// 单态化之后编译器还能把驱动的方法内联掉。
pub struct Session<D: Driver> {
    driver: D,
    in_tx: bool,
}

impl<D: Driver> Session<D> {
    pub fn new(driver: D) -> Self {
        Self {
            driver,
            in_tx: false,
        }
    }

    pub fn driver(&self) -> &D {
        &self.driver
    }

    pub fn driver_mut(&mut self) -> &mut D {
        &mut self.driver
    }

    /// 取出驱动（用于换库、关闭等）。
    pub fn into_driver(self) -> D {
        self.driver
    }

    pub fn dialect(&self) -> Dialect {
        self.driver.dialect()
    }

    pub fn in_transaction(&self) -> bool {
        self.in_tx
    }

    /// 执行 DML / DDL，返回受影响行数。
    ///
    /// 接受 `impl ToSql`：可以直接传 `select! { .. }` 之类的 builder，也可以传
    /// 已经 `build` 好的 `&Sql`（`Sql` 自己实现了 `ToSql`）。
    pub fn execute(&mut self, stmt: impl ToSql) -> Result<usize, DriverError> {
        let sql = stmt.to_sql(self.dialect());
        self.driver.execute(&sql)
    }

    /// 对象式插入：把模型里所有字段收集成 `INSERT`，执行一条语句。
    ///
    /// 这是最接近 SQLAlchemy `session.add(User(name="张三", age=20))` 的形态：
    /// 构造好模型，交给 Session，不用手写列。
    ///
    /// 两条语义要记住：
    /// - `Option<T>` 字段为 `None` 时**该列不出现**在 `INSERT` 里，交给数据库默认值；
    ///   要写 NULL 用 `insert::<E>().set_null(...)`。
    /// - 列——值来自 `Entity::column_values`，由 `#[derive(Entity)]` 按字段生成，
    ///   所以 `#[rsbatis(column = "...")]` 的重命名同样生效。
    pub fn add<E: rsbatis_core::Entity>(&mut self, model: &E) -> Result<usize, DriverError> {
        let mut cols: Vec<(&'static str, rsbatis_core::Value)> = Vec::new();
        model.column_values(&mut cols);
        if cols.is_empty() {
            return Err(DriverError::new(
                ErrorKind::Other,
                format!(
                    "{} 没有可写的列：模型所有字段都是 None，或者这个 Entity 没有实现 column_values",
                    E::TABLE
                ),
            ));
        }

        let mut stmt = rsbatis_core::insert::<E>();
        for (col, value) in cols {
            stmt = stmt.set_raw(col, value);
        }
        self.execute(stmt.build(self.dialect()))
    }

    /// upsert：按主键插入，冲突时用本次的字段值覆盖其余列。
    ///
    /// 对应 SQLAlchemy 里 `session.merge(obj)` 想达到的效果，但**只发一条语句**
    /// （`merge` 是"先 SELECT 再 INSERT/UPDATE"两步）。要求实体用
    /// `#[rsbatis(primary_key)]` 声明了主键，且**主键列在本次模型里给了值**——
    /// 主键为 `None`（自增新行）时退化成一普通 `add`，因为此时不存在"冲突"。
    ///
    /// 三条语义：
    /// - 主键列不参与 `SET`：拿主键去更新主键是无意义的，也是覆盖别人数据的常见误用；
    /// - `None` 字段不进 `INSERT`，因此也**不会**出现在 `SET` 里——不会把已有值冲成 NULL；
    /// - 除主键外一列都没有时（只有主键值），退化成"冲突则什么都不做"，避免空的 SET 子句。
    pub fn save<E: rsbatis_core::Entity>(&mut self, model: &E) -> Result<usize, DriverError> {
        let pk = E::PRIMARY_KEY;
        if pk.is_empty() {
            return Err(DriverError::new(
                ErrorKind::Unsupported,
                format!(
                    "{} 没有声明主键，无法判断 upsert 的冲突目标：请给字段加 #[rsbatis(primary_key)]，\
                     或改用 add() 做纯插入",
                    E::TABLE
                ),
            ));
        }

        let mut cols: Vec<(&'static str, rsbatis_core::Value)> = Vec::new();
        model.column_values(&mut cols);
        if cols.is_empty() {
            return Err(DriverError::new(
                ErrorKind::Other,
                format!("{} 没有任何可写列，save 无法构造 INSERT", E::TABLE),
            ));
        }

        // 主键是否在本次模型里真的给了值（None 的自增主键不会出现在 cols 里）
        let has_pk_value = cols.iter().any(|(c, _)| pk.contains(c));

        let mut stmt = rsbatis_core::insert::<E>();
        for (col, value) in &cols {
            stmt = stmt.set_raw(col, value.clone());
        }

        if !has_pk_value {
            // 没有主键值 = 新行，没有可冲突的对象，交给数据库生成主键
            return self.execute(stmt.build(self.dialect()));
        }

        let updatable: Vec<&'static str> = cols
            .iter()
            .map(|(c, _)| *c)
            .filter(|c| !pk.contains(c))
            .collect();

        if updatable.is_empty() {
            stmt = stmt.on_conflict_do_nothing();
        } else {
            stmt = stmt.on_conflict_do_update(pk.iter().copied(), updatable);
        }
        self.execute(stmt.build(self.dialect()))
    }

    /// 批量对象式插入：逐条执行，返回累计受影响行数。
    ///
    /// 逐条而不是拼一条多行 `INSERT`：多行插入在值里出现 `DEFAULT` / 自增列时语义会变，
    /// 而这里的语义是"一批模型写进去"。批量优化留给需要它的场景（届时用 `execute_batch`）。
    /// 不在事务里时，中途失败会留下前面已写入的行——需要全有或全无，自己包 `transaction()`。
    pub fn add_all<E: rsbatis_core::Entity>(&mut self, models: &[E]) -> Result<usize, DriverError> {
        let mut total = 0;
        for m in models {
            total += self.add(m)?;
        }
        Ok(total)
    }

    /// 读回所有行。
    pub fn fetch(&mut self, stmt: impl ToSql) -> Result<Vec<Row>, DriverError> {
        let sql = stmt.to_sql(self.dialect());
        self.driver.fetch(&sql)
    }

    /// 读回第一行（没有则 `None`）。注意这不是 LIMIT 1 优化，驱动仍会读完整个结果集。
    pub fn fetch_one(&mut self, stmt: impl ToSql) -> Result<Option<Row>, DriverError> {
        Ok(self.fetch(stmt)?.into_iter().next())
    }

    /// 读回并转成模型。
    pub fn fetch_all<T: FromRow>(&mut self, stmt: impl ToSql) -> Result<Vec<T>, DriverError> {
        let sql = stmt.to_sql(self.dialect());
        self.driver
            .fetch(&sql)?
            .iter()
            .map(T::from_row)
            .collect::<Result<Vec<T>, DriverError>>()
    }

    pub fn fetch_one_as<T: FromRow>(&mut self, stmt: impl ToSql) -> Result<Option<T>, DriverError> {
        let sql = stmt.to_sql(self.dialect());
        match self.driver.fetch(&sql)?.first() {
            Some(row) => Ok(Some(T::from_row(row)?)),
            None => Ok(None),
        }
    }

    /// 执行一批 SQL（建表、迁移脚本）。DDL 用它，不再需要 `driver_mut()`。
    pub fn execute_batch(&mut self, sql: &str) -> Result<(), DriverError> {
        self.driver.execute_batch(sql)
    }

    /// 开一个事务。作用域结束没 `commit` 就自动回滚。
    ///
    /// 不支持嵌套：SQLite 靠 `SAVEPOINT` 能做，MySQL 不能统一语义，
    /// 与其两边行为不一致，不如一律拒绝，让调用方自己决定要不要拆。
    pub fn transaction(&mut self) -> Result<Transaction<'_, D>, DriverError> {
        if self.in_tx {
            return Err(DriverError::new(
                ErrorKind::Sql,
                "不支持嵌套事务：请在外层事务里直接用 `&mut Session` 的 execute/fetch",
            ));
        }
        self.driver.begin()?;
        self.in_tx = true;
        Ok(Transaction {
            session: self,
            finished: false,
        })
    }
}

/// RAII 事务包装。
///
/// **Drop 时会尝试回滚，且回滚失败被吞掉**——这不是偷懒：`Drop` 里 panic 会
/// 在 unwinding 期间造成 double panic 直接 abort。需要确认回滚结果时，请显式调用
/// `rollback()`，它会返回错误。
pub struct Transaction<'a, D: Driver> {
    session: &'a mut Session<D>,
    finished: bool,
}

impl<'a, D: Driver> Transaction<'a, D> {
    pub fn execute(&mut self, stmt: impl ToSql) -> Result<usize, DriverError> {
        self.session.execute(stmt)
    }

    /// 事务内执行多语句脚本（迁移的 DDL 走这里）。
    pub fn execute_batch(&mut self, sql: &str) -> Result<(), DriverError> {
        self.session.execute_batch(sql)
    }

    /// 事务内的对象式插入（`Session::add` 的转发）。
    pub fn add<E: rsbatis_core::Entity>(&mut self, model: &E) -> Result<usize, DriverError> {
        self.session.add(model)
    }

    /// 事务内的批量对象式插入。
    pub fn add_all<E: rsbatis_core::Entity>(&mut self, models: &[E]) -> Result<usize, DriverError> {
        self.session.add_all(models)
    }

    /// 事务内的 upsert。
    pub fn save<E: rsbatis_core::Entity>(&mut self, model: &E) -> Result<usize, DriverError> {
        self.session.save(model)
    }

    pub fn fetch(&mut self, stmt: impl ToSql) -> Result<Vec<Row>, DriverError> {
        self.session.fetch(stmt)
    }

    pub fn fetch_one(&mut self, stmt: impl ToSql) -> Result<Option<Row>, DriverError> {
        self.session.fetch_one(stmt)
    }

    pub fn fetch_all<T: FromRow>(&mut self, stmt: impl ToSql) -> Result<Vec<T>, DriverError> {
        self.session.fetch_all(stmt)
    }

    pub fn fetch_one_as<T: FromRow>(&mut self, stmt: impl ToSql) -> Result<Option<T>, DriverError> {
        self.session.fetch_one_as(stmt)
    }

    pub fn commit(mut self) -> Result<(), DriverError> {
        // 提交失败时不标 finished：让 Drop 再尝试回滚，符合"提交失败即视为未提交"
        self.session.driver.commit()?;
        self.finished = true;
        self.session.in_tx = false;
        Ok(())
    }

    pub fn rollback(mut self) -> Result<(), DriverError> {
        self.session.driver.rollback()?;
        self.finished = true;
        self.session.in_tx = false;
        Ok(())
    }
}

impl<D: Driver> Drop for Transaction<'_, D> {
    fn drop(&mut self) {
        if !self.finished {
            // 故意忽略错误：见结构体文档
            let _ = self.session.driver.rollback();
            self.session.in_tx = false;
        }
    }
}
