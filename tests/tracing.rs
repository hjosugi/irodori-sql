use irodori_sql::{
    dialect::PostgresDialect,
    migration::{build_migration_plan, MigrationSpec},
    parser::parse_select,
};
use tracing_subscriber::EnvFilter;

#[test]
fn library_events_accept_subscriber_with_appender_writer() {
    let (writer, _guard) = tracing_appender::non_blocking(std::io::sink());
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_env_filter(EnvFilter::new("irodori_sql=debug"))
        .with_writer(writer)
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        parse_select("select 1", &PostgresDialect).expect("select parses");
        let _ = build_migration_plan(&MigrationSpec::default());
    });
}
