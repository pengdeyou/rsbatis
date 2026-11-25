# rsbatis-core

The SQLAlchemy-Core-like layer of [rsbatis](https://crates.io/crates/rsbatis): expression AST,
column proxies, dialects and SQL rendering. **No database connection, no macros, no third-party
dependencies** by default — this is the part you can unit-test without a database.

```rust
use rsbatis_core::{select, Column, Dialect, Entity};

struct User;
impl Entity for User {
    const TABLE: &'static str = "users";
    const COLUMNS: &'static [&'static str] = &["id", "age"];
}

let age = Column::<i32>::new(User::TABLE, "age");
let sql = select::<User>().where_(age.ge(18)).build(Dialect::Sqlite);

assert_eq!(sql.text, r#"SELECT * FROM "users" WHERE "age" >= ?"#);
```

What lives here:

- `Expr` — 20 nodes including `Paren`, so precedence survives parsing instead of being
  reconstructed by a printer
- `Column<T>` — a `Copy` value carrying table, name and an optional alias; `.ge()`, `.eq()`,
  `.like()`, `.between()`, `.in_()` keep the value type checked (`Column<i32>.ge("x")` does not
  compile)
- `Dialect` — SQLite / MySQL (PostgreSQL is planned): identifier quoting, placeholders, `RETURNING`
- `Sql::warnings` — a dialect downgrade is reported, never silently dropped
- statement builders: `select` / `insert` / `update` / `delete` (DML included), joins, aliases
- `SelectStmt::build_count` — `COUNT(*)` reusing the same FROM/JOIN/WHERE, so pagination totals
  cannot drift from the list conditions

Empty `IN` renders `0 = 1` and empty `NOT IN` renders `1 = 1` — the two cases where a naive
implementation yields a syntax error or, worse, an always-true predicate.

In practice you depend on `rsbatis` rather than this crate directly: the `Entity` impls and
`col_<field>()` helpers are produced by `rsbatis-macros`, and drivers live in `rsbatis-driver`.

## License

MIT OR Apache-2.0.
