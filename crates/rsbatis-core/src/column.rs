//! 列代理。
//!
//! `Column<T>` 里的 `T` 是**值的类型**，不是可空性。可空列在业务侧用 `Option<T>`，
//! 但在列定义上仍写非空 `T`，因为 `Column<i64>` 同时要支持 `eq(3i64)` 与
//! `eq(Option::<i64>::None)`。
//!
//! 这一层负责把"Rust 里长得像字段的东西"翻译成 `Expr`，并把类型约束压到编译期：
//! `Column<i32>` 不接受 `i64` 参数，因为没有对应 `IntoValue<i32> for i64` 实现。

use std::marker::PhantomData;

use crate::expr::{BinOp, Expr};
use crate::value::{IntoValue, Value};

#[derive(Clone, Copy)]
pub struct Column<T> {
    table: &'static str,
    name: &'static str,
    /// SQL 别名。为空时用实体表名。
    alias: Option<&'static str>,
    _ty: PhantomData<T>,
}

impl<T> Column<T> {
    pub const fn new(table: &'static str, name: &'static str) -> Self {
        Self {
            table,
            name,
            alias: None,
            _ty: PhantomData,
        }
    }

    /// 把这列绑定到 SQL 别名。
    ///
    /// 为什么必须显式绑定而不是"渲染时自动判断"：列对象是 `Copy` 的纯数据，
    /// 它不持有任何上下文；"该用哪个别名"只能由调用点决定。自连接时两张表
    /// 同名列必须指向不同别名，否则会静默取到同一个值。
    pub const fn of_table(mut self, alias: &'static str) -> Self {
        self.alias = Some(alias);
        self
    }

