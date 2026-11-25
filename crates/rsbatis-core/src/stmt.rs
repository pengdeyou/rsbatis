//! 语句构建器：SELECT / INSERT / UPDATE / DELETE。
//!
//! 语句不持有实体实例，只持有 `&'static str` 表名与 `Expr` / `Value`，因此构建
//! 阶段零分配，直到 `build()` 才开始拼字符串。
//!
//! # 一条硬规则：不静默丢语义
//!
//! 如果某个特性在该方言下不存在（例如 MySQL 没有 `RETURNING`），构建器**不会**
//! 悄悄删掉它，而是把说明放进 `Sql::warnings`。调用方可以因此决定换方案，而不是
//! 在运行期才发现"我以为拿到了自增 id"。

use crate::dialect::Dialect;
use crate::expr::Expr;
use crate::render::{render_expr, render_expr_q};
use crate::value::{IntoValue, Value};

/// 渲染结果：SQL 文本 + 按出现顺序排列的参数 + 降级说明。
///
/// 参数顺序即占位符顺序，PostgreSQL 的 `$N` 也按这个顺序编号，所以驱动层
/// 只要按顺序 `bind` 即可，不需要再做重排。
#[derive(Clone, Debug)]
pub struct Sql {
    pub text: String,
    pub params: Vec<Value>,
    /// 该方言下无法表达、已被降级的特性说明（正常情况下应为空）。
    pub warnings: Vec<String>,
}

impl Sql {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            params: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// 直接包裹一段 SQL 文本，不经过任何渲染。驱动层的 batch / DDL 用这个。
    pub fn raw(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            params: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn has_warning(&self) -> bool {
        !self.warnings.is_empty()
    }
}

impl Default for Sql {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Asc,
    Desc,
}

/// `Order` 的更好读的别名：`Order` 这个名字在电商/交易类项目里几乎一定被实体占用
/// （`struct Order`），两者一起 `use` 会撞名。排序语义用 `SortOrder` 不会歧义：
///
/// ```ignore
/// use rsbatis::SortOrder;
/// stmt.order_by(col, SortOrder::Desc);
/// ```
pub type SortOrder = Order;

impl Order {
    pub fn as_sql(self) -> &'static str {
        match self {
            Order::Asc => "ASC",
            Order::Desc => "DESC",
        }
    }
}

// ============================ JOIN ============================

/// 连接类型。
///
/// 只做 Inner / Left / Cross 三种：Right 可以改写成 Left（交换两侧），
/// Full Outer 在 MySQL 上不存在（要模拟成两次 Left + UNION），MVP 阶段宁可不提供，
/// 也不提供一个在某些方言下悄悄降级的版本。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinKind {
    Inner,
    Left,
    Cross,
}

impl JoinKind {
    pub fn as_sql(self) -> &'static str {
        match self {
            JoinKind::Inner => "INNER JOIN",
            JoinKind::Left => "LEFT JOIN",
            JoinKind::Cross => "CROSS JOIN",
        }
    }
}

/// 一次连接。`columns` 用于自动投影（联表时默认把每张表的列都选出来）。
#[derive(Clone, Debug)]
pub struct Join {
    pub kind: JoinKind,
    pub table: &'static str,
    /// 连接源是派生表时放这里：`JOIN (子查询) AS 别名`。
    /// 存在时 `table` 记的是别名（与 `alias` 相同），投影前缀不用另开分支。
    pub sub: Option<Box<SelectStmt>>,
    /// SQL 别名（自连接 / 同表多次连接时必填）。
    pub alias: Option<&'static str>,
    pub columns: Vec<&'static str>,
    pub on: Option<Expr>,
}

impl Join {
    /// 投影前缀：有别名用别名，否则用表名。
    fn prefix(&self) -> &'static str {
        self.alias.unwrap_or(self.table)
    }
}

/// 联表自动投影：`"users"."id" AS "users__id"`。
///
/// 别名用 `表__列` 而不是直接沿用列名，是因为两张表几乎必然都有 `id` / `name`；
/// 结果行里按列名取值时，同名列只会剩一个，另一张表的数据就静默丢了。
fn push_projection(out: &mut String, d: Dialect, table: &str, col: &str, first: &mut bool) {
    if !*first {
        out.push_str(", ");
    }
    *first = false;
    out.push_str(&d.quote(table));
    out.push('.');
    out.push_str(&d.quote(col));
    out.push_str(" AS ");
    out.push_str(&d.quote(&format!("{table}__{col}")));
}

// ============================ SELECT ============================

#[derive(Clone, Debug)]
pub struct SelectStmt {
    table: &'static str,
    /// 公共表表达式：`WITH 名 AS (子查询), ...`，出现在 `SELECT` 之前。
    ctes: Vec<Cte>,
    /// 派生表：`FROM (子查询) AS 别名`。存在时 `table` 的值就是别名，
    /// 这样 `column("x")` / 列前缀推断不用另开一条路径。
    from_sub: Option<(Box<SelectStmt>, &'static str)>,
    /// 主表别名（`FROM users AS u`）。
    main_alias: Option<&'static str>,
    /// 主表列名，用于联表时的自动投影（`select::<E>()` 填入）。
    main_columns: Vec<&'static str>,
    joins: Vec<Join>,
    columns: Vec<Expr>,
    distinct: bool,
    filter: Option<Expr>,
    group_by: Vec<Expr>,
    having: Option<Expr>,
    order_by: Vec<(Expr, Order)>,
    limit: Option<u64>,
    offset: Option<u64>,
    /// 本语句的 `FROM` 是不是在引用一个（可能是递归的）CTE。`Some(名)` 表示是。
    ref_cte: Option<&'static str>,
    /// 集合运算：`UNION` / `UNION ALL` 的后续段。
    ///
    /// 存在时本语句是"复合查询"：`ORDER BY` / `LIMIT` 作用于**整条**复合查询，
    /// 因此渲染顺序是 `各段 → ORDER BY → LIMIT`，且顶层的 `ORDER BY` 只能用
    /// 裸列名（带表名的引用在 `UNION` 上非法）。
    unions: Vec<(UnionKind, Box<SelectStmt>)>,
}

/// 集合运算的类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnionKind {
    /// `UNION`：去重合并。
    Distinct,
    /// `UNION ALL`：保留重复（更快，绝大多数"合并统计"场景要的是它）。
    All,
    /// `INTERSECT`：交集（两边都有的行）。
    Intersect,
    /// `EXCEPT`：差集（左边有、右边没有的行）。
    Except,
}

