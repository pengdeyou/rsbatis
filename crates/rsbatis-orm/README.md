# rsbatis-orm

Session, transactions, connection pool, row mapping and eager-loading relations for
[rsbatis](https://crates.io/crates/rsbatis).

```rust
use rsbatis_orm::Session;
use rsbatis_driver::sqlite::SqliteDriver;

let mut db = Session::new(SqliteDriver::open_in_memory()?);

// RAII: dropping without commit rolls back
{
    let tx = db.transaction()?;
    tx.execute(insert_stmt)?;
    tx.commit()?;
}
```

### What is here

- `Session<D>` — `execute`, `execute_batch`, `fetch`, `first`, `all::<T>`,
  `one`, `transaction`, `in_transaction`, `driver`, `driver_mut`
- `Transaction<'_, D>` — RAII rollback on `Drop`, refuses double commit, refuses nesting
  (a nested transaction looks like a transaction but is not one; pretending otherwise would
  hide a real bug)
- `Pool<D>` — a self-contained `Mutex<Vec<D>>` pool; an exhausted pool returns an error instead of
  blocking. A connection is rolled back before it goes back to the pool if the caller left a
  transaction open.
- `Session::add` / `add_all` — object-style insert (`Entity::column_values`, `None` fields skip the column)
- `Session::paginate` — `PageRequest` / `Page<T>`: count and page in two queries, no implicit transaction
- `FromRow` — row-to-struct mapping, `from_row_prefix` for join projections (`u__id`, `o__amount`),
  and tuple impls for `(A, B)` / `(A, B, C)`
- relations — `HasMany` (selectin) and `BelongsTo` (batched parent fetch), plus
  `joined_has_many` for a single-join strategy

Relation loading is explicit. Rust cannot intercept field access, so there is no transparent
lazy loading — `user.posts` can never work without changing what `user.posts` means.

## License

MIT OR Apache-2.0.
