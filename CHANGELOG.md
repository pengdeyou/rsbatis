# Changelog

本项目的包各自独立发版（Core 的语法层比门面的 API 稳定得多，没必要绑在一起），
所以下面按**发布批次**记录，每批列出涉及的包版本。日期均为 2026-11-25（首日）。

## 0.1.0-alpha.29 — 多对多关系（**尚未发布**，与 alpha.28 同批待发）

### Added

- `ManyToMany<C>` 描述符 + `Session::load_many_to_many`：关联表 `IN` → 子表 `IN`，
  3 次查询拿全场（父 1 + 关联表 1 + 子表 1），与父/子数量无关
- 描述符列归属校验：`link_parent` / `link_child` 必须是关联表列、`child_col` 必须是子表列，
  不一致直接报错并指明是哪一列（不做校验时写错只会返回空集合）

### Semantics

- 父的 key 不在关联表 → 空 `Vec`；指向已删除子的关联 → 静默跳过；同一对出现两次 →
  **不去重**（悄悄去重会掩盖脏数据）
- 关联表零命中时不发第二次查询
- 重复父输入仍各自拿到子集合（`get` + `cloned`，不是 `remove`）
- 反向关联不提供 `reversed()`，对调关联表两列再建描述符即可

### Fixed（测试）

- `window_mysql::top_n_per_group_mysql` 是 flaky：外层只按 `user_id` 排序，
  同组内 `amount` 顺序由查询计划决定（MariaDB 上曾把 100 和 200 排反）。
  补第二排序键 `amount DESC`

## 0.1.0-alpha.28 — 集合运算收齐 + 递归 CTE（**尚未发布**）

六个包本地都在 alpha.28，仍等门面限流窗口（预计北京时间 2026-11-26 10:28 起释放）。
alpha.26 / alpha.27 两个号**从未单独发布**，内容已并入本批，所以 crates.io 上会从
alpha.20（门面）/ alpha.25（内层）直接跳到 alpha.28。

### Added

- `SelectStmt::union(&sub)` / `union_all(&sub)`，可链式多段；`UnionKind` 枚举
- `SelectStmt::intersect(&sub)` / `except(&sub)`：交集与差集，补齐集合运算
- `SelectStmt::recursive_cte(name, &base, &step)`：`WITH RECURSIVE name AS (base UNION ALL step)`
- `SelectStmt::no_table()`：不带 `FROM` 的语句（`SELECT 1 AS n`），递归 CTE 基础段必需
- `SelectStmt::from_cte(name)`：以 CTE 名作 `FROM` 源，列引用带限定名
- `Expr::bare_col(name)`：裸列引用（不带表名）。复合查询顶层的 `ORDER BY` 只能用裸列名或别名
- `Cte` 结构（含 `recursive` 标记），`push_ctes` 支持子句级 `WITH RECURSIVE`
- `build_count` 在 `UNION` / `INTERSECT` / `EXCEPT` 存在时包成子查询

### Safety（都是"不报错但结果错"的场景）

- **`WHERE` 只作用于第一段时告警**：`.union_all(&b).and_where(…)` 的过滤只写进第一段，
  其余段不受约束 —— 结果是"多出来的行"，不会报错。提示改用 `from_subquery` 外层过滤
- **段内 `ORDER BY` / `LIMIT` 不渲染**（`UNION (SELECT … ORDER BY …)` 在 SQLite 非法），
  丢弃并告警，提示用派生表
- **列数不一致告警**：写明"本段 N 列 / 另一段 M 列"，不让用户去猜引擎那句
  "each SELECT must have the same number of columns"
- **`INTERSECT` 与 `UNION`/`EXCEPT` 混用告警（跨引擎语义差异）**：
  SQLite 三者同级、左结合 → `(A UNION B) INTERSECT C`；
  MySQL/MariaDB 遵循 SQL 标准 → `A UNION (B INTERSECT C)`。
  **同一条 SQL 在两个引擎上结果不同**（实测：SQLite 返回 `3`，MariaDB 返回 `1,2,3,3,4`）。
  段无法用括号隔开（SQLite 语法错误），所以只能告警并建议拆派生表

### Semantics

