//! `FromRow`：把驱动读回的一行转成模型。
//!
//! 这个 trait 故意只做"逐列按名取值"这一件事，不做类型推断：
//! 列名与字段名的对应关系、每列用什么 Rust 类型，全部由 `#[derive(FromRow)]`
//! 在编译期定死，因此运行时唯一的失败模式是"列不存在"或"类型对不上"，
//! 两者都会带列名返回。

use rsbatis_core::Entity;
use rsbatis_driver::{DriverError, ErrorKind, Row};

/// 从一行构造模型。
pub trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, DriverError>;

    /// 按 `前缀__列名` 读取，用于联表时的自动投影（列名形如 `users__id`）。
    ///
    /// 默认实现退回裸列名：单表模型不需要关心前缀，只有联表参与者才覆盖它。
    /// `#[derive(FromRow)]` 会同时生成两个方法。
    fn from_row_prefix(row: &Row, prefix: &str) -> Result<Self, DriverError> {
        let _ = prefix;
        Self::from_row(row)
    }
}

/// 便利实现：`Row` 自己就是"原样返回"。
impl FromRow for Row {
    fn from_row(row: &Row) -> Result<Self, DriverError> {
        Ok(row.clone())
    }
}

/// 从结果集的列名推断投影前缀，按出现顺序去重。
///
/// 联表投影渲染成 `"u"."id" AS "u__id"`，所以列名里 `__` 之前那段就是别名（或表名）。
/// 顺序即投影顺序（主表在前、join 表依次在后），正好对应元组元素的顺序。
///
/// 为什么不直接用 `Entity::TABLE`：一旦查询里用了别名（`User as u`），投影前缀是 `u`，
/// 而 `TABLE` 是 `users`，元组会去找并不存在的 `users__id` 列。这个错误只有在用别名
/// 时才会出现，且报错信息是"结果集中没有列 users__id"，看上去像用户的查询写错了。
fn projection_prefixes(row: &Row) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in row.columns() {
        if let Some((p, _)) = c.split_once("__") {
            if !out.iter().any(|x| x == p) {
                out.push(p.to_string());
            }
        }
    }
    out
}

/// 取第 `idx` 个前缀；没有前缀时回退到 `fallback`（无 join 的 `SELECT *` 场景）。
fn prefix_at(row: &Row, idx: usize, fallback: &'static str) -> String {
    projection_prefixes(row)
        .get(idx)
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

/// 两表联表 → 元组。列按 `<前缀>__<列名>` 分别取回，前缀取自结果集的投影顺序。
///
/// 这是在 Rust 里能给出的、最接近 SQLAlchemy `select(User, Order)` 返回元的形态：
/// 位置固定、类型固定、不需要用户手写投影和别名。
impl<A, B> FromRow for (A, B)
where
    A: FromRow + Entity,
    B: FromRow + Entity,
{
    fn from_row(row: &Row) -> Result<Self, DriverError> {
        reject_self_join::<A, B>()?;
        let pa = prefix_at(row, 0, A::TABLE);
        let pb = prefix_at(row, 1, B::TABLE);
        Ok((A::from_row_prefix(row, &pa)?, B::from_row_prefix(row, &pb)?))
    }
}

impl<A, B, C> FromRow for (A, B, C)
where
    A: FromRow + Entity,
    B: FromRow + Entity,
    C: FromRow + Entity,
{
    fn from_row(row: &Row) -> Result<Self, DriverError> {
        reject_self_join::<A, B>()?;
        reject_self_join::<A, C>()?;
        reject_self_join::<B, C>()?;
        let pa = prefix_at(row, 0, A::TABLE);
        let pb = prefix_at(row, 1, B::TABLE);
        let pc = prefix_at(row, 2, C::TABLE);
        Ok((
            A::from_row_prefix(row, &pa)?,
            B::from_row_prefix(row, &pb)?,
            C::from_row_prefix(row, &pc)?,
        ))
    }
}

/// 自连接无法用元组映射：两张"表"的列别名完全相同，取出来的会是同一组值。
/// 与其静默返回错数据，不如明确拒绝，让用户改用显式投影 + 自定义模型。
fn reject_self_join<A: Entity, B: Entity>() -> Result<(), DriverError> {
    if A::TABLE == B::TABLE {
        return Err(DriverError::new(
            ErrorKind::Unsupported,
            format!(
                "自连接（{0} 与 {0}）不能用元组映射：列别名会完全重复。\
                 请显式投影并自定义 FromRow 模型",
                A::TABLE
            ),
        ));
    }
    Ok(())
}
