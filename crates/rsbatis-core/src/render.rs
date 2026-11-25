//! SQL 渲染器。
//!
//! 唯一的职责：把 `Expr` 变成文本 + 参数列表。**不做**方言判断（问 `Dialect`），
//! **不做**语义改写（那是 AST 构造期的事），**不做**缓存。
//!
//! 括号策略：
//! 1. 子表达式优先级低于父要求的 `min_prec` → 补括号（保证语义）；
//! 2. `Expr::Paren` → 无条件输出括号（保证可读性，E1 F1 的修复）；
//! 3. `NOT` 后接比较 → 补括号（E1 F3 的修复，可读性）。

use crate::dialect::Dialect;
use crate::expr::Expr;
use crate::value::Value;

/// 渲染一个表达式。
///
/// `min_prec` 是"父节点能接受的最低优先级"，低于它的子表达式会被括号包起来。
pub fn render_expr(
    e: &Expr,
    d: Dialect,
    out: &mut String,
    params: &mut Vec<Value>,
    warnings: &mut Vec<String>,
    min_prec: u8,
) {
    render_expr_q(e, d, out, params, warnings, min_prec, false)
}

/// 渲染表达式，并决定列引用是否带表名限定。
///
/// `qualify = true` 时 `"users"."age"`，否则 `"age"`。
/// 为什么要这个开关而不是无脑加表名：单表查询里 `WHERE "age" >= ?` 更短、更符合
/// 手写 SQL 的习惯，也让已有 golden 基线保持稳定；只有联表时歧义才真的存在。
pub(crate) fn render_expr_q(
    e: &Expr,
    d: Dialect,
    out: &mut String,
    params: &mut Vec<Value>,
    warnings: &mut Vec<String>,
    min_prec: u8,
    qualify: bool,
) {
    let p = e.precedence();
    let need_paren = p < min_prec;
    if need_paren {
        out.push('(');
    }

    match e {
        Expr::Col(table, name) => {
            // 空表名 = 裸列引用（`Expr::bare_col`）。`UNION` 的顶层 `WHERE` / `ORDER BY`
            // 只能用裸列名或别名，带表名的引用在复合查询上非法。
            if qualify && !table.is_empty() {
                out.push_str(&d.quote(table));
                out.push('.');
            }
            out.push_str(&d.quote(name));
        }

        Expr::Wildcard => out.push('*'),

        Expr::Val(v) => {
            params.push(v.clone());
            out.push_str(&d.placeholder(params.len()));
        }

        Expr::Cmp { op, left, right } => {
            // 左结合：左侧允许同优先级，右侧要求更高，避免 `a = b = c` 的歧义
            render_expr_q(left, d, out, params, warnings, 4, qualify);
            out.push(' ');
            out.push_str(op.as_sql());
            out.push(' ');
            render_expr_q(right, d, out, params, warnings, 5, qualify);
        }

        Expr::And(items) => {
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(" AND ");
                }
                render_expr_q(x, d, out, params, warnings, 2, qualify);
            }
        }

        Expr::Or(items) => {
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(" OR ");
                }
                render_expr_q(x, d, out, params, warnings, 1, qualify);
            }
        }

        Expr::Not(x) => {
            out.push_str("NOT ");
            // 要求原子级（8）以上就不加括号；比较（4）会被包起来 → `NOT ("a" >= ?)`
            render_expr_q(x, d, out, params, warnings, 5, qualify);
        }

        Expr::Paren(x) => {
            out.push('(');
            render_expr_q(x, d, out, params, warnings, 0, qualify);
            out.push(')');
        }

        Expr::Like {
            expr,
            pattern,
            negated,
        } => {
            render_expr_q(expr, d, out, params, warnings, 4, qualify);
            out.push_str(if *negated { " NOT LIKE " } else { " LIKE " });
            params.push(Value::Text(pattern.clone()));
            out.push_str(&d.placeholder(params.len()));
        }

        Expr::IsNull { expr, negated } => {
            render_expr_q(expr, d, out, params, warnings, 4, qualify);
            out.push_str(if *negated { " IS NOT NULL" } else { " IS NULL" });
        }

        Expr::InList {
            expr,
            values,
            negated,
        } => {
            if values.is_empty() {
                // 空集合没有任何方言支持 `IN ()`。用恒真/恒假式保持语义，
                // 而不是悄悄丢掉这个条件（丢掉会让 `NOT IN ()` 变成全表命中，是安全事故）。
                out.push_str(if *negated { "1 = 1" } else { "0 = 1" });
            } else {
                render_expr_q(expr, d, out, params, warnings, 4, qualify);
                out.push_str(if *negated { " NOT IN (" } else { " IN (" });
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    params.push(v.clone());
                    out.push_str(&d.placeholder(params.len()));
                }
                out.push(')');
            }
        }

        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => {
            render_expr_q(expr, d, out, params, warnings, 4, qualify);
            out.push_str(if *negated {
                " NOT BETWEEN "
            } else {
                " BETWEEN "
            });
            params.push(low.clone());
            out.push_str(&d.placeholder(params.len()));
            out.push_str(" AND ");
            params.push(high.clone());
            out.push_str(&d.placeholder(params.len()));
        }

        Expr::Arith { op, left, right } => {
            render_expr_q(left, d, out, params, warnings, p, qualify);
            out.push(' ');
            out.push_str(op.as_sql());
            out.push(' ');
            render_expr_q(right, d, out, params, warnings, p + 1, qualify);
        }

        Expr::Neg(x) => {
            out.push('-');
            render_expr_q(x, d, out, params, warnings, 8, qualify);
        }

        Expr::Func { name, args } => {
            // SQL 函数名一律大写：多数方言不区分大小写，但日志/审计里统一更好读，
            // 也避免同一函数在两条 SQL 里出现 `lower` / `LOWER` 两种写法。
            out.push_str(&name.to_uppercase());
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_expr_q(a, d, out, params, warnings, 0, qualify);
            }
            out.push(')');
        }

        Expr::Over { func, window } => {
            // `OVER` 是后缀，优先级按原子处理：`SUM(x) OVER (…) + 1` 里
            // 整个窗口表达式必须先算完再参与算术，括号由 `OVER (…)` 自带。
            render_expr_q(func, d, out, params, warnings, 8, qualify);
            out.push_str(" OVER (");
            render_window(window, d, out, params, warnings, qualify);
            out.push(')');
        }

        Expr::Case {
            operand,
            whens,
            otherwise,
        } => {
            out.push_str("CASE");
            if let Some(op) = operand {
                out.push(' ');
                render_expr_q(op, d, out, params, warnings, 8, qualify);
            }
            for (cond, then) in whens {
                out.push_str(" WHEN ");
                render_expr_q(cond, d, out, params, warnings, 0, qualify);
                out.push_str(" THEN ");
                render_expr_q(then, d, out, params, warnings, 0, qualify);
            }
            if let Some(e) = otherwise {
                out.push_str(" ELSE ");
                render_expr_q(e, d, out, params, warnings, 0, qualify);
            }
            out.push_str(" END");
        }

        Expr::Cast { expr, ty } => {
            out.push_str("CAST(");
            render_expr_q(expr, d, out, params, warnings, 0, qualify);
            out.push_str(" AS ");
            // `INTEGER` 在 MySQL 里不是合法的 CAST 目标类型（要写 `SIGNED`）。
            // 把这一种常见类型名做方言映射，`Expr::cast(x, "INTEGER")` 就能三库通用。
            // 起因是真实踩到的：MySQL 的 `SUM(BIGINT)` 返回 DECIMAL（字符串），
            // 想读成 i64 必须 cast，而 cast 的类型名各库不同。
            let ty = if d == Dialect::MySql && *ty == "INTEGER" {
                "SIGNED"
            } else {
                ty
            };
            out.push_str(ty);
            out.push(')');
        }

        Expr::Exists(sub) => {
            // 子查询与外层共享 `out` / `params` / `warnings`：占位符编号接在外层之后，
            // 告警也不会丢。`force_qualify = true` 见 render_body 的注释。
            out.push_str("EXISTS (");
            sub.render_body(d, out, params, warnings, true);
            out.push(')');
        }

        Expr::InSubquery { expr, sub, negated } => {
            // MariaDB / MySQL 直接拒绝 `x IN (SELECT ... LIMIT n)`：
            //   1235 This version of MariaDB doesn't yet support
            //        'LIMIT & IN/ALL/ANY/SOME subquery'
            // 这是数据库侧的限制，改不了 SQL 生成能绕过去，只能提前告诉调用方。
            if d == Dialect::MySql && sub.has_limit_or_offset() {
                warnings.push(
                    "MySQL/MariaDB 不支持 `IN (子查询 ... LIMIT/OFFSET)`（错误 1235）；\
                     请改用 EXISTS，或在子查询外包一层派生表"
                        .to_string(),
                );
            }
            render_expr_q(expr, d, out, params, warnings, 4, qualify);
            out.push_str(if *negated { " NOT IN (" } else { " IN (" });
            sub.render_body(d, out, params, warnings, true);
            out.push(')');
        }

        Expr::Alias { expr, alias } => {
            render_expr_q(expr, d, out, params, warnings, 8, qualify);
            out.push_str(" AS ");
            out.push_str(&d.quote(alias));
        }
    }

    if need_paren {
        out.push(')');
    }
}

