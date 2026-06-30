# irodori-sql

SQL helpers used by Irodori Table and other Rust hosts.

## Provides

- dialect metadata
- identifier quoting
- placeholder and paging helpers
- query parameter detection
- schema/metamodel query builders
- schema diff helpers
- migration SQL and validation SQL builders

This crate does not connect to databases.

## Use

```toml
[dependencies]
irodori-sql = "0.3"
```

```rust
use irodori_sql::dialect::{quote_identifier, DbEngine};

let name = quote_identifier(DbEngine::Postgres, "order");
assert_eq!(name, "\"order\"");
```

## Develop

```sh
cargo test
```

License: `MIT OR 0BSD`.
