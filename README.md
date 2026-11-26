<p align="center">
  <img src="docs/rsbatis-logo.svg" alt="rsbatis" width="200">
</p>

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

## Defining an entity

```rust
use rsbatis::{Entity, FromRow};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "users")]
pub struct User {
    #[rsbatis(primary_key)]
    pub id: i64,
    pub name: String,
    pub age: Option<i32>,   // nullable column; col_age() is still Column<i32>
}
```

- `#[rsbatis(table = "...")]` names the table. Omitted, it falls back to the struct name in
  `snake_case`, which is usually not what you want for a plural table.
- `#[rsbatis(primary_key)]` marks the key. Without it `save()` refuses to run (it cannot guess an
  upsert conflict target) and `insert_one_id()` cannot use `RETURNING`.
- **Auto-increment keys are `Option<i64>`, not `i64`.** `None` means "let the database assign it";
  `Some(v)` inserts `v` explicitly. With a bare `i64` you must always supply a value, and `0`
  will be inserted as a real key.
- Nullable columns are `Option<T>`; the generated `col_x()` is still `Column<T>`, so predicates
  never have to deal with `Option`.
- Field types are checked against a whitelist (`i8..i64`, `u8..u64`, `f32`, `f64`, `bool`,
  `String`, `Vec<u8>`, `NaiveDateTime` with the `chrono` feature). Anything else is a compile
  error, not a runtime `TypeMismatch` three layers down.
- `#[rsbatis(column = "...")]` overrides a column name; there is no "ignore this field" yet, so
  the struct must mirror the table.

`Entity` generates the column accessors (`User::col_id()`, `User::col_name()`, …).
`FromRow` generates the row decoder. Both are needed: the first feeds `where_!` and the builders,
the second feeds `fetch_all` / `fetch_one_as` / `paginate`.

## Queries

Every snippet below is a line of [`examples/queries.rs`](crates/rsbatis/examples/queries.rs), which
runs in CI. If the README and the example disagree, the example is what actually compiles.

### Fetch one by primary key

```rust
let one: Option<User> = db.fetch_one_as(&select! { User where User.id == 1 })?;
```

`Option`, not `Result`: a missing row is `None`, not an error. Use `fetch_one_as` when you need
the typed model, `fetch_one` when you want the raw `Row`.

### Fetch a list

```rust
let all: Vec<User> = db.fetch_all(&select! { User order_by User.id asc })?;
```

### Pagination

```rust
let stmt = select::<User>().order_by(User::col_id().expr(), rsbatis::Order::Asc);
let page: rsbatis::Page<User> = db.paginate(&stmt, PageRequest::new(1, 2))?;  // page 1, size 2
```

`paginate` runs `COUNT(*)` then fetches the page. When the requested offset is past `total` it
**skips the second query** and returns empty `items` with the correct `total` — the common case
when a UI walks past the last page. `Page` carries `items`, `total`, `page`, `size`, plus
`pages()`, `has_next()`, `is_empty()`.

### Predicates

```rust
// macros: the whole query in one call
let adults: Vec<User> = db.fetch_all(&select! {
    User where User.age >= 18 && User.name.like("张%")
})?;

// builder + expression macro, when the query is assembled piecemeal
let q = select::<User>().where_(where_!(
    User.age.between(18, 30) || User.name.in_(["李四"])
));
```

`where_!` only rewrites tokens into `Expr` builder calls; type checking stays with `rustc`. A
misspelled column is `E0599` with rustc's own suggestion, a wrong literal type is `E0277`.

Available predicates: `eq`, `ne`, `gt`, `ge`, `lt`, `le`, `like`, `not_like`, `in_`, `not_in`,
`between`, `not_between`, `is_null`, `is_not_null`, `eq_col`, `in_subquery`, `not_in_subquery`.
Combine with `&&` / `||` (AND / OR) and `!` (NOT).

### Insert

```rust
// object form: values come from the model
db.add(&User { id: 4, name: "赵六".into(), age: Some(25) })?;

// generated key: the model key must be Option<i64>
let id: i64 = db.insert_one_id(&Log { id: None, msg: "started".into() })?;

// statement form: write NULL explicitly, or add ON CONFLICT
db.execute(
    insert::<User>()
        .set(User::col_name(), "孙八")
        .set_null(User::col_age())
        .build(Dialect::Sqlite),
)?;
```

