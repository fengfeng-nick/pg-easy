//! The sidebar's table browsing path against a real server: a fresh database
//! with tables in several schemas (one schema hidden from the user) is
//! catalogued, and the first and second page of a table are read.
//!
//! Skipped unless `PG_TEST_URL` points at a server, e.g.
//! `PG_TEST_URL=postgresql://user:pass@localhost:5432/postgres cargo test --test catalog_browse`

use pg_easy_lib::testing::{self, pg, ConnectParams, SslMode};

fn params(database: &str) -> Option<ConnectParams> {
    let url = std::env::var("PG_TEST_URL").ok()?;
    Some(ConnectParams {
        ssl: SslMode::Disable,
        database: database.to_string(),
        ..testing::parse_url(&url)
    })
}

async fn connect(params: &ConnectParams) -> pg::Connection {
    let stream = testing::open_stream(params).await.expect("open socket");
    pg::connect(stream, &testing::startup_params(params))
        .await
        .expect("login")
}

async fn exec(conn: &mut pg::Connection, sql: &str) -> Result<(), String> {
    for statement in sql.split(';').filter(|part| !part.trim().is_empty()) {
        conn.run(statement, usize::MAX)
            .await
            .map_err(|(err, _)| err.message)?;
    }
    Ok(())
}

#[tokio::test]
async fn lists_tables_and_reads_their_rows() {
    let Some(base) = params("postgres") else {
        eprintln!("PG_TEST_URL not set; skipping");
        return;
    };
    let mut admin = connect(&base).await;
    let database = format!("browse_{}", std::process::id());
    let schema = format!("browse_{}", std::process::id());

    exec(
        &mut admin,
        &format!("drop database if exists {database} with (force)"),
    )
    .await
    .unwrap();
    exec(&mut admin, &format!("create database {database}"))
        .await
        .unwrap();

    let params = params(&database).unwrap();
    let mut conn = connect(&params).await;

    exec(
        &mut conn,
        &format!(
            "create schema {schema};
             create table {schema}.items (id int primary key, label text);
             insert into {schema}.items
                  select g, 'row ' || g from generate_series(1, 600) g;
             create table {schema}.empty (id int);
             create view {schema}.labels as select label from {schema}.items;
             create table public.public_table (v text);
             create schema {schema}_hidden;
             create table {schema}_hidden.secret (v text);"
        ),
    )
    .await
    .unwrap();
    // A schema the user cannot see must not show up in the tree either.
    exec(
        &mut conn,
        &format!(
            "revoke usage on schema {schema}_hidden from public;
             drop role if exists browse_restricted;
             create role browse_restricted login password 'secret';
             grant usage on schema {schema} to browse_restricted;
             grant select on all tables in schema {schema} to browse_restricted;"
        ),
    )
    .await
    .unwrap();

    // The owner sees every schema and relation kind.
    let catalog = pg_easy_lib::testing::catalog::load_catalog(
        &mut testing::dump::session("t", &params, &database, conn),
    )
    .await
    .unwrap();
    let names: Vec<&str> = catalog.relations[&schema]
        .iter()
        .map(|relation| relation.name.as_str())
        .collect();
    assert_eq!(names, vec!["empty", "items", "labels"]);
    let items = catalog.relations[&schema]
        .iter()
        .find(|relation| relation.name == "items")
        .unwrap();
    assert_eq!(items.kind, "table");
    // ANALYZE has not run, so the estimate may be 0; the row count itself is
    // what the page query below proves.
    assert_eq!(items.columns, 2);
    assert!(catalog.relations.contains_key("public"));

    let restricted = ConnectParams {
        user: "browse_restricted".into(),
        password: Some("secret".into()),
        allow_unencrypted_password: true,
        ..params.clone()
    };
    let restricted_conn = connect(&restricted).await;
    let restricted_catalog = pg_easy_lib::testing::catalog::load_catalog(
        &mut testing::dump::session("r", &restricted, &database, restricted_conn),
    )
    .await
    .unwrap();
    assert!(restricted_catalog.relations.contains_key(&schema));
    assert!(!restricted_catalog
        .relations
        .keys()
        .any(|name| name.ends_with("_hidden")));

    // Pages: the same statements `firstPageSql` and `tablePageSql` build.
    let mut conn = connect(&params).await;
    let first = conn
        .run(
            &format!("select * from {schema}.items limit 10"),
            usize::MAX,
        )
        .await
        .unwrap()
        .rows
        .unwrap();
    assert_eq!(first.values.len(), 10);
    assert_eq!(first.columns.len(), 2);

    let second = conn
        .run(
            &format!("select * from {schema}.items order by \"id\" limit 10 offset 10"),
            usize::MAX,
        )
        .await
        .unwrap()
        .rows
        .unwrap();
    assert_eq!(second.values[0][0].as_deref(), Some("11"));

    let total = conn
        .run(&format!("select count(*) from {schema}.items"), usize::MAX)
        .await
        .unwrap()
        .rows
        .unwrap();
    assert_eq!(total.values[0][0].as_deref(), Some("600"));

    let empty = conn
        .run(&format!("select * from {schema}.empty limit 10"), usize::MAX)
        .await
        .unwrap()
        .rows
        .unwrap();
    assert!(empty.values.is_empty());
    assert_eq!(empty.columns.len(), 1);

    // A table the user may not read fails with the server's own message.
    let mut denied = connect(&restricted).await;
    let error = denied
        .run("select * from public.public_table limit 10", usize::MAX)
        .await
        .unwrap_err()
        .0;
    assert_eq!(error.kind, "database");
    assert!(error.message.contains("permission denied"), "{}", error.message);

    exec(
        &mut admin,
        &format!(
            "drop database if exists {database} with (force);
             drop role if exists browse_restricted"
        ),
    )
    .await
    .unwrap();
}
