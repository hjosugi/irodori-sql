use irodori_sql::migration::{
    keyed_diff_sql, manifest_table_sql, row_hash_select_sql, MigrationEngine, MigrationSpec,
};

fn sample_spec() -> MigrationSpec {
    MigrationSpec {
        source_engine: MigrationEngine::Postgres,
        target_engine: MigrationEngine::Snowflake,
        source_table: "public.orders".to_string(),
        target_table: "analytics.orders".to_string(),
        key_columns: vec!["order_id".to_string()],
        compare_columns: vec![
            "order_id".to_string(),
            "amount".to_string(),
            "updated_at".to_string(),
        ],
        partition_column: "sales_dt".to_string(),
        partition_predicate: "sales_dt >= '2026-01-01'".to_string(),
        diff_limit: 25,
        hash_bucket_prefix_len: 3,
        normalize_case: true,
        ..MigrationSpec::default()
    }
}

#[test]
fn snapshots_engine_specific_migration_sql() {
    let spec = sample_spec();

    insta::assert_snapshot!(
        "postgres_row_hash_select",
        row_hash_select_sql(
            MigrationEngine::Postgres,
            &spec.source_table,
            &spec.key_columns,
            &spec.compare_columns,
            &spec.partition_predicate,
            &spec,
        )
    );
    insta::assert_snapshot!(
        "mysql_keyed_diff",
        keyed_diff_sql(MigrationEngine::MySql, &spec.key_columns, spec.diff_limit)
    );
    insta::assert_snapshot!(
        "snowflake_manifest_tables",
        manifest_table_sql(
            MigrationEngine::Snowflake,
            &spec.key_columns,
            &spec.partition_column,
        )
    );
}
