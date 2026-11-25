# rsbatis-migrate

Versioned SQL migrations for [rsbatis](https://crates.io/crates/rsbatis): `up`, `down`, `status`,
and rollback to a specific version.

```rust
use rsbatis::{from_dir, Migrator};

let migrations = from_dir("migrations")?; // 0001_create_users.sql + 0001_create_users.down.sql
let mut m = Migrator::new(&mut session, migrations)?;
m.up()?;          // apply all pending
m.up_to(3)?;      // stop after version 3
m.down()?;        // roll back the newest one
m.status()?;      // applied / pending
```

Applied versions are recorded in `_rsbatis_migrations (version, name, applied_at)`, together with
each migration inside the same transaction.

### Deliberate choices

- **A metadata table, not `PRAGMA user_version`.** The pragma stores one integer: it cannot record
  which version was applied when, nor roll back to a version in the middle of the chain.
- **`supports_transactional_ddl()` tells the truth.** On MySQL, DDL implicitly commits, so a
  migration that creates a table and then fails leaves the half-built table behind. The API reports
  `false` there instead of wrapping the failure in a reassuring story.
- **Batches are executed statement by statement.** A multi-statement migration that fails midway
  reports "statement N failed" — the earlier silent-success behaviour is a test-guarded regression,
  with an assertion that statement N+1 does not run after statement N fails.

## License

MIT OR Apache-2.0.
