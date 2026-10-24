//! Connection parameters, TLS setup and the socket layer.
//!
//! The wire client reports the server's transaction state from the
//! `ReadyForQuery` that ends each response, which is what lets the status bar
//! show an open or aborted transaction without asking the server again.

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
    /// PEM bundle. Only consulted for `verify-ca` / `verify-full`; empty means
    /// "use the operating system trust store".
    #[serde(default)]
    pub ssl_root_cert: Option<String>,
    #[serde(default)]
    pub connect_timeout_ms: Option<u64>,
    #[serde(default)]
    pub application_name: Option<String>,
    /// Trust the channel for authentication even when TLS is not in use, the
    /// way libpq's `PGSSLMODE=allow` does. Off by default because SCRAM over a
    /// clear-text channel is exactly what a downgrade attack looks like.
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
// Transport

/// Opens the TCP socket and, if the mode asks for it, wraps it in TLS.
///
/// The flag says whether the password may travel over the result: the channel
/// is encrypted by a handshake that completed, or the user allowed a
/// clear-text channel. Certificate checking is a separate guarantee and is not
/// what decides this, matching libpq (`verify-full` is not required to send a
/// password, and `prefer` does not fall back to clear text without saying so).
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
            // A completed TLS handshake is enough to carry the password, even
            // under `require`, where only the chain of trust is skipped.
            Ok((stream, true))
        }
        Err(TlsFailure::Refused(reason)) if params.ssl == SslMode::Prefer => {
            // The server declined encryption (or its certificate did not
            // verify). `prefer` falls back to plain TCP, as libpq does.
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

/// Why a TLS attempt did not produce a stream.
enum TlsFailure {
    /// The server answered `N` to the SSL request (does not speak TLS).
    Refused(Reason),
    /// TLS started but did not complete, typically a certificate problem.
    Failed(Reason),
}

/// Why a TLS attempt failed: a token the UI translates, or free text from the
/// I/O stack.
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

/// Plain TCP.
struct Plain {
    inner: tokio::io::BufStream<tokio::net::TcpStream>,
}

/// TLS.
struct Pinned {
    inner: tokio_rustls::client::TlsStream<tokio::io::BufStream<tokio::net::TcpStream>>,
}

/// Forwards reads and writes to the real socket.
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
// TLS configuration

fn build_tls(mode: SslMode, root_cert: Option<&str>) -> Result<Arc<ClientConfig>, AppError> {
    // Installing the process-wide provider twice is harmless; subsequent
    // calls return the already-installed one.
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
        // Encrypted but not authenticated: libpq's `require`. Signatures are
        // still verified against the server's certificate; only the chain of
        // trust is skipped.
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

/// Encrypts the channel without authenticating the server. Handshake
/// signatures are still verified, so this is not a no-op verifier.
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