- `EXCEPT` / `INTERSECT` 与 `UNION` 共用同一套段渲染与校验路径（含列数校验、count 包装）；
  只提供不带 `ALL` 的形态（SQLite 不支持 `INTERSECT ALL`）
- `RECURSIVE` 只出现在 `WITH` 之后一次，不在单个 CTE 上（混用普通 CTE 时用
  `matches("RECURSIVE").count() == 1` 钉住）
- 递归段不能带 `ORDER BY` / `LIMIT`（沿用 UNION 的丢弃 + 告警规则）
- 递归 CTE 的 `build_count` 不包子查询：UNION 在 CTE 体内，外层 `COUNT(*)` 数的就是展开行数；
  `paginate` 在递归结果上页数正确（7 行每页 3 行 → 3 页）

### Fixed

- 基础段若用 `SelectStmt::new(cte_name)` 会渲染成 `SELECT … FROM <同名表>`，即"从递归 CTE
  自己里读种子" —— 这是 `no_table()` 存在的原因，用错时 SQLite 报错、MySQL 可能返回空集
- 顶层 `ORDER BY` 在复合查询上强制不带表名前缀（`ORDER BY t.col` 在复合查询上非法）
- `force_qualify` 不穿透复合查询各段，避免"第一段带前缀、第二段裸列"的风格不一致

### Notes

- 段内 `GROUP BY` / `HAVING` 是合法的（作用域就在该段），不告警

## 0.1.0-alpha.25 — 窗口函数（**门面未发布**）

内层五包已发 alpha.25；门面 `rsbatis` 仍是 alpha.20（限流窗口未恢复，见文末）。

### Added

- `Expr::Over { func, window }` + `Window { partition_by, order_by, frame }` +
  `Frame` / `FrameBound`
- 便捷构造器：`row_number()` / `rank()` / `dense_rank()` / `ntile(n)` / `lag(e, off)` /
  `lead(e, off)` / `first_value(e)` / `last_value(e)`；任意 `Expr` 可 `.over(w)`
- `Window::rows_cumulative()`（累计和）、`rows_between(start, end)`
- `Expr::has_window()`：递归判断表达式里是否含窗口

### Changed

- `.over()` 对已是 `Over` 的表达式是**替换规格**，不会渲染出嵌套的 `OVER () OVER (…)`

### Safety

- `build()` / `build_count()` 检测 `WHERE` / `HAVING` 里的窗口并写入 `Sql::warnings`：
  这两处出现窗口是语法非法（SQLite `misuse of window function`），提示改派生表 + 外层过滤

### Notes

- 帧只建模 `ROWS`，不做 `RANGE`（重复值/浮点下边界语义随方言漂移）
- `LAST_VALUE` 默认帧只到当前行，整分区需显式 `UnboundedFollowing`
- MySQL 的窗口 `SUM(BIGINT)` 同样返回 DECIMAL，取整数要 `Expr::cast(…, "INTEGER")`
- 最低版本：SQLite 3.25 / MySQL 8.0 / MariaDB 10.2（不做版本探测）

## 0.1.0-alpha.24 — 写入宏 update! / delete!

内层五包（core/macros/driver/orm/migrate）已发 alpha.24；**门面 `rsbatis` 仍是 alpha.20**
（24h 发布额度被门面自己占满，见文末"发布状态"）。

### Added

- `update!(User { age: 25 } where User.age >= 18)`：段语法的批量更新。字段清单里的每个
  字段变成 `<E>::col_<field>()`，值走 `impl IntoValue<T>`，所以类型错误是编译期的
- `delete!(User where User.age < 0)`：段语法的批量删除；`delete!(User)` 是全表删除
  （合法、危险、显式）
- 值位置接受任意 Rust 表达式（`update!(User { age: bump + 5 } …)`）
- 宏层 14 个单元测试：把展开函数入参从 `proc_macro::TokenStream` 改为 `proc_macro2`，
  错误分支（空字段、小写实体、缺冒号、多余 token）才可在测试里构造
- trybuild 诊断基线新增 2 例：`update_empty_fields` / `update_unknown_field`

### Notes

- **不支持方法位宏**：`.where_!(expr)` 是非法 Rust 语法（宏只能在表达式位）。条件请写
  `.where_(where_!(…))` 或走 `select! { User where … }` 段语法。文档已同步修正
