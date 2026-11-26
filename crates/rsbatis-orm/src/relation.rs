//! 关系与预加载（M7）。
//!
//! ## 为什么没有"懒加载"
//!
//! Rust 不能在字段访问上插入代码：`user.posts` 是字段，不是方法，没有任何钩子。
//! 所以 SQLAlchemy 那套 `lazy="select"`（访问属性时才查）在这里**不可能透明实现**。
//! 本模块的选择是把它变成显式的、一次性的预加载：
//!
//! ```ignore
//! let users = db.fetch_all(select! { User })?;
//! let posts = db.load_has_many(&Post::user(), &users, |u| u.id)?;   // 一条 IN 查询
//! ```
//!
//! ## 两种策略
//!
//! | 策略 | 查询次数 | 适用 |
//! | --- | --- | --- |
//! | `load_has_many` / `load_belongs_to`（selectin） | 2（父 1 + 子 1） | 绝大多数场景；子集合大时更省 |
//! | `load_joined_has_many`（joined） | 1（JOIN 带回父与子） | 子行较少、想少一次往返 |
//!
//! ## 外键类型
//!
//! MVP 只支持整数外键（`Column<i64>`）。字符串/UUID 外键会让整个描述符多一层泛型，
//! 收益不明显——真正的复合主键场景需要另外一套 API，那是 v0.5 的事。

use std::collections::HashMap;
use std::marker::PhantomData;

use rsbatis_core::{select, BinOp, Column, Entity, Expr, JoinKind, SelectStmt};
use rsbatis_driver::{Driver, DriverError, ErrorKind, Row};

use crate::from_row::FromRow;
use crate::session::Session;

/// 一对多：父持有一组子，子表里有一列指向父。
pub struct HasMany<C> {
    fk: Column<i64>,
    key_of: fn(&C) -> i64,
    _c: PhantomData<C>,
}

impl<C> HasMany<C> {
    /// `fk` 是**子表**里指向父的那一列；`key_of` 从子实例取出该列的值，用于分组。
    pub fn new(fk: Column<i64>, key_of: fn(&C) -> i64) -> Self {
        Self {
            fk,
            key_of,
            _c: PhantomData,
        }
    }

    pub fn fk(&self) -> Column<i64> {
        self.fk
    }

    /// `EXISTS (SELECT 1 FROM 子表 WHERE 子表.fk = parent_col)` —— 直接当筛选条件用。
    ///
    /// ```ignore
    /// // 有文章的用户
    /// let stmt = select::<User>().where_(Post::user().exists_filter(User::id()));
    /// ```
    ///
    /// `parent_col` 必须是**外层**查询里那张父表的列。外层用了别名时记得
    /// `.of_table("u")` —— 否则渲染出 `users.id` 而 `FROM` 是 `u`，SQLite 直接报
    /// "no such column"，MySQL 会把它当成……不，MySQL 也报错，但报的是
    /// "Unknown column 'users.id' in 'where clause'"，找起来一样费劲。
    pub fn exists_filter(&self, parent_col: Column<i64>) -> Expr {
        Expr::exists(&self.exists_sub(parent_col))
    }

    /// `NOT EXISTS (…)`：没有任何子行的父。
    pub fn not_exists_filter(&self, parent_col: Column<i64>) -> Expr {
        Expr::not(self.exists_filter(parent_col))
    }

    /// 子查询本体。公开出来是为了让调用方能叠加条件（比如"有已发布的文章"）。
    pub fn exists_sub(&self, parent_col: Column<i64>) -> SelectStmt {
        // 用 `fk.base_table()` 而不是某个 `C::TABLE`：`HasMany<C>` 不要求 `C: Entity`，
        // 而描述符已经把子表名带在 fk 上了，两者必须一致（`ManyToMany` 那边专门校验过）。
        SelectStmt::new(self.fk.base_table())
            .column_expr(Expr::lit(1_i64).as_("__exists"))
            .and_where(Expr::cmp(BinOp::Eq, self.fk.expr(), parent_col.expr()))
    }
}

