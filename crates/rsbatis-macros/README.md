# rsbatis-macros

Procedural macros for [rsbatis](https://crates.io/crates/rsbatis): token rewriting, nothing else.

```rust
// what you write
let stmt = select! { User where User.age >= 18 && User.name.like("zhang%") order_by User.age desc };

// what it becomes (rendered as SQL)
// SELECT * FROM "users" WHERE "age" >= ? AND "name" LIKE ? ORDER BY "age" DESC
```

| Macro | Purpose |
| --- | --- |
| `where_!` | rewrites a full Rust expression tree into `Expr` construction calls |
| `select!` | one macro for the whole query: `[distinct] Entity [as alias] [join ...] [where EXPR] [order_by EXPR [asc\|desc]] [limit N] [offset M]` |
| `#[derive(Entity)]` | generates `TABLE`, `COLUMNS`, `NULLABLE` and `col_<field>()` per field |
| `#[derive(FromRow)]` | generates row-to-struct mapping |
| `insert!` | `insert! { User { name: "张三", age: 20 } }` — literal insert; column names go through `col_<field>()` |

### Why this works at all

Rust binds `>=` to `PartialOrd::ge(&self, other) -> bool`. The return type is fixed by the
language, so `User.age >= 18` can only be a `bool` — no operator overloading produces an
expression tree. Rewriting the tokens *before* `rustc` type-checks them is the only way to get
SQLAlchemy-like syntax.

The macro does not re-implement type checking; it only rewrites. `User.age` becomes
`User::col_age()`, so a misspelled column is `E0599` with rustc's own "did you mean `col_age`?"
hint, and a wrong value type is `E0277` pointing at the literal you wrote. The diagnostic
baseline (`tests/ui/*.stderr`) pins this behaviour so a macro change cannot quietly degrade it.

### Two things to know

- It is `where_!`, not `where!` — `where` is a Rust keyword.
- A proc-macro is not a method: `.where_(where_!(...))`, never `.where_!(...)`.

The expansion references `::rsbatis::__private::...`. Proc-macros have no `$crate`, so the
`rsbatis` dependency cannot be renamed.

## License

MIT OR Apache-2.0.
