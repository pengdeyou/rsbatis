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
column is `E0599` with rustc's own "did you mean `age`?" hint, and a wrong value type is
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
    pub age: Option<i32>,   // nullable column; age() is still Column<i32>
}
```

- `#[rsbatis(table = "...")]` names the table. Omitted, it falls back to the struct name in
  `snake_case`, which is usually not what you want for a plural table.
- `#[rsbatis(primary_key)]` marks the key. Without it `save()` refuses to run (it cannot guess an
  upsert conflict target) and `insert()` cannot use `RETURNING`.
- **Auto-increment keys are `Option<i64>`, not `i64`.** `None` means "let the database assign it";
  `Some(v)` inserts `v` explicitly. With a bare `i64` you must always supply a value, and `0`
  will be inserted as a real key.
- Nullable columns are `Option<T>`; the generated `x()` is still `Column<T>`, so predicates
- The column accessor takes the field name verbatim (`User::age()`), so a field that
  shadows an `Entity`/`FromRow` item (`table`, `from_row`, ...) is rejected at derive time
  never have to deal with `Option`.
- Field types are checked against a whitelist (`i8..i64`, `u8..u64`, `f32`, `f64`, `bool`,
  `String`, `Vec<u8>`, `NaiveDateTime` with the `chrono` feature). Anything else is a compile
  error, not a runtime `TypeMismatch` three layers down.
- `#[rsbatis(column = "...")]` overrides a column name; there is no "ignore this field" yet, so
  the struct must mirror the table.

`Entity` generates the column accessors (`User::id()`, `User::name()`, …).
`FromRow` generates the row decoder. Both are needed: the first feeds `filter!` and the builders,
the second feeds `fetch_all` / `fetch_one_as` / `paginate`.

## Queries

The dialect is a property of the connection, not of the statement: `Session` reads it from the
driver it was opened with, so `db.execute(stmt)` / `db.fetch_all(stmt)` render on their own and you
never pass `Dialect` at a call site. `stmt.build(Dialect)` is still there for the one case that
needs it — you want the SQL *text* (printing, logging, a migration file, an assertion). For that,
`db.explain(&stmt)` renders using the connection's dialect without consuming the statement, so the
same statement can be explained and then executed.

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
let stmt = select::<User>().order_by(User::id().expr(), rsbatis::Order::Asc);
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
let q = select::<User>().filter(filter!(
    User.age.between(18, 30) || User.name.in_(["李四"])
));
```

The builder method is `filter`, not `where`: `where` is a reserved Rust keyword and cannot be an
identifier (writing `r#where` is legal but forces `r#` at every call site). The `select!` grammar
keeps the SQL-looking `where` because macro input is raw tokens and never parsed as an identifier.

`filter!` only rewrites tokens into `Expr` builder calls; type checking stays with `rustc`. A
misspelled column is `E0599` with rustc's own suggestion, a wrong literal type is `E0277`.

Available predicates: `eq`, `ne`, `gt`, `ge`, `lt`, `le`, `like`, `not_like`, `in_`, `not_in`,
`between`, `not_between`, `is_null`, `is_not_null`, `eq_col`, `in_subquery`, `not_in_subquery`.
Combine with `&&` / `||` (AND / OR) and `!` (NOT).

### Insert

```rust
// object form: values come from the model
db.add(&User { id: 4, name: "赵六".into(), age: Some(25) })?;

// generated key: the model key must be Option<i64>
let id: i64 = db.insert(&Log { id: None, msg: "started".into() })?;

// statement form: write NULL explicitly, or add ON CONFLICT
db.execute(
    &insert::<User>()
        .set(User::name(), "孙八")
        .set_null(User::age()),
)?;
```

`add` / `add_all` write every model column, including the key when it has a value, and return
the number of rows written — they never ask the engine for a key. `insert` is `add` plus
"give me the key back": same write, one extra return value.
`insert` prefers `INSERT … RETURNING` (SQLite ≥ 3.35, MariaDB); where that is unavailable
it falls back to `last_insert_id()` **and verifies the counter actually moved**, so an engine that
silently returns a stale id fails loudly instead of handing back a plausible-looking number.

### Update

```rust
// by primary key
db.execute(
    &update::<User>()
        .set(User::age(), 21)
        .filter(filter!(User.id == 1))
        ,
)?;

// by condition, hitting several rows — the return value is the affected count
let affected = db.execute(
    &update::<User>()
        .set(User::age(), 18)
        .filter(filter!(User.age < 18))
        ,
)?;
```

There is no dirty tracking and no attribute setter, so an update is always explicit: you name the
columns and the predicate. `save(&model)` is the upsert path — with a declared primary key it
compiles to `ON CONFLICT DO UPDATE`, and without one it refuses rather than silently inserting a
duplicate.

### Delete

```rust
db.execute(&delete::<User>().filter(filter!(User.id == 4)))?;
// by condition: delete::<User>().filter(filter!(User.age < 0))
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
let rel = HasMany::new(Order::user_id(), |o: &Order| o.user_id);
let per_user: Vec<Vec<Order>> = db.load_has_many(&rel, &users, |u| u.id)?;
```

