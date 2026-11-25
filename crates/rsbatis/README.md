# rsbatis

A SQLAlchemy-style ORM for Rust — write queries that read like business code,
with column names and value types still checked by `rustc`.

```rust
let users: Vec<User> = db.fetch_all(&select! {
    User where User.age >= 18 order_by User.age desc limit 10
})?;
// SELECT * FROM "users" WHERE "age" >= ? ORDER BY "age" DESC LIMIT ?
```

Compare with the alternative, which is what every Rust ORM makes you write:

```rust
User::find().filter(Column::Age.gte(18)).order_by_desc(Column::Age).limit(10).all(db)?
```

## Status: `0.1.0-alpha.18` — alpha, not production-ready

### Working today

| Area | What you get |
| --- | --- |
| Entities | `#[derive(Entity)]` → `TABLE`, `COLUMNS`, `NULLABLE`, and one `col_<field>()` per field (`#[rsbatis(table = "...")]`, `#[rsbatis(column = "...")]`) |
| Query macros | `where_!` (full Rust expression tree via `syn`), `select! { User where ... order_by ... limit ... }` |
| Expressions | comparisons, `AND`/`OR`/`NOT`, explicit parentheses, `LIKE`, `IS NULL`, `IN`, `BETWEEN`, arithmetic, functions, `CASE`, `CAST` |
| Statements | `select` / `insert` / `update` / `delete`, distinct, order, limit, offset, `set_null` |
| Inserting | `insert! { User { name: "张三", age: 20 } }` (literal form) and `db.add(&user)` / `db.add_all(&users)` (object form) |
| Pagination | `db.paginate::<User>(&stmt, PageRequest::new(2, 20))` → `Page<T>` with `total` / `pages()` / `has_next()` |
| Dialects | SQLite and MySQL (quoting, placeholders, `RETURNING` / `LAST_INSERT_ID()`), PostgreSQL is *planned* |
| Execution | `SqliteDriver` (bundled), `MySqlDriver`, `Session`, `Transaction` (RAII rollback), `Pool` |
| Mapping | `#[derive(FromRow)]`, tuple mapping for joins, `from_row_prefix` for prefixed projections |
| Joins | `join` / `left_join` / `cross_join`, table aliases (`User as u join Order as o on ...`), self-join via aliases |
| Relations | `HasMany` / `BelongsTo` with selectin and joined eager loading |
| Migrations | `rsbatis-migrate`: versioned SQL files, `up`/`down`/`status`, `_rsbatis_migrations` metadata table |
| Dates | `chrono::NaiveDateTime` behind the `chrono` feature (stored as microsecond-precision text on SQLite) |

### Not implemented yet

Subqueries, CTE, window functions, `UNION`, many-to-many, upsert (`ON CONFLICT` / `ON DUPLICATE KEY`),
async API, streaming cursors, identity map, `rsboot` CLI.

Nothing above is a silent downgrade: unsupported features land in `Sql::warnings` instead of
being quietly dropped from the query.

## Quick start

```toml
[dependencies]
rsbatis = { version = "0.1.0-alpha.18", features = ["sqlite"] }
```

```rust
use rsbatis::{select, insert, Dialect, Entity, FromRow, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "users")]
pub struct User {
    pub id: i64,
    pub name: String,
    pub age: Option<i32>, // nullable column; col_age() is still Column<i32>
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Session::new(SqliteDriver::open_in_memory()?);
    db.driver_mut().execute_batch(
        r#"CREATE TABLE "users" (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER);"#,
    )?;

    db.execute(insert::<User>()
        .set(User::col_name(), "张三")
        .set(User::col_age(), 20)
        .build(Dialect::Sqlite))?;

    let adults: Vec<User> = db.fetch_all(&select! { User where User.age >= 18 })?;
    println!("{} adults", adults.len());
    Ok(())
}
```

Two calling conventions, both valid:

```rust
// 1. one macro for the whole query
let stmt = select! { User where User.age >= 18 && User.name.like("zhang%") };

// 2. builder + expression macro
let stmt = select::<User>().where_(where_!(User.age >= 18 && User.name.like("zhang%")));
```

`.where_(where_!(...))` and not `.where_!(...)`: `where` is a Rust keyword, hence the trailing
underscore, and a proc-macro is not a method — it produces a value that the builder accepts.

## Writing rows