/// 多对一：子持有唯一父，子表里有一列指向父。
/// 一对一的描述符。
///
/// 与 `HasMany` 的区别只在返回类型：父侧拿 `Option<C>` 而不是 `Vec<C>`。查询次数据
/// 相同——都是**一条 `IN` 查询**把整批父的子行取回来。SQL 里没法对每个父分别
/// `LIMIT 1`，所以"取第一条"发生在内存里。
///
/// 这条约束必须由使用者保证：**数据本身是唯一的一对一**。如果表里同一个外键值
/// 出现多行，这里返回哪一行取决于数据库的返回顺序，是静默的不确定结果——需要
/// "多条里挑特定一条"的场景请显式用 `select!` 加条件查询，别用这个描述符。
#[derive(Clone, Copy)]
pub struct HasOne<C> {
    fk: Column<i64>,
    key_of: fn(&C) -> i64,
}

impl<C> HasOne<C> {
    pub fn new(fk: Column<i64>, key_of: fn(&C) -> i64) -> Self {
        Self { fk, key_of }
    }

    pub fn fk(&self) -> Column<i64> {
        self.fk
    }
}

pub struct BelongsTo<P> {
    fk: Column<i64>,
    /// 父表上被引用的列（通常是主键）。
    ///
    /// 必须有它：预加载时查的是**父表**，过滤条件得用父表的列。
    /// 只拿子表外键去查父表，渲染出来是 `SELECT * FROM parent WHERE child_fk IN (...)`，
    /// 列名在父表里根本不存在——轻则报错，重则撞上同名列表静默查错。
    parent_col: Column<i64>,
    key_of: fn(&P) -> i64,
    _p: PhantomData<P>,
}

impl<P> BelongsTo<P> {
    pub fn new(fk: Column<i64>, parent_col: Column<i64>, key_of: fn(&P) -> i64) -> Self {
        Self {
            fk,
            parent_col,
            key_of,
            _p: PhantomData,
        }
    }

    /// 子表里指向父的列。
    pub fn fk(&self) -> Column<i64> {
        self.fk
    }

    /// 父表里被引用的列。
    pub fn parent_col(&self) -> Column<i64> {
        self.parent_col
    }
}

/// 多对多：父与子靠一张**关联表**连着，两边表里都没有对方的列。
///
/// 和 `HasMany` 的根本差别：`HasMany` 一条 `IN` 就能把子全取回来（子表里有父的列），
/// 多对多必须**先查关联表拿到 id 对，再查子表** —— 两次查询，无法合并成一条
/// （除了一长串 `JOIN`，那要求把子表每一列都枚举出来做别名，`COLUMNS` 为空时就做不了）。
///
/// ```ignore
/// let rel = ManyToMany::<Course>::new("enroll", Student::id(), Course::id(),
///                                     Course::id(), |c| c.id);
/// let courses = db.load_many_to_many(&rel, &students, |s| s.id)?;
/// ```
pub struct ManyToMany<C> {
    link_table: &'static str,
    /// 关联表里指向**父**的列。
    link_parent: Column<i64>,
    /// 关联表里指向**子**的列。
    link_child: Column<i64>,
    /// **子表**里被引用的列（通常是主键）。
    ///
    /// 必须单独给：查子表时过滤条件要用子表的列，拿 `link_child` 的名字去查子表
    /// 会拼出不存在的列名。这和 `BelongsTo::parent_col` 是同一个理由。
    child_col: Column<i64>,
    key_of: fn(&C) -> i64,
}

impl<C> ManyToMany<C> {
    pub fn new(
        link_table: &'static str,
        link_parent: Column<i64>,
        link_child: Column<i64>,
        child_col: Column<i64>,
        key_of: fn(&C) -> i64,
    ) -> Self {
        Self {
            link_table,
            link_parent,
            link_child,
            child_col,
            key_of,
        }
    }