Parents with no children get an empty `Vec`, not an error.

### Many-to-one

```rust
let rel = BelongsTo::new(Order::user_id(), User::id(), |u: &User| u.id);
let authors: Vec<Option<User>> = db.load_belongs_to(&rel, &orders, |o| o.user_id)?;
```

A missing parent yields `None`; the child is never dropped and the call never panics.

### One-to-one

`HasOne` returns `Option<C>` per parent. It costs the same as `load_has_many` — one `IN` query
for the whole batch — because SQL cannot `LIMIT 1` per parent in a single round trip; the
"first one" is chosen in memory.

```rust
let rel = HasOne::new(Order::user_id(), |o: &Order| o.user_id);
let order_per_user: Vec<Option<Order>> = db.load_has_one(&rel, &users, |u| u.id)?;
```

It relies on the data actually being one-to-one. If a foreign key holds several rows, which
one you get is decided by the database's row order — that is a silent, unspecified answer.
When you need "the specific one of several", write an explicit `select!` with a filter instead
of using this descriptor.

Forward direction, when the parent may or may not exist, uses `BelongsTo`.

### Many-to-many

```rust
let rel = ManyToMany::new(
    "enrollments",                    // link table
    Enrollment::student_id(),     // link column pointing at the parent
    Enrollment::course_id(),      // link column pointing at the child
    Course::id(),                 // column in the child table being referenced
    |c: &Course| c.id,
);
let courses: Vec<Vec<Course>> = db.load_many_to_many(&rel, &students, |s| s.id)?;
```

All four columns are required on purpose. Three of them live in two different tables, so a wrong
one does not fail — it silently returns empty sets. The loader validates the table names before
querying instead of trusting the descriptor.

## Generated ids (snowflake)

A database sequence cannot give you the id before the `INSERT`: you either pay a second
round trip (`RETURNING`) or model the key as `Option<i64>`. `Snowflake` moves uniqueness
to the application, so the key is a plain `i64` set before the write — and it is roughly
time-ordered, which keeps B-tree inserts at the right edge of the index.

```text
63      62                22        12         0
+--------+-----------------+---------+----------+
| 0 (sign)| timestamp (41) | node(10) | seq (12) |
+--------+-----------------+---------+----------+
```

41 bits of milliseconds last until 2109, 10 bits give 1024 instances, 12 bits give
4096 ids per millisecond per instance (~4.1M/s single node).

Declare it once on the entity, and the id is filled at insert time:

```rust
#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "events")]
pub struct Event {
    #[rsbatis(primary_key, snowflake)]
    pub id: i64,          // plain i64, not Option<i64>
    pub name: String,
}

let id: i64 = db.insert(&Event { id: 0, name: "created".into() })?;  // 0 means "generate"
```

`id = 0` is the "not given" marker: 0 is not a valid snowflake id (the timestamp bits are
never zero), so a real key and a "please generate one" request cannot be confused. A non-zero
key is written as-is — importing rows from another system keeps its ids intact.

You can still call `Snowflake::global()` directly when you need the id *before* the insert
(building a batch, referencing it in a second table within the same transaction):

```rust
let sf = Snowflake::global();          // node id: RSBATIS_NODE_ID, else hostname+pid hash
let row = Event { id: sf.next()?, name: "created".into() };
db.add(&row)?;                          // id is i64 — no RETURNING, no Option
```

The generator is lock-free: timestamp, node and sequence live in one `u64` updated by a
single CAS, so contention degrades to "retry with the next sequence number" rather than
to a queue behind a mutex.

Three things it does not solve for you:

1. **Node ids must be unique.** `global()` falls back to a `hostname + pid` hash, which
   only prevents the worst default (every instance being node 0). In production set
   `RSBATIS_NODE_ID` explicitly per replica — two machines claiming node 3 will collide
   inside the same millisecond.
2. **Clock going backwards.** A backward jump of ≤5 ms is absorbed by waiting for the
   previously issued millisecond; a larger jump returns
   `IdError::ClockMovedBackwards` instead of guessing. A failed insert is better than a
   duplicate primary key.
3. **`i64` above 2^53.** These ids are 19 decimal digits and lose precision in
   JavaScript. Serialize them as strings in JSON — that is the tax every snowflake
   scheme pays, not a defect of this one.

Why not the alternatives: auto-increment needs `Option<i64>` or a second round trip and
breaks across sharded tables; UUIDv4 is random (bad for index locality) and 16 bytes;
UUIDv7 is time-ordered and easier, but still 16 bytes and needs `BINARY(16)` handling in
MySQL. If you are already on `i64` keys, snowflake is the shape that costs nothing extra.

## Crates

| Crate | Role |
| --- | --- |
| `rsbatis` | facade: re-exports everything, feature-gated (`sqlite` / `mysql` / `chrono` / `migrate` / `orm`) |
| `rsbatis-core` | expression AST, `Column<T>`, dialects, SQL rendering (no I/O, no macros, zero deps by default) |
| `rsbatis-macros` | `filter!`, `select!`, `#[derive(Entity)]`, `#[derive(FromRow)]` |
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

## License

MIT OR Apache-2.0.
