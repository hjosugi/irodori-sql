# irodori-sql

Shared SQL helpers extracted from Irodori Table.

This crate contains the pieces that are useful outside the desktop app:

- dialect metadata, identifier quoting, placeholders, and paging helpers;
- parameter detection and prompt modeling;
- information-schema/metamodel query builders;
- schema diff and migration-preview primitives.
- cross-engine migration planning helpers for Hive, Snowflake, PostgreSQL,
  MySQL/MariaDB, Oracle, DuckDB, Iceberg REST, and S3 Tables;
- deterministic row-hash, fingerprint, manifest-table, and high-signal diff SQL
  builders for validating large data moves, including partition fingerprints
  before row-level diff.
- migration snippets for PK hash checks, FK integrity checks, TSV loading, and
  failed-bucket row diffs with VS Code-style tabstop variables.

It intentionally has no dependency on the Irodori desktop shell.

## Migration helpers

The `migration` module does not open database connections or move data. It
generates the runbook and SQL that an app can execute with its own credentials:

```rust
use irodori_sql::migration::{build_migration_plan, MigrationSpec};

let spec = MigrationSpec::hive_to_snowflake("legacy.orders", "analytics.orders");
let plan = build_migration_plan(&spec);

println!("{}", plan.source_sql); // Hive Parquet export + row hash manifest
println!("{}", plan.target_sql); // Snowflake stage/COPY + validation SQL
println!("{}", plan.diff_sql);   // keyed row-hash diff
```

The generated validation flow is count -> key count -> hash fingerprint ->
partition fingerprint -> row-level diff, so callers can avoid expensive
row-by-row inspection until a partition or batch fails the cheap gates.

Snippets expose both a named-variable SQL template and a VS Code-compatible
body:

```rust
use irodori_sql::migration::{build_migration_snippets, MigrationSnippetKind};

let snippets = build_migration_snippets(&spec, &[]);
let diff = snippets
    .iter()
    .find(|snippet| snippet.kind == MigrationSnippetKind::FailedBucketDiff)
    .unwrap();

println!("{}", diff.sql);  // ... '${IRODORI_HASH_BUCKET}' ...
println!("{}", diff.body); // ... '${1:hash_bucket}' ...
```

## Development

```sh
cargo test
```

Snapshot tests cover engine-specific migration SQL generation. Update them
intentionally with:

```sh
INSTA_UPDATE=always cargo test --test migration_snapshots
```

Real-database integration tests use testcontainers and are ignored by default
because they require Docker:

```sh
cargo test --test migration_real_db -- --ignored
```

Irodori Table consumes this crate as a version-tagged Git dependency so the app
can stay slimmer while the SQL contract evolves independently.

## License

Irodori-authored code in this repository is available under `MIT OR 0BSD` unless
a file says otherwise. See [LICENSE](LICENSE).

## Disclaimer

SQL generation, formatting, migration, and diff helpers can produce incomplete
or destructive statements when used with real systems. Review generated SQL,
permissions, query plans, backups, and target connections before execution. For
the broader product disclaimer, see
<https://hjosugi.github.io/irodori-docs/disclaimer.html>.
