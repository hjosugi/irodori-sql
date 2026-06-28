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
}

impl MigrationExportFormat {
    fn as_upper(self) -> &'static str {
        match self {
            Self::Parquet => "PARQUET",
            Self::Csv => "CSV",
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
    ]);
    let diff_sql = statement(&keyed_diff_sql(spec.target_engine, &keys, spec.diff_limit));
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
    let mut lines = vec![
        "-- Row hash manifest query.".to_string(),
        "SELECT".to_string(),
        select_columns
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
        _ => format!("LOWER(MD5({concatenated}))"),
    }
}

pub fn fingerprint_sql(engine: MigrationEngine, row_hash_sql: &str, keys: &[String]) -> String {
    let key_count = if keys.is_empty() {
        "0 AS key_count,".to_string()
    } else {
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
    };

    [
        "-- Fast validation fingerprint. Use this before running row-level diff.".to_string(),
        "WITH row_hashes AS (".to_string(),
        indent(row_hash_sql),
        ")".to_string(),
        "SELECT".to_string(),
        "  COUNT(*) AS row_count,".to_string(),
        format!("  {key_count}"),
        "  MIN(irodori_row_hash) AS min_row_hash,".to_string(),
        "  MAX(irodori_row_hash) AS max_row_hash".to_string(),
        "FROM row_hashes".to_string(),
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
    let stored_as = match spec.export_format {
        MigrationExportFormat::Parquet => "STORED AS PARQUET".to_string(),
        MigrationExportFormat::Csv => {
            "ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' STORED AS TEXTFILE".to_string()
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
        select_columns
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
        MigrationEngine::Hive | MigrationEngine::Databricks | MigrationEngine::TrinoPresto => {
            format!("CAST({reference} AS STRING)")
        }
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
            "Hive CSV extraction is slower and riskier than Parquet for Snowflake loads."
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
        assert!(plan.target_sql.contains("COPY INTO analytics.orders"));
        assert!(plan.diff_sql.contains("FULL OUTER JOIN"));
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
    fn empty_keys_explain_diff_requirement() {
        let sql = keyed_diff_sql(MigrationEngine::Snowflake, &[], 10);

        assert!(sql.contains("stable business key"));
    }
}