Two shapes, same semantics for `Option` fields (`None` means "skip this column, use the database
default" — not "write NULL"):

```rust
// literal form: field names map through col_<field>(), so #[rsbatis(column = "...")] renames apply
db.execute(insert! { User { name: "张三", age: 20 } })?;

// object form: build the model, hand it to the session
let u = User { id: None, name: "李四".into(), age: None };
db.add(&u)?;          // id None → AUTOINCREMENT; age None → column omitted
```

`None` skipping the column is the whole reason `Option<i64>` for `id` is worth writing:
a literal `NULL` would defeat `AUTOINCREMENT` / `DEFAULT`. Writing an explicit NULL goes through
`insert::<User>().set_null(User::col_age())`.

## Joins and aliases

```rust
let rows: Vec<(User, Order)> = db.fetch_all(&select! {
    User as u join Order as o on o.user_id == u.id
    where u.age >= 18 order_by o.amount desc
})?;
```

Without a join the projection stays `SELECT *`; with a join every column is projected as
`"u"."id" AS "u__id"`, and the tuple mapper reads the `u__` / `o__` prefixes back. Aliases must
start with a lowercase letter — the rewriter distinguishes aliases from entity names by case.

Self-join needs aliases (`Emp as e join Emp as m on e.manager_id == m.id`); mapping into a
single struct from a self-join is refused rather than guessed.

## Relations

```rust
let rel = HasMany::new(Post::col_user_id(), |p: &Post| p.user_id);
let posts = db.load_has_many(&rel, &users, |u| u.id)?;          // Vec<Vec<Post>>, 2 queries total

let rel = BelongsTo::new(Post::col_user_id(), User::col_id(), |u: &User| u.id);
let authors = db.load_belongs_to(&rel, &posts, |p| p.user_id)?; // Vec<Option<User>>
```

`load_has_many` is a selectin: one `IN` query regardless of how many parents you pass, and no
query at all for an empty parent set. `load_joined_has_many` folds everything into a single join
and deduplicates parents. Counting drivers in the test suite assert the query counts, so
"it is not N+1" is a checked claim rather than a comment.

There is no transparent lazy loading — Rust cannot intercept field access. Loading a relation is
always an explicit method.

## Migrations

```rust
use rsbatis::{from_dir, Migrator};

let migrations = from_dir("migrations")?; // <version>_<name>.sql + <version>_<name>.down.sql
let mut m = Migrator::new(&mut session, migrations)?;
m.up()?;      // apply pending
m.status()?;  // what is applied, what is pending
```

Each migration runs in a transaction together with its metadata row. `supports_transactional_ddl()`
returns `false` on MySQL, where DDL implicitly commits — a failed multi-statement migration can
leave half-built tables behind. That is a database limitation, and the API says so instead of
pretending otherwise.

## Why a macro at all

Rust binds `>=` to `PartialOrd::ge(&self, other) -> bool`. The return type is fixed by the
language, so `User.age >= 18` can only ever be a `bool`; no operator overloading turns it into an
expression tree. The only way to get SQLAlchemy-like syntax is to rewrite the tokens before
`rustc` sees them.

The macro rewrites tokens only. Type checking still belongs to `rustc`, which is why a column typo
is `E0599` and a wrong value type is `E0277` — at compile time, on the line you wrote.

## Nullable columns and the type whitelist

`Option<T>` maps to a nullable column, but `col_age()` returns `Column<i32>`, not
`Column<Option<i32>>`: nullability must not infect comparison types, or `col_age().ge(18)` would
not compile. Nullability is exposed as `Entity::NULLABLE`; writing NULL goes through
`set_null(col_age())`.

Field types are a whitelist (`i8..i64`, `u8..u64`, `f32`, `f64`, `bool`, `String`, `Vec<u8>`,
and `Option` of those). An unmapped type fails at the field, not as a `TypeMismatch` at read time.
Reading an `i64` value that does not fit `i32` reports `OutOfRange`; reading NULL into a
non-`Option` field reports `TypeMismatch`.

## Do not rename this dependency

The macro expands to `::rsbatis::__private::...`. Proc-macros have no `$crate`, so renaming the
dependency (`rsbatis = { package = "rsbatis", ... }` under another key) breaks the expansion
path. `serde` has the same constraint.

## Name

The crates.io name `rbatis` belongs to an unrelated project (4.x, XML-based SQL). This project
publishes as `rsbatis`.

## License

MIT OR Apache-2.0.
