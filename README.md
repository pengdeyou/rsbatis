# rsbatis

A SQLAlchemy-style ORM for Rust — `select! { User where User.age >= 18 }` instead of
`filter(Column::Age.gte(18))`, with column names and value types still checked by `rustc`.

> Status: **0.1.0-alpha** — architecture is proven end to end against real SQLite and MariaDB,
> but the API is not frozen. Do not use in production yet.

## Why this exists

Rust has no operator overloading that could turn `User.age >= 18` into an expression tree:
`>=` binds to `PartialOrd::ge(&self, other) -> bool`, and the return type is fixed by the
language. The only way to get SQLAlchemy-like syntax is to rewrite the tokens **before** `rustc`
type-checks them — which is what this project does with procedural macros.

The macro rewrites and nothing else. Type checking still belongs to `rustc`, so a misspelled
column is `E0599` with rustc's own "did you mean `col_age`?" hint, and a wrong value type is
`E0277` on the literal you wrote.

## Crates

| Crate | Role |
| --- | --- |
| `rsbatis` | facade: re-exports everything, feature-gated (`sqlite` / `mysql` / `chrono` / `migrate` / `orm`) |
| `rsbatis-core` | expression AST, `Column<T>`, dialects, SQL rendering (no I/O, no macros, zero deps by default) |
| `rsbatis-macros` | `where_!`, `select!`, `#[derive(Entity)]`, `#[derive(FromRow)]` |
| `rsbatis-driver` | `Driver` trait + SQLite (`rusqlite`) and MySQL (`mysql`) implementations |
| `rsbatis-orm` | `Session`, `Transaction`, `Pool`, `FromRow`, relations |
| `rsbatis-migrate` | versioned SQL migrations: `up` / `down` / `status` |
| `rsboot` | reserved name for the planned CLI (no API yet) |

## Layout

```
crates/
  rsbatis-core/      src/{value,expr,column,dialect,render,stmt}.rs   tests/{golden,dml,join,alias}.rs
  rsbatis-macros/    src/lib.rs                                       (rewriter + derives + ui baseline)
  rsbatis-driver/    src/{sqlite,mysql_driver,sql_split,row}.rs        tests/{sqlite_crud,mysql_crud,batch_strict}.rs
  rsbatis-orm/       src/{session,from_row,pool,relation}.rs
  rsbatis-migrate/   src/{migration,migrator,store}.rs
  rsbatis/           src/lib.rs  examples/{quick_start,join_and_alias,relations,migrations}.rs
  rsboot/            reserved
scripts/             bump-version.sh, publish.sh, dev-mysql.sh
.github/workflows/   ci.yml (fmt + clippy -D warnings + tests on SQLite & MySQL 8)
```

## Build and test

```bash
# 起本地 MariaDB（幂等；把库、账号、表都重建一遍）
bash scripts/dev-mysql.sh

# 全量测试：SQLite 用例无外部依赖，MySQL 用例在没有 RSBATIS_MYSQL_URL 时自动跳过
RSBATIS_MYSQL_URL='mysql://rsbatis:rsbatis@127.0.0.1:3306/rsbatis_test' \
  cargo test --workspace \
    --features rsbatis/sqlite,rsbatis/mysql,rsbatis/chrono,rsbatis/migrate \
    -- --test-threads=1

# 编译期诊断基线（改了宏之后必须看这一条）
TRYBUILD=overwrite cargo test -p rsbatis --features sqlite --test ui_diagnostics
```

`--test-threads=1` is not caution for its own sake: several cases `DROP`/`CREATE` tables with the
same name, and parallel execution makes them step on each other.

## Publish

```bash
./scripts/bump-version.sh rsbatis-core 0.1.0-alpha.10   # 只改 [package] 段，并同步依赖它的包
./scripts/publish.sh --dry-run                           # tarball 校验
./scripts/publish.sh                                     # core → macros → driver → orm → migrate → facade
./scripts/publish.sh --from rsbatis-orm                  # 从中断处续跑
```

Order matters: publishing a downstream crate before its upstream version reaches the index fails
with "failed to find version". The script verifies every version through the crates.io API
instead of trusting the exit code.

## Known constraints (by design)

- **No transparent lazy loading.** Rust cannot intercept field access, so loading a relation is
  always an explicit call (`load_has_many` / `load_belongs_to`).
- **No transparent dirty tracking.** Attribute setters do not exist; updates go through an
  explicit `Changeset`.
- **No `sqlx` underneath.** `sqlx` binds through a generic tuple, so the parameter count must be
  known at compile time — impossible for a query whose shape is produced by macro rewriting.
- **Aliased joins + tuple mapping**: prefixes are inferred from the projection order, so
  `(User, Order)` works with or without aliases; a self-join still cannot be mapped into a tuple.
- **MySQL DDL cannot be rolled back**: DDL implicitly commits, so a migration that fails midway
  leaves partial tables. `supports_transactional_ddl()` reports `false` there.

## Design documents

`01-开发方案.md` / `02-详细设计.md` / `03-实现计划与工作量.md` and the per-milestone completion
reports live next to this repository (`04`–`19`). `CHANGELOG.md` records what shipped and when.

## License

MIT OR Apache-2.0.