    pub fn link_table(&self) -> &'static str {
        self.link_table
    }

    pub fn link_parent(&self) -> Column<i64> {
        self.link_parent
    }

    pub fn link_child(&self) -> Column<i64> {
        self.link_child
    }

    pub fn child_col(&self) -> Column<i64> {
        self.child_col
    }

    /// `EXISTS (SELECT 1 FROM 关联表 JOIN 子表 ON … WHERE 关联表.link_parent = parent_col)`
    ///
    /// 为什么不是"只查关联表"：关联表里有记录不等于有子 —— 子被删掉时关联行还在，
    /// 只查关联表会给出"有课程"的假结果。JOIN 一次子表就能排除孤儿关联。
    ///
    /// 代价是多一层 JOIN；收益是这条 EXISTS 与 `load_many_to_many` 的口径一致
    /// （那边也会跳过孤儿关联）。
    pub fn exists_filter(&self, parent_col: Column<i64>) -> Expr {
        let on = Expr::cmp(
            BinOp::Eq,
            self.child_col.expr(),
            self.link_child.expr(),
        );
        let sub = SelectStmt::new(self.link_table)
            .column_expr(Expr::lit(1_i64).as_("__exists"))
            .join_table(JoinKind::Inner, self.child_col.base_table(), on)
            .and_where(Expr::cmp(
                BinOp::Eq,
                self.link_parent.expr(),
                parent_col.expr(),
            ));
        Expr::exists(&sub)
    }

    /// `NOT EXISTS (…)`：没有关联到任何现存子的父。
    pub fn not_exists_filter(&self, parent_col: Column<i64>) -> Expr {
        Expr::not(self.exists_filter(parent_col))
    }
}

/// 去重但保序：结果的分组顺序要与调用方传入的父顺序一致，
/// 用 `HashSet` 去重再排序会打乱这个顺序。
fn unique_keys<I: IntoIterator<Item = i64>>(keys: I) -> Vec<i64> {
    let mut seen = std::collections::HashSet::new();
    keys.into_iter().filter(|k| seen.insert(*k)).collect()
}

impl<D: Driver> Session<D> {
    /// 一对多预加载（selectin）：父已查出来后，用**一条** `IN` 查询把子全取回来。
    ///
    /// 返回的 `Vec` 与 `parents` 一一对应、顺序一致；没有子的父给空 `Vec`
    /// （而不是 `None`——"零个孩子"和"还没查"是两件事，后者不该出现在返回值里）。
    ///
    /// 查询次数与父的数量无关。这是它相对于"循环里逐条查"的全部意义：
    /// 100 个父是 2 次查询，不是 101 次。
    pub fn load_has_many<C, P>(
        &mut self,
        rel: &HasMany<C>,
        parents: &[P],
        parent_key: impl Fn(&P) -> i64,
    ) -> Result<Vec<Vec<C>>, DriverError>
    where
        C: Entity + FromRow + Clone,
    {
        let keys = unique_keys(parents.iter().map(&parent_key));
        if keys.is_empty() {
            // 父集合为空就不要发查询了：空 IN 会被渲染成恒假式，白跑一次往返。
            return Ok(parents.iter().map(|_| Vec::new()).collect());
        }

        let rows: Vec<C> = self.fetch_all(select::<C>().where_(rel.fk.in_(keys.clone())))?;

        let mut buckets: HashMap<i64, Vec<C>> = HashMap::new();
        for row in rows {
            buckets.entry((rel.key_of)(&row)).or_default().push(row);
        }

        // 用 `get` 而不是 `remove`：同一个父被传入两次时，`remove` 会让第二次拿到空集合。
        // 代价是子需要 `Clone`——模型 derive Clone 是常态，这个约束换取"重复输入也正确"。
        Ok(parents
            .iter()
            .map(|p| buckets.get(&parent_key(p)).cloned().unwrap_or_default())
            .collect())
    }