/// 渲染 `OVER (…)` 括号**内部**的内容（不含括号本身）。
///
/// 子句顺序固定 `PARTITION BY` → `ORDER BY` → frame，这是 SQL 语法要求；
/// 参数按这个顺序入列，PG 的 `$N` 才不会错位。
pub(crate) fn render_window(
    w: &crate::expr::Window,
    d: Dialect,
    out: &mut String,
    params: &mut Vec<Value>,
    warnings: &mut Vec<String>,
    qualify: bool,
) {
    let mut parts: Vec<String> = Vec::new();

    if !w.partition_by.is_empty() {
        let mut buf = String::from("PARTITION BY ");
        for (i, e) in w.partition_by.iter().enumerate() {
            if i > 0 {
                buf.push_str(", ");
            }
            render_expr_q(e, d, &mut buf, params, warnings, 0, qualify);
        }
        parts.push(buf);
    }

    if !w.order_by.is_empty() {
        let mut buf = String::from("ORDER BY ");
        for (i, (e, o)) in w.order_by.iter().enumerate() {
            if i > 0 {
                buf.push_str(", ");
            }
            render_expr_q(e, d, &mut buf, params, warnings, 0, qualify);
            buf.push(' ');
            buf.push_str(o.as_sql());
        }
        parts.push(buf);
    }

    if let Some(f) = &w.frame {
        parts.push(format!(
            "ROWS BETWEEN {} AND {}",
            f.start.as_sql(),
            f.end.as_sql()
        ));
    }

    out.push_str(&parts.join(" "));
}
