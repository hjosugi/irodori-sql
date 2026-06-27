# irodori-sql

Shared SQL helpers extracted from Irodori Table.

This crate contains the pieces that are useful outside the desktop app:

- dialect metadata, identifier quoting, placeholders, and paging helpers;
- parameter detection and prompt modeling;
- information-schema/metamodel query builders;
- schema diff and migration-preview primitives.

It intentionally has no dependency on the Irodori desktop shell.

## Development

```sh
cargo test
```

Irodori Table consumes this crate as a version-tagged Git dependency so the app
can stay slimmer while the SQL contract evolves independently.