/// 一个 CTE 定义。
///
/// `recursive` 存在的原因：`WITH RECURSIVE` 是**整个 WITH 子句**上的关键字，
/// 只要有一个递归 CTE，`RECURSIVE` 就得写在 `WITH` 后面（不能写在单个 CTE 上）。
/// 所以这个标记挂在每个 CTE 上，渲染时聚合判断。
#[derive(Clone, Debug)]
pub struct Cte {
    pub name: &'static str,
    pub body: Box<SelectStmt>,
    pub recursive: bool,
}

impl UnionKind {
    pub fn as_sql(&self) -> &'static str {
        match self {
            UnionKind::Distinct => "UNION",
            UnionKind::All => "UNION ALL",
            UnionKind::Intersect => "INTERSECT",
            UnionKind::Except => "EXCEPT",
        }
    }
}

impl SelectStmt {
    pub fn new(table: &'static str) -> Self {
        Self {
            table,
            ctes: Vec::new(),
            from_sub: None,
            main_alias: None,
            main_columns: Vec::new(),
            joins: Vec::new(),
            columns: Vec::new(),
            distinct: false,
            filter: None,
            group_by: Vec::new(),
            having: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            unions: Vec::new(),
            ref_cte: None,
        }
    }

    /// 用实体元信息构造：记下表名与全部列名，联表时才能自动投影。
    pub fn for_entity<E: crate::Entity>() -> Self {
        let mut s = Self::new(E::TABLE);
        s.main_columns = E::COLUMNS.to_vec();
        s
    }

    /// 追加一个 CTE：`WITH 名 AS (子查询)`。可链式追加多个。
    ///
    /// 引用方式：把主表名写成 CTE 名（`SelectStmt::new("young_users")`），
    /// 或作为 `join_subquery` 的来源。**参数顺序**：CTE 在 `SELECT` 之前渲染，
    /// 所以它的参数排在最前面。
    pub fn cte(mut self, name: &'static str, sub: &SelectStmt) -> Self {
        self.ctes.push(Cte {
            name,
            body: Box::new(sub.clone()),
            recursive: false,
        });
        self
    }

    /// 递归 CTE：`WITH RECURSIVE 名 AS (基础段 UNION ALL 递归段)`。
    ///
    /// 递归段里引用自己必须用 `SelectStmt::from_cte(name)` 构造 —— 否则那一层的
    /// `table` 只是个普通物理表名，渲染时既不会带表名限定，也容易和同名的真实表混淆。
    ///
    /// ```ignore
    /// let base = SelectStmt::new("nums").column_expr(Expr::lit(1).as_("n"));
    /// let step = SelectStmt::from_cte("nums")
    ///     .column_expr(Expr::arith(ArithOp::Add, Expr::col("nums", "n"), Expr::lit(1)).as_("n"))
    ///     .and_where(Expr::cmp(BinOp::Lt, Expr::col("nums", "n"), Expr::lit(5)));
    /// let q = SelectStmt::new("nums").column("n").recursive_cte("nums", &base, &step);
    /// ```
    ///
    /// 注意：递归段**不能带 `ORDER BY` / `LIMIT`**（SQLite 与 MySQL 都不接受），
    /// 防死循环请靠递归段自身的 `WHERE` 终止条件，或依赖引擎的递归深度上限
    /// （MySQL 默认 1000、可用 `cte_max_recursion_depth` 调整）。
    pub fn recursive_cte(
        mut self,
        name: &'static str,
        base: &SelectStmt,
        step: &SelectStmt,
    ) -> Self {
        let body = base.clone().union_all(step);
        self.ctes.push(Cte {
            name,
            body: Box::new(body),
            recursive: true,
        });
        self
    }

    /// 以某个 CTE 名作为 `FROM` 源（递归段引用自己时用）。
    ///
    /// 与 `new(name)` 的区别：这里标记了"这是 CTE 引用"，渲染时列引用会带表名限定
    /// （`"nums"."n"`），与 `WHERE` 里的条件保持一致，也不会被误当成物理表。
    pub fn from_cte(name: &'static str) -> Self {
        let mut s = Self::new(name);
        s.ref_cte = Some(name);
        s
    }

    /// 追加 `GROUP BY` 表达式。
    pub fn group_by(mut self, e: Expr) -> Self {
        self.group_by.push(e);
        self
    }

    /// 批量追加 `GROUP BY`。
    pub fn group_by_all<I: IntoIterator<Item = Expr>>(mut self, es: I) -> Self {
        self.group_by.extend(es);
        self
    }

    /// 设置 `HAVING`（覆盖语义，与 `where_` 一致）。
    ///
    /// 没有 `GROUP BY` 时 `HAVING` 在 SQL 里行为依方言而定（SQLite 允许、MySQL 允许但语义
    /// 等同 WHERE），所以这里不拦——但文档里说明它是分组过滤。
    pub fn having(mut self, e: Expr) -> Self {
        self.having = Some(e);
        self
    }