- `update!(User { aeg: 1 })` 的 `E0599` 会附一条 rustc 拼写建议，提示 `col_aeg`——那是
  内部方法名，宏里应当写裸字段名。这条建议无法由宏消除
- `delete!` / `update!` 暂不支持 `order_by` / `limit`，出现多余 token 会明确报错而不是忽略

## 0.1.0-alpha.23 — 分组聚合（**尚未发布**）

与 alpha.22 同一批未发布改动（crates.io 门面 24h 额度未恢复）。

### Added

- `SelectStmt::group_by(expr)` / `group_by_all(iter)` / `having(expr)`
- 聚合构造器：`Expr::count(e)` / `count_star()` / `sum` / `avg` / `min` / `max`
- `SelectStmt::join_table(kind, table, on)` / `join_table_as(kind, table, alias, on)`：
  不给实体类型也能 join（`join<E: Entity>` 的表名来自元数据，core 层用不了）
- `SortOrder`：`Order` 的别名。`Order` 在交易类项目里几乎一定被实体占用，一起 `use` 会撞名

### Fixed

- **`build_count` 的分组语义**：有 `GROUP BY` 时 `COUNT(*)` 数的是组数而非行数，改为
  `SELECT COUNT(*) FROM (原查询) AS "__rsbatis_count"`；并丢弃内层 `ORDER BY`/`LIMIT`/`OFFSET`
  （总数不该被用户设的截断影响）。`paginate` 在分组查询上由此得到正确页数
- **`SELECT DISTINCT` 误渲染**：实现 `GROUP BY` 时这个判断被误写进"渲染 DISTINCT 关键字"处，
  导致所有分组查询变成 `SELECT DISTINCT … GROUP BY …`（语义变化）。测试抓到后修正
- **`CAST(... AS INTEGER)` 在 MySQL 无效**：MySQL 不接受 `INTEGER` 作 CAST 目标，
  渲染时映射为 `SIGNED`，`Expr::cast(x, "INTEGER")` 三库通用

### Notes（真实方言行为，不是缺陷）

- MySQL/MariaDB 的 `SUM(BIGINT)` 返回 **DECIMAL**（字符串），驱动严格回读成 `i64` 会报
  `TypeMismatch`，不静默转换（DECIMAL→i64 会丢精度）。要读整数用 `CAST(... AS INTEGER)`
- `COUNT(*)` 与 `COUNT(col)` 语义不同：`LEFT JOIN` 下前者给 1、后者给 0（不数 NULL）

## 0.1.0-alpha.22 — 派生表与 CTE（**尚未发布**）

代码已完成、测试与示例全绿（275 例），但**版本号未 bump、未上传**：crates.io 对
`rsbatis` 门面的 24 小时版本数额度已用尽（alpha.21 卡在 429）。内层包同样未升版。
下个发布窗口统一发布 `core`/`driver`/`orm`/`migrate`/`macros` + 门面。

### Added

- `SelectStmt::from_subquery(&sub, alias)`：`FROM (子查询) AS alias`（派生表）
- `SelectStmt::join_subquery(kind, &sub, alias, on)`：`JOIN (子查询) AS alias ON …`
- `SelectStmt::cte(name, &sub)`：`WITH name AS (子查询)`，可链式追加多个
- `Join` 增加 `sub: Option<Box<SelectStmt>>`，连接源与物理表共用同一结构

### Semantics

- **参数顺序三段式**：`WITH` → `FROM/JOIN` 子查询 → `WHERE`，全部共用同一个参数缓冲，
  占位符编号连续
- 派生表把 `table` 设成别名，`column("x")` 自动是 `alias.x`；列集合未知，投影给 `*`
- CTE 引用（`is_cte_source()`）同样强制表名限定，多 CTE / 混物理表时不歧义
- `build_count` 一并渲染 `WITH`（否则 COUNT 引用的表名不存在）

## 0.1.0-alpha.21 — 子查询（EXISTS / IN / NOT IN）

`core` alpha.13 · `macros` alpha.13 · `driver` alpha.13 · `orm` alpha.13 · `migrate` alpha.7 ·
`rsbatis` **alpha.21 待发**（crates.io 单 crate 24h 版本数限流 429，需等窗口）

### Added

