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

/// 主键值为 0 时换成 `Entity::next_id()` 生成的值。
///
/// 只在 0 上动手，因为 0 不是合法的雪花 id（时间戳部分不为 0）。其余值原样保留——
/// 拿到一个真实主键就当它已经是最终值，不"纠正"用户的数据。
fn fill_generated_pk<E: rsbatis_core::Entity>(cols: &mut [(&'static str, rsbatis_core::Value)]) {
    let Some(pk) = E::PRIMARY_KEY.first() else {
        return;
    };
    let Some(id) = E::next_id() else {
        return;
    };
    for (name, value) in cols.iter_mut() {
        if *name == *pk && matches!(value, rsbatis_core::Value::I64(0)) {
            *value = rsbatis_core::Value::I64(id);
        }
    }
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

    /// 渲染成 SQL 文本，不消耗语句、不执行。
    ///
    /// 想打印/记录/断言生成的 SQL 时用它；`build(Dialect)` 是同一件事的显式版本，
    /// 差别只在方言从哪来——这里取的是初始化时驱动定下的那个。
    pub fn explain(&self, stmt: &impl ToSql) -> rsbatis_core::Sql {
        stmt.to_sql(self.dialect())
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
        fill_generated_pk::<E>(&mut cols);
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

    /// 插入一条并取回它的主键值。
    ///
    /// 两条路径，优先前者：
    /// 1. 实体用 `#[rsbatis(primary_key)]` 声明了主键 **且**方言支持 `RETURNING`
    ///    → `INSERT … RETURNING <pk>`，拿回来的是**业务主键本身**，最准确；
    /// 2. 否则 `add` + `Driver::last_insert_id()`，拿的是引擎的自增值。
    ///
    /// 第 2 条路带一个检测：插入前后 `last_insert_id` 必须**变化**，否则报错。这不只是
    /// 防御性代码 —— SQLite 的 `last_insert_rowid()` 是**内部 rowid**，与
    /// `TEXT PRIMARY KEY` 这类业务主键毫无关系；MySQL 在没有自增列的表上插入后
    /// 该值根本不变（返回上一次的）。两种情况都会静默给出一个"看起来像 id"的数。
    pub fn insert<E: rsbatis_core::Entity>(&mut self, model: &E) -> Result<i64, DriverError> {
        let mut cols: Vec<(&'static str, rsbatis_core::Value)> = Vec::new();
        model.column_values(&mut cols);
        fill_generated_pk::<E>(&mut cols);
        if cols.is_empty() {
            return Err(DriverError::new(
                ErrorKind::Other,
                format!("{} 没有可写的列", E::TABLE),
            ));
        }

        // 路径 1：RETURNING（SQLite 3.35+ / MariaDB）。只有声明了主键才有列可返回。
        if let Some(pk) = E::PRIMARY_KEY.first().copied() {
            if self.driver.supports_returning() {
                let mut stmt = rsbatis_core::insert::<E>();
                for (col, value) in cols {
                    stmt = stmt.set_raw(col, value);
                }
                let sql = stmt.returning([pk]).build(self.dialect());
                return match self.first(sql)? {
                    Some(row) => row.get::<i64>(0),
                    None => Err(DriverError::new(
                        ErrorKind::Other,
                        format!("INSERT … RETURNING 没有返回行（{}）", E::TABLE),
                    )),
                };
            }
        }

        // 调用方显式给了主键：插进去之后原样返回，不必去碰自增计数器。
        // 这条必须放在检测之前 —— MariaDB 没有 `RETURNING`，若显式插入后仍去读
        // `LAST_INSERT_ID()`，它根本不会变化（自增没被触发），会被"检测"误判成失败。
        if let Some(pk) = E::PRIMARY_KEY.first().copied() {
            if let Some((_, rsbatis_core::Value::I64(v))) =
                cols.iter().find(|(col, _)| *col == pk)
            {
                let v = *v;
                self.add(model)?;
                return Ok(v);
            }
        }

        // 路径 2：只剩"没有 RETURNING 可用"的情况（当前是 MySQL）。
        //
        // SQLite 在这条路上是不可信的：`last_insert_rowid()` 返回内部 rowid，与
        // `TEXT PRIMARY KEY` 之类毫无关系，而且会被子表插入覆盖。所以 SQLite 上要求
        // 声明主键走 RETURNING，没声明就直接拒绝 —— 比给一个"看起来像 id"的错数值好。
        if self.dialect() == rsbatis_core::Dialect::Sqlite {
            return Err(DriverError::new(
                ErrorKind::Unsupported,
                format!(
                    "{} 没有声明主键，SQLite 上无法可靠回填自增 id：请给主键字段加 \
                     #[rsbatis(primary_key)]，insert 会走 INSERT … RETURNING 拿回真实主键",
                    E::TABLE
                ),
            ));
        }

        // 插入前把会话的 LAST_INSERT_ID 归零，否则读到的可能是**别的表**（比如刚插完的子表）
        // 的值，再一比对"没变化"就把一次合法插入误判成失败。
        self.driver.reset_last_insert_id()?;
        self.add(model)?;
        let after = self.driver.last_insert_id()?;
        if after == 0 {
            return Err(DriverError::new(
                ErrorKind::Unsupported,
                format!(
                    "插入 {} 后没有拿到自增值（LAST_INSERT_ID 为 0）：这张表可能没有 \
                     自增主键。请给主键字段加 #[rsbatis(primary_key)]，或改用自己生成主键 \
                     再 add()",
                    E::TABLE
                ),
            ));
        }
        Ok(after)
    }

    /// 父 + 子一起插：插父 → 取回 id 回填到父 → 用这个 id 造子 → 批量插子。
    ///
    /// 这是 SQLAlchemy `cascade="save-update"` 的显式版本。透明版做不到：
    /// `add` 是泛型函数，无法在"父模型插入时"发现它挂着哪些子 —— Rust 没有运行时的
    /// 关系元数据，`#[derive(Entity)]` 也没法凭空生成"我的子集合"。
    ///
    /// ```ignore
    /// db.insert_tree(
    ///     &mut users,                       // 逐个插入，id 回填到每个 user
    ///     |u, id| u.id = id,                // 回填方式由调用方决定（字段名它知道）
    ///     |u| u.posts.iter().map(|p| Post { user_id: u.id, ..p.clone() }).collect(),
    /// )
    /// ```
    ///
    /// **整批在一个事务里**：任何一条失败（父、子都一样）整批回滚，不会留下"父已入库、
    /// 子没跟上"的半截数据。
    ///
    /// 返回 `(插入的父数, 插入的子数)`。
    pub fn insert_tree<P, C>(
        &mut self,
        parents: &mut [P],
        set_id: impl Fn(&mut P, i64),
        mut children_of: impl FnMut(&P) -> Vec<C>,
    ) -> Result<(usize, usize), DriverError>
    where
        P: rsbatis_core::Entity,
        C: rsbatis_core::Entity,
    {
        let mut tx = self.transaction()?;
        let mut parent_count = 0usize;
        let mut child_count = 0usize;
        for p in parents.iter_mut() {
            let id = tx.insert(&*p)?;
            set_id(p, id);
            parent_count += 1;
            // 必须在 set_id **之后**取子：闭包里要用刚回填的 id
            let children = children_of(&*p);
            if !children.is_empty() {
                child_count += children.len();
                tx.add_all(&children)?;
            }
        }
        tx.commit()?;
        Ok((parent_count, child_count))
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
        fill_generated_pk::<E>(&mut cols);
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
    pub fn first(&mut self, stmt: impl ToSql) -> Result<Option<Row>, DriverError> {
        Ok(self.fetch(stmt)?.into_iter().next())
    }

    /// 读回并转成模型。
    pub fn all<T: FromRow>(&mut self, stmt: impl ToSql) -> Result<Vec<T>, DriverError> {
        let sql = stmt.to_sql(self.dialect());
        self.driver
            .fetch(&sql)?
            .iter()
            .map(T::from_row)
            .collect::<Result<Vec<T>, DriverError>>()
    }

    pub fn one<T: FromRow>(&mut self, stmt: impl ToSql) -> Result<Option<T>, DriverError> {
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
    /// 事务内的 `explain`（只会渲染，不参与事务）。
    pub fn explain(&self, stmt: &impl ToSql) -> rsbatis_core::Sql {
        self.session.explain(stmt)
    }

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
    /// 事务内插入并取回主键（转发到 `Session::insert`，同一个连接）。
    pub fn insert<E: rsbatis_core::Entity>(&mut self, model: &E) -> Result<i64, DriverError> {
        self.session.insert(model)
    }

    pub fn save<E: rsbatis_core::Entity>(&mut self, model: &E) -> Result<usize, DriverError> {
        self.session.save(model)
    }

    pub fn fetch(&mut self, stmt: impl ToSql) -> Result<Vec<Row>, DriverError> {
        self.session.fetch(stmt)
    }

    pub fn first(&mut self, stmt: impl ToSql) -> Result<Option<Row>, DriverError> {
        self.session.first(stmt)
    }

    pub fn all<T: FromRow>(&mut self, stmt: impl ToSql) -> Result<Vec<T>, DriverError> {
        self.session.all(stmt)
    }

    pub fn one<T: FromRow>(&mut self, stmt: impl ToSql) -> Result<Option<T>, DriverError> {
        self.session.one(stmt)
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
