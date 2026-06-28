//! Cross-engine migration planning and data validation SQL builders.
//!
//! This module is intentionally execution-free. It produces runbooks and SQL
//! snippets for upstream applications that own connections, credentials, job
//! scheduling, and object storage access.

use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MigrationEngine {
    Postgres,
    MySql,
    MariaDb,
    Oracle,
    Snowflake,
    Hive,
    DuckDb,
    Iceberg,
    S3Tables,
    Redshift,
    Databricks,
    TrinoPresto,
}

impl MigrationEngine {
    pub fn label(self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL",
            Self::MySql => "MySQL",
            Self::MariaDb => "MariaDB",
            Self::Oracle => "Oracle",
            Self::Snowflake => "Snowflake",
            Self::Hive => "Apache Hive",
            Self::DuckDb => "DuckDB / DuckDB-Wasm",
            Self::Iceberg => "Apache Iceberg REST",
            Self::S3Tables => "AWS S3 Tables",
            Self::Redshift => "Redshift",
            Self::Databricks => "Databricks / Spark SQL",
            Self::TrinoPresto => "Trino / Presto",
        }
    }

    pub fn is_duckdb_lakehouse(self) -> bool {
        matches!(self, Self::Iceberg | Self::S3Tables)
    }

    fn uses_backticks(self) -> bool {
        matches!(self, Self::MySql | Self::MariaDb | Self::Hive)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationExportFormat {
    Parquet,
    Csv,
    Tsv,
}

impl MigrationExportFormat {
    fn as_upper(self) -> &'static str {
        match self {
            Self::Parquet => "PARQUET",
            Self::Csv => "CSV",
            Self::Tsv => "TSV",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationTaskLevel {
    Ready,
    Manual,
    Risk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationTask {
    pub title: String,
    pub detail: String,
    pub level: MigrationTaskLevel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKeySpec {
    pub name: String,
    pub child_table: String,
    pub parent_table: String,
    pub child_columns: Vec<String>,
    pub parent_columns: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationSnippetKind {
    PrimaryKeyHash,
    HashBucketFingerprint,
    FailedBucketDiff,
    ForeignKeyHash,
    TsvLoad,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationSnippet {
    pub title: String,
    pub detail: String,
    pub kind: MigrationSnippetKind,
    /// SQL with named `${IRODORI_*}` variables for engines or CLIs that do their
    /// own substitution.
    pub sql: String,
    /// VS Code-compatible snippet body with numbered tabstops.
    pub body: String,
    pub variables: Vec<MigrationSnippetVariable>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationSnippetVariable {
    pub name: String,
    pub tabstop: usize,
    pub default_value: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationSpec {
    pub source_engine: MigrationEngine,
    pub target_engine: MigrationEngine,
    pub source_version: String,
    pub target_version: String,
    pub source_table: String,
    pub target_table: String,
    pub key_columns: Vec<String>,
    pub compare_columns: Vec<String>,
    pub partition_column: String,
    pub partition_predicate: String,
    pub export_format: MigrationExportFormat,
    pub batch_size: u64,
    pub diff_limit: usize,
    pub hash_bucket_prefix_len: usize,
    pub null_token: String,
    pub delimiter: String,
    pub normalize_whitespace: bool,
    pub normalize_case: bool,
}

impl Default for MigrationSpec {
    fn default() -> Self {
        Self::hive_to_snowflake("legacy.orders", "analytics.orders")
    }
}

impl MigrationSpec {
    pub fn hive_to_snowflake(
        source_table: impl Into<String>,
        target_table: impl Into<String>,
    ) -> Self {
        Self {
            source_engine: MigrationEngine::Hive,
            target_engine: MigrationEngine::Snowflake,
            source_version: "Hive 2/3".to_string(),
            target_version: "Snowflake".to_string(),
            source_table: source_table.into(),
            target_table: target_table.into(),
            key_columns: vec!["order_id".to_string(), "line_id".to_string()],
            compare_columns: vec![
                "order_id".to_string(),
                "line_id".to_string(),
                "customer_id".to_string(),
                "status".to_string(),
                "amount".to_string(),
                "updated_at".to_string(),
            ],
            partition_column: "sales_dt".to_string(),
            partition_predicate: "sales_dt >= '2026-01-01'".to_string(),
            export_format: MigrationExportFormat::Parquet,
            batch_size: 5_000_000,
            diff_limit: 1_000,
            hash_bucket_prefix_len: 4,
            null_token: "__IRODORI_NULL__".to_string(),
            delimiter: "|#|".to_string(),
            normalize_whitespace: true,
            normalize_case: false,
        }
    }

    fn normalized(&self) -> Self {
        let mut next = self.clone();
        next.key_columns = unique_case_insensitive(next.key_columns);
        next.compare_columns = unique_case_insensitive(next.compare_columns);
        next.batch_size = next.batch_size.clamp(1_000, 100_000_000);
        next.diff_limit = next.diff_limit.clamp(10, 100_000);
        next.hash_bucket_prefix_len = next.hash_bucket_prefix_len.clamp(1, 12);
        if next.null_token.is_empty() {
            next.null_token = "__IRODORI_NULL__".to_string();
        }
        if next.delimiter.is_empty() {
            next.delimiter = "|#|".to_string();
        }
        next
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationPlan {
    pub title: String,
    pub source_label: String,
    pub target_label: String,
    pub keys: Vec<String>,
    pub compare_columns: Vec<String>,
    pub hash_columns: Vec<String>,
    pub warnings: Vec<String>,
    pub tasks: Vec<MigrationTask>,
    pub pair_notes: Vec<String>,
    pub source_sql: String,
    pub target_sql: String,
    pub diff_sql: String,
    pub runbook: String,
}

/// Parse a pasted newline/comma separated column list, preserving first-seen
/// spelling while deduplicating case-insensitively.
pub fn parse_column_list(value: &str) -> Vec<String> {
    unique_case_insensitive(
        value
            .split(|ch| ch == '\n' || ch == ',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

pub fn build_migration_plan(spec: &MigrationSpec) -> MigrationPlan {
    let spec = spec.normalized();
    let source_label = spec.source_engine.label().to_string();
    let target_label = spec.target_engine.label().to_string();
    let keys = spec.key_columns.clone();
    let compare_columns = spec.compare_columns.clone();
    let hash_columns = if compare_columns.is_empty() {
        keys.clone()
    } else {
        compare_columns.clone()
    };
    let source_hash_sql = row_hash_select_sql(
        spec.source_engine,
        &spec.source_table,
        &keys,
        &hash_columns,
        &spec.partition_predicate,
        &spec,
    );
    let target_hash_sql = row_hash_select_sql(
        spec.target_engine,
        &spec.target_table,
        &keys,
        &hash_columns,
        &spec.partition_predicate,
        &spec,
    );
    let source_sql = join_blocks([
        source_extraction_sql(&spec, &keys, &hash_columns, &source_hash_sql),
        statement(&fingerprint_sql(
            spec.source_engine,
            &source_hash_sql,
            &keys,
        )),
        partition_fingerprint_block(spec.source_engine, &source_hash_sql, &keys, &spec),
    ]);
    let target_sql = join_blocks([
        target_load_sql(&spec),
        statement(&manifest_table_sql(
            spec.target_engine,
            &keys,
            &spec.partition_column,
        )),
        statement(&fingerprint_sql(
            spec.target_engine,
            &target_hash_sql,
            &keys,
        )),
        partition_fingerprint_block(spec.target_engine, &target_hash_sql, &keys, &spec),
    ]);
    let partitioned = !spec.partition_column.trim().is_empty();
    let diff_sql = if keys.is_empty() {
        statement(&keyed_diff_sql(spec.target_engine, &keys, spec.diff_limit))
    } else {
        join_blocks([
            statement(&hash_bucket_diff_sql(
                spec.target_engine,
                partitioned,
                spec.hash_bucket_prefix_len,
                spec.diff_limit,
            )),
            statement(&failed_bucket_row_diff_sql(
                spec.target_engine,
                &keys,
                partitioned,
                spec.hash_bucket_prefix_len,
                spec.diff_limit,
            )),
        ])
    };
    let warnings = build_warnings(&spec, &keys, &compare_columns);
    let pair_notes = build_pair_notes(&spec);
    let tasks = build_tasks(&spec, &keys, &hash_columns);
    let title = format!(
        "{} {} -> {} {}",
        source_label, spec.source_version, target_label, spec.target_version
    )
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ");

    MigrationPlan {
        title: title.clone(),
        source_label,
        target_label,
        keys,
        compare_columns,
        hash_columns: hash_columns.clone(),
        warnings: warnings.clone(),
        tasks,
        pair_notes: pair_notes.clone(),
        source_sql,
        target_sql,
        diff_sql,
        runbook: build_runbook(&title, &spec, &hash_columns, &warnings, &pair_notes),
    }
}

pub fn build_migration_snippets(
    spec: &MigrationSpec,
    foreign_keys: &[ForeignKeySpec],
) -> Vec<MigrationSnippet> {
    let spec = spec.normalized();
    let partitioned = !spec.partition_column.trim().is_empty();
    let mut snippets = Vec::new();

    if !spec.key_columns.is_empty() {
        snippets.push(migration_snippet(
            "Primary-key hash profile",
            "Find duplicate business keys and show the deterministic PK hash.",
            MigrationSnippetKind::PrimaryKeyHash,
            statement(&key_hash_profile_sql(
                spec.source_engine,
                &spec.source_table,
                &spec.key_columns,
                &spec.partition_predicate,
                &spec,
                spec.diff_limit,
            )),
            vec![],
        ));
        snippets.push(migration_snippet(
            "Hash bucket fingerprint diff",
            "Compare source/target manifests by PK hash prefix before row-level diff.",
            MigrationSnippetKind::HashBucketFingerprint,
            statement(&hash_bucket_diff_sql(
                spec.target_engine,
                partitioned,
                spec.hash_bucket_prefix_len,
                spec.diff_limit,
            )),
            vec![],
        ));
        let mut variables = vec![snippet_variable(
            "IRODORI_HASH_BUCKET",
            1,
            "hash_bucket",
            "Hash bucket returned by the bucket-level diff.",
        )];
        if partitioned {
            variables.push(snippet_variable(
                "IRODORI_PARTITION",
                2,
                "partition_value",
                "Partition returned by the bucket-level diff.",
            ));
        }
        snippets.push(migration_snippet(
            "Failed bucket row diff",
            "Set IRODORI_HASH_BUCKET and optional IRODORI_PARTITION to inspect only failed rows.",
            MigrationSnippetKind::FailedBucketDiff,
            statement(&failed_bucket_row_diff_sql(
                spec.target_engine,
                &spec.key_columns,
                partitioned,
                spec.hash_bucket_prefix_len,
                spec.diff_limit,
            )),
            variables,
        ));
    }

    if spec.export_format == MigrationExportFormat::Tsv {
        snippets.push(migration_snippet(
            "TSV load",
            "Use explicit tab-delimited file-format settings for Hive text exports.",
            MigrationSnippetKind::TsvLoad,
            statement(&target_load_sql(&spec)),
            vec![snippet_variable(
                "EXPORT_PATH",
                1,
                "s3://bucket/path",
                "External storage path used by DuckDB/Iceberg loaders.",
            )],
        ));
    }

    for foreign_key in foreign_keys {
        snippets.push(migration_snippet(
            format!("Foreign-key hash check: {}", foreign_key.name),
            format!(
                "{} -> {}",
                foreign_key.child_table, foreign_key.parent_table
            ),
            MigrationSnippetKind::ForeignKeyHash,
            statement(&foreign_key_integrity_sql(
                spec.target_engine,
                foreign_key,
                &spec,
                spec.diff_limit,
            )),
            vec![],
        ));
    }

    snippets
}

pub fn vscode_snippet_body(sql: &str, variables: &[MigrationSnippetVariable]) -> String {
    let mut body = sql.to_string();
    for variable in variables {
        let token = format!("${{{}}}", variable.name);
        let placeholder = format!(
            "${{{}:{}}}",
            variable.tabstop,
            escape_vscode_snippet_placeholder(&variable.default_value)
        );
        body = body.replace(&token, &placeholder);
    }
    body
}

pub fn row_hash_select_sql(
    engine: MigrationEngine,
    table: &str,
    keys: &[String],
    hash_columns: &[String],
    predicate: &str,
    spec: &MigrationSpec,
) -> String {
    let data_columns = unique_case_insensitive(
        keys.iter()
            .chain(hash_columns.iter())
            .cloned()
            .collect::<Vec<_>>(),
    );
    let select_columns = data_columns
        .iter()
        .map(|column| format!("  {}", column_ref(engine, column)))
        .collect::<Vec<_>>();
    let hash = row_hash_expression(engine, hash_columns, spec);
    let mut projected = select_columns;
    if !spec.partition_column.trim().is_empty() {
        projected.push(format!(
            "  {} AS irodori_partition",
            column_ref(engine, &spec.partition_column)
        ));
    }
    if !keys.is_empty() {
        projected.push(format!(
            "  {} AS irodori_key_hash",
            key_hash_expression(engine, keys, spec)
        ));
    }
    let mut lines = vec![
        "-- Row hash manifest query.".to_string(),
        "SELECT".to_string(),
        projected
            .into_iter()
            .chain([format!("  {hash} AS irodori_row_hash")])
            .collect::<Vec<_>>()
            .join(",\n"),
        format!("FROM {}", table_ref(engine, table)),
    ];
    if !predicate.trim().is_empty() {
        lines.push(format!("WHERE {}", predicate.trim()));
    }
    lines.join("\n")
}

pub fn row_hash_expression(
    engine: MigrationEngine,
    columns: &[String],
    spec: &MigrationSpec,
) -> String {
    if columns.is_empty() {
        return "'configure_compare_columns_before_hashing'".to_string();
    }
    let values = columns
        .iter()
        .map(|column| normalized_column_value(engine, column, spec))
        .collect::<Vec<_>>();
    let concatenated = concat_expression(engine, &values, &spec.delimiter);

    match engine {
        MigrationEngine::Oracle => {
            format!("LOWER(RAWTOHEX(STANDARD_HASH({concatenated}, 'SHA256')))")
        }
        MigrationEngine::MySql | MigrationEngine::MariaDb => {
            format!("LOWER(SHA2({concatenated}, 256))")
        }
        MigrationEngine::Snowflake => format!("LOWER(SHA2({concatenated}, 256))"),
        MigrationEngine::TrinoPresto => format!("LOWER(TO_HEX(MD5(TO_UTF8({concatenated}))))"),
        _ => format!("LOWER(MD5({concatenated}))"),
    }
}

pub fn key_hash_expression(
    engine: MigrationEngine,
    key_columns: &[String],
    spec: &MigrationSpec,
) -> String {
    row_hash_expression(engine, key_columns, spec)
}

pub fn fingerprint_sql(engine: MigrationEngine, row_hash_sql: &str, keys: &[String]) -> String {
    [
        "-- Fast validation fingerprint. Use this before running row-level diff.".to_string(),
        "WITH row_hashes AS (".to_string(),
        indent(row_hash_sql),
        ")".to_string(),
        "SELECT".to_string(),
        "  COUNT(*) AS row_count,".to_string(),
        format!("  {}", key_count_projection(engine, keys)),
        "  MIN(irodori_row_hash) AS min_row_hash,".to_string(),
        "  MAX(irodori_row_hash) AS max_row_hash".to_string(),
        "FROM row_hashes".to_string(),
    ]
    .join("\n")
}

pub fn partition_fingerprint_sql(
    engine: MigrationEngine,
    row_hash_sql: &str,
    partition_alias: &str,
    keys: &[String],
) -> String {
    let partition = partition_alias.trim();
    if partition.is_empty() {
        return fingerprint_sql(engine, row_hash_sql, keys);
    }
    let partition_ref = identifier_ref(engine, partition);

    [
        "-- Partition fingerprint. Run row-level diff only for failed partitions.".to_string(),
        "WITH row_hashes AS (".to_string(),
        indent(row_hash_sql),
        ")".to_string(),
        "SELECT".to_string(),
        format!("  {partition_ref} AS irodori_partition,"),
        "  COUNT(*) AS row_count,".to_string(),
        format!("  {}", key_count_projection(engine, keys)),
        "  MIN(irodori_row_hash) AS min_row_hash,".to_string(),
        "  MAX(irodori_row_hash) AS max_row_hash".to_string(),
        "FROM row_hashes".to_string(),
        format!("GROUP BY {partition_ref}"),
        format!("ORDER BY {partition_ref}"),
    ]
    .join("\n")
}

pub fn key_hash_profile_sql(
    engine: MigrationEngine,
    table: &str,
    keys: &[String],
    predicate: &str,
    spec: &MigrationSpec,
    limit: usize,
) -> String {
    if keys.is_empty() {
        return "-- Primary-key hash profile needs key columns.".to_string();
    }
    let key_columns = keys
        .iter()
        .map(|key| format!("  {}", column_ref(engine, key)))
        .collect::<Vec<_>>()
        .join(",\n");
    let key_group = keys
        .iter()
        .map(|key| column_ref(engine, key))
        .collect::<Vec<_>>()
        .join(", ");
    let mut lines = vec![
        "-- Primary-key hash profile. Duplicate keys are migration blockers.".to_string(),
        "SELECT".to_string(),
        format!("{key_columns},"),
        format!(
            "  {} AS irodori_key_hash,",
            key_hash_expression(engine, keys, spec)
        ),
        "  COUNT(*) AS rows_per_key".to_string(),
        format!("FROM {}", table_ref(engine, table)),
    ];
    if !predicate.trim().is_empty() {
        lines.push(format!("WHERE {}", predicate.trim()));
    }
    lines.extend([
        format!("GROUP BY {key_group}"),
        "HAVING COUNT(*) > 1".to_string(),
        "ORDER BY rows_per_key DESC".to_string(),
        limit_clause(engine, limit),
    ]);
    lines.join("\n")
}

pub fn hash_bucket_fingerprint_sql(
    engine: MigrationEngine,
    manifest_table: &str,
    partitioned: bool,
    bucket_prefix_len: usize,
) -> String {
    hash_bucket_fingerprint_sql_inner(engine, manifest_table, partitioned, bucket_prefix_len, true)
}

fn hash_bucket_fingerprint_sql_inner(
    engine: MigrationEngine,
    manifest_table: &str,
    partitioned: bool,
    bucket_prefix_len: usize,
    include_order_by: bool,
) -> String {
    let bucket_expr = hash_bucket_expr("irodori_key_hash", bucket_prefix_len);
    let partition_projection = if partitioned {
        "  COALESCE(irodori_partition, '__IRODORI_NULL_PARTITION__') AS irodori_partition,\n"
    } else {
        ""
    };
    let group_by = if partitioned {
        format!("GROUP BY COALESCE(irodori_partition, '__IRODORI_NULL_PARTITION__'), {bucket_expr}")
    } else {
        format!("GROUP BY {bucket_expr}")
    };
    let order_by = if partitioned {
        "ORDER BY irodori_partition, irodori_hash_bucket"
    } else {
        "ORDER BY irodori_hash_bucket"
    };
    let mut lines = vec![
        "-- Hash bucket fingerprint. Use this before row-level diff on very large tables."
            .to_string(),
        "SELECT".to_string(),
        format!("{partition_projection}  {bucket_expr} AS irodori_hash_bucket,"),
        "  COUNT(*) AS row_count,".to_string(),
        "  COUNT(DISTINCT irodori_key_hash) AS key_count,".to_string(),
        "  MIN(irodori_row_hash) AS min_row_hash,".to_string(),
        "  MAX(irodori_row_hash) AS max_row_hash".to_string(),
        format!("FROM {}", table_ref(engine, manifest_table)),
        group_by,
    ];
    if include_order_by {
        lines.push(order_by.to_string());
    }

    lines.join("\n")
}

pub fn hash_bucket_diff_sql(
    engine: MigrationEngine,
    partitioned: bool,
    bucket_prefix_len: usize,
    diff_limit: usize,
) -> String {
    if matches!(engine, MigrationEngine::MySql | MigrationEngine::MariaDb) {
        return mysql_hash_bucket_diff_sql(engine, partitioned, bucket_prefix_len, diff_limit);
    }

    let source_sql = hash_bucket_fingerprint_sql_inner(
        engine,
        "irodori_source_manifest",
        partitioned,
        bucket_prefix_len,
        false,
    );
    let target_sql = hash_bucket_fingerprint_sql_inner(
        engine,
        "irodori_target_manifest",
        partitioned,
        bucket_prefix_len,
        false,
    );
    let partition_projection = if partitioned {
        "  COALESCE(s.irodori_partition, t.irodori_partition) AS irodori_partition,\n"
    } else {
        ""
    };
    let partition_join = if partitioned {
        "s.irodori_partition = t.irodori_partition\n  AND "
    } else {
        ""
    };
    let order_by = if partitioned { "1, 2" } else { "1" };

    [
        "-- Bucket-level diff. Feed the returned bucket into failed_bucket_row_diff_sql."
            .to_string(),
        "WITH source_buckets AS (".to_string(),
        indent(&source_sql),
        "),".to_string(),
        "target_buckets AS (".to_string(),
        indent(&target_sql),
        ")".to_string(),
        "SELECT".to_string(),
        format!(
            "{partition_projection}  COALESCE(s.irodori_hash_bucket, t.irodori_hash_bucket) AS irodori_hash_bucket,"
        ),
        "  CASE".to_string(),
        "    WHEN s.irodori_hash_bucket IS NULL THEN 'target_only_bucket'".to_string(),
        "    WHEN t.irodori_hash_bucket IS NULL THEN 'source_only_bucket'".to_string(),
        "    ELSE 'changed_bucket'".to_string(),
        "  END AS diff_kind,".to_string(),
        "  s.row_count AS source_row_count,".to_string(),
        "  t.row_count AS target_row_count,".to_string(),
        "  s.key_count AS source_key_count,".to_string(),
        "  t.key_count AS target_key_count,".to_string(),
        "  s.min_row_hash AS source_min_row_hash,".to_string(),
        "  t.min_row_hash AS target_min_row_hash,".to_string(),
        "  s.max_row_hash AS source_max_row_hash,".to_string(),
        "  t.max_row_hash AS target_max_row_hash".to_string(),
        "FROM source_buckets s".to_string(),
        "FULL OUTER JOIN target_buckets t".to_string(),
        format!("  ON {partition_join}s.irodori_hash_bucket = t.irodori_hash_bucket"),
        "WHERE s.irodori_hash_bucket IS NULL".to_string(),
        "   OR t.irodori_hash_bucket IS NULL".to_string(),
        "   OR s.row_count <> t.row_count".to_string(),
        "   OR s.key_count <> t.key_count".to_string(),
        "   OR s.min_row_hash <> t.min_row_hash".to_string(),
        "   OR s.max_row_hash <> t.max_row_hash".to_string(),
        format!("ORDER BY {order_by}"),
        limit_clause(engine, diff_limit),
    ]
    .join("\n")
}

pub fn failed_bucket_row_diff_sql(
    engine: MigrationEngine,
    keys: &[String],
    partitioned: bool,
    bucket_prefix_len: usize,
    diff_limit: usize,
) -> String {
    if keys.is_empty() {
        return [
            "-- Row-level diff needs a stable business key.",
            "-- Add key columns, regenerate this plan, then load both manifest tables.",
        ]
        .join("\n");
    }
    if matches!(engine, MigrationEngine::MySql | MigrationEngine::MariaDb) {
        return mysql_failed_bucket_row_diff_sql(
            engine,
            keys,
            partitioned,
            bucket_prefix_len,
            diff_limit,
        );
    }

    let key_projection = keys
        .iter()
        .map(|key| {
            let quoted = identifier_ref(engine, key);
            format!("  COALESCE(s.{quoted}, t.{quoted}) AS {quoted}")
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let join = key_join(engine, keys);
    let order_by = positional_order_by(keys.len());
    let source_filter = manifest_bucket_filter(bucket_prefix_len, partitioned);
    let target_filter = source_filter.clone();

    [
        "-- Row-level diff scoped to one failed hash bucket.".to_string(),
        "WITH source_rows AS (".to_string(),
        format!(
            "  SELECT * FROM {}\n  WHERE {}",
            table_ref(engine, "irodori_source_manifest"),
            source_filter
        ),
        "),".to_string(),
        "target_rows AS (".to_string(),
        format!(
            "  SELECT * FROM {}\n  WHERE {}",
            table_ref(engine, "irodori_target_manifest"),
            target_filter
        ),
        ")".to_string(),
        "SELECT".to_string(),
        format!("{key_projection},"),
        "  CASE".to_string(),
        "    WHEN s.irodori_row_hash IS NULL THEN 'target_only'".to_string(),
        "    WHEN t.irodori_row_hash IS NULL THEN 'source_only'".to_string(),
        "    ELSE 'changed'".to_string(),
        "  END AS diff_kind,".to_string(),
        "  s.irodori_row_hash AS source_hash,".to_string(),
        "  t.irodori_row_hash AS target_hash".to_string(),
        "FROM source_rows s".to_string(),
        "FULL OUTER JOIN target_rows t".to_string(),
        format!("  ON {join}"),
        "WHERE s.irodori_row_hash IS NULL".to_string(),
        "   OR t.irodori_row_hash IS NULL".to_string(),
        "   OR s.irodori_row_hash <> t.irodori_row_hash".to_string(),
        format!("ORDER BY {order_by}"),
        limit_clause(engine, diff_limit),
    ]
    .join("\n")
}

pub fn foreign_key_integrity_sql(
    engine: MigrationEngine,
    foreign_key: &ForeignKeySpec,
    spec: &MigrationSpec,
    diff_limit: usize,
) -> String {
    if foreign_key.child_columns.is_empty()
        || foreign_key.parent_columns.is_empty()
        || foreign_key.child_columns.len() != foreign_key.parent_columns.len()
    {
        return "-- Foreign-key hash check needs matching child and parent columns.".to_string();
    }

    let child_hash_columns = qualified_columns("c", &foreign_key.child_columns);
    let parent_hash_columns = qualified_columns("p", &foreign_key.parent_columns);
    let child_hash = key_hash_expression(engine, &child_hash_columns, spec);
    let parent_hash = key_hash_expression(engine, &parent_hash_columns, spec);
    let child_not_null = not_null_predicate(engine, "c", &foreign_key.child_columns);
    let parent_not_null = not_null_predicate(engine, "p", &foreign_key.parent_columns);

    if matches!(engine, MigrationEngine::MySql | MigrationEngine::MariaDb) {
        return mysql_foreign_key_integrity_sql(
            engine,
            foreign_key,
            &child_hash,
            &parent_hash,
            &child_not_null,
            &parent_not_null,
            diff_limit,
        );
    }

    [
        format!("-- Foreign-key hash integrity check: {}", foreign_key.name),
        "WITH child_keys AS (".to_string(),
        "  SELECT".to_string(),
        format!("    {child_hash} AS fk_hash,"),
        "    COUNT(*) AS child_row_count".to_string(),
        format!("  FROM {} c", table_ref(engine, &foreign_key.child_table)),
        format!("  WHERE {child_not_null}"),
        format!("  GROUP BY {child_hash}"),
        "),".to_string(),
        "parent_keys AS (".to_string(),
        "  SELECT".to_string(),
        format!("    {parent_hash} AS fk_hash,"),
        "    COUNT(*) AS parent_row_count".to_string(),
        format!("  FROM {} p", table_ref(engine, &foreign_key.parent_table)),
        format!("  WHERE {parent_not_null}"),
        format!("  GROUP BY {parent_hash}"),
        ")".to_string(),
        "SELECT".to_string(),
        "  COALESCE(c.fk_hash, p.fk_hash) AS fk_hash,".to_string(),
        "  CASE".to_string(),
        "    WHEN p.fk_hash IS NULL THEN 'orphan_child'".to_string(),
        "    WHEN c.fk_hash IS NULL THEN 'parent_only'".to_string(),
        "    ELSE 'matched'".to_string(),
        "  END AS fk_status,".to_string(),
        "  c.child_row_count,".to_string(),
        "  p.parent_row_count".to_string(),
        "FROM child_keys c".to_string(),
        "FULL OUTER JOIN parent_keys p".to_string(),
        "  ON c.fk_hash = p.fk_hash".to_string(),
        "WHERE c.fk_hash IS NULL".to_string(),
        "   OR p.fk_hash IS NULL".to_string(),
        "ORDER BY fk_status, fk_hash".to_string(),
        limit_clause(engine, diff_limit),
    ]
    .join("\n")
}

pub fn manifest_table_sql(
    engine: MigrationEngine,
    keys: &[String],
    partition_column: &str,
) -> String {
    let text_type = string_type(engine);
    let mut columns = keys
        .iter()
        .map(|key| format!("  {} {text_type}", identifier_ref(engine, key)))
        .collect::<Vec<_>>();
    if !keys.is_empty() {
        columns.push(format!("  irodori_key_hash {text_type}"));
    }
    columns.push(format!("  irodori_row_hash {text_type}"));
    if !partition_column.trim().is_empty() {
        columns.push(format!("  irodori_partition {text_type}"));
    }

    [
        "-- Manifest tables hold source and target row hashes for fast diff.".to_string(),
        format!(
            "CREATE OR REPLACE TEMP TABLE {} (",
            table_ref(engine, "irodori_source_manifest")
        ),
        columns.join(",\n"),
        ");".to_string(),
        String::new(),
        format!(
            "CREATE OR REPLACE TEMP TABLE {} (",
            table_ref(engine, "irodori_target_manifest")
        ),
        columns.join(",\n"),
        ")".to_string(),
    ]
    .join("\n")
}

pub fn keyed_diff_sql(engine: MigrationEngine, keys: &[String], diff_limit: usize) -> String {
    if keys.is_empty() {
        return [
            "-- Row-level diff needs a stable business key.",
            "-- Add key columns, regenerate this plan, then load both manifest tables.",
        ]
        .join("\n");
    }
    if matches!(engine, MigrationEngine::MySql | MigrationEngine::MariaDb) {
        return mysql_keyed_diff_sql(engine, keys, diff_limit);
    }

    let key_projection = keys
        .iter()
        .map(|key| {
            let quoted = identifier_ref(engine, key);
            format!("  COALESCE(s.{quoted}, t.{quoted}) AS {quoted}")
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let join = key_join(engine, keys);
    let order_by = positional_order_by(keys.len());

    [
        "-- High-signal diff: missing rows first, changed rows with both hashes.".to_string(),
        "WITH source_rows AS (".to_string(),
        format!(
            "  SELECT * FROM {}",
            table_ref(engine, "irodori_source_manifest")
        ),
        "),".to_string(),
        "target_rows AS (".to_string(),
        format!(
            "  SELECT * FROM {}",
            table_ref(engine, "irodori_target_manifest")
        ),
        ")".to_string(),
        "SELECT".to_string(),
        format!("{key_projection},"),
        "  CASE".to_string(),
        "    WHEN s.irodori_row_hash IS NULL THEN 'target_only'".to_string(),
        "    WHEN t.irodori_row_hash IS NULL THEN 'source_only'".to_string(),
        "    ELSE 'changed'".to_string(),
        "  END AS diff_kind,".to_string(),
        "  s.irodori_row_hash AS source_hash,".to_string(),
        "  t.irodori_row_hash AS target_hash".to_string(),
        "FROM source_rows s".to_string(),
        "FULL OUTER JOIN target_rows t".to_string(),
        format!("  ON {join}"),
        "WHERE s.irodori_row_hash IS NULL".to_string(),
        "   OR t.irodori_row_hash IS NULL".to_string(),
        "   OR s.irodori_row_hash <> t.irodori_row_hash".to_string(),
        format!("ORDER BY {order_by}"),
        limit_clause(engine, diff_limit),
    ]
    .join("\n")
}

fn mysql_keyed_diff_sql(engine: MigrationEngine, keys: &[String], diff_limit: usize) -> String {
    let source_key_projection = keys
        .iter()
        .map(|key| {
            let quoted = identifier_ref(engine, key);
            format!("  s.{quoted} AS {quoted}")
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let target_key_projection = keys
        .iter()
        .map(|key| {
            let quoted = identifier_ref(engine, key);
            format!("  t.{quoted} AS {quoted}")
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let join = key_join(engine, keys);
    let order_by = positional_order_by(keys.len());

    [
        "-- MySQL/MariaDB do not support native full joins; emulate them with two anti-joins.".to_string(),
        "WITH source_rows AS (".to_string(),
        format!("  SELECT * FROM {}", table_ref(engine, "irodori_source_manifest")),
        "),".to_string(),
        "target_rows AS (".to_string(),
        format!("  SELECT * FROM {}", table_ref(engine, "irodori_target_manifest")),
        ")".to_string(),
        "SELECT".to_string(),
        format!("{source_key_projection},"),
        "  CASE WHEN t.irodori_row_hash IS NULL THEN 'source_only' ELSE 'changed' END AS diff_kind,"
            .to_string(),
        "  s.irodori_row_hash AS source_hash,".to_string(),
        "  t.irodori_row_hash AS target_hash".to_string(),
        "FROM source_rows s".to_string(),
        "LEFT JOIN target_rows t".to_string(),
        format!("  ON {join}"),
        "WHERE t.irodori_row_hash IS NULL".to_string(),
        "   OR s.irodori_row_hash <> t.irodori_row_hash".to_string(),
        "UNION ALL".to_string(),
        "SELECT".to_string(),
        format!("{target_key_projection},"),
        "  'target_only' AS diff_kind,".to_string(),
        "  s.irodori_row_hash AS source_hash,".to_string(),
        "  t.irodori_row_hash AS target_hash".to_string(),
        "FROM target_rows t".to_string(),
        "LEFT JOIN source_rows s".to_string(),
        format!("  ON {join}"),
        "WHERE s.irodori_row_hash IS NULL".to_string(),
        format!("ORDER BY {order_by}"),
        limit_clause(engine, diff_limit),
    ]
    .join("\n")
}

fn mysql_hash_bucket_diff_sql(
    engine: MigrationEngine,
    partitioned: bool,
    bucket_prefix_len: usize,
    diff_limit: usize,
) -> String {
    let source_sql = hash_bucket_fingerprint_sql_inner(
        engine,
        "irodori_source_manifest",
        partitioned,
        bucket_prefix_len,
        false,
    );
    let target_sql = hash_bucket_fingerprint_sql_inner(
        engine,
        "irodori_target_manifest",
        partitioned,
        bucket_prefix_len,
        false,
    );
    let source_projection = if partitioned {
        "  s.irodori_partition,\n  s.irodori_hash_bucket,"
    } else {
        "  s.irodori_hash_bucket,"
    };
    let target_projection = if partitioned {
        "  t.irodori_partition,\n  t.irodori_hash_bucket,"
    } else {
        "  t.irodori_hash_bucket,"
    };
    let join = if partitioned {
        "s.irodori_partition = t.irodori_partition AND s.irodori_hash_bucket = t.irodori_hash_bucket"
    } else {
        "s.irodori_hash_bucket = t.irodori_hash_bucket"
    };
    let null_check = if partitioned {
        "t.irodori_partition IS NULL AND t.irodori_hash_bucket IS NULL"
    } else {
        "t.irodori_hash_bucket IS NULL"
    };
    let reverse_null_check = if partitioned {
        "s.irodori_partition IS NULL AND s.irodori_hash_bucket IS NULL"
    } else {
        "s.irodori_hash_bucket IS NULL"
    };
    let order_by = if partitioned { "1, 2" } else { "1" };

    [
        "-- MySQL/MariaDB bucket-level diff with full-join emulation.".to_string(),
        "WITH source_buckets AS (".to_string(),
        indent(&source_sql),
        "),".to_string(),
        "target_buckets AS (".to_string(),
        indent(&target_sql),
        ")".to_string(),
        "SELECT".to_string(),
        source_projection.to_string(),
        "  CASE WHEN t.irodori_hash_bucket IS NULL THEN 'source_only_bucket' ELSE 'changed_bucket' END AS diff_kind,"
            .to_string(),
        "  s.row_count AS source_row_count,".to_string(),
        "  t.row_count AS target_row_count,".to_string(),
        "  s.key_count AS source_key_count,".to_string(),
        "  t.key_count AS target_key_count,".to_string(),
        "  s.min_row_hash AS source_min_row_hash,".to_string(),
        "  t.min_row_hash AS target_min_row_hash,".to_string(),
        "  s.max_row_hash AS source_max_row_hash,".to_string(),
        "  t.max_row_hash AS target_max_row_hash".to_string(),
        "FROM source_buckets s".to_string(),
        "LEFT JOIN target_buckets t".to_string(),
        format!("  ON {join}"),
        format!("WHERE {null_check}"),
        "   OR s.row_count <> t.row_count".to_string(),
        "   OR s.key_count <> t.key_count".to_string(),
        "   OR s.min_row_hash <> t.min_row_hash".to_string(),
        "   OR s.max_row_hash <> t.max_row_hash".to_string(),
        "UNION ALL".to_string(),
        "SELECT".to_string(),
        target_projection.to_string(),
        "  'target_only_bucket' AS diff_kind,".to_string(),
        "  s.row_count AS source_row_count,".to_string(),
        "  t.row_count AS target_row_count,".to_string(),
        "  s.key_count AS source_key_count,".to_string(),
        "  t.key_count AS target_key_count,".to_string(),
        "  s.min_row_hash AS source_min_row_hash,".to_string(),
        "  t.min_row_hash AS target_min_row_hash,".to_string(),
        "  s.max_row_hash AS source_max_row_hash,".to_string(),
        "  t.max_row_hash AS target_max_row_hash".to_string(),
        "FROM target_buckets t".to_string(),
        "LEFT JOIN source_buckets s".to_string(),
        format!("  ON {join}"),
        format!("WHERE {reverse_null_check}"),
        format!("ORDER BY {order_by}"),
        limit_clause(engine, diff_limit),
    ]
    .join("\n")
}

fn mysql_failed_bucket_row_diff_sql(
    engine: MigrationEngine,
    keys: &[String],
    partitioned: bool,
    bucket_prefix_len: usize,
    diff_limit: usize,
) -> String {
    let source_key_projection = keys
        .iter()
        .map(|key| {
            let quoted = identifier_ref(engine, key);
            format!("  s.{quoted} AS {quoted}")
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let target_key_projection = keys
        .iter()
        .map(|key| {
            let quoted = identifier_ref(engine, key);
            format!("  t.{quoted} AS {quoted}")
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let join = key_join(engine, keys);
    let order_by = positional_order_by(keys.len());
    let filter = manifest_bucket_filter(bucket_prefix_len, partitioned);

    [
        "-- MySQL/MariaDB bucket-scoped row diff with full-join emulation.".to_string(),
        "WITH source_rows AS (".to_string(),
        format!(
            "  SELECT * FROM {}\n  WHERE {}",
            table_ref(engine, "irodori_source_manifest"),
            filter
        ),
        "),".to_string(),
        "target_rows AS (".to_string(),
        format!(
            "  SELECT * FROM {}\n  WHERE {}",
            table_ref(engine, "irodori_target_manifest"),
            filter
        ),
        ")".to_string(),
        "SELECT".to_string(),
        format!("{source_key_projection},"),
        "  CASE WHEN t.irodori_row_hash IS NULL THEN 'source_only' ELSE 'changed' END AS diff_kind,"
            .to_string(),
        "  s.irodori_row_hash AS source_hash,".to_string(),
        "  t.irodori_row_hash AS target_hash".to_string(),
        "FROM source_rows s".to_string(),
        "LEFT JOIN target_rows t".to_string(),
        format!("  ON {join}"),
        "WHERE t.irodori_row_hash IS NULL".to_string(),
        "   OR s.irodori_row_hash <> t.irodori_row_hash".to_string(),
        "UNION ALL".to_string(),
        "SELECT".to_string(),
        format!("{target_key_projection},"),
        "  'target_only' AS diff_kind,".to_string(),
        "  s.irodori_row_hash AS source_hash,".to_string(),
        "  t.irodori_row_hash AS target_hash".to_string(),
        "FROM target_rows t".to_string(),
        "LEFT JOIN source_rows s".to_string(),
        format!("  ON {join}"),
        "WHERE s.irodori_row_hash IS NULL".to_string(),
        format!("ORDER BY {order_by}"),
        limit_clause(engine, diff_limit),
    ]
    .join("\n")
}

fn mysql_foreign_key_integrity_sql(
    engine: MigrationEngine,
    foreign_key: &ForeignKeySpec,
    child_hash: &str,
    parent_hash: &str,
    child_not_null: &str,
    parent_not_null: &str,
    diff_limit: usize,
) -> String {
    [
        format!(
            "-- Foreign-key hash integrity check with full-join emulation: {}",
            foreign_key.name
        ),
        "WITH child_keys AS (".to_string(),
        "  SELECT".to_string(),
        format!("    {child_hash} AS fk_hash,"),
        "    COUNT(*) AS child_row_count".to_string(),
        format!("  FROM {} c", table_ref(engine, &foreign_key.child_table)),
        format!("  WHERE {child_not_null}"),
        format!("  GROUP BY {child_hash}"),
        "),".to_string(),
        "parent_keys AS (".to_string(),
        "  SELECT".to_string(),
        format!("    {parent_hash} AS fk_hash,"),
        "    COUNT(*) AS parent_row_count".to_string(),
        format!("  FROM {} p", table_ref(engine, &foreign_key.parent_table)),
        format!("  WHERE {parent_not_null}"),
        format!("  GROUP BY {parent_hash}"),
        ")".to_string(),
        "SELECT".to_string(),
        "  c.fk_hash,".to_string(),
        "  'orphan_child' AS fk_status,".to_string(),
        "  c.child_row_count,".to_string(),
        "  p.parent_row_count".to_string(),
        "FROM child_keys c".to_string(),
        "LEFT JOIN parent_keys p ON c.fk_hash = p.fk_hash".to_string(),
        "WHERE p.fk_hash IS NULL".to_string(),
        "UNION ALL".to_string(),
        "SELECT".to_string(),
        "  p.fk_hash,".to_string(),
        "  'parent_only' AS fk_status,".to_string(),
        "  c.child_row_count,".to_string(),
        "  p.parent_row_count".to_string(),
        "FROM parent_keys p".to_string(),
        "LEFT JOIN child_keys c ON c.fk_hash = p.fk_hash".to_string(),
        "WHERE c.fk_hash IS NULL".to_string(),
        "ORDER BY fk_status, fk_hash".to_string(),
        limit_clause(engine, diff_limit),
    ]
    .join("\n")
}

fn migration_snippet(
    title: impl Into<String>,
    detail: impl Into<String>,
    kind: MigrationSnippetKind,
    sql: String,
    variables: Vec<MigrationSnippetVariable>,
) -> MigrationSnippet {
    let variables = variables
        .into_iter()
        .filter(|variable| sql.contains(&format!("${{{}}}", variable.name)))
        .collect::<Vec<_>>();
    let body = vscode_snippet_body(&sql, &variables);
    MigrationSnippet {
        title: title.into(),
        detail: detail.into(),
        kind,
        sql,
        body,
        variables,
    }
}

fn snippet_variable(
    name: impl Into<String>,
    tabstop: usize,
    default_value: impl Into<String>,
    description: impl Into<String>,
) -> MigrationSnippetVariable {
    MigrationSnippetVariable {
        name: name.into(),
        tabstop,
        default_value: default_value.into(),
        description: description.into(),
    }
}

fn escape_vscode_snippet_placeholder(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}")
}

fn source_extraction_sql(
    spec: &MigrationSpec,
    keys: &[String],
    hash_columns: &[String],
    source_hash_sql: &str,
) -> String {
    if spec.source_engine == MigrationEngine::Hive {
        return statement(&hive_export_sql(spec, keys, hash_columns));
    }
    if spec.source_engine.is_duckdb_lakehouse() {
        return statement(&join_blocks([
            duckdb_iceberg_bootstrap_sql(spec.source_engine),
            source_hash_sql.to_string(),
        ]));
    }
    statement(source_hash_sql)
}

fn target_load_sql(spec: &MigrationSpec) -> String {
    if spec.target_engine == MigrationEngine::Snowflake {
        return snowflake_load_sql(spec);
    }
    if spec.target_engine.is_duckdb_lakehouse() {
        return duckdb_iceberg_load_sql(spec);
    }
    [
        "-- Load the exported files with the target engine's bulk loader.",
        "-- Keep the irodori_row_hash column in a staging or manifest table until validation passes.",
    ]
    .join("\n")
}

fn duckdb_iceberg_bootstrap_sql(engine: MigrationEngine) -> String {
    let label = engine.label();
    [
        format!("-- {label} via DuckDB: local/browser compute with Iceberg REST Catalog access."),
        "-- DuckDB-Wasm runs this shape inside a browser tab; desktop DuckDB runs the same SQL locally."
            .to_string(),
        "INSTALL httpfs;".to_string(),
        "LOAD httpfs;".to_string(),
        "INSTALL iceberg;".to_string(),
        "LOAD iceberg;".to_string(),
        String::new(),
        "CREATE OR REPLACE SECRET irodori_s3_secret (".to_string(),
        "  TYPE S3,".to_string(),
        "  KEY_ID '${AWS_ACCESS_KEY_ID}',".to_string(),
        "  SECRET '${AWS_SECRET_ACCESS_KEY}',".to_string(),
        "  REGION '${AWS_REGION}'".to_string(),
        ");".to_string(),
        String::new(),
        "ATTACH '${ICEBERG_WAREHOUSE}' AS irodori_iceberg (".to_string(),
        "  TYPE ICEBERG,".to_string(),
        "  ENDPOINT_URL '${ICEBERG_REST_ENDPOINT}'".to_string(),
        ");".to_string(),
        String::new(),
        "-- For AWS S3 Tables, use the S3 Tables bucket ARN as ICEBERG_WAREHOUSE.".to_string(),
        "-- Browser caution: never put real credentials into a shareable URL.".to_string(),
    ]
    .join("\n")
}

fn duckdb_iceberg_load_sql(spec: &MigrationSpec) -> String {
    let scan = match spec.export_format {
        MigrationExportFormat::Parquet => "read_parquet('${EXPORT_PATH}/*.parquet')",
        MigrationExportFormat::Csv => "read_csv_auto('${EXPORT_PATH}/*.csv')",
        MigrationExportFormat::Tsv => "read_csv_auto('${EXPORT_PATH}/*.tsv', delim='\\t')",
    };
    [
        duckdb_iceberg_bootstrap_sql(spec.target_engine),
        String::new(),
        "-- First-load pattern. For incremental loads, INSERT/MERGE after DDL and partition mapping are validated."
            .to_string(),
        format!(
            "CREATE OR REPLACE TABLE {} AS",
            table_ref(spec.target_engine, &spec.target_table)
        ),
        format!("SELECT * FROM {scan};"),
        String::new(),
        "-- Keep source/target hash manifests available until row count, key count, fingerprint, and row-level diff pass."
            .to_string(),
    ]
    .join("\n")
}

fn hive_export_sql(spec: &MigrationSpec, keys: &[String], hash_columns: &[String]) -> String {
    let data_columns = unique_case_insensitive(
        keys.iter()
            .chain(hash_columns.iter())
            .cloned()
            .collect::<Vec<_>>(),
    );
    let select_columns = data_columns
        .iter()
        .map(|column| format!("  {}", column_ref(MigrationEngine::Hive, column)))
        .collect::<Vec<_>>();
    let hash = row_hash_expression(MigrationEngine::Hive, hash_columns, spec);
    let mut projected = select_columns;
    if !spec.partition_column.trim().is_empty() {
        projected.push(format!(
            "  {} AS irodori_partition",
            column_ref(MigrationEngine::Hive, &spec.partition_column)
        ));
    }
    if !keys.is_empty() {
        projected.push(format!(
            "  {} AS irodori_key_hash",
            key_hash_expression(MigrationEngine::Hive, keys, spec)
        ));
    }
    let stored_as = match spec.export_format {
        MigrationExportFormat::Parquet => "STORED AS PARQUET".to_string(),
        MigrationExportFormat::Csv => {
            "ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' STORED AS TEXTFILE".to_string()
        }
        MigrationExportFormat::Tsv => {
            "ROW FORMAT DELIMITED FIELDS TERMINATED BY '\\t' STORED AS TEXTFILE".to_string()
        }
    };

    let mut lines = vec![
        "-- Hive extraction: partitioned files plus deterministic row hashes.".to_string(),
        "SET hive.execution.engine=tez;".to_string(),
        "SET hive.vectorized.execution.enabled=true;".to_string(),
        "SET hive.exec.compress.output=true;".to_string(),
        String::new(),
        "INSERT OVERWRITE DIRECTORY '${EXPORT_PATH}'".to_string(),
        stored_as,
        "SELECT".to_string(),
        projected
            .into_iter()
            .chain([format!("  {hash} AS irodori_row_hash")])
            .collect::<Vec<_>>()
            .join(",\n"),
        format!(
            "FROM {}",
            table_ref(MigrationEngine::Hive, &spec.source_table)
        ),
    ];
    if !spec.partition_predicate.trim().is_empty() {
        lines.push(format!("WHERE {}", spec.partition_predicate.trim()));
    }
    lines.join("\n")
}

fn snowflake_load_sql(spec: &MigrationSpec) -> String {
    let format_name = "irodori_migration_file_format";
    let stage_name = "irodori_migration_stage";
    let file_format = match spec.export_format {
        MigrationExportFormat::Parquet => {
            format!("CREATE OR REPLACE FILE FORMAT {format_name} TYPE = PARQUET USE_VECTORIZED_SCANNER = TRUE;")
        }
        MigrationExportFormat::Csv => [
            format!("CREATE OR REPLACE FILE FORMAT {format_name}"),
            "  TYPE = CSV".to_string(),
            "  FIELD_DELIMITER = ','".to_string(),
            "  SKIP_HEADER = 1".to_string(),
            "  FIELD_OPTIONALLY_ENCLOSED_BY = '\"'".to_string(),
            "  NULL_IF = ('', 'NULL');".to_string(),
        ]
        .join("\n"),
        MigrationExportFormat::Tsv => [
            format!("CREATE OR REPLACE FILE FORMAT {format_name}"),
            "  TYPE = CSV".to_string(),
            "  FIELD_DELIMITER = '\\t'".to_string(),
            "  SKIP_HEADER = 0".to_string(),
            "  EMPTY_FIELD_AS_NULL = TRUE".to_string(),
            "  NULL_IF = ('', 'NULL', '\\\\N');".to_string(),
        ]
        .join("\n"),
    };

    [
        "-- Snowflake load: point the stage at the exported files.".to_string(),
        file_format,
        format!("CREATE OR REPLACE STAGE {stage_name} FILE_FORMAT = {format_name};"),
        String::new(),
        format!(
            "COPY INTO {}",
            table_ref(MigrationEngine::Snowflake, &spec.target_table)
        ),
        format!("FROM @{stage_name}"),
        "MATCH_BY_COLUMN_NAME = CASE_INSENSITIVE".to_string(),
        format!("FILE_FORMAT = (FORMAT_NAME = {format_name});"),
    ]
    .join("\n")
}

fn normalized_column_value(engine: MigrationEngine, column: &str, spec: &MigrationSpec) -> String {
    let reference = column_ref(engine, column);
    let mut value = match engine {
        MigrationEngine::Postgres | MigrationEngine::Redshift => {
            format!("CAST({reference} AS TEXT)")
        }
        MigrationEngine::Oracle => format!("TO_CHAR({reference})"),
        MigrationEngine::Snowflake => format!("TO_VARCHAR({reference})"),
        MigrationEngine::MySql | MigrationEngine::MariaDb => format!("CAST({reference} AS CHAR)"),
        MigrationEngine::DuckDb | MigrationEngine::Iceberg | MigrationEngine::S3Tables => {
            format!("CAST({reference} AS VARCHAR)")
        }
        MigrationEngine::Hive | MigrationEngine::Databricks => {
            format!("CAST({reference} AS STRING)")
        }
        MigrationEngine::TrinoPresto => format!("CAST({reference} AS VARCHAR)"),
    };
    if spec.normalize_whitespace {
        value = regexp_replace_whitespace(engine, &value);
    }
    if spec.normalize_case {
        value = format!("LOWER({value})");
    }
    format!("COALESCE({value}, {})", sql_string(&spec.null_token))
}

fn regexp_replace_whitespace(engine: MigrationEngine, value: &str) -> String {
    if matches!(
        engine,
        MigrationEngine::Postgres | MigrationEngine::Redshift
    ) {
        format!("REGEXP_REPLACE({value}, '\\s+', ' ', 'g')")
    } else {
        format!("REGEXP_REPLACE({value}, '\\s+', ' ')")
    }
}

fn concat_expression(engine: MigrationEngine, values: &[String], delimiter: &str) -> String {
    if values.is_empty() {
        return "''".to_string();
    }
    if engine == MigrationEngine::Oracle {
        return values.join(&format!(" || {} || ", sql_string(delimiter)));
    }
    format!(
        "CONCAT_WS({}, {})",
        sql_string(delimiter),
        values.join(", ")
    )
}

fn partition_fingerprint_block(
    engine: MigrationEngine,
    row_hash_sql: &str,
    keys: &[String],
    spec: &MigrationSpec,
) -> String {
    if spec.partition_column.trim().is_empty() {
        String::new()
    } else {
        statement(&partition_fingerprint_sql(
            engine,
            row_hash_sql,
            "irodori_partition",
            keys,
        ))
    }
}

fn key_count_projection(engine: MigrationEngine, keys: &[String]) -> String {
    if keys.is_empty() {
        return "0 AS key_count,".to_string();
    }
    let key_values = keys
        .iter()
        .map(|key| {
            normalized_column_value(
                engine,
                key,
                &MigrationSpec {
                    null_token: "__IRODORI_NULL__".to_string(),
                    delimiter: "|#|".to_string(),
                    normalize_whitespace: false,
                    normalize_case: false,
                    ..MigrationSpec::default()
                },
            )
        })
        .collect::<Vec<_>>();
    format!(
        "COUNT(DISTINCT {}) AS key_count,",
        concat_expression(engine, &key_values, "|#|")
    )
}

fn build_warnings(
    spec: &MigrationSpec,
    keys: &[String],
    compare_columns: &[String],
) -> Vec<String> {
    let mut warnings = Vec::new();
    if spec.source_table.trim().is_empty() || spec.target_table.trim().is_empty() {
        warnings.push("Source and target table names are required before execution.".to_string());
    }
    if keys.is_empty() {
        warnings.push("A stable business key is required for row-level diff.".to_string());
    }
    if compare_columns.is_empty() {
        warnings.push("Compare columns are empty, so only key columns will be hashed.".to_string());
    }
    if spec.source_engine == MigrationEngine::Hive
        && spec.export_format != MigrationExportFormat::Parquet
    {
        warnings.push(
            "Hive text extraction is slower and riskier than Parquet for Snowflake loads; prefer TSV over CSV if Parquet is unavailable."
                .to_string(),
        );
    }
    if matches!(spec.source_engine, MigrationEngine::Oracle)
        || matches!(spec.target_engine, MigrationEngine::Oracle)
    {
        warnings.push("Oracle empty string, NLS date format, NUMBER precision, and timezone semantics need explicit mapping.".to_string());
    }
    if matches!(
        spec.source_engine,
        MigrationEngine::MySql | MigrationEngine::MariaDb
    ) || matches!(
        spec.target_engine,
        MigrationEngine::MySql | MigrationEngine::MariaDb
    ) {
        warnings.push("MySQL/MariaDB zero dates, unsigned numerics, charset, collation, and lack of FULL OUTER JOIN can change comparison behavior.".to_string());
    }
    if spec.target_engine == MigrationEngine::Snowflake {
        warnings.push("Snowflake quoted identifiers are case-sensitive. Keep generated identifiers aligned with table DDL.".to_string());
    }
    if spec.source_engine.is_duckdb_lakehouse() || spec.target_engine.is_duckdb_lakehouse() {
        warnings.push("Browser DuckDB/Iceberg flows must keep credentials out of shareable URLs and exported runbooks.".to_string());
        warnings.push("Iceberg REST Catalog and object-store endpoints must be reachable from the browser/runtime, including CORS where applicable.".to_string());
    }
    warnings
}

fn build_pair_notes(spec: &MigrationSpec) -> Vec<String> {
    let mut notes = vec![
        "Use an inventory scan before moving data: schema, row counts, partitions, primary keys, nullability, and incompatible types.".to_string(),
        "Use recipe-style transforms for DDL and SQL rewrites, then gate every batch with count, hash, and sampled row checks.".to_string(),
    ];

    if spec.source_engine == MigrationEngine::Hive
        && spec.target_engine == MigrationEngine::Snowflake
    {
        notes.insert(0, "Avoid row-by-row JDBC extraction from Hive for large tables; push projection, partition predicates, and hashing down to Hive.".to_string());
        notes.insert(0, "Hive -> Snowflake: export partitioned Parquet, stage files, COPY into Snowflake, then compare source and target hash manifests inside Snowflake.".to_string());
    }
    if spec.source_engine == MigrationEngine::Oracle
        && spec.target_engine == MigrationEngine::Postgres
    {
        notes.insert(0, "Oracle -> PostgreSQL: map NUMBER precision, DATE/TIMESTAMP timezone behavior, empty string NULL behavior, sequences, and LOB columns before data compare.".to_string());
    }
    if spec.source_engine == MigrationEngine::MySql && spec.target_engine == MigrationEngine::Oracle
    {
        notes.insert(0, "MySQL -> Oracle: map AUTO_INCREMENT, unsigned integers, zero dates, text/blob limits, and case/collation before hash validation.".to_string());
    }
    if spec.source_engine.is_duckdb_lakehouse() || spec.target_engine.is_duckdb_lakehouse() {
        notes.insert(0, "For S3 Tables, treat the bucket ARN as the Iceberg warehouse and keep catalog credentials in a secure connection profile, not in URL fragments.".to_string());
        notes.insert(0, "DuckDB/Iceberg: use DuckDB as the client-is-the-server compute layer, attaching the Iceberg REST Catalog directly and keeping validation local.".to_string());
    }
    if spec.target_engine == MigrationEngine::Snowflake {
        notes.push("For very large tables, compare by partition or hash bucket first, then run row-level diff only for failed buckets.".to_string());
    }
    notes
}

fn build_tasks(
    spec: &MigrationSpec,
    keys: &[String],
    hash_columns: &[String],
) -> Vec<MigrationTask> {
    vec![
        MigrationTask {
            title: "Inventory".to_string(),
            detail: format!(
                "{} -> {} with {} key column(s).",
                if spec.source_table.is_empty() {
                    "source table"
                } else {
                    &spec.source_table
                },
                if spec.target_table.is_empty() {
                    "target table"
                } else {
                    &spec.target_table
                },
                keys.len()
            ),
            level: if keys.is_empty() {
                MigrationTaskLevel::Risk
            } else {
                MigrationTaskLevel::Ready
            },
        },
        MigrationTask {
            title: "Extract".to_string(),
            detail: if spec.source_engine == MigrationEngine::Hive {
                format!(
                    "{} export with pushed-down row hash and partition predicate.",
                    spec.export_format.as_upper()
                )
            } else if spec.source_engine.is_duckdb_lakehouse() {
                "Attach the Iceberg REST Catalog in DuckDB and materialize a source hash manifest locally.".to_string()
            } else {
                "Run the source row hash query and persist the result as a manifest.".to_string()
            },
            level: MigrationTaskLevel::Manual,
        },
        MigrationTask {
            title: "Validate".to_string(),
            detail: format!(
                "{} compare column(s), row count, key count, and min/max hash fingerprint before row diff.",
                hash_columns.len()
            ),
            level: if hash_columns.is_empty() {
                MigrationTaskLevel::Risk
            } else {
                MigrationTaskLevel::Ready
            },
        },
        MigrationTask {
            title: "Diff".to_string(),
            detail: format!(
                "Load both manifests into {} and inspect the first {} mismatches.",
                spec.target_engine.label(),
                spec.diff_limit
            ),
            level: if keys.is_empty() {
                MigrationTaskLevel::Risk
            } else {
                MigrationTaskLevel::Ready
            },
        },
    ]
}

fn build_runbook(
    title: &str,
    spec: &MigrationSpec,
    hash_columns: &[String],
    warnings: &[String],
    notes: &[String],
) -> String {
    let warning_lines = if warnings.is_empty() {
        vec!["- No blocking warning generated.".to_string()]
    } else {
        warnings
            .iter()
            .map(|warning| format!("- {warning}"))
            .collect()
    };
    [
        format!("# {title}"),
        String::new(),
        "## 1. Inventory".to_string(),
        format!(
            "- Source: {} {} / {}",
            spec.source_engine.label(),
            spec.source_version,
            non_empty_or(&spec.source_table, "(missing)")
        ),
        format!(
            "- Target: {} {} / {}",
            spec.target_engine.label(),
            spec.target_version,
            non_empty_or(&spec.target_table, "(missing)")
        ),
        format!(
            "- Keys: {}",
            if spec.key_columns.is_empty() {
                "(missing)".to_string()
            } else {
                spec.key_columns.join(", ")
            }
        ),
        format!(
            "- Hash columns: {}",
            if hash_columns.is_empty() {
                "(missing)".to_string()
            } else {
                hash_columns.join(", ")
            }
        ),
        String::new(),
        "## 2. Recipe Plan".to_string(),
        "- Build source schema inventory, type mapping, and SQL rewrite recipes before data movement.".to_string(),
        "- Treat DDL conversion and application modernization like recipe-based automation: scan, propose, apply, verify.".to_string(),
        "- Keep a migration scorecard: unsupported types, lossy casts, timezone handling, and manual cutover items.".to_string(),
        String::new(),
        "## 3. Extract And Load".to_string(),
        format!(
            "- Batch size target: {} rows per partition or bucket.",
            spec.batch_size
        ),
        format!(
            "- Partition predicate: {}",
            non_empty_or(&spec.partition_predicate, "(none)")
        ),
        "- Persist a source hash manifest before loading target data.".to_string(),
        "- Load data first, then create the target hash manifest from the loaded table.".to_string(),
        String::new(),
        "## 4. Compare Gates".to_string(),
        "- Gate 1: row count and key count match.".to_string(),
        "- Gate 2: min/max hash fingerprint matches for each partition or hash bucket.".to_string(),
        "- Gate 3: row-level FULL OUTER JOIN diff returns zero rows.".to_string(),
        "- Gate 4: sampled value-level checks for failed hashes and high-risk data types.".to_string(),
        String::new(),
        "## 5. Notes".to_string(),
        notes
            .iter()
            .map(|note| format!("- {note}"))
            .collect::<Vec<_>>()
            .join("\n"),
        String::new(),
        "## 6. Warnings".to_string(),
        warning_lines.join("\n"),
    ]
    .join("\n")
}

fn key_join(engine: MigrationEngine, keys: &[String]) -> String {
    keys.iter()
        .map(|key| {
            let quoted = identifier_ref(engine, key);
            format!("s.{quoted} = t.{quoted}")
        })
        .collect::<Vec<_>>()
        .join("\n  AND ")
}

fn qualified_columns(alias: &str, columns: &[String]) -> Vec<String> {
    columns
        .iter()
        .map(|column| format!("{alias}.{column}"))
        .collect()
}

fn not_null_predicate(engine: MigrationEngine, alias: &str, columns: &[String]) -> String {
    if columns.is_empty() {
        return "1 = 1".to_string();
    }
    columns
        .iter()
        .map(|column| {
            format!(
                "{alias}.{} IS NOT NULL",
                column
                    .split('.')
                    .last()
                    .map(|part| identifier_ref(engine, part))
                    .unwrap_or_else(|| identifier_ref(engine, column))
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn hash_bucket_expr(key_hash_ref: &str, bucket_prefix_len: usize) -> String {
    format!(
        "SUBSTR({key_hash_ref}, 1, {})",
        bucket_prefix_len.clamp(1, 12)
    )
}

fn manifest_bucket_filter(bucket_prefix_len: usize, partitioned: bool) -> String {
    let mut filters = vec![format!(
        "{} = '${{IRODORI_HASH_BUCKET}}'",
        hash_bucket_expr("irodori_key_hash", bucket_prefix_len)
    )];
    if partitioned {
        filters.push(
            "COALESCE(irodori_partition, '__IRODORI_NULL_PARTITION__') = '${IRODORI_PARTITION}'"
                .to_string(),
        );
    }
    filters.join("\n    AND ")
}

fn positional_order_by(width: usize) -> String {
    (1..=width)
        .map(|index| index.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn table_ref(engine: MigrationEngine, name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "(missing_table)".to_string();
    }
    trimmed
        .split('.')
        .map(|part| identifier_ref(engine, part))
        .collect::<Vec<_>>()
        .join(".")
}

fn column_ref(engine: MigrationEngine, name: &str) -> String {
    name.split('.')
        .map(|part| identifier_ref(engine, part))
        .collect::<Vec<_>>()
        .join(".")
}

fn identifier_ref(engine: MigrationEngine, value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "\"\"".to_string();
    }
    if trimmed.starts_with('"')
        || trimmed.starts_with('`')
        || trimmed.contains('(')
        || trimmed.contains(')')
    {
        return trimmed.to_string();
    }
    if is_simple_identifier(trimmed) {
        return trimmed.to_string();
    }
    let quote = if engine.uses_backticks() { '`' } else { '"' };
    format!(
        "{quote}{}{quote}",
        trimmed.replace(quote, &format!("{quote}{quote}"))
    )
}

fn is_simple_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn limit_clause(engine: MigrationEngine, value: usize) -> String {
    let limit = value.clamp(10, 100_000);
    if engine == MigrationEngine::Oracle {
        format!("FETCH FIRST {limit} ROWS ONLY")
    } else {
        format!("LIMIT {limit}")
    }
}

fn string_type(engine: MigrationEngine) -> &'static str {
    match engine {
        MigrationEngine::Oracle => "VARCHAR2(4000)",
        MigrationEngine::MySql | MigrationEngine::MariaDb => "VARCHAR(4000)",
        MigrationEngine::DuckDb | MigrationEngine::Iceberg | MigrationEngine::S3Tables => "VARCHAR",
        MigrationEngine::Snowflake => "STRING",
        _ => "TEXT",
    }
}

fn statement(sql: &str) -> String {
    let trimmed = sql.trim();
    if trimmed.is_empty() || trimmed.ends_with(';') {
        trimmed.to_string()
    } else {
        format!("{trimmed};")
    }
}

fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn indent(value: &str) -> String {
    value
        .split('\n')
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn join_blocks<const N: usize>(blocks: [String; N]) -> String {
    blocks
        .into_iter()
        .map(|block| block.trim().to_string())
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn unique_case_insensitive(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for value in values {
        let key = value.to_lowercase();
        if seen.insert(key) {
            result.push(value);
        }
    }
    result
}

fn non_empty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_columns_case_insensitively() {
        assert_eq!(
            parse_column_list("id, email\nID\ncreated_at"),
            vec!["id", "email", "created_at"]
        );
    }

    #[test]
    fn builds_hive_to_snowflake_plan() {
        let plan = build_migration_plan(&MigrationSpec::default());

        assert!(plan
            .source_sql
            .contains("INSERT OVERWRITE DIRECTORY '${EXPORT_PATH}'"));
        assert!(plan.source_sql.contains("STORED AS PARQUET"));
        assert!(plan.source_sql.contains("LOWER(MD5(CONCAT_WS"));
        assert!(plan.source_sql.contains("sales_dt AS irodori_partition"));
        assert!(plan.source_sql.contains("irodori_key_hash"));
        assert!(plan.source_sql.contains("Partition fingerprint"));
        assert!(plan.target_sql.contains("COPY INTO analytics.orders"));
        assert!(plan.diff_sql.contains("Bucket-level diff"));
        assert!(plan.diff_sql.contains("${IRODORI_HASH_BUCKET}"));
        assert!(plan
            .pair_notes
            .iter()
            .any(|note| note.contains("Hive -> Snowflake")));
    }

    #[test]
    fn duckdb_iceberg_plan_bootstraps_catalog_access() {
        let spec = MigrationSpec {
            source_engine: MigrationEngine::Iceberg,
            target_engine: MigrationEngine::Snowflake,
            source_table: "lake.orders".to_string(),
            target_table: "analytics.orders".to_string(),
            key_columns: vec!["order_id".to_string()],
            compare_columns: vec!["order_id".to_string(), "amount".to_string()],
            ..MigrationSpec::default()
        };

        let plan = build_migration_plan(&spec);

        assert!(plan.source_sql.contains("INSTALL iceberg;"));
        assert!(plan.source_sql.contains("TYPE ICEBERG"));
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("shareable URLs")));
    }

    #[test]
    fn oracle_hash_uses_standard_hash() {
        let spec = MigrationSpec {
            source_engine: MigrationEngine::Oracle,
            compare_columns: vec!["CUSTOMER ID".to_string(), "AMOUNT".to_string()],
            delimiter: "|".to_string(),
            normalize_whitespace: false,
            ..MigrationSpec::default()
        };

        let sql = row_hash_expression(MigrationEngine::Oracle, &spec.compare_columns, &spec);

        assert!(sql.contains("STANDARD_HASH"));
        assert!(sql.contains("\"CUSTOMER ID\""));
        assert!(sql.contains(" || '|' || "));
    }

    #[test]
    fn mysql_diff_emulates_full_outer_join() {
        let keys = vec!["id".to_string(), "line id".to_string()];
        let sql = keyed_diff_sql(MigrationEngine::MySql, &keys, 500);

        assert!(!sql.contains("FULL OUTER JOIN"));
        assert!(sql.contains("UNION ALL"));
        assert!(sql.contains("LEFT JOIN target_rows"));
        assert!(sql.contains("`line id`"));
        assert!(sql.ends_with("LIMIT 500"));
    }

    #[test]
    fn snippets_include_vscode_style_variables() {
        let snippets = build_migration_snippets(&MigrationSpec::default(), &[]);
        let failed_bucket = snippets
            .iter()
            .find(|snippet| snippet.kind == MigrationSnippetKind::FailedBucketDiff)
            .expect("failed bucket snippet");

        assert!(failed_bucket.sql.contains("${IRODORI_HASH_BUCKET}"));
        assert!(failed_bucket.sql.contains("${IRODORI_PARTITION}"));
        assert!(failed_bucket.body.contains("${1:hash_bucket}"));
        assert!(failed_bucket.body.contains("${2:partition_value}"));
        assert_eq!(failed_bucket.variables.len(), 2);
        assert_eq!(failed_bucket.variables[0].name, "IRODORI_HASH_BUCKET");
    }

    #[test]
    fn foreign_key_snippet_hashes_child_and_parent_keys() {
        let fk = ForeignKeySpec {
            name: "orders_customer_fk".to_string(),
            child_table: "analytics.orders".to_string(),
            parent_table: "analytics.customers".to_string(),
            child_columns: vec!["customer_id".to_string()],
            parent_columns: vec!["id".to_string()],
        };
        let snippets = build_migration_snippets(&MigrationSpec::default(), &[fk]);
        let fk_snippet = snippets
            .iter()
            .find(|snippet| snippet.kind == MigrationSnippetKind::ForeignKeyHash)
            .expect("fk snippet");

        assert!(fk_snippet.sql.contains("Foreign-key hash integrity check"));
        assert!(fk_snippet.sql.contains("orphan_child"));
        assert!(fk_snippet.sql.contains("analytics.orders c"));
        assert!(fk_snippet.sql.contains("analytics.customers p"));
    }

    #[test]
    fn tsv_export_and_load_use_tab_delimiters() {
        let spec = MigrationSpec {
            export_format: MigrationExportFormat::Tsv,
            ..MigrationSpec::default()
        };
        let plan = build_migration_plan(&spec);

        assert!(plan.source_sql.contains("FIELDS TERMINATED BY '\\t'"));
        assert!(plan.target_sql.contains("FIELD_DELIMITER = '\\t'"));
    }

    #[test]
    fn trino_hash_uses_varbinary_md5_shape() {
        let spec = MigrationSpec {
            compare_columns: vec!["payload".to_string()],
            ..MigrationSpec::default()
        };

        let sql = row_hash_expression(MigrationEngine::TrinoPresto, &spec.compare_columns, &spec);

        assert!(sql.contains("TO_HEX(MD5(TO_UTF8("));
        assert!(sql.contains("CAST(payload AS VARCHAR)"));
    }

    #[test]
    fn empty_keys_explain_diff_requirement() {
        let sql = keyed_diff_sql(MigrationEngine::Snowflake, &[], 10);

        assert!(sql.contains("stable business key"));
    }
}