- `Expr::exists(&sub)` / `Expr::in_subquery(expr, &sub)` / `Expr::not_in_subquery(expr, &sub)`
- `Column::in_subquery(&sub)` / `Column::not_in_subquery(&sub)`（列上直接调，最常用形态）
- `Expr::Exists(Box<SelectStmt>)` 与 `Expr::InSubquery{expr, sub, negated}` 两个变体，
  优先级与 `IN`/比较同级（4）
- `SelectStmt::render_body(d, out, params, warnings, force_qualify)`：子查询与外层共享
  输出缓冲与参数列表，**占位符编号连续**（不再有内层从 `?` 重新开始导致整体错位）

### Semantics

- 子查询渲染强制带表名限定（`force_qualify = true`）：相关子查询里引用外层列时，
  不会被解析成子查询自己表的同名列
- `NOT IN` 保持 SQL 标准语义：集合含 `NULL` 时返回零行。不自动过滤 `NULL`——那会改变查询含义

### Warnings

- MySQL/MariaDB 拒绝 `IN (子查询 … LIMIT/OFFSET)`（服务端错误 1235）。`build()` 时通过
  `Sql::warnings` 提前提示，建议改用 `EXISTS` 或派生表

## 0.1.0-alpha.20 — upsert 与 MySQL do-nothing 修复

`rsbatis` alpha.20 · `core` alpha.12 · `macros` alpha.13 · `driver` alpha.12 · `orm` alpha.12 · `migrate` alpha.6

### Added

- `InsertStmt::on_conflict_do_update(target, columns)`：冲突时用本次插入值覆盖这些列
  （SQLite/PG → `SET c = excluded.c`；MySQL → `SET c = VALUES(c)`）
- `InsertStmt::on_conflict_do_update_with(target, sets)`：每列可选 `ConflictSet::KeepInserted`（用插入值）
  或 `ConflictSet::Value(v)`（写常量，额外占一个参数位）
- `Entity::PRIMARY_KEY`（默认空数组）+ 字段属性 `#[rsbatis(primary_key)]`
- `Session::save` / `Transaction::save`：对象式 upsert，按主键写入，冲突时覆盖其余已给值的列

### Fixed

- MySQL 上 `on_conflict_do_nothing` 此前只是**告警后忽略**，等于没实现（唯一键冲突照样抛错）。
  现渲染为 `ON DUPLICATE KEY UPDATE <首列> = <首列>`：真正的空操作，且不像 `INSERT IGNORE`
  那样顺手吞掉类型截断等其他错误。

### Semantics

- 主键未声明时 `save` 返回 `Unsupported`（拒绝猜一列当主键）
- 主键为 `None` → 退化为普通插入（无冲突对象，交给数据库生成主键）
- 主键列不进 `SET`；`None` 字段既不进 `INSERT` 也不进 `SET`（不会把已有值冲成 NULL）
- 只有主键值时退化为 do-nothing，避免空 `SET` 子句
- MySQL 冲突更新返回 `affected_rows = 2`（插 1 / 更新 2 / 未变 0），调用方勿当作"插入了 2 行"

## 0.1.0-alpha.19 — feature 组合修复与覆盖率补齐

`rsbatis` alpha.19 · `core` alpha.11 · `macros` alpha.12 · `driver` alpha.11 · `orm` alpha.11 · `migrate` alpha.5

### Fixed

- **feature 组合缺陷**：`core` 单独开启 `chrono`（而未开 `driver/chrono`）时，`rsbatis-driver` 的
  `Value::DateTime` 匹配非穷尽，导致**整个 crate 编译失败**。Cargo 的 feature 在依赖图上全局合并，
  所以消费者只要写 `rsbatis-core = { features = ["chrono"] }` 就会踩到，且报错出现在他并未直接依赖的 crate 里。
  修法：SQLite 与 MySQL 的 `to_param` 各加一条兜底分支并 panic 指向缺失的 feature
  （不静默写 NULL——配置错必须炸得响亮）。

### Tests

- 新增 `crates/rsbatis-core/tests/value_model.rs`（12 例）：`Value` 的 `debug_literal`/`type_name`/
  `Display` 全变体、`From` 七个实现、`i32→i64` 与 `f32→f64` 无损提升、四种文本/字节入口、
  `Option` 委派、chrono 微秒字面量