    /// 一对一预加载：与 `load_has_many` 同一条 `IN` 查询，父侧只留一个。
    ///
    /// 找不到子时给 `None`。同一个父在 `parents` 里出现两次时，两边拿到的是
    /// **同一个** `Option<C>` 的克隆，不会出现"第一份有、第二份 None"。
    pub fn load_has_one<C, P>(
        &mut self,
        rel: &HasOne<C>,
        parents: &[P],
        parent_key: impl Fn(&P) -> i64,
    ) -> Result<Vec<Option<C>>, DriverError>
    where
        C: Entity + FromRow + Clone,
    {
        let keys = unique_keys(parents.iter().map(&parent_key));
        if keys.is_empty() {
            return Ok(parents.iter().map(|_| None).collect());
        }

        let rows: Vec<C> = self.fetch_all(select::<C>().where_(rel.fk.in_(keys)))?;

        let mut buckets: HashMap<i64, C> = HashMap::new();
        for row in rows {
            // 先到先得：数据是一对一时只有一行；不是一对一的话，结果是确定的"第一条"，
            // 但哪个"第一条"由数据库决定——文档里写明了，不假装它能挑。
            buckets.entry((rel.key_of)(&row)).or_insert(row);
        }

        Ok(parents
            .iter()
            .map(|p| buckets.get(&parent_key(p)).cloned())
            .collect())
    }

    /// 多对一预加载：子已查出来后，用一条 `IN` 把父取回来。
    ///
    /// 找不到父时给 `None`。这不只是"外键为 NULL"——也包含外键指向一条已被删除的父，
    /// 那属于数据一致性问题，但读取侧不该 panic。
    pub fn load_belongs_to<P, C>(
        &mut self,
        rel: &BelongsTo<P>,
        children: &[C],
        child_key: impl Fn(&C) -> i64,
    ) -> Result<Vec<Option<P>>, DriverError>
    where
        P: Entity + FromRow + Clone,
    {
        let keys = unique_keys(children.iter().map(&child_key));
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let rows: Vec<P> = self.fetch_all(select::<P>().where_(rel.parent_col.in_(keys)))?;
        let mut by_key: HashMap<i64, P> = HashMap::new();
        for row in rows {
            by_key.insert((rel.key_of)(&row), row);
        }

        // 同一个父会被多个子共享，所以这里必须是 `get` + `cloned`：
        // `remove` 会让第二个引用同一个父的子拿到 `None`，看起来像"作者不存在"。
        Ok(children
            .iter()
            .map(|c| by_key.get(&child_key(c)).cloned())
            .collect())
    }

    /// 多对多预加载：`关联表 IN` → `子表 IN`，共 3 次查询（父那 1 次由调用方负责）。
    ///
    /// 返回与 `parents` 一一对应、顺序一致；没连到子的父给空 `Vec`。
    ///
    /// 三种异常都按"读取侧不 panic"处理：
    /// - 父的 key 不在关联表 → 空 `Vec`（正常，就是没关联）
    /// - 关联表指向一条**已被删除**的子 → 该条静默跳过（父少一个子，而不是报错）
    /// - 关联表里同一对出现两次 → 子出现两份，**不去重**
    ///
    /// 最后一条是刻意的：ORM 不该替调用方编造"这算一条还是两条"的结论。需要去重就在
    /// 关联表上加唯一约束，或者调用方自己去重。悄悄去重会掩盖脏数据。
    pub fn load_many_to_many<C, P>(
        &mut self,
        rel: &ManyToMany<C>,
        parents: &[P],
        parent_key: impl Fn(&P) -> i64,
    ) -> Result<Vec<Vec<C>>, DriverError>
    where
        C: Entity + FromRow + Clone,
    {
        // 描述符里三个列名分属两张表，写错不会报错 —— 只会静默查出空集合。
        // 这是我自己第一版就犯过的错：把 `Student::id()`（列名 `id`）当成
        // `link_parent` 传进来，关联表里那列其实叫 `student_id`，渲染出
        // `WHERE "id" IN (…)` 去查一张没有 `id` 列的表。所以这里先校验表名。
        self.check_descriptor(rel)?;

        let keys = unique_keys(parents.iter().map(&parent_key));
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        // 第一步：关联表。只取两列，避免把关联表上的其他负载字段也搬回来。
        let link_stmt = SelectStmt::new(rel.link_table)
            .column(rel.link_parent.name())
            .column(rel.link_child.name())
            .and_where(rel.link_parent.in_(keys));
        let link_rows: Vec<Row> = self.fetch(link_stmt.build(self.dialect()))?;

        // 关联表一条都没命中时立刻返回：省掉第二次往返。
        if link_rows.is_empty() {
            return Ok(parents.iter().map(|_| Vec::new()).collect());
        }

        let mut links: Vec<(i64, i64)> = Vec::with_capacity(link_rows.len());
        for row in &link_rows {
            let p = row.get::<i64>(0)?;
            let c = row.get::<i64>(1)?;
            links.push((p, c));
        }

        // 第二步：子表。用子表自己的列做 `IN`。
        let child_keys = unique_keys(links.iter().map(|(_, c)| *c));
        let children: Vec<C> = self.fetch_all(select::<C>().where_(rel.child_col.in_(child_keys)))?;

        let mut by_id: HashMap<i64, C> = HashMap::new();
        for row in children {
            by_id.insert((rel.key_of)(&row), row);
        }

        // 第三步：组装。按关联表的顺序塞进各自的桶；指向已删除子的条目跳过。
        let mut buckets: HashMap<i64, Vec<C>> = HashMap::new();
        for (p, c) in &links {
            if let Some(object) = by_id.get(c) {
                buckets.entry(*p).or_default().push(object.clone());
            }
        }

        Ok(parents
            .iter()
            .map(|p| buckets.get(&parent_key(p)).cloned().unwrap_or_default())
            .collect())
    }

