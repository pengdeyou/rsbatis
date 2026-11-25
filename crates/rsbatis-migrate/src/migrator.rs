//! 迁移执行器。

use std::collections::HashSet;

use rsbatis_core::Dialect;
use rsbatis_driver::Driver;
use rsbatis_orm::Session;

use crate::error::MigrateError;
use crate::migration::Migration;
use crate::store;

/// 一支迁移的状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub version: i64,
    pub name: String,
    pub applied: bool,
}

pub struct Migrator<'a, D: Driver> {
    session: &'a mut Session<D>,
    migrations: Vec<Migration>,
}

impl<'a, D: Driver> Migrator<'a, D> {
    /// 构造时做两件事：校验版本唯一、确保元数据表存在。
    ///
    /// 版本重复必须当场拒绝——两个迁移共用版本号意味着"应用顺序"没有确定答案，
    /// 不同机器上可能跑出不同结果。
    pub fn new(
        session: &'a mut Session<D>,
        mut migrations: Vec<Migration>,
    ) -> Result<Self, MigrateError> {
        migrations.sort_by_key(|m| m.version);
        for w in migrations.windows(2) {
            if w[0].version == w[1].version {
                return Err(MigrateError::Invalid(format!(
                    "版本 {} 重复（`{}` 与 `{}`）",
                    w[0].version, w[0].name, w[1].name
                )));
            }
        }
        store::ensure(session)?;
        Ok(Self {
            session,
            migrations,
        })
    }

    /// 该方言的 DDL 是否可参与事务回滚。
    ///
    /// MySQL 的 DDL 会隐式提交，迁移中途失败时已执行的语句不会回滚——
    /// 这不是我们的取舍，是数据库本身的限制。调用方可以据此决定要不要在
    /// 生产环境先备份。
    pub fn supports_transactional_ddl(&self) -> bool {
        self.session.dialect() != Dialect::MySql
    }

    /// 临时取回会话（查表验证、读迁移后的数据等）。持有借用期间不能再用 `Migrator`。
    pub fn session_mut(&mut self) -> &mut Session<D> {
        self.session
    }

    pub fn status(&mut self) -> Result<Vec<Status>, MigrateError> {
        let applied: HashSet<i64> = store::applied(self.session)?
            .into_iter()
            .map(|a| a.version)
            .collect();
        Ok(self
            .migrations
            .iter()
            .map(|m| Status {
                version: m.version,
                name: m.name.clone(),
                applied: applied.contains(&m.version),
            })
            .collect())
    }

    pub fn applied_versions(&mut self) -> Result<Vec<i64>, MigrateError> {
        Ok(store::applied(self.session)?
            .into_iter()
            .map(|a| a.version)
            .collect())
    }

    /// 应用所有未应用的迁移，返回实际执行的版本号。
    pub fn up(&mut self) -> Result<Vec<i64>, MigrateError> {
        self.up_to(i64::MAX)
    }

    /// 应用到指定版本（含）。
    pub fn up_to(&mut self, target: i64) -> Result<Vec<i64>, MigrateError> {
        let applied: HashSet<i64> = store::applied(self.session)?
            .into_iter()
            .map(|a| a.version)
            .collect();
        let mut done = Vec::new();
        for m in self.migrations.clone() {
            if applied.contains(&m.version) || m.version > target {
                continue;
            }
            self.apply(&m)?;
            done.push(m.version);
        }
        Ok(done)
    }

    /// 回滚最后一支（版本号最大的已应用迁移）。
    pub fn down(&mut self) -> Result<Option<i64>, MigrateError> {
        let Some(last) = store::applied(self.session)?.pop() else {
            return Ok(None);
        };
        let m = self
            .migrations
            .iter()
            .find(|m| m.version == last.version)
            .cloned()
            .ok_or_else(|| {
                MigrateError::Invalid(format!(
                    "数据库记录了版本 {}，但它不在当前迁移列表里——\
                     检查是不是切到了旧代码分支",
                    last.version
                ))
            })?;
        let down = m.down.clone().ok_or_else(|| {
            MigrateError::Invalid(format!(
                "迁移 {}（{}）没有 down 脚本，无法回滚",
                m.version, m.name
            ))
        })?;

        let d = self.session.dialect();
        let mut tx = self.session.transaction()?;
        tx.execute_batch(&down)?;
        tx.execute(store::record_down_sql(d, m.version))?;
        tx.commit()?;
        Ok(Some(m.version))
    }

    /// 回滚到指定版本（该版本本身保留）。返回被回滚的版本号，从新到旧。
    pub fn down_to(&mut self, target: i64) -> Result<Vec<i64>, MigrateError> {
        let mut out = Vec::new();
        loop {
            let applied = store::applied(self.session)?;
            let Some(max) = applied
                .iter()
                .map(|a| a.version)
                .filter(|v| *v > target)
                .max()
            else {
                break;
            };
            self.down()?;
            out.push(max);
        }
        Ok(out)
    }

    fn apply(&mut self, m: &Migration) -> Result<(), MigrateError> {
        if !m.has_up_statements() {
            // 只有注释的 up 如果照常执行，元数据会记成"已应用"，而数据库里什么都没建。
            // 之后每次 up 都会跳过它——最难查的一类问题。
            return Err(MigrateError::Invalid(format!(
                "迁移 {}（{}）里没有语句（只有注释）。请写 SQL，或删掉这支迁移",
                m.version, m.name
            )));
        }
        let d = self.session.dialect();
        let mut tx = self.session.transaction()?;
        tx.execute_batch(&m.up)?;
        // 元数据写入与迁移 SQL 在同一个事务里：宁可两边都不生效，
        // 也不要出现"表建好了但没记账"，那会让下一次 up 重复执行。
        tx.execute(store::record_up_sql(d, m.version, &m.name))?;
        tx.commit()?;
        Ok(())
    }
}

impl<D: Driver> std::fmt::Debug for Migrator<'_, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Migrator({} 支迁移)", self.migrations.len())
    }
}
