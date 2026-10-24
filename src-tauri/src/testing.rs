//! 让集成测试对着真实服务器驱动客户端的若干小工具。编译进库（本 crate 没有
//! 单独的内部测试目标），但应用本身不使用。

pub use crate::connection::{ConnectParams, SslMode};
/// 线协议客户端，暴露出来好让集成测试直接驱动。
pub mod pg {
    pub use crate::pg::*;
}
/// 目录加载器，集成测试可以用它检查侧边栏会为某个用户列出什么。
pub mod catalog {
    pub use crate::catalog::load_catalog;
}
/// 导入器，集成测试可以用应用自己的回放路径把转储送进真实服务器。
pub mod import {
    pub use crate::import::{plan, run, Plan};
    pub use crate::protocol::ImportOptions;
}
/// 转储器，集成测试可以对真实数据库做往返验证。这些辅助函数接收一个
/// [`Session`]，通常由命令层构建。
pub mod dump {
    pub use crate::dump::{plan, run, DumpSink, PlanRelation};
    pub use crate::protocol::{DumpFormat, DumpOptions};

    use std::time::Instant;

    use crate::connection::ConnectParams;
    use crate::state::Session;

    /// 把已认证的线协议连接包装成一条会话。
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

/// `postgresql://user:pass@host:port/db` —— 测试够用的 URL。
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

/// 类似 [`open_stream`]，同时报告密码是否可以发送。
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
        // 服务器由测试框架选定，所以这里不做检查。
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
