use irodori_sql::migration::{keyed_diff_sql, MigrationEngine};
use postgres::{Client, NoTls};
use testcontainers::runners::SyncRunner;
use testcontainers_modules::postgres::Postgres;

type TestResult = Result<(), Box<dyn std::error::Error + 'static>>;

#[test]
#[ignore = "requires Docker"]
fn postgres_executes_generated_keyed_diff_sql() -> TestResult {
    let node = match Postgres::default().start() {
        Ok(node) => node,
        Err(error) if std::env::var_os("CI").is_none() => {
            eprintln!("skipping real DB test because the container did not start: {error}");
            return Ok(());
        }
        Err(error) => return Err(Box::new(error)),
    };
    let connection_string = format!(
        "postgres://postgres:postgres@{}:{}/postgres",
        node.get_host()?,
        node.get_host_port_ipv4(5432)?
    );
    let mut client = Client::connect(&connection_string, NoTls)?;

    client.batch_execute(
        r#"
        CREATE TEMP TABLE irodori_source_manifest (
            id text,
            irodori_row_hash text
        );
        CREATE TEMP TABLE irodori_target_manifest (
            id text,
            irodori_row_hash text
        );

        INSERT INTO irodori_source_manifest (id, irodori_row_hash) VALUES
            ('1', 'same'),
            ('2', 'source_hash'),
            ('3', 'source_only_hash');
        INSERT INTO irodori_target_manifest (id, irodori_row_hash) VALUES
            ('1', 'same'),
            ('2', 'target_hash'),
            ('4', 'target_only_hash');
        "#,
    )?;

    let keys = vec!["id".to_string()];
    let sql = keyed_diff_sql(MigrationEngine::Postgres, &keys, 10);
    let rows = client.query(&sql, &[])?;
    let mut diffs = rows
        .iter()
        .map(|row| {
            (
                row.get::<_, String>("id"),
                row.get::<_, String>("diff_kind"),
            )
        })
        .collect::<Vec<_>>();
    diffs.sort();

    assert_eq!(
        diffs,
        vec![
            ("2".to_string(), "changed".to_string()),
            ("3".to_string(), "source_only".to_string()),
            ("4".to_string(), "target_only".to_string()),
        ]
    );
    Ok(())
}