    /// 渲染 `WITH ... ` 前缀（含尾部空格）。无 CTE 时什么都不做。
    fn push_ctes(
        &self,
        d: Dialect,
        out: &mut String,
        params: &mut Vec<Value>,
        warnings: &mut Vec<String>,
    ) {
        if self.ctes.is_empty() {
            return;
        }
        // `RECURSIVE` 是 WITH 子句级的关键字：只要有一个递归 CTE 就必须写
        if self.ctes.iter().any(|c| c.recursive) {
            out.push_str("WITH RECURSIVE ");
        } else {
            out.push_str("WITH ");
        }
        for (i, c) in self.ctes.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&d.quote(c.name));
            out.push_str(" AS (");
            c.body.render_body(d, out, params, warnings, true);
            out.push(')');
        }
    }

    /// 用子查询作为 `FROM` 源（派生表）：`FROM (子查询) AS alias`。
    ///
    /// `table` 被设成 alias，所以 `column("cnt")` 生成的列引用自动是 `alias.cnt`。
    /// 派生表的列集合由子查询决定，`main_columns` 在这里没有意义——`columns` 为空时
    /// 投影直接输出 `*`。
    ///
    /// **参数顺序**：派生表在 `FROM` 里最先出现，所以它的参数排在 `WHERE` 参数之前。
    /// 这一点由 `push_from_joins_where` 的渲染顺序天然保证（先 FROM 后 WHERE）。
    pub fn from_subquery(sub: &SelectStmt, alias: &'static str) -> Self {
        let mut s = Self::new(alias);
        s.from_sub = Some((Box::new(sub.clone()), alias));
        s
    }

    /// `UNION`：与另一段去重合并。
    ///
    /// 段会被克隆进语句，所以可以链式多次：`.union(&a).union(&b)`。
    pub fn union(mut self, sub: &SelectStmt) -> Self {
        self.unions
            .push((UnionKind::Distinct, Box::new(sub.clone())));
        self
    }

    /// `UNION ALL`：保留重复。合并统计类需求用这个，比 `UNION` 快得多。
    pub fn union_all(mut self, sub: &SelectStmt) -> Self {
        self.unions.push((UnionKind::All, Box::new(sub.clone())));
        self
    }

    /// `INTERSECT`：交集。
    pub fn intersect(mut self, sub: &SelectStmt) -> Self {
        self.unions.push((UnionKind::Intersect, Box::new(sub.clone())));
        self
    }

    /// `EXCEPT`：差集（本段有、`sub` 里没有的行）。
    pub fn except(mut self, sub: &SelectStmt) -> Self {
        self.unions.push((UnionKind::Except, Box::new(sub.clone())));
        self
    }

    /// 是否复合查询（含 `UNION` / `INTERSECT` / `EXCEPT` 任一段）。
    pub(crate) fn has_set_op(&self) -> bool {
        !self.unions.is_empty()
    }

    /// 投影宽度（列数）。用于检查各 UNION 段列数是否一致——列数不等在 SQL 里是硬错误，
    /// 但报错信息通常只说"each SELECT must have the same number of columns"，不告诉你是哪两段。
    ///
    /// 返回 `None` 表示"本层投影是 `*` 或列集合来自子查询"，无法静态判断。
    fn projection_width(&self) -> Option<usize> {
        if !self.columns.is_empty() {
            return Some(self.columns.len());
        }
        if self.is_derived() || self.is_cte_source() {
            return None;
        }
        if self.main_columns.is_empty() && self.joins.is_empty() {
            // 纯 `SELECT *`
            return None;
        }
        Some(self.main_columns.len() + self.joins.iter().map(|j| j.columns.len()).sum::<usize>())
    }

    /// 不带 `FROM` 的语句：只投影表达式，例如 `SELECT 1 AS "n"`。
    ///
    /// 递归 CTE 的**基础段**必须是这种形态 —— 它不读任何表，只产种子行。
    /// 若这里误写成 `SelectStmt::new("nums")`，会渲染成 `SELECT 1 AS n FROM nums`，
    /// 也就是"从递归 CTE 自己里读种子"，在 SQLite 上直接报错、在 MySQL 上可能返回空集。
    pub fn no_table() -> Self {
        Self::new("")
    }

    /// 是否是派生表（`FROM` 是子查询而非物理表）。
    pub(crate) fn is_derived(&self) -> bool {
        self.from_sub.is_some()
    }

    /// 主表名是否是本查询定义的某个 CTE 名。
    ///
    /// 用于让 CTE 引用的列也带表名——与派生表保持一致，多 CTE / 与物理表混用时才不会歧义。
    pub(crate) fn is_cte_source(&self) -> bool {
        self.ctes.iter().any(|c| c.name == self.table)
    }

    /// 给主表起别名。列引用必须配合 `Column::of_table(别名)` 才会用上它。
    pub fn from_alias(mut self, alias: &'static str) -> Self {
        self.main_alias = Some(alias);
        self
    }

    pub fn main_alias(&self) -> Option<&'static str> {
        self.main_alias
    }

    /// 追加一个连接。`on` 是连接条件，两侧列引用会自动带表名。
    pub fn join<E: crate::Entity>(mut self, kind: JoinKind, on: Expr) -> Self {
        self.joins.push(Join {
            kind,
            table: E::TABLE,
            sub: None,
            alias: None,
            columns: E::COLUMNS.to_vec(),
            on: Some(on),
        });
        self
    }

    /// 带别名的连接。同一张表连两次（含自连接）时必须用它，
    /// 否则两个 `Join` 的表名相同，`ON` 条件两侧会指向同一张表。
    pub fn join_as<E: crate::Entity>(
        mut self,
        kind: JoinKind,
        alias: &'static str,
        on: Expr,
    ) -> Self {
        self.joins.push(Join {
            kind,
            table: E::TABLE,
            sub: None,
            alias: Some(alias),
            columns: E::COLUMNS.to_vec(),
            on: Some(on),
        });
        self
    }

    /// 无条件笛卡尔连接（很少用，但标准的 CROSS JOIN 就是这个语义）。
    pub fn cross_join<E: crate::Entity>(mut self) -> Self {
        self.joins.push(Join {
            kind: JoinKind::Cross,
            table: E::TABLE,
            sub: None,
            alias: None,
            columns: E::COLUMNS.to_vec(),
            on: None,
        });
        self
    }

    /// 带别名、无条件的笛卡尔连接。
    pub fn cross_join_as<E: crate::Entity>(mut self, alias: &'static str) -> Self {
        self.joins.push(Join {
            kind: JoinKind::Cross,
            table: E::TABLE,
            sub: None,
            alias: Some(alias),
            columns: E::COLUMNS.to_vec(),
            on: None,
        });
        self
    }

    /// 按**表名字符串**连接（不需要实体类型）。
    ///
    /// `join<E: Entity>` 的表名来自实体元数据，在没有实体定义时（例如 core 层的纯
    /// SQL 测试、或者连一张视图）用不了。这个入口补上那个缺口。
    pub fn join_table(mut self, kind: JoinKind, table: &'static str, on: Expr) -> Self {
        self.joins.push(Join {
            kind,
            table,
            sub: None,
            alias: None,
            columns: Vec::new(),
            on: Some(on),
        });
        self
    }

    /// 按表名字符串 + 别名连接（同一张表连两次时必须用它）。
    pub fn join_table_as(
        mut self,
        kind: JoinKind,
        table: &'static str,
        alias: &'static str,
        on: Expr,
    ) -> Self {
        self.joins.push(Join {
            kind,
            table,
            sub: None,
            alias: Some(alias),
            columns: Vec::new(),
            on: Some(on),
        });
        self
    }

    /// 连接一个派生表：`JOIN (子查询) AS alias ON ...`。
    ///
    /// 这是"命名子查询"的主要用途——子查询算一次、被外层按别名引用。
    /// 派生表必须有别名（MySQL 也强制要求），所以这里 `alias` 不是可选参数。
    /// 它的列不参与自动投影（列集合由子查询决定，我们无从得知）。
    pub fn join_subquery(
        mut self,
        kind: JoinKind,
        sub: &SelectStmt,
        alias: &'static str,
        on: Expr,
    ) -> Self {
        self.joins.push(Join {
            kind,
            table: alias,
            sub: Some(Box::new(sub.clone())),
            alias: Some(alias),
            columns: Vec::new(),
            on: Some(on),
        });
        self
    }

    /// 追加一个裸列。空列列表时输出 `*`。
    pub fn column(mut self, name: &'static str) -> Self {
        self.columns.push(Expr::Col(self.table, name));
        self
    }

    /// 子查询是否带 `LIMIT` / `OFFSET`。渲染层用它给 MySQL 一个真实存在的限制告警。
    pub(crate) fn has_limit_or_offset(&self) -> bool {
        self.limit.is_some() || self.offset.is_some()
    }

    pub fn columns<I>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = &'static str>,
    {
        self.columns
            .extend(names.into_iter().map(|n| Expr::Col(self.table, n)));
        self
    }

    /// 追加任意表达式（函数、别名、算术……）。
    pub fn column_expr(mut self, e: Expr) -> Self {
        self.columns.push(e);
        self
    }

    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    pub fn where_(mut self, e: Expr) -> Self {
        self.filter = Some(e);
        self
    }

    /// 追加条件，自动用 AND 连接（链式写法）。
    pub fn and_where(mut self, e: Expr) -> Self {
        self.filter = Some(match self.filter.take() {
            Some(f) => Expr::and(vec![f, e]),
            None => e,
        });
        self
    }

    pub fn order_by(mut self, e: Expr, o: Order) -> Self {
        self.order_by.push((e, o));
        self
    }

    pub fn limit(mut self, n: u64) -> Self {
        self.limit = Some(n);
        self
    }

    pub fn offset(mut self, n: u64) -> Self {
        self.offset = Some(n);
        self
    }

    /// 渲染 count 查询：`SELECT COUNT(*) FROM … WHERE …`。
    ///
    /// 没有 `DISTINCT` 时直接复用 `FROM / JOIN / WHERE`（丢掉 `ORDER BY`、`LIMIT`、`OFFSET`）：
    /// 分页只需要总行数，排序与截断对总数没有意义，而且 `LIMIT` 留着会让 `COUNT(*)` 数错。
    ///
    /// 有 `DISTINCT` 或 `GROUP BY` 时不能直接数：`SELECT COUNT(*) … DISTINCT` 在 SQL 里是非法的，
    /// `GROUP BY` 下 `COUNT(*)` 返回的是每组一行而非总数；
    /// 而"去重后 / 分组后的行数"只能靠子查询表达，所以把完整查询包一层
    /// （`SELECT COUNT(*) FROM (…原查询…) AS __rsbatis_count`）。代价是内层要跑一遍去重，
    /// 但这个语义本来就要求它跑。
    pub fn build_count(&self, d: Dialect) -> Sql {
        let mut warnings: Vec<String> = Vec::new();

        // 有 `GROUP BY` 时不能直接 `COUNT(*)`：那会数"组数 × 每组行数"之外的东西，
        // 语义是"每组一行"，总数只能靠把原查询包一层再数。
        if self.distinct || !self.group_by.is_empty() || self.has_set_op() {
            // 子查询里要丢掉 ORDER BY / LIMIT / OFFSET：总数不该被"用户设的截断"影响，
            // 排序对计数也没有意义。留着 LIMIT 会让 COUNT 数成"截断后的行数"。
            let mut inner_stmt = self.clone();
            inner_stmt.order_by.clear();
            inner_stmt.limit = None;
            inner_stmt.offset = None;
            let inner = inner_stmt.build(d);
            warnings.extend(inner.warnings);
            let mut params = inner.params;
            let mut out = String::from("SELECT COUNT(*) FROM (");
            out.push_str(&inner.text);
            out.push_str(") AS ");
            out.push_str(&d.quote("__rsbatis_count"));
            return Sql {
                text: out,
                params: std::mem::take(&mut params),
                warnings,
            };
        }

        let mut out = String::new();
        let mut params = Vec::new();
        let qualify = !self.joins.is_empty()
            || self.main_alias.is_some()
            || self.is_derived()
            || self.is_cte_source()
            || self.ref_cte.is_some();
        if !self.ctes.is_empty() {
            // COUNT 也必须带 WITH，否则引用的 CTE 名不存在
            self.push_ctes(d, &mut out, &mut params, &mut warnings);
            out.push(' ');
        }
        out.push_str("SELECT COUNT(*)");
        self.push_from_joins_where(d, qualify, &mut out, &mut params, &mut warnings);

        Sql {
            text: out,
            params,
            warnings,
        }
    }

    /// `FROM` / `JOIN` / `WHERE` 三段。`build` 与 `build_count` 共用：
    /// 两处各写一遍是这类代码最容易漂移的地方（一处改了别名、另一处没改）。
    fn push_from_joins_where(
        &self,
        d: Dialect,
        qualify: bool,
        out: &mut String,
        params: &mut Vec<Value>,
        warnings: &mut Vec<String>,
    ) {
        // 窗口函数不能出现在 WHERE；HAVING 同理（HAVING 在 GROUP BY 之后求值，
        // 窗口还没算出来）。这两处若真的含窗口，SQL 一定被引擎拒，提前告警。
        if let Some(f) = &self.filter {
            if f.has_window() {
                warnings.push(
                    "窗口函数不能出现在 WHERE：请改用派生表（`from_subquery`）再在外层过滤，\
                     或把排序需求交给 `ORDER BY` / `LIMIT`"
                        .to_string(),
                );
            }
        }
        if let Some(h) = &self.having {
            if h.has_window() {
                warnings.push("窗口函数不能出现在 HAVING：请改用派生表再在外层过滤".to_string());
            }
        }
        // 空表名 = 无 FROM（`SELECT 1 AS n` 这种常量投影）
        if self.table.is_empty() && self.from_sub.is_none() {
            if let Some(f) = &self.filter {
                out.push_str(" WHERE ");
                render_expr_q(f, d, out, params, warnings, 0, qualify);
            }
            return;
        }
        out.push_str(" FROM ");
        if let Some((sub, alias)) = &self.from_sub {
            // 派生表：子查询与外层共享 out / params，所以它的参数排在 WHERE 参数之前，
            // 编号连续。内部强制带表名限定，避免与外层派生表别名混淆。
            out.push('(');
            sub.render_body(d, out, params, warnings, true);
            out.push_str(") AS ");
            out.push_str(&d.quote(alias));
        } else {
            out.push_str(&d.quote(self.table));
            if let Some(a) = self.main_alias {
                out.push_str(" AS ");
                out.push_str(&d.quote(a));
            }
        }

        for j in &self.joins {
            out.push(' ');
            out.push_str(j.kind.as_sql());
            out.push(' ');
            if let Some(sub) = &j.sub {
                out.push('(');
                sub.render_body(d, out, params, warnings, true);
                out.push_str(") AS ");
                out.push_str(&d.quote(j.alias.expect("派生表必须带别名")));
            } else {
                out.push_str(&d.quote(j.table));
                if let Some(a) = j.alias {
                    out.push_str(" AS ");
                    out.push_str(&d.quote(a));
                }
            }
            if let Some(on) = &j.on {
                out.push_str(" ON ");
                render_expr_q(on, d, out, params, warnings, 0, true);
            }
        }

        if let Some(f) = &self.filter {
            out.push_str(" WHERE ");
            render_expr_q(f, d, out, params, warnings, 0, qualify);
        }
    }

    pub fn build(&self, d: Dialect) -> Sql {
        let mut out = String::new();
        let mut params = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        self.render_body(d, &mut out, &mut params, &mut warnings, false);
        Sql {
            text: out,
            params,
            warnings,
        }
    }

    /// 把整条 `SELECT …` 追加到 `out`，参数压进共享的 `params`，告警压进 `warnings`。
    ///
    /// 子查询（`EXISTS` / `IN (…)`）全靠这个入口：**共享同一个参数列表**，
    /// 内层的占位符编号才是接在外层后面的。如果给子查询单独开一个 `Vec`，
    /// 它自己从 1 开始编号，最终拼出来的 SQL 会把参数错配到别的 `?` 上——
    /// 这是子查询实现里最容易出错、也最难通过"能跑"来发现的一处。
    pub(crate) fn render_body(
        &self,
        d: Dialect,
        out: &mut String,
        params: &mut Vec<Value>,
        warnings: &mut Vec<String>,
        force_qualify: bool,
    ) {
        if !self.ctes.is_empty() {
            // WITH 必须出现在 SELECT 之前；参数也因此在最前面入列
            self.push_ctes(d, out, params, warnings);
            out.push(' ');
        }
        out.push_str("SELECT ");
        if self.distinct {
            out.push_str("DISTINCT ");
        }
        // 联表（或主表带别名）时列引用必须带限定名，否则同名列在 WHERE / ORDER BY 里产生歧义
        // 子查询强制带表名限定（`force_qualify`）：相关子查询里引用外层表的列时，
        // 不带限定名会被解析成子查询自己表的同名列——静默查错表，比报错难查得多。
        // 复合查询（UNION）时，`force_qualify` **不穿透**到各段：它来自外层派生表，
        // 而每一段是独立的单表 SELECT，各自按自己的情况决定要不要表名。
        // 否则会出现"第一段带前缀、第二段不带"这种同一条 SQL 里风格不一致的情况
        //（结果仍然正确，但读起来像两段来自不同的生成器）。
        let qualify = if self.has_set_op() {
            !self.joins.is_empty()
                || self.main_alias.is_some()
                || self.is_cte_source()
                || self.ref_cte.is_some()
        } else {
            force_qualify
                || !self.joins.is_empty()
                || self.main_alias.is_some()
                || self.is_derived()
                || self.is_cte_source()
                || self.ref_cte.is_some()
        };
        let main_prefix = self.main_alias.unwrap_or(self.table);

        // 派生表的投影直接 `*`：它的列集合由子查询决定，这里没有元数据可用，
        // 硬拼 `main_columns` 只会拼出一串不存在的列名。
        // CTE 引用与派生表一样：列集合不属于本层，投影直接 `*`
        if self.columns.is_empty() && (self.is_derived() || self.is_cte_source() || !qualify) {
            out.push('*');
        } else if self.columns.is_empty() {
            if self.main_columns.is_empty() {
                warnings.push(
                    "联表查询缺少主表列信息（用 SelectStmt::new 而非 select::<E>() 构造），\
                     投影只包含被连接的表"
                        .to_string(),
                );
            }
            let mut first = true;
            for c in &self.main_columns {
                push_projection(out, d, main_prefix, c, &mut first);
            }
            for j in &self.joins {
                for c in &j.columns {
                    push_projection(out, d, j.prefix(), c, &mut first);
                }
            }
        } else {
            for (i, c) in self.columns.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_expr_q(c, d, out, params, warnings, 8, qualify);
            }
        }

        self.push_from_joins_where(d, qualify, out, params, warnings);

        if !self.group_by.is_empty() {
            out.push_str(" GROUP BY ");
            for (i, e) in self.group_by.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_expr_q(e, d, out, params, warnings, 0, qualify);
            }
        }

        if let Some(h) = &self.having {
            out.push_str(" HAVING ");
            render_expr_q(h, d, out, params, warnings, 0, qualify);
        }

        if self.has_set_op() {
            // `filter` 渲染在 FROM/WHERE 段里，也就是**第一段**内部。
            // 用户写 `.union_all(&b).and_where(…)` 时几乎都以为是"合并后再过滤"，
            // 实际只有第一段被过滤 —— 结果集里第二段的行不受约束。这类错误不会报错，
            // 只会给出多出来的行，所以必须告警。
            if self.filter.is_some() {
                warnings.push(
                    "WHERE 只作用于 UNION 的第一段（它写在第一段 SELECT 里）：\
                     要对整条复合查询过滤，请先用 `from_subquery` 把 UNION 包成派生表，再在外层 `and_where`"
                        .to_string(),
                );
            }
            // 优先级陷阱：`INTERSECT` 的优先级**高于** `UNION` / `EXCEPT`（SQL 标准与
            // MariaDB 都是），所以 `A UNION B INTERSECT C` 在引擎里是
            // `A UNION (B INTERSECT C)`，而不是从左到右的 `(A UNION B) INTERSECT C`。
            // 复合查询的段不能用括号隔开（SQLite 直接语法错误），只能告警让用户拆成派生表。
            let mut seen_union_family = false;
            for (kind, _) in &self.unions {
                match kind {
                    UnionKind::Distinct | UnionKind::All => seen_union_family = true,
                    UnionKind::Intersect if seen_union_family => {
                        warnings.push(
                            "INTERSECT 与 UNION/EXCEPT 混用：优先级**因引擎而异** —— \
                             SQLite 三者同级、左结合（`(A UNION B) INTERSECT C`），\
                             MySQL/MariaDB 遵循 SQL 标准（INTERSECT 更高，`A UNION (B INTERSECT C)`）。\
                             同一条 SQL 在两个引擎上会得到不同结果，请用派生表（`from_subquery`）\
                             把左侧部分包起来以固定语义"
                                .to_string(),
                        );
                        break;
                    }
                    _ => {}
                }
            }
            // 列数不等是硬错误，提前说清楚是哪一层
            let width = self.projection_width();
            if let Some(w) = width {
                for (kind, sub) in &self.unions {
                    if let Some(sw) = sub.projection_width() {
                        if sw != w {
                            warnings.push(format!(
                                "UNION 各段列数必须一致：本段 {} 列，{} 段 {} 列",
                                w,
                                kind.as_sql(),
                                sw
                            ));
                        }
                    }
                }
            }
            for (kind, sub) in &self.unions {
                out.push(' ');
                out.push_str(kind.as_sql());
                out.push(' ');
                // 段内自带 ORDER BY / LIMIT 时**不渲染**：`UNION (SELECT … ORDER BY …)`
                // 在 SQLite 上是语法错误，MySQL 的解释也只在部分版本成立。
                // 这与本 crate 一贯的做法一致：宁可不支持 + 明确告警，也不生成必然被拒的 SQL。
                if !sub.order_by.is_empty() || sub.limit.is_some() || sub.offset.is_some() {
                    warnings.push(
                        "UNION 的某一段带了 ORDER BY / LIMIT：这些子句只作用于整条复合查询，\
                         段内排序/截断请改用派生表（`SELECT * FROM (…) AS t ORDER BY …`）"
                            .to_string(),
                    );
                }
                let mut seg = sub.clone();
                seg.order_by.clear();
                seg.limit = None;
                seg.offset = None;
                seg.render_body(d, out, params, warnings, false);
            }
        }

        if !self.order_by.is_empty() {
            out.push_str(" ORDER BY ");
            // 复合查询的顶层 ORDER BY 只能用裸列名/别名/序号：
            // `SELECT a FROM t UNION SELECT b FROM u ORDER BY t.a` 在 SQL 里非法。
            // 所以带 UNION 时强制去掉表名限定（用户写 `Expr::col("t","a")` 也能得到合法 SQL）。
            let ob_qualify = if self.has_set_op() { false } else { qualify };
            for (i, (e, o)) in self.order_by.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_expr_q(e, d, out, params, warnings, 0, ob_qualify);
                out.push(' ');
                out.push_str(o.as_sql());
            }
        }

        if let Some(n) = self.limit {
            out.push_str(" LIMIT ");
            out.push_str(&n.to_string());
        }
        if let Some(n) = self.offset {
            out.push_str(" OFFSET ");
            out.push_str(&n.to_string());
        }
    }
}