`add` / `add_all` write every model column, including the key when it has a value.
`insert_one_id` prefers `INSERT … RETURNING` (SQLite ≥ 3.35, MariaDB); where that is unavailable
it falls back to `last_insert_id()` **and verifies the counter actually moved**, so an engine that
silently returns a stale id fails loudly instead of handing back a plausible-looking number.

### Update

```rust
// by primary key
db.execute(
    update::<User>()
        .set(User::col_age(), 21)
        .where_(where_!(User.id == 1))
        .build(Dialect::Sqlite),
)?;

// by condition, hitting several rows — the return value is the affected count
let affected = db.execute(
    update::<User>()
        .set(User::col_age(), 18)
        .where_(where_!(User.age < 18))
        .build(Dialect::Sqlite),
)?;
```

There is no dirty tracking and no attribute setter, so an update is always explicit: you name the
columns and the predicate. `save(&model)` is the upsert path — with a declared primary key it
compiles to `ON CONFLICT DO UPDATE`, and without one it refuses rather than silently inserting a
duplicate.

### Delete

```rust
db.execute(delete::<User>().where_(where_!(User.id == 4)).build(Dialect::Sqlite))?;
// by condition: delete::<User>().where_(where_!(User.age < 0))
```

### Inner join

```rust
let pairs: Vec<(User, Order)> = db.fetch_all(&select! {
    User as u join Order as o on o.user_id == u.id
    where u.id == 1 order_by o.amount desc
})?;
```

Once a join is present every column is projected as `"u"."id" AS "u__id"` and the tuple decoder
maps it back by prefix. Without a join the projection stays `SELECT *`. Aliases are optional —
`(User, Order)` works either way — but a self-join cannot be mapped into a tuple, because both
sides would share the prefix.

### Left join

```rust
#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "orders")]
struct OrderOpt { id: Option<i64>, user_id: Option<i64>, amount: Option<i64> }

let left: Vec<(User, OrderOpt)> = db.fetch_all(&select! {
    User as u left_join OrderOpt as o on o.user_id == u.id
    where u.id <= 3
})?;
```

The right side may be absent, so its model has to be all-`Option` — a non-optional `i64` will
fail to decode on the first unmatched row. `LEFT JOIN` does not de-duplicate parent rows; that is
the relation loader's job, not the join's.

## Relations

No lazy loading: Rust cannot intercept `user.posts`, so a relation is always an explicit call.
What the loader guarantees is **no N+1** — one `IN` query per child table, asserted in tests, not
promised in prose.

### One-to-many

```rust
let rel = HasMany::new(Order::col_user_id(), |o: &Order| o.user_id);
let per_user: Vec<Vec<Order>> = db.load_has_many(&rel, &users, |u| u.id)?;
```

Parents with no children get an empty `Vec`, not an error.

### Many-to-one

```rust
let rel = BelongsTo::new(Order::col_user_id(), User::col_id(), |u: &User| u.id);
let authors: Vec<Option<User>> = db.load_belongs_to(&rel, &orders, |o| o.user_id)?;
```

A missing parent yields `None`; the child is never dropped and the call never panics.

### One-to-one

There is no dedicated `HasOne`. A one-to-one is either `BelongsTo` when only one row can match,
or `HasMany` plus `.first()` on the parent side:

```rust
let rel = HasMany::new(Order::col_user_id(), |o: &Order| o.user_id);
let per_user = db.load_has_many(&rel, &users, |u| u.id)?;
let first_order = per_user[0].first();
```

### Many-to-many

```rust
let rel = ManyToMany::new(
    "enrollments",                    // link table
    Enrollment::col_student_id(),     // link column pointing at the parent
    Enrollment::col_course_id(),      // link column pointing at the child
    Course::col_id(),                 // column in the child table being referenced
    |c: &Course| c.id,
);
let courses: Vec<Vec<Course>> = db.load_many_to_many(&rel, &students, |s| s.id)?;
```

All four columns are required on purpose. Three of them live in two different tables, so a wrong
one does not fail — it silently returns empty sets. The loader validates the table names before
querying instead of trusting the descriptor.

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
  rsbatis/           src/lib.rs  examples/{quick_start,queries,join_and_alias,relations,migrations}.rs
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