    /// 校验多对多描述符的列归属：`link_parent` / `link_child` 必须是关联表的列，
    /// `child_col` 必须是子表的列。
    ///
    /// 不做这道校验的话，写错误报的是"查到 0 行"，而不是"你的描述符写错了" ——
    /// 前者会让人去查数据，后者才指得出问题在哪。
    fn check_descriptor<C>(&self, rel: &ManyToMany<C>) -> Result<(), DriverError>
    where
        C: Entity,
    {
        for (col, expected, label) in [
            (rel.link_parent, rel.link_table, "link_parent"),
            (rel.link_child, rel.link_table, "link_child"),
        ] {
            if col.base_table() != expected {
                return Err(DriverError::new(
                    ErrorKind::Other,
                    format!(
                        "ManyToMany 的 `{label}` 应该是关联表 `{expected}` 的列，\
                         实际给的是 `{}`.`{}`",
                        col.base_table(),
                        col.name()
                    ),
                ));
            }
        }
        if rel.child_col.base_table() != C::TABLE {
            return Err(DriverError::new(
                ErrorKind::Other,
                format!(
                    "ManyToMany 的 `child_col` 应该是子表 `{}` 的列，实际给的是 `{}`.`{}`",
                    C::TABLE,
                    rel.child_col.base_table(),
                    rel.child_col.name()
                ),
            ));
        }
        Ok(())
    }

    /// 一对多预加载（joined）：一次 JOIN 把父与子一起取回，然后按父分组。
    ///
    /// `stmt` 需要已经是"父 JOIN 子"的形状，并且子表**必须带别名**
    /// （否则两表同名列无法区分，同一张表连两次更是直接渲染成两张同名表）。
    /// 分组时父按 `parent_prefix` 取值，子按 `child_prefix` 取值。
    ///
    /// 同一父的多行只保留一份父实例：JOIN 会让父行重复出现，
    /// 直接 `collect` 会得到 N 份父的副本，调用方还得自己去重。
    pub fn load_joined_has_many<P, C>(
        &mut self,
        stmt: SelectStmt,
        parent_prefix: &'static str,
        child_prefix: &'static str,
        parent_key: impl Fn(&P) -> i64,
    ) -> Result<Vec<(P, Vec<C>)>, DriverError>
    where
        P: FromRow,
        C: FromRow,
    {
        let rows: Vec<Row> = self.fetch(stmt)?;

        let mut order: Vec<i64> = Vec::new();
        let mut groups: HashMap<i64, (P, Vec<C>)> = HashMap::new();
        for row in &rows {
            let p: P = P::from_row_prefix(row, parent_prefix)?;
            let c: C = C::from_row_prefix(row, child_prefix)?;
            let k = parent_key(&p);
            match groups.get_mut(&k) {
                Some((_, children)) => children.push(c),
                None => {
                    order.push(k);
                    groups.insert(k, (p, vec![c]));
                }
            }
        }

        Ok(order
            .into_iter()
            .filter_map(|k| groups.remove(&k))
            .collect())
    }
}