// ============================ INSERT ============================

/// 冲突时某一列要写入的值。
///
/// 为什么需要这个枚举而不是直接传 `Value`：upsert 最常见的形式是
/// "用这次插入的值覆盖"，而那个值**此刻还不知道**（它在 `VALUES` 里）——
/// 只能在 SQL 里用方言自己的引用语法表达（SQLite/PG 是 `excluded.col`，
/// MySQL 是 `VALUES(col)`）。把它建模成枚举，就不会出现"把 NULL 当成覆盖值"
/// 这种静默错误。
#[derive(Clone, Debug)]
pub enum ConflictSet {
    /// 用本次插入的值覆盖：`SET c = excluded.c` / `SET c = VALUES(c)`。
    KeepInserted,
    /// 写一个常量值（会额外占一个参数位）。
    Value(Value),
}

#[derive(Clone, Debug)]
pub struct InsertStmt {
    table: &'static str,
    /// (列名, 值)。列顺序同时决定 VALUES 里的参数顺序。
    values: Vec<(&'static str, Value)>,
    returning: Vec<&'static str>,
    on_conflict_do_nothing: bool,
    /// 冲突目标列。SQLite / PG 的 `ON CONFLICT (…)` 需要它；MySQL 按主键/唯一键
    /// 自动判断，这里是空也照样渲染 `ON DUPLICATE KEY UPDATE`。
    conflict_target: Vec<&'static str>,
    /// 冲突时要执行的 SET 列表。**非空即表示走 upsert**，优先于 do_nothing。
    conflict_sets: Vec<(&'static str, ConflictSet)>,
}

impl InsertStmt {
    pub fn new(table: &'static str) -> Self {
        Self {
            table,
            values: Vec::new(),
            returning: Vec::new(),
            on_conflict_do_nothing: false,
            conflict_target: Vec::new(),
            conflict_sets: Vec::new(),
        }
    }

    /// 设置一列。`T` 是列的值类型，所以 `Column<i32>` 只能收 `i32`/`Option<i32>`。
    pub fn set<T>(mut self, col: crate::column::Column<T>, v: impl IntoValue<T>) -> Self {
        self.values.push((col.name(), v.into_value()));
        self
    }

    /// 显式写入 NULL。类型参数只用来取列名——NULL 本身与 T 无关。
    pub fn set_null<T>(mut self, col: crate::column::Column<T>) -> Self {
        self.values.push((col.name(), Value::Null));
        self
    }

    /// 设置一列（用列名，类型由调用方保证）。用于动态列场景。
    pub fn set_raw(mut self, col: &'static str, v: Value) -> Self {
        self.values.push((col, v));
        self
    }

    /// `INSERT ... RETURNING cols`。MySQL 不支持时会进入 `Sql::warnings`。
    pub fn returning<I>(mut self, cols: I) -> Self
    where
        I: IntoIterator<Item = &'static str>,
    {
        self.returning.extend(cols);
        self
    }

    pub fn on_conflict_do_nothing(mut self) -> Self {
        self.on_conflict_do_nothing = true;
        self
    }

    /// upsert 的常用形态：冲突时**用本次插入的值**覆盖这些列。
    ///
    /// ```ignore
    /// insert::<User>()
    ///     .set(User::col_id(), 1)
    ///     .set(User::col_name(), "张三")
    ///     .on_conflict_do_update(["id"], ["name"]);
    /// // SQLite/PG: ON CONFLICT ("id") DO UPDATE SET "name" = excluded."name"
    /// // MySQL    : ON DUPLICATE KEY UPDATE "name" = VALUES("name")
    /// ```
    ///
    /// `target` 只在 SQLite / PostgreSQL 生效（MySQL 按主键与唯一键自动判断冲突）。
    pub fn on_conflict_do_update<T, U>(self, target: T, columns: U) -> Self
    where
        T: IntoIterator<Item = &'static str>,
        U: IntoIterator<Item = &'static str>,
    {
        let sets: Vec<(&'static str, ConflictSet)> = columns
            .into_iter()
            .map(|c| (c, ConflictSet::KeepInserted))
            .collect();
        self.on_conflict_do_update_with(target, sets)
    }

    /// upsert 的完整形态：每列可以自己决定"用插入值覆盖"还是"写常量"。
    pub fn on_conflict_do_update_with<T, U>(mut self, target: T, sets: U) -> Self
    where
        T: IntoIterator<Item = &'static str>,
        U: IntoIterator<Item = (&'static str, ConflictSet)>,
    {
        self.conflict_target = target.into_iter().collect();
        self.conflict_sets = sets.into_iter().collect();
        self
    }

    pub fn build(&self, d: Dialect) -> Sql {
        let mut out = String::from("INSERT INTO ");
        out.push_str(&d.quote(self.table));

        let mut params = Vec::new();
        let mut warnings = Vec::new();

        if self.values.is_empty() {
            // 没有任何列时用 DEFAULT VALUES（SQLite / PostgreSQL 支持）
            if d.supports_returning() {
                out.push_str(" DEFAULT VALUES");
            } else {
                warnings.push(format!(
                    "{} 不支持没有列的 INSERT，必须至少给一列赋值",
                    d.name()
                ));
            }
        } else {
            out.push_str(" (");
            for (i, (c, _)) in self.values.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&d.quote(c));
            }
            out.push_str(") VALUES (");
            for (i, (_, v)) in self.values.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                params.push(v.clone());
                out.push_str(&d.placeholder(params.len()));
            }
            out.push(')');
        }

        if !self.conflict_sets.is_empty() {
            // upsert 优先于 do_nothing（同时设置时以后者无意义）
            self.append_upsert(&mut out, d, &mut params, &mut warnings);
        } else if self.on_conflict_do_nothing && d.uses_on_conflict() {
            out.push_str(" ON CONFLICT DO NOTHING");
        } else if self.on_conflict_do_nothing {
            // MySQL 没有 ON CONFLICT DO NOTHING。用"把某列赋给它自己"达成真正的
            // 空操作：它只处理重复键，**不会**像 INSERT IGNORE 那样顺手吞掉
            // 类型截断之类的其他错误。
            match self.values.first() {
                Some((c, _)) => {
                    let q = d.quote(c);
                    out.push_str(&format!(" ON DUPLICATE KEY UPDATE {q} = {q}"));
                }
                None => warnings.push(format!(
                    "{} 不支持 ON CONFLICT DO NOTHING，且本次 INSERT 没有任何列可自指，已忽略",
                    d.name()
                )),
            }
        }

        self.append_returning(&mut out, d, &mut warnings);

        Sql {
            text: out,
            params,
            warnings,
        }
    }

    /// 渲染 upsert 的尾部子句。
    fn append_upsert(
        &self,
        out: &mut String,
        d: Dialect,
        params: &mut Vec<Value>,
        warnings: &mut Vec<String>,
    ) {
        let sets = &self.conflict_sets;

        if d.uses_on_conflict() {
            if self.conflict_target.is_empty() {
                // SQLite 允许不带目标的 DO UPDATE；PostgreSQL 对此会直接报错，
                // 所以只在 PG 上给警告，而不是一律拒绝。
                if matches!(d, Dialect::Postgres) {
                    warnings.push(
                        "PostgreSQL 的 ON CONFLICT DO UPDATE 必须指定冲突目标列（主键或唯一索引）"
                            .to_string(),
                    );
                }
                out.push_str(" ON CONFLICT DO UPDATE SET ");
            } else {
                out.push_str(" ON CONFLICT (");
                for (i, c) in self.conflict_target.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&d.quote(c));
                }
                out.push_str(") DO UPDATE SET ");
            }
            for (i, (c, v)) in sets.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let q = d.quote(c);
                match v {
                    ConflictSet::KeepInserted => {
                        // SQLite 与 PostgreSQL 用 excluded 伪表引用"本会被插入的那一行"
                        out.push_str(&format!("{q} = excluded.{q}"));
                    }
                    ConflictSet::Value(val) => {
                        params.push(val.clone());
                        out.push_str(&format!("{q} = {}", d.placeholder(params.len())));
                    }
                }
            }
        } else {
            // MySQL：冲突目标由主键与唯一键自动决定，`VALUES(col)` 指向本次要插入的值
            if !self.conflict_target.is_empty() {
                warnings.push(format!(
                    "{} 忽略冲突目标列（{:?}）：MySQL 按主键与唯一键自动判定冲突",
                    d.name(),
                    self.conflict_target
                ));
            }
            out.push_str(" ON DUPLICATE KEY UPDATE ");
            for (i, (c, v)) in sets.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let q = d.quote(c);
                match v {
                    ConflictSet::KeepInserted => {
                        out.push_str(&format!("{q} = VALUES({q})"));
                    }
                    ConflictSet::Value(val) => {
                        params.push(val.clone());
                        out.push_str(&format!("{q} = {}", d.placeholder(params.len())));
                    }
                }
            }
        }
    }

    fn append_returning(&self, out: &mut String, d: Dialect, warnings: &mut Vec<String>) {
        if self.returning.is_empty() {
            return;
        }
        if d.supports_returning() {
            out.push_str(" RETURNING ");
            for (i, c) in self.returning.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&d.quote(c));
            }
        } else {
            warnings.push(format!(
                "{} 不支持 RETURNING，已省略；请改用 {} 获取自增主键",
                d.name(),
                if d == Dialect::MySql {
                    "LAST_INSERT_ID()"
                } else {
                    "库自身的返回机制"
                }
            ));
        }
    }
}

