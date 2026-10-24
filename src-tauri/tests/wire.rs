//! 应用自带 PostgreSQL 客户端对真实服务器的端到端检查。
//!
//! 除非设置了 `PG_TEST_URL`，否则跳过，例如
//! `PG_TEST_URL=postgresql://user:pass@localhost:5432/db cargo test --test wire -- --nocapture`

use std::time::{Duration, Instant};

use pg_easy_lib::testing::{self, pg, ConnectParams, SslMode};

fn params() -> Option<ConnectParams> {
    let url = std::env::var("PG_TEST_URL").ok()?;
    Some(testing::parse_url(&url))
}


#[tokio::test]
async fn logs_in_lists_rows_and_reports_tags() {
    let Some(params) = params() else {
        eprintln!("PG_TEST_URL not set; skipping");
        return;
    };
    let params = ConnectParams {
        ssl: SslMode::Disable,
        ..params
    };
    let stream = testing::open_stream(&params).await.expect("open socket");
    let startup = testing::startup_params(&params);
    eprintln!("opened socket; logging in");
    let mut conn = pg::connect(stream, &startup).await.expect("login");
    eprintln!("logged in");

    assert!(conn.backend_pid > 0);
    assert!(conn.parameter("server_version").is_some());

    let output = conn
        .run("select 1 as one, null::text as nothing, 'it''s' as quote", 100)
        .await
        .expect("query");
    assert_eq!(output.tag, "SELECT 1");
    let rows = output.rows.expect("rows");
    assert_eq!(rows.columns.len(), 3);
    assert_eq!(rows.columns[1].type_name, "text");
    assert_eq!(
        rows.values[0],
        vec![Some("1".into()), None, Some("it's".into())]
    );

    let output = conn.run("create temp table t (id int)", 100).await.unwrap();
    assert_eq!(output.tag, "CREATE TABLE");
    assert!(output.rows.is_none());

    let output = conn
        .run("insert into t values (1), (2), (3)", 100)
        .await
        .unwrap();
    assert_eq!(output.tag, "INSERT 0 3");

    // 失败的语句不影响连接继续使用，并报告它的 SQLSTATE。
    let (error, _) = conn.run("select * from nope", 100).await.unwrap_err();
    assert_eq!(error.code.as_deref(), Some("42P01"));

    let output = conn.run("select count(*) from t", 100).await.unwrap();
    assert_eq!(output.rows.unwrap().values[0][0].as_deref(), Some("3"));
    assert_eq!(conn.status(), b'I');
}

#[tokio::test]
async fn truncates_at_the_row_limit_and_counts_the_rest() {
    let Some(params) = params() else { return };
    let params = ConnectParams {
        ssl: SslMode::Disable,
        ..params
    };
    let stream = testing::open_stream(&params).await.unwrap();
    let mut conn = pg::connect(stream, &testing::startup_params(&params))
        .await
        .unwrap();
    let output = conn
        .run("select generate_series(1, 1000)", 10)
        .await
        .unwrap();
    let rows = output.rows.unwrap();
    assert_eq!(rows.values.len(), 10);
    assert_eq!(rows.dropped, 990);
}

#[tokio::test]
async fn cancel_stops_a_long_statement() {
    let Some(params) = params() else { return };
    let params = ConnectParams {
        ssl: SslMode::Disable,
        ..params
    };
    let port = params.port;
    let host = testing::host(&params);
    let stream = testing::open_stream(&params).await.unwrap();
    let mut conn = pg::connect(stream, &testing::startup_params(&params))
        .await
        .unwrap();
    let request = conn.cancel_request();
    let pid = conn.backend_pid;

    let started = Instant::now();
    let mut query = Box::pin(conn.run("select pg_sleep(30)", 10));
    tokio::select! {
        _ = &mut query => panic!("the statement finished before it was cancelled"),
        _ = tokio::time::sleep(Duration::from_millis(400)) => {}
    }
    testing::send_cancel(&host, port, request).await.expect("cancel");
    let (error, _) = tokio::time::timeout(Duration::from_secs(10), query)
        .await
        .expect("server honoured the cancel")
        .unwrap_err();
    assert_eq!(error.code.as_deref(), Some("57014"));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(conn.backend_pid, pid);

    // 会话在取消之后仍然存活。
    let output = conn.run("select 42", 10).await.unwrap();
    assert_eq!(output.rows.unwrap().values[0][0].as_deref(), Some("42"));
}

#[tokio::test]
async fn scripts_report_transaction_state() {
    let Some(params) = params() else { return };
    let params = ConnectParams {
        ssl: SslMode::Disable,
        ..params
    };
    let stream = testing::open_stream(&params).await.unwrap();
    let mut conn = pg::connect(stream, &testing::startup_params(&params))
        .await
        .unwrap();

    conn.run("begin", 10).await.unwrap();
    assert_eq!(conn.status(), b'T');
    conn.run("select 1/0", 10).await.unwrap_err();
    assert_eq!(conn.status(), b'E');
    conn.run("rollback", 10).await.unwrap();
    assert_eq!(conn.status(), b'I');

    // 语句发出的通知会被收集起来交给界面。
    conn.run("do $$ begin raise notice 'hello from a notice'; end $$", 10)
        .await
        .unwrap();
    let notices = conn.take_notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].arg_values().last().unwrap(), "hello from a notice");
}

#[tokio::test]
async fn tls_negotiation_follows_the_mode() {
    let Some(params) = params() else { return };
    // 测试服务器不启用 TLS，这正好是 `prefer` 的回退路径：服务器必须回 'N'，
    // 连接以明文继续。
    let prefer = ConnectParams {
        ssl: SslMode::Prefer,
        ..params.clone()
    };
    let (stream, safe) = testing::open_verified(&prefer).await.expect("prefer falls back");
    assert!(
        safe,
        "prefer falls back to clear text, and the harness allows that channel"
    );

    // 没有用户的首肯，登录会被拒绝，而不是在降级信道上泄漏密码。
    let mut startup = testing::startup_params(&prefer);
    startup.ssl_verified = false;
    let error = pg::connect(stream, &startup).await.err().expect("refused");
    assert_eq!(error.kind, "auth");

    // 有了首肯，用户的密码就走服务器选择的明文信道。
    let stream = testing::open_stream(&prefer).await.unwrap();
    let mut conn = pg::connect(stream, &testing::startup_params(&prefer))
        .await
        .unwrap();
    conn.run("select 'plain'", 10).await.unwrap();

    let required = ConnectParams {
        ssl: SslMode::Require,
        ..params
    };
    let error = testing::open_stream(&required).await.err();
    assert!(
        error.is_some_and(|error| error.kind == "tls"),
        "require must refuse a server that does not speak TLS"
    );
}