- `rsbatis-driver/src/lib.rs` 内联 5 例：钉住 `Driver` 默认实现"拒绝而非假装成功"
  （`begin`/`commit`/`rollback` 默认返回 `Unsupported`、`in_transaction` 默认 `false`）

### Coverage

行覆盖合计 **79.7%**（补测前 77.4%）：core **85.9%**、orm **80.0%**、migrate 88.2%、
macros 74.9%、driver **77.6%**。重点：`rsbatis-driver/src/lib.rs` 0% → **95.0%**，
`rsbatis-core/src/value.rs` 51.5% → **97.0%**。测试总数 191 → 206，全绿。

## 0.1.0-alpha.18 — 分页与两种写入形态

`rsbatis` alpha.18 · `core` alpha.11 · `macros` alpha.12 · `driver` alpha.10 · `orm` alpha.10 · `migrate` alpha.4

### Added

- 分页：`PageRequest` / `Page<T>`（`total` / `pages()` / `has_next()` / `first_index()`）+
  `Session::paginate`；`SelectStmt::build_count` 与 `build` 共用 FROM/JOIN/WHERE 渲染，
  避免 count 与列表条件漂移
- 对象式插入：`Session::add` / `add_all`（`Transaction` 同款转发）+ `Entity::column_values`，
  由 `#[derive(Entity)]` 生成；`Option::None` 字段**不进 INSERT**（用数据库默认值，不写 NULL）
- 字面量式插入：`insert! { User { name: "张三", age: 20 } }`，列名走 `col_<field>()`，
  因此 `#[rsbatis(column = "...")]` 重命名同样生效

### Notes

- `page = 0` / `size = 0` 明确报错，不静默纠正成第一页/默认页长
- `offset >= total`（翻过末页）不再发第二次查询（有计数驱动断言）
- `DISTINCT` 查询的 count 走子查询：`COUNT(*) + DISTINCT` 在 SQL 里不合法
- 分页**不隐式包事务**：需要强一致时调用方自己开事务，隔离级别由调用方决定

### Coverage

首个覆盖率基线（`cargo-llvm-cov 0.9.2`，行覆盖）：core **81.6%**、orm **80.0%**、
driver 72.7%、macros 74.9%、migrate 88.2%，合计 **77.4%**。

## 0.1.0-alpha.17 — 质量门槛、文档与发布工程

`rsbatis` alpha.17 · `core` alpha.10 · `macros` alpha.11 · `driver` alpha.9 · `orm` alpha.9 · `migrate` alpha.3

- 别名联表的**元组映射前缀**改为从投影顺序推断：`select! { User as u join Order as o ... }`
  现在能直接映射成 `Vec<(User, Order)>`；此前会报"结果集中没有列 `users__id`"
  （`tests/alias_tuple.rs` 锁死，含"别名 + 裸表名混合"与"无别名"两条旧路径）
- CI 收紧：`cargo fmt --check`、`clippy -- -D warnings`（含默认 feature 一轮）、
  SQLite + MySQL 8 双库测试、无数据库 feature 一轮
- 仓库根 `README.md` / `LICENSE-MIT` / `LICENSE-APACHE` / `rust-toolchain.toml`（锁 1.100.0，
  trybuild 快照依赖 rustc 版本）
- 四个可运行示例与 README 示例一一对应，由 CI 编译把关
- `scripts/bump-version.sh` / `scripts/publish.sh` 把发版流程沉淀成脚本
- 各 crate README 补成真实用法（此前多为"占位、无 API"的模板，与事实不符）
- 依赖版本漂移修正：`rsbatis-driver` / `rsbatis-migrate` 声明的 `rsbatis-core`、
  `rsbatis-migrate` 声明的 `rsbatis-orm` 曾落后于上游已发布版本

## 0.1.0-alpha.16 — 关系与预加载

`rsbatis` alpha.16 · `core` alpha.9 · `macros` alpha.10 · `driver` alpha.8 · `orm` alpha.8 · `migrate` alpha.2

### Added

- `HasMany<C>` / `BelongsTo<P>` 关系描述符
- `Session::load_has_many`（selectin：一条 `IN` 查询，与父数量无关；空父集合不发查询）
- `Session::load_belongs_to`（缺失的父返回 `None`，不丢子行）
- `Session::load_joined_has_many`（单次 JOIN + 父去重）
- 测试内 `Counting<D>` 驱动包装，把"不是 N+1"从说法变成断言