// ============================ UPDATE ============================

#[derive(Clone, Debug)]
pub struct UpdateStmt {
    table: &'static str,
    sets: Vec<(&'static str, Value)>,
    filter: Option<Expr>,
    returning: Vec<&'static str>,
}

impl UpdateStmt {
    pub fn new(table: &'static str) -> Self {
        Self {
            table,
            sets: Vec::new(),
            filter: None,
            returning: Vec::new(),
        }
    }

    pub fn set<T>(mut self, col: crate::column::Column<T>, v: impl IntoValue<T>) -> Self {
        self.sets.push((col.name(), v.into_value()));
        self
    }

    /// 把一列更新为 NULL。
    pub fn set_null<T>(mut self, col: crate::column::Column<T>) -> Self {
        self.sets.push((col.name(), Value::Null));
        self
    }

    pub fn set_raw(mut self, col: &'static str, v: Value) -> Self {
        self.sets.push((col, v));
        self
    }

    pub fn where_(mut self, e: Expr) -> Self {
        self.filter = Some(e);
        self
    }

    pub fn and_where(mut self, e: Expr) -> Self {
        self.filter = Some(match self.filter.take() {
            Some(f) => Expr::and(vec![f, e]),
            None => e,
        });
        self
    }

    pub fn returning<I>(mut self, cols: I) -> Self
    where
        I: IntoIterator<Item = &'static str>,
    {
        self.returning.extend(cols);
        self
    }

