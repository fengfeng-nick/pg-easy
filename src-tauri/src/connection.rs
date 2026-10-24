//! 连接参数、TLS 配置和 socket 层。
//!
//! 线协议客户端从每次响应末尾的 `ReadyForQuery` 里读出服务器的事务状态，
//! 状态栏因此不必再问服务器就能显示事务是打开还是已中止。

use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use rustls_pki_types::pem::PemObject;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Translated};
use crate::pg;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectParams {
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub database: String,
    pub user: String,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub ssl: SslMode,
    /// PEM 证书包。只在 `verify-ca` / `verify-full` 下读取；为空表示“使用
    /// 操作系统信任库”。
    #[serde(default)]
    pub ssl_root_cert: Option<String>,
    #[serde(default)]
    pub connect_timeout_ms: Option<u64>,
    #[serde(default)]
    pub application_name: Option<String>,
    /// 即使没有使用 TLS 也信任该信道用于认证，相当于 libpq 的
    /// `PGSSLMODE=allow`。默认关闭，因为明文信道上的 SCRAM 正是降级攻击的
    /// 样子。
    #[serde(default, alias = "allow_unencrypted_password")]
    pub allow_unencrypted_password: bool,
}

fn default_port() -> u16 {
    5432
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    Disable,
    Prefer,
    Require,
    #[default]
    VerifyFull,
}

pub fn effective_host(host: &str) -> &str {
    let host = host.trim();
    if host.is_empty() {
        "localhost"
    } else {
        host
    }
}

// ---------------------------------------------------------------------------
// 传输层

/// 打开 TCP socket，如果模式要求就再套上 TLS。
///
/// 返回的标记表示密码能否在这个信道上传输：握手已完成因而信道加密，或者用户
/// 允许了明文信道。证书校验是另一重保证，不参与这个判断，与 libpq 一致
/// （发送密码并不要求 `verify-full`，而 `prefer` 也不会不作声就退回明文）。
pub async fn open_stream(params: &ConnectParams) -> Result<(pg::Stream, bool), AppError> {
    let host = effective_host(&params.host).to_string();
    let port = params.port;
    let timeout = Duration::from_millis(params.connect_timeout_ms.unwrap_or(10_000));

    let socket = tokio::time::timeout(timeout, tokio::net::TcpStream::connect((host.as_str(), port)))
        .await
        .map_err(|_| {
            AppError::new("timeout", "connect_timed_out").with_args([host.as_str(), &port.to_string()])
        })?
        .map_err(|err| {
            AppError::new("connection", "connect_failed")
                .with_args([host.as_str(), &port.to_string()])
                .detail(io_cause(&err))
        })?;
    socket.set_nodelay(true).ok();
    let socket = tokio::io::BufStream::new(socket);

    if params.ssl == SslMode::Disable {
        let stream = pg::Stream::new(Box::new(Plain { inner: socket }));
        return Ok((stream, params.allow_unencrypted_password));
    }

    let connector = tokio_rustls::TlsConnector::from(build_tls(
        params.ssl,
        params.ssl_root_cert.as_deref(),
    )?);
    match negotiate_tls(socket, &connector, &host).await {
        Ok(stream) => {
            let stream = pg::Stream::new(Box::new(Pinned { inner: stream }));
            // TLS 握手完成就足以承载密码，即使在 `require` 下也是如此——
            // 那里只是跳过了信任链校验。
            Ok((stream, true))
        }
        Err(TlsFailure::Refused(reason)) if params.ssl == SslMode::Prefer => {
            // 服务器拒绝了加密（或证书未通过校验）。`prefer` 会像 libpq 一样
            // 退回明文 TCP。
            let _ = reason;
            let socket = tokio::time::timeout(timeout, tokio::net::TcpStream::connect((host.as_str(), port)))
                .await
                .map_err(|_| {
                    AppError::new("timeout", "connect_timed_out")
                        .with_args([host.as_str(), &port.to_string()])
                })?
                .map_err(|err| {
                    AppError::new("connection", "connect_failed")
                        .with_args([host.as_str(), &port.to_string()])
                        .detail(io_cause(&err))
                })?;
            socket.set_nodelay(true).ok();
            let stream = pg::Stream::new(Box::new(Plain {
                inner: tokio::io::BufStream::new(socket),
            }));
            Ok((stream, params.allow_unencrypted_password))
        }
        Err(TlsFailure::Refused(reason)) => Err(reason.into_error(
            AppError::new("tls", "not_accepting_tls").with_args([host.as_str(), &port.to_string()]),
        )),
        Err(TlsFailure::Failed(reason)) => Err(reason.into_error(
            AppError::new("tls", "tls_handshake_failed").with_args([&host]),
        )),
    }
}

/// TLS 尝试没有产出流的原因。
enum TlsFailure {
    /// 服务器对 SSL 请求回了 `N`（不支持 TLS）。
    Refused(Reason),
    /// TLS 已开始但没有完成，通常是证书问题。
    Failed(Reason),
}

/// TLS 尝试失败的原因：一个由界面翻译的 token，或来自 I/O 栈的自由文本。
enum Reason {
    Token(Translated),
    Text(String),
}

impl Reason {
    fn new(token: &str) -> Self {
        Self::Token(Translated::new(token))
    }