### Fixed

- `BelongsTo` 初版缺 `parent_col`，会拿子表外键去查父表（同名列存在时静默查错）
- `HashMap::remove` 让第二个引用同一父的子拿到 `None`（父被第一个子"消费"掉了）

## 0.1.0-alpha.15 — 表别名

`rsbatis` alpha.15 · `core` alpha.9 · `orm` alpha.8

- `Column::of_table(alias)`、`SelectStmt::from_alias` / `join_as` / `cross_join_as`
- 宏语法 `select! { Emp as e join Emp as m on e.manager_id == m.id }`（别名强制小写开头）
- 自连接改用 `from_row_prefix` 映射；元组映射仍明确拒绝

## 0.1.0-alpha.14 / migrate alpha.2 — 迁移工具

`rsbatis-migrate` alpha.2

- `Migration` / `from_dir`（`<版本>_<名字>.sql` + `.down.sql`）、`Migrator`（`up` / `up_to` / `down` /
  `down_to` / `status`）、元数据表 `_rsbatis_migrations`
- `supports_transactional_ddl()`：MySQL 的 DDL 隐式提交如实上报

### Fixed

- MySQL 多语句批处理此前会**静默吞掉中间语句的错误并返回成功**；改为按状态机切分、
  逐条执行逐条检查，错误信息带"第 N 条语句失败"，并断言失败后后续语句不再执行

## 0.1.0-alpha.12 / driver alpha.8 / orm alpha.8 — 执行层

- `Session<D>`：`execute` / `execute_batch` / `fetch` / `fetch_one` / `fetch_all` / `transaction`
- `Transaction<'_, D>` RAII：`Drop` 未提交即回滚，禁止二次提交与嵌套
- `Pool<D>` + `PooledConn`：自研池，池满报错不阻塞，归还前检查未提交事务并回滚
- `#[derive(FromRow)]`、元组 `(A, B)` / `(A, B, C)` 映射
- `chrono::NaiveDateTime` 支持（SQLite 存定长微秒文本，字典序等于时间序）

## 0.1.0-alpha.10 / 11 / 9 — 宏与 Core

- `where_!`（syn 版，替换 268 行手写扫描器）：三态 `Col` / `Expr` / `Val`，只有"左列右值"
  才走 `Column::ge(impl IntoValue<T>)`，从而保住类型检查；`null` → `IS NULL`
- `select! { [distinct] Entity [join ...] [where] [order_by] [limit] [offset] }`
- `#[derive(Entity)]`：`TABLE` / `COLUMNS` / `NULLABLE` + 每字段 `col_<name>()`
- Core：`Expr` 含 `Paren`、`Sql::warnings`、空 `IN` → `0 = 1`

### Fixed

- `select!` 段关键字识别遇 Ident 就无条件切段，表里有列叫 `limit` / `order_by` 时查询被截断

## 0.1.0-alpha.1 — 占名

七个包名全部占位发布：`rsbatis`、`rsbatis-core`、`rsbatis-macros`、`rsbatis-driver`、
`rsbatis-orm`、`rsbatis-migrate`、`rsboot`。`rsbatis` 与 `rsbatis-macros` 的 alpha.1 已 yank
（宏展开路径 `::rsbatis_core::` 在下游不可见，门面补 `extern crate self as rsbatis;` 后修好）。

## 截至 alpha.24 的发布状态

| 包 | crates.io 最新 | 说明 |
|---|---|---|
| `rsbatis-core` / `-macros` / `-driver` / `-orm` / `-migrate` | **0.1.0-alpha.25** | 已发布（alpha.26/27 改动在本地，未发） |
| `rsbatis`（门面） | **0.1.0-alpha.20** | 24h 额度 429 连续卡住 alpha.21–alpha.27 七批 |

门面一旦恢复额度，一次发布即可把四批改动全部带出去（内层已是 alpha.24，门面
`alpha.24` 已指向它们）。在此之前，消费者通过 `rsbatis = "0.1.0-alpha.20"` 拿不到
子查询、派生表、CTE、分组聚合、`update!`/`delete!`——可用性由 workspace 内 389 个测试
与 9 个示例（走 path 依赖）保证。
