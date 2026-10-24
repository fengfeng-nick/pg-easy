//! Small helpers for integration tests that drive the client against a real
//! server. Compiled into the library (the crate has no separate test target
//! for internals) but not used by the app itself.

pub use crate::connection::{ConnectParams, SslMode};
/// The wire client, exposed so integration tests can drive it directly.
pub mod pg {
    pub use crate::pg::*;
}
/// The catalog loader, so integration tests can check what the sidebar will
/// list for a given user.
pub mod catalog {
    pub use crate::catalog::load_catalog;
}
/// The dumper, so integration tests can round-trip real databases. The
/// helpers take a [`Session`], which the command layer normally builds.
pub mod dump {
    pub use crate::dump::{plan, run, DumpSink, PlanRelation};
    pub use crate::protocol::{DumpFormat, DumpOptions};

    use std::time::Instant;

    use crate::connection::ConnectParams;
    use crate::state::Session;

    /// Wraps an already authenticated wire connection as a session.
    pub fn session(
        id: &str,
        params: &ConnectParams,
        database: &str,
        conn: crate::pg::Connection,
    ) -> Session {
        let server_version = conn
            .parameter("server_version")
            .unwrap_or_default()
            .to_string();
        let backend_pid = conn.backend_pid;
        Session {
            id: id.to_string(),
            params: params.clone(),
            database: database.to_string(),
            user: params.user.clone(),
            server_version,
            conn,
            busy: false,
            backend_pid,
            idle_since: Instant::now(),
        }
    }
}

use crate::connection;
use crate::pg::StartupParams;

/// `postgresql://user:pass@host:port/db` — enough of a URL for tests.
pub fn parse_url(url: &str) -> ConnectParams {
    let rest = url
        .strip_prefix("postgresql://")
        .or_else(|| url.strip_prefix("postgres://"))
        .expect("PG_TEST_URL must start with postgresql://");
    let (authority, database) = match rest.split_once('/') {
        Some((authority, database)) => (authority, database.split('?').next().unwrap_or("")),
        None => (rest, ""),
    };
    let (credentials, hostport) = match authority.rsplit_once('@') {
        Some((credentials, hostport)) => (Some(credentials), hostport),
        None => (None, authority),
    };
    let (user, password) = match credentials {
        Some(credentials) => match credentials.split_once(':') {
            Some((user, password)) => (user.to_string(), Some(password.to_string())),
            None => (credentials.to_string(), None),
        },
        None => (String::new(), None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().unwrap_or(5432)),
        None => (hostport.to_string(), 5432),
    };
    ConnectParams {
        host,
        port,
        database: database.to_string(),
        user,
        password,
        ssl: SslMode::Prefer,
        ssl_root_cert: None,
        connect_timeout_ms: Some(5_000),
        application_name: Some("pg easy tests".into()),
        allow_unencrypted_password: true,
    }
}

pub async fn open_stream(
    params: &ConnectParams,
) -> Result<crate::pg::Stream, crate::error::AppError> {
    connection::open_stream(params).await.map(|(stream, _)| stream)
}

/// Like [`open_stream`], also reporting whether the password may be sent.
pub async fn open_verified(
    params: &ConnectParams,
) -> Result<(crate::pg::Stream, bool), crate::error::AppError> {
    connection::open_stream(params).await
}

pub fn startup_params(params: &ConnectParams) -> StartupParams {
    StartupParams {
        user: params.user.clone(),
        database: params.database.clone(),
        password: params.password.clone(),
        application_name: params.application_name.clone(),
        // The server is chosen by the test harness, so no check is made here.
        ssl_verified: true,
    }
}

pub fn host(params: &ConnectParams) -> String {
    connection::effective_host(&params.host).to_string()
}

pub async fn send_cancel(host: &str, port: u16, request: Vec<u8>) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut socket = tokio::net::TcpStream::connect((host, port)).await?;
    socket.write_all(&request).await?;
    socket.shutdown().await
}
