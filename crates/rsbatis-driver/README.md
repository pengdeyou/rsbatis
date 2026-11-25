# rsbatis-driver

Query execution layer of [rsbatis](https://crates.io/crates/rsbatis): a `Driver` trait plus one
implementation per database, with **runtime-shaped parameters**.

```rust
use rsbatis_core::{select, Column, Dialect, Entity};
use rsbatis_driver::{sqlite::SqliteDriver, Driver};

struct User;
impl Entity for User { const TABLE: &'static str = "users"; const COLUMNS: &'static [&'static str] = &["id", "age"]; }

let mut db = SqliteDriver::open_in_memory()?;
db.execute_batch(r#"CREATE TABLE "users" (id INTEGER PRIMARY KEY, age INTEGER)"#)?;

let rows = db.fetch(&select::<User>().where_(Column::<i32>::new(User::TABLE, "age").ge(18))
    .build(Dialect::Sqlite))?;
```

### Why not `sqlx`

`sqlx` binds parameters through a generic tuple, which means the parameter count is fixed at
compile time. An expression that only exists after macro rewriting can produce 1 argument or 5,
so the SQL has to be built as a string with a `Vec` of values — and then bound in one go. `sqlx`
cannot express that without nightly or a fixed arity. The drivers here take a `&[Value]` and bind
it positionally through `params_from_iter`.

### What is here

| Driver | Crate | Notes |
| --- | --- | --- |
| SQLite | `rusqlite` (bundled) | no system dependency; strict read-back typing |
| MySQL | `mysql` 28.x | pure-Rust protocol; `affected_rows` read from the connection, not the response |

`Driver` methods: `execute`, `fetch`, `execute_batch`, `dialect`, `supports_returning`, `begin`,
`commit`, `rollback`, `in_transaction`.

Two behaviours worth knowing:

- **Strict read-back**: an `i64` that does not fit an `i32` field reports `OutOfRange`, and NULL
  into a non-`Option` field reports `TypeMismatch`. Silent truncation is not an option.
- **Batch statements are executed one by one on MySQL**: the protocol layer used to report
  success while skipping an error in the middle of a multi-statement string, so batch execution
  splits on `;` (quote/comment aware) and checks every statement.

PostgreSQL is planned (`tokio-postgres`); it is not implemented and nothing here claims it.

## License

MIT OR Apache-2.0.