    /// 渲染时用的限定名：有别名用别名，否则用实体表名。
    pub const fn table(&self) -> &'static str {
        match self.alias {
            Some(a) => a,
            None => self.table,
        }
    }

    /// 实体表名（不受别名影响）。
    pub const fn base_table(&self) -> &'static str {
        self.table
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// 转成通用表达式，供 `Expr::cmp` / 函数调用等自由组合使用。
    pub fn expr(self) -> Expr {
        Expr::Col(self.table(), self.name)
    }

    fn cmp_with(self, op: BinOp, v: impl IntoValue<T>) -> Expr {
        Expr::cmp(op, self.expr(), Expr::Val(v.into_value()))
    }

    // ---- 比较 ----

    pub fn eq(self, v: impl IntoValue<T>) -> Expr {
        self.cmp_with(BinOp::Eq, v)
    }
    pub fn ne(self, v: impl IntoValue<T>) -> Expr {
        self.cmp_with(BinOp::Ne, v)
    }
    pub fn gt(self, v: impl IntoValue<T>) -> Expr {
        self.cmp_with(BinOp::Gt, v)
    }
    pub fn ge(self, v: impl IntoValue<T>) -> Expr {
        self.cmp_with(BinOp::Ge, v)
    }
    pub fn lt(self, v: impl IntoValue<T>) -> Expr {
        self.cmp_with(BinOp::Lt, v)
    }
    pub fn le(self, v: impl IntoValue<T>) -> Expr {
        self.cmp_with(BinOp::Le, v)
    }

    /// 列比列（同类型）。
    pub fn eq_col(self, other: Column<T>) -> Expr {
        Expr::cmp(BinOp::Eq, self.expr(), other.expr())
    }

    /// `本列 IN (子查询)`。例：`User::col_id().in_subquery(&orders)`。
    pub fn in_subquery(self, sub: &crate::stmt::SelectStmt) -> Expr {
        Expr::in_subquery(self.expr(), sub)
    }

    /// `本列 NOT IN (子查询)`。
    pub fn not_in_subquery(self, sub: &crate::stmt::SelectStmt) -> Expr {
        Expr::not_in_subquery(self.expr(), sub)
    }

    // ---- 模式匹配 ----

    /// `LIKE`。参数是模式串，不做转义——通配符语义交给调用方，避免"偷偷转义"
    /// 让 `%` 失效。
    pub fn like(self, pattern: impl Into<String>) -> Expr {
        Expr::Like {
            expr: Box::new(self.expr()),
            pattern: pattern.into(),
            negated: false,
        }
    }

    pub fn not_like(self, pattern: impl Into<String>) -> Expr {
        Expr::Like {
            expr: Box::new(self.expr()),
            pattern: pattern.into(),
            negated: true,
        }
    }

    // ---- NULL ----

    pub fn is_null(self) -> Expr {
        Expr::IsNull {
            expr: Box::new(self.expr()),
            negated: false,
        }
    }

    pub fn is_not_null(self) -> Expr {
        Expr::IsNull {
            expr: Box::new(self.expr()),
            negated: true,
        }
    }

    // ---- 集合 ----

    /// `IN (...)`。空集合渲染成恒假式（`0 = 1`），方言都不支持 `IN ()`。
    pub fn in_<I>(self, values: I) -> Expr
    where
        I: IntoIterator,
        I::Item: IntoValue<T>,
    {
        Expr::InList {
            expr: Box::new(self.expr()),
            values: values
                .into_iter()
                .map(IntoValue::into_value)
                .collect::<Vec<Value>>(),
            negated: false,
        }
    }

    pub fn not_in<I>(self, values: I) -> Expr
    where
        I: IntoIterator,
        I::Item: IntoValue<T>,
    {
        Expr::InList {
            expr: Box::new(self.expr()),
            values: values
                .into_iter()
                .map(IntoValue::into_value)
                .collect::<Vec<Value>>(),
            negated: true,
        }
    }

    // ---- 区间 ----

    pub fn between(self, low: impl IntoValue<T>, high: impl IntoValue<T>) -> Expr {
        Expr::Between {
            expr: Box::new(self.expr()),
            low: low.into_value(),
            high: high.into_value(),
            negated: false,
        }
    }

    pub fn not_between(self, low: impl IntoValue<T>, high: impl IntoValue<T>) -> Expr {
        Expr::Between {
            expr: Box::new(self.expr()),
            low: low.into_value(),
            high: high.into_value(),
            negated: true,
        }
    }

    // ---- 投影 ----

    /// `SELECT ... AS alias`
    pub fn as_(self, alias: &'static str) -> Expr {
        Expr::alias(self.expr(), alias)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age() -> Column<i32> {
        Column::new("user", "age")
    }

    #[test]
    fn eq_builds_cmp_expr() {
        match age().ge(18) {
            Expr::Cmp { op, .. } => assert_eq!(BinOp::Ge, op),
            _ => panic!("expected Cmp"),
        }
    }

    #[test]
    fn in_list_collects_values() {
        match age().in_([18, 19, 20]) {
            Expr::InList {
                values, negated, ..
            } => {
                assert_eq!(3, values.len());
                assert!(!negated);
                assert_eq!(Value::I32(18), values[0]);
            }
            _ => panic!("expected InList"),
        }
    }

    #[test]
    fn of_table_reroutes_qualification() {
        let c = age().of_table("u");
        assert_eq!("u", c.table());
        assert_eq!("user", c.base_table(), "实体表名不受影响");
        match c.expr() {
            Expr::Col(t, n) => {
                assert_eq!("u", t);
                assert_eq!("age", n);
            }
            _ => panic!("expected Col"),
        }
        // 别名只影响限定名，比较语义不变
        match c.ge(18) {
            Expr::Cmp {
                left,
                op: BinOp::Ge,
                ..
            } => match left.as_ref() {
                Expr::Col(t, _) => assert_eq!("u", *t),
                _ => panic!(),
            },
            _ => panic!("expected Cmp"),
        }
    }

    #[test]
    fn column_is_copy_and_const_constructible() {
        const C: Column<i64> = Column::new("t", "c");
        let a = C;
        let b = C;
        assert_eq!(a.name(), b.name());
    }
}