    fn args<I, A>(self, args: I) -> Self
    where
        I: IntoIterator<Item = A>,
        A: ToString,
    {
        match self {
            Self::Token(translated) => {
                Self::Token(translated.args(args.into_iter().map(|arg| arg.to_string())))
            }
            Self::Text(text) => Self::Text(text),
        }
    }

    fn io(text: String) -> Self {
        Self::Text(text)
    }

    fn into_error(self, error: AppError) -> AppError {
        match self {
            Self::Token(translated) => error.with_translated_detail(translated),
            Self::Text(text) => error.detail(text),
        }
    }
}

fn io_cause(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

async fn negotiate_tls(
    socket: tokio::io::BufStream<tokio::net::TcpStream>,
    connector: &tokio_rustls::TlsConnector,
    host: &str,
) -> Result<tokio_rustls::client::TlsStream<tokio::io::BufStream<tokio::net::TcpStream>>, TlsFailure> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut socket = socket;
    let mut request = bytes::BytesMut::new();
    postgres_protocol::message::frontend::ssl_request(&mut request);
    socket
        .write_all(&request)
        .await
        .map_err(|err| TlsFailure::Failed(Reason::io(io_cause(&err))))?;
    socket
        .flush()
        .await
        .map_err(|err| TlsFailure::Failed(Reason::io(io_cause(&err))))?;

    let mut answer = [0u8; 1];
    socket
        .read_exact(&mut answer)
        .await
        .map_err(|err| TlsFailure::Failed(Reason::io(io_cause(&err))))?;
    if answer[0] != b'S' {
        return Err(TlsFailure::Refused(Reason::new("server_said_plaintext")));
    }

    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| TlsFailure::Failed(Reason::new("invalid_server_name").args([host])))?;
    connector
        .connect(server_name, socket)
        .await
        .map_err(|err| TlsFailure::Failed(Reason::io(io_cause(&err))))
}

/// 明文 TCP。
struct Plain {
    inner: tokio::io::BufStream<tokio::net::TcpStream>,
}

/// TLS。
struct Pinned {
    inner: tokio_rustls::client::TlsStream<tokio::io::BufStream<tokio::net::TcpStream>>,
}

/// 把读写转发给真正的 socket。
macro_rules! delegate_io {
    ($ty:ty) => {
        impl tokio::io::AsyncRead for $ty {
            fn poll_read(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
            }
        }

        impl tokio::io::AsyncWrite for $ty {
            fn poll_write(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                buf: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
            }

            fn poll_flush(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::pin::Pin::new(&mut self.inner).poll_flush(cx)
            }

            fn poll_shutdown(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
            }
        }
    };
}

delegate_io!(Plain);
delegate_io!(Pinned);

macro_rules! impl_conn {
    ($ty:ty) => {
        impl pg::Conn for $ty {}
    };
}

impl_conn!(Plain);
impl_conn!(Pinned);

// ---------------------------------------------------------------------------
// TLS 配置

fn build_tls(mode: SslMode, root_cert: Option<&str>) -> Result<Arc<ClientConfig>, AppError> {
    // 进程级的 provider 重复安装无害；后续调用会返回已安装的那个。
    let _ = CryptoProvider::install_default(ring::default_provider());

    let roots = match root_cert.map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) => roots_from_file(path)?,
        None => {
            let mut store = RootCertStore::empty();
            let native = rustls_native_certs::load_native_certs();
            for cert in native.certs {
                let _ = store.add(cert);
            }
            store
        }
    };

    let mut config = ClientConfig::builder_with_protocol_versions(&[
        &rustls::version::TLS13,
        &rustls::version::TLS12,
    ])
    .with_root_certificates(roots)
    .with_no_client_auth();

    if mode == SslMode::Require {
        // 加密但不认证服务器：对应 libpq 的 `require`。签名仍会拿服务器证书
        // 校验，只是跳过了信任链。
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(AcceptAnyServerCert::new()));
    }

    Ok(Arc::new(config))
}

fn roots_from_file(path: &str) -> Result<RootCertStore, AppError> {
    let der = CertificateDer::pem_file_iter(path)
        .map_err(|err| {
            AppError::new("config", "root_cert_read")
                .with_io_path(std::path::Path::new(path), &err)
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            AppError::new("config", "root_cert_invalid")
                .with_io_path(std::path::Path::new(path), &err)
        })?;

    let mut store = RootCertStore::empty();
    let (added, _) = store.add_parsable_certificates(der);
    if added == 0 {
        return Err(AppError::new(
            "config",
            "root_cert_no_cert",
        ));
    }
    Ok(store)
}

/// 加密信道但不认证服务器。握手签名仍会校验，所以这不是一个空操作校验器。
#[derive(Debug)]
struct AcceptAnyServerCert {
    provider: Arc<CryptoProvider>,
}

impl AcceptAnyServerCert {
    fn new() -> Self {
        Self {
            provider: CryptoProvider::get_default()
                .cloned()
                .unwrap_or_else(|| Arc::new(ring::default_provider())),
        }
    }
}

impl ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_host_means_localhost() {
        assert_eq!(effective_host("  "), "localhost");
        assert_eq!(effective_host("db.internal"), "db.internal");
    }

}