    pub fn build(&self, d: Dialect) -> Sql {
        let mut out = String::from("UPDATE ");
        out.push_str(&d.quote(self.table));
        out.push_str(" SET ");

        let mut params = Vec::new();
        let mut warnings = Vec::new();

        if self.sets.is_empty() {
            warnings.push("UPDATE 没有任何 SET 赋值，语句将无效".to_string());
        }

        for (i, (c, v)) in self.sets.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&d.quote(c));
            out.push_str(" = ");
            params.push(v.clone());
            out.push_str(&d.placeholder(params.len()));
        }

        if let Some(f) = &self.filter {
            out.push_str(" WHERE ");
            render_expr(f, d, &mut out, &mut params, &mut warnings, 0);
        } else {
            // 不阻止全表更新，但必须让调用方知道
            warnings.push("UPDATE 没有 WHERE 条件，将更新整张表".to_string());
        }

        if !self.returning.is_empty() {
            if d.supports_returning() {
                out.push_str(" RETURNING ");
                for (i, c) in self.returning.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&d.quote(c));
                }
            } else {
                warnings.push(format!("{} 不支持 RETURNING，已省略", d.name()));
            }
        }

        Sql {
            text: out,
            params,
            warnings,
        }
    }
}

// ============================ DELETE ============================

