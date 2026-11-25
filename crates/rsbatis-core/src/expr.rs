//! 表达式 AST。
//!
//! 这是整个项目的核心数据结构：SQL 的 `WHERE` / `SELECT` 列表 / `ORDER BY` 都表达成
//! `Expr`，再由渲染器翻译成各方言 SQL。
//!
//! **优先级表**（数字越大结合越紧）：
//!
//! ```text
//! 1  OR
//! 2  AND
//! 3  NOT
//! 4  比较（= <> > >= < <=）、IS NULL、LIKE、IN、BETWEEN
//! 5  + -
//! 6  * / %
//! 7  一元负号
//! 8  原子（列、值、函数、CASE、CAST、显式括号）
//! ```
//!
//! 渲染器按这张表决定"要不要补括号"，同时 `Expr::Paren` 保留用户手写的括号
//! （E1 报告 F1：PoC 会丢弃用户括号，M1 必须修掉）。

use crate::value::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl BinOp {
    pub fn as_sql(self) -> &'static str {
        match self {
            BinOp::Eq => "=",
            BinOp::Ne => "<>",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

impl ArithOp {
    pub fn as_sql(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
            ArithOp::Mod => "%",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Expr {
    /// 列引用：`(表名, 列名)`。JOIN 场景需要限定表名，所以表名是必填的。
    Col(&'static str, &'static str),
    /// 参数占位值：渲染时变成 `?` / `$N`，并压进参数列表。
    Val(Value),
    /// `COUNT(*)` 之类需要的星号。
    Wildcard,
    Cmp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    /// 用户显式写的括号：渲染时原样保留。
    Paren(Box<Expr>),
    Like {
        expr: Box<Expr>,
        pattern: String,
        negated: bool,
    },
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    InList {
        expr: Box<Expr>,
        values: Vec<Value>,
        negated: bool,
    },
    Between {
        expr: Box<Expr>,
        low: Value,
        high: Value,
        negated: bool,
    },
    Arith {
        op: ArithOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Neg(Box<Expr>),
    Func {
        name: &'static str,
        args: Vec<Expr>,
    },
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,
        otherwise: Option<Box<Expr>>,
    },
    Cast {
        expr: Box<Expr>,
        ty: &'static str,
    },
    /// `expr AS "alias"`，主要用在 SELECT 列上。
    Alias {
        expr: Box<Expr>,
        alias: &'static str,
    },
    /// `EXISTS (SELECT …)`。
    ///
    /// 子查询持有整条 `SelectStmt` 而不是渲染好的字符串：方言（引号、占位符、RETURNING）
    /// 要到 `build()` 才知道，提前渲染等于把方言钉死。
    Exists(Box<crate::stmt::SelectStmt>),
    /// `expr IN (SELECT …)` / `expr NOT IN (SELECT …)`。
    InSubquery {
        expr: Box<Expr>,
        sub: Box<crate::stmt::SelectStmt>,
        negated: bool,
    },
    /// `<expr> OVER (…)`：窗口函数。
    ///
    /// 单独持有 `func` 而不是把 `OVER` 拼进函数名，是因为同一个函数既能当聚合
    /// （`SUM(x)`）又能当窗口（`SUM(x) OVER (…)`），语义完全不同，必须能分开表达。
    Over {
        func: Box<Expr>,
        window: Window,
    },
}

// ------------- 窗口规格 -------------

/// `OVER (…)` 括号里的内容。
///
/// 三个子句都可选，全空时渲染成 `OVER ()` —— SQL 合法（整表一个窗口），
/// 也是 `ROW_NUMBER() OVER ()` 这种"只要个行号"的常见写法。
#[derive(Clone, Debug, Default)]
pub struct Window {
    pub partition_by: Vec<Expr>,
    pub order_by: Vec<(Expr, crate::stmt::Order)>,
    pub frame: Option<Frame>,
}

/// `ROWS BETWEEN … AND …`。
///
/// 只建模 `ROWS`（按物理行）不建模 `RANGE`：`RANGE` 依附于"值"排序，遇到重复值、
/// 浮点、日期时边界语义随方言变化，先不做，避免给出看起来对、实际范围不对的窗口。
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub start: FrameBound,
    pub end: FrameBound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameBound {
    UnboundedPreceding,
    Preceding(u64),
    CurrentRow,
    Following(u64),
    UnboundedFollowing,
}

impl Window {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn partition_by(mut self, e: Expr) -> Self {
        self.partition_by.push(e);
        self
    }

    pub fn order_by(mut self, e: Expr, o: crate::stmt::Order) -> Self {
        self.order_by.push((e, o));
        self
    }

    /// 累积窗口：`ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW`，做累计和用。
    pub fn rows_cumulative(self) -> Self {
        self.rows_between(FrameBound::UnboundedPreceding, FrameBound::CurrentRow)
    }

    pub fn rows_between(mut self, start: FrameBound, end: FrameBound) -> Self {
        self.frame = Some(Frame { start, end });
        self
    }
}

impl FrameBound {
    pub fn as_sql(&self) -> String {
        match self {
            FrameBound::UnboundedPreceding => "UNBOUNDED PRECEDING".into(),
            FrameBound::Preceding(n) => format!("{n} PRECEDING"),
            FrameBound::CurrentRow => "CURRENT ROW".into(),
            FrameBound::Following(n) => format!("{n} FOLLOWING"),
            FrameBound::UnboundedFollowing => "UNBOUNDED FOLLOWING".into(),
        }
    }
}

impl Expr {
    // ---- 与宏的契约：这三个构造器被 `where_!` 生成的代码直接调用，签名不可改 ----

    pub fn and(items: Vec<Expr>) -> Expr {
        Expr::And(items)
    }

    pub fn or(items: Vec<Expr>) -> Expr {
        Expr::Or(items)
    }

    // `Column`/`Expr` 不是布尔表达式，`std::ops::Not` 的返回类型被语言锁死成 `bool`，
    // 实现了也用不上；这里只是构造 SQL 的 NOT 节点，与 trait 无关。
    #[allow(clippy::should_implement_trait)]
    pub fn not(e: Expr) -> Expr {
        Expr::Not(Box::new(e))
    }

    // ---- 通用构造器 ----

    /// 带表名的列引用。单表查询里 `qualify = false` 时不会渲染前缀；
    /// `JOIN` / 别名 / 派生表场景会渲染成 `"表"."列"`。
    pub fn col(table: &'static str, name: &'static str) -> Expr {
        Expr::Col(table, name)
    }

    /// 裸列引用：不带表名前缀。
    ///
    /// 只在**复合查询（`UNION`）的顶层**用到：那里的 `WHERE` / `ORDER BY` 引用的是
    /// 结果集的列名/别名，`t.col` 形式在 SQL 里非法。普通单表查询请用 `Expr::col`。
    pub fn bare_col(name: &'static str) -> Expr {
        Expr::Col("", name)
    }

    pub fn lit(v: impl Into<Value>) -> Expr {
        Expr::Val(v.into())
    }

    pub fn paren(e: Expr) -> Expr {
        Expr::Paren(Box::new(e))
    }

    pub fn func(name: &'static str, args: Vec<Expr>) -> Expr {
        Expr::Func { name, args }
    }

    pub fn cast(e: Expr, ty: &'static str) -> Expr {
        Expr::Cast {
            expr: Box::new(e),
            ty,
        }
    }

    pub fn alias(e: Expr, alias: &'static str) -> Expr {
        Expr::Alias {
            expr: Box::new(e),
            alias,
        }
    }

    pub fn arith(op: ArithOp, left: Expr, right: Expr) -> Expr {
        Expr::Arith {
            op,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    // 同上：一元负号只构造 AST 节点，不做数值取负。
    #[allow(clippy::should_implement_trait)]
    pub fn neg(e: Expr) -> Expr {
        Expr::Neg(Box::new(e))
    }

    /// 任意表达式之间做比较（列比列、函数比值都走这个）。
    pub fn cmp(op: BinOp, left: Expr, right: Expr) -> Expr {
        Expr::Cmp {
            op,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    pub fn case(operand: Option<Expr>, whens: Vec<(Expr, Expr)>, otherwise: Option<Expr>) -> Expr {
        Expr::Case {
            operand: operand.map(Box::new),
            whens,
            otherwise: otherwise.map(Box::new),
        }
    }

    // ---- 链式组合（让条件能像 SQLAlchemy 那样拼起来） ----

    /// `self AS alias`
    /// `COUNT(expr)`。
    pub fn count(e: Expr) -> Expr {
        Expr::func("COUNT", vec![e])
    }

    /// `COUNT(*)`。参数不是列，所以单独一个构造器——`COUNT(*)` 与 `COUNT(col)` 语义不同
    /// （后者不数 NULL）。
    pub fn count_star() -> Expr {
        Expr::func("COUNT", vec![Expr::Wildcard])
    }

    pub fn sum(e: Expr) -> Expr {
        Expr::func("SUM", vec![e])
    }

    pub fn avg(e: Expr) -> Expr {
        Expr::func("AVG", vec![e])
    }

    pub fn min(e: Expr) -> Expr {
        Expr::func("MIN", vec![e])
    }

    pub fn max(e: Expr) -> Expr {
        Expr::func("MAX", vec![e])
    }

    /// `EXISTS (subquery)`。最常用的形态：相关子查询过滤（`EXISTS (SELECT 1 FROM orders WHERE …)`）。
    pub fn exists(sub: &crate::stmt::SelectStmt) -> Expr {
        Expr::Exists(Box::new(sub.clone()))
    }

    /// `expr IN (subquery)`。`expr` 通常是外层表的列（`User::col_id().expr()`）。
    pub fn in_subquery(expr: Expr, sub: &crate::stmt::SelectStmt) -> Expr {
        Expr::InSubquery {
            expr: Box::new(expr),
            sub: Box::new(sub.clone()),
            negated: false,
        }
    }

    /// `expr NOT IN (subquery)`。
    pub fn not_in_subquery(expr: Expr, sub: &crate::stmt::SelectStmt) -> Expr {
        Expr::InSubquery {
            expr: Box::new(expr),
            sub: Box::new(sub.clone()),
            negated: true,
        }
    }

    // ---- 窗口函数 ----

    /// 给任意表达式套上窗口规格。已经是 `OVER` 的表达式会被**替换**规格而不是叠加，
    /// 所以 `Expr::row_number().over(spec)` 不会渲染成 `ROW_NUMBER() OVER () OVER (…)`。
    pub fn over(self, w: Window) -> Expr {
        match self {
            Expr::Over { func, .. } => Expr::Over { func, window: w },
            other => Expr::Over {
                func: Box::new(other),
                window: w,
            },
        }
    }

    /// `ROW_NUMBER()` —— 默认 `OVER ()`，需要排序请自己 `.over(…)`。
    pub fn row_number() -> Expr {
        Expr::func("ROW_NUMBER", vec![]).over(Window::new())
    }

    /// `RANK()`：并列同名次，名次会有空洞（1,1,3）。
    pub fn rank() -> Expr {
        Expr::func("RANK", vec![]).over(Window::new())
    }

    /// `DENSE_RANK()`：并列同名次，名次不跳号（1,1,2）。
    pub fn dense_rank() -> Expr {
        Expr::func("DENSE_RANK", vec![]).over(Window::new())
    }

    /// `NTILE(n)`：把窗口切成 n 桶。n 是参数，所以走占位符而不是字面量。
    pub fn ntile(n: i64) -> Expr {
        Expr::func("NTILE", vec![Expr::lit(n)]).over(Window::new())
    }

    /// `LAG(expr, offset)`：取前 offset 行。不提供 `LAG` 的默认值参数——
    /// 各家对第三参的支持与 NULL 处理不一致，先不建模。
    pub fn lag(e: Expr, offset: i64) -> Expr {
        Expr::func("LAG", vec![e, Expr::lit(offset)]).over(Window::new())
    }

    /// `LEAD(expr, offset)`：取后 offset 行。
    pub fn lead(e: Expr, offset: i64) -> Expr {
        Expr::func("LEAD", vec![e, Expr::lit(offset)]).over(Window::new())
    }

    /// `FIRST_VALUE(expr)`。
    pub fn first_value(e: Expr) -> Expr {
        Expr::func("FIRST_VALUE", vec![e]).over(Window::new())
    }

    /// `LAST_VALUE(expr)`。
    ///
    /// 注意：默认帧是 `RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW`，
    /// 所以它取的是"到当前行为止的最后一行"，不是整个分区。要整分区请显式
    /// `.rows_between(FrameBound::UnboundedPreceding, FrameBound::UnboundedFollowing)`。
    pub fn last_value(e: Expr) -> Expr {
        Expr::func("LAST_VALUE", vec![e]).over(Window::new())
    }

    pub fn as_(self, alias: &'static str) -> Expr {
        Expr::alias(self, alias)
    }

    /// `self AND other`
    pub fn and_(self, other: Expr) -> Expr {
        Expr::and(vec![self, other])
    }

    /// `self OR other`
    pub fn or_(self, other: Expr) -> Expr {
        Expr::or(vec![self, other])
    }

    /// `NOT self`
    pub fn not_(self) -> Expr {
        Expr::not(self)
    }

    /// 优先级，见模块文档的表格。
    pub fn precedence(&self) -> u8 {
        match self {
            Expr::Or(_) => 1,
            Expr::And(_) => 2,
            Expr::Not(_) => 3,
            Expr::Cmp { .. }
            | Expr::Like { .. }
            | Expr::IsNull { .. }
            | Expr::InList { .. }
            | Expr::InSubquery { .. }
            | Expr::Between { .. } => 4,
            // EXISTS 是谓词整体，和比较同级；它自带括号，不需要再包一层
            Expr::Exists(_) => 4,
            Expr::Arith { op, .. } => match op {
                ArithOp::Add | ArithOp::Sub => 5,
                ArithOp::Mul | ArithOp::Div | ArithOp::Mod => 6,
            },
            Expr::Neg(_) => 7,
            Expr::Col(..)
            | Expr::Val(_)
            | Expr::Wildcard
            | Expr::Paren(_)
            | Expr::Over { .. }
            | Expr::Func { .. }
            | Expr::Case { .. }
            | Expr::Cast { .. }
            | Expr::Alias { .. } => 8,
        }
    }

    /// 这个表达式里有没有窗口函数。
    ///
    /// 用来提前拦住"把窗口写进 `WHERE` / `HAVING`"这类**语法上非法**的查询：
    /// 各库的报错五花八门（SQLite `misuse of window function`，MySQL
    /// `Window functions are not allowed in WHERE`），在渲染阶段就告警比让用户
    /// 去读引擎错误更省事。
    pub fn has_window(&self) -> bool {
        match self {
            Expr::Over { .. } => true,
            Expr::Cmp { left, right, .. } => left.has_window() || right.has_window(),
            Expr::And(v) | Expr::Or(v) => v.iter().any(Expr::has_window),
            Expr::Not(x) | Expr::Paren(x) | Expr::Neg(x) => x.has_window(),
            Expr::Like { expr, .. } | Expr::IsNull { expr, .. } => expr.has_window(),
            Expr::InList { expr, .. } | Expr::Between { expr, .. } => expr.has_window(),
            Expr::Arith { left, right, .. } => left.has_window() || right.has_window(),
            Expr::Func { args, .. } => args.iter().any(Expr::has_window),
            Expr::Case {
                operand,
                whens,
                otherwise,
            } => {
                operand.as_deref().is_some_and(Expr::has_window)
                    || whens.iter().any(|(a, b)| a.has_window() || b.has_window())
                    || otherwise.as_deref().is_some_and(Expr::has_window)
            }
            Expr::Cast { expr, .. } => expr.has_window(),
            Expr::Alias { expr, .. } => expr.has_window(),
            Expr::Col(..) | Expr::Val(_) | Expr::Wildcard => false,
            // 子查询里的窗口是子查询自己的事（在它自己的 SELECT 里合法）
            Expr::Exists(_) | Expr::InSubquery { .. } => false,
        }
    }

    /// 这个表达式会不会产生参数。用于"`WHERE` 全空就整段省略"之类的判断。
    pub fn has_param(&self) -> bool {
        match self {
            Expr::Val(_) => true,
            Expr::Col(..) | Expr::Wildcard => false,
            Expr::Cmp { left, right, .. } => left.has_param() || right.has_param(),
            Expr::And(v) | Expr::Or(v) => v.iter().any(Expr::has_param),
            Expr::Not(x) | Expr::Paren(x) | Expr::Neg(x) => x.has_param(),
            Expr::Like { expr, .. } | Expr::IsNull { expr, .. } => expr.has_param(),
            Expr::InList { expr, values, .. } => expr.has_param() || !values.is_empty(),
            Expr::Between { expr, .. } => expr.has_param(),
            Expr::Arith { left, right, .. } => left.has_param() || right.has_param(),
            Expr::Func { args, .. } => args.iter().any(Expr::has_param),
            Expr::Case {
                operand,
                whens,
                otherwise,
            } => {
                operand.as_deref().is_some_and(Expr::has_param)
                    || whens.iter().any(|(a, b)| a.has_param() || b.has_param())
                    || otherwise.as_deref().is_some_and(Expr::has_param)
            }
            Expr::Cast { expr, .. } => expr.has_param(),
            Expr::Alias { expr, .. } => expr.has_param(),
            // 子查询里到底有没有参数，要 `build()` 之后才知道。这里保守返回 true：
            // 调用方用它决定"要不要省略 WHERE"，宁可多渲染一个恒真条件也不要不该省略时省略。
            Expr::Exists(_) | Expr::InSubquery { .. } => true,
            // 窗口内部可能带参数（`NTILE(?)` / `LAG(x, ?)`），分区与排序里也可能有
            Expr::Over { func, window } => {
                func.has_param()
                    || window.partition_by.iter().any(Expr::has_param)
                    || window.order_by.iter().any(|(e, _)| e.has_param())
            }
        }
    }
}

// `Expr::lit` 靠 `From<T> for Value` 接受常见 Rust 标量，
// 具体实现在 value.rs（那里是值模型的归属地）。