#[derive(Clone, Debug)]
pub struct DeleteStmt {
    table: &'static str,
    filter: Option<Expr>,
    returning: Vec<&'static str>,
}

impl DeleteStmt {
    pub fn new(table: &'static str) -> Self {
        Self {
            table,
            filter: None,
            returning: Vec::new(),
        }
    }

    pub fn where_(mut self, e: Expr) -> Self {
        self.filter = Some(e);
        self
    }

    pub fn and_where(mut self, e: Expr) -> Self {
        self.filter = Some(match self.filter.take() {
            Some(f) => Expr::and(vec![f, e]),
            None => e,
        });
        self
    }

    pub fn returning<I>(mut self, cols: I) -> Self
    where
        I: IntoIterator<Item = &'static str>,
    {
        self.returning.extend(cols);
        self
    }

    pub fn build(&self, d: Dialect) -> Sql {
        let mut out = String::from("DELETE FROM ");
        out.push_str(&d.quote(self.table));

        let mut params = Vec::new();
        let mut warnings = Vec::new();

        if let Some(f) = &self.filter {
            out.push_str(" WHERE ");
            render_expr(f, d, &mut out, &mut params, &mut warnings, 0);
        } else {
            warnings.push("DELETE 没有 WHERE 条件，将删除整张表的所有行".to_string());
        }

        if !self.returning.is_empty() {
            if d.supports_returning() {
                out.push_str(" RETURNING ");
                for (i, c) in self.returning.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&d.quote(c));
                }
            } else {
                warnings.push(format!("{} 不支持 RETURNING，已省略", d.name()));
            }
        }

        Sql {
            text: out,
            params,
            warnings,
        }
    }
}
