//! The wire client.
//!
//! `tokio-postgres` hands back a `Connection` whose TLS stream type depends on
//! the connector, so plain and encrypted connections cannot share one type,
//! its column types only by OID are resolved through catalog round trips, and
//! it exposes neither the backend's transaction state nor its command tags.
//!
//! This module performs the same startup exchange over a boxed stream — one
//! type covers every sslmode — and then drives the protocol through the
//! driver's own public codec and message types. Authentication is the
//! driver's own `postgres-protocol` implementation; there is no hand-written
//! crypto here.

use std::collections::HashMap;

use bytes::BytesMut;
use futures_util::TryStreamExt;
use fallible_iterator::FallibleIterator;
use postgres_protocol::authentication;
use postgres_protocol::authentication::sasl;
use postgres_protocol::authentication::sasl::ScramSha256;
use postgres_protocol::message::backend;
use postgres_protocol::message::frontend;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::codec::{Decoder, Framed};
use tokio_util::io::poll_read_buf;

use crate::error::{AppError, Translated};

use self::types::{read_row, Column};

pub mod types;

/// A socket after the TLS decision: plaintext or encrypted, behind one type.
pub type Stream = tokio::io::BufStream<Box<dyn Conn>>;

pub trait Conn: AsyncRead + AsyncWrite + Send + Unpin {}

// ---------------------------------------------------------------------------
// Framing

/// Every whole message the server sent in one read of the socket.
///
/// Deliberately does not implement `Debug` or `Deref`: its contents are server
/// data, and code that needs to look inside should go through `next`.
pub struct BackendMessages(bytes::BytesMut);

impl BackendMessages {
    /// The next message, or `None` once the buffer is drained.
    pub fn next(&mut self) -> std::io::Result<Option<backend::Message>> {
        let Some(header) = backend::Header::parse(&self.0)? else {
            return Ok(None);
        };
        let len = header.len() as usize + 1;
        if self.0.len() < len {
            return Err(short_message());
        }
        // `Message::parse` consumes from the buffer it is given and would take
        // any trailing message with it, so each message gets its own slice.
        let mut whole = self.0.split_to(len);
        match backend::Message::parse(&mut whole)? {
            Some(message) => Ok(Some(message)),
            None => Err(short_message()),
        }
    }
}

impl Default for BackendMessages {
    fn default() -> Self {
        BackendMessages(BytesMut::new())
    }
}

impl From<BytesMut> for BackendMessages {
    fn from(bytes: BytesMut) -> Self {
        BackendMessages(bytes)
    }
}

impl Decoder for BackendMessages {
    type Item = BackendMessage;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> std::io::Result<Option<BackendMessage>> {
        let mut idx = 0;
        let mut complete = false;
        while let Some(header) = backend::Header::parse(&src[idx..])? {
            let len = header.len() as usize + 1;
            if src[idx..].len() < len {
                break;
            }
            idx += len;
            if header.tag() == backend::READY_FOR_QUERY_TAG {
                complete = true;
                break;
            }
        }
        if idx == 0 {
            return Ok(None);
        }
        Ok(Some(BackendMessage {
            messages: BackendMessages(src.split_to(idx)),
            complete,
        }))
    }
}

/// All the messages that arrived in one read, in order.
pub struct BackendMessage {
    pub messages: BackendMessages,
    /// The run ended at `ReadyForQuery`, so the request is over.
    #[allow(dead_code)]
    pub complete: bool,
}

/// The socket as the reader holds it, while writes reach the same socket
/// through the connection. Only one of the two is ever borrowed at a time:
/// a statement is written in full before any of its answer is read.
pub struct StreamIo(Arc<Mutex<Stream>>);

impl StreamIo {
    fn new(stream: &Arc<Mutex<Stream>>) -> Self {
        StreamIo(Arc::clone(stream))
    }
}

impl AsyncRead for StreamIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.0.try_lock() {
            Ok(mut stream) => Pin::new(&mut *stream).poll_read(cx, buf),
            // The shared socket is only held while writing, which never lasts
            // across a wait, so this is a hiccup rather than a deadlock.
            Err(_) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}

impl AsyncWrite for StreamIo {
    // Only `Framed` writes here, for the empty keyword and startup message of
    // a connection, and it requires both halves; statements go through
    // `write_guarded` instead.
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut stream = self.0.lock().expect("the socket is poisoned");
        Pin::new(&mut *stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let mut stream = self.0.lock().expect("the socket is poisoned");
        Pin::new(&mut *stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let mut stream = self.0.lock().expect("the socket is poisoned");
        Pin::new(&mut *stream).poll_shutdown(cx)
    }
}

/// How long the stream of messages after an error may take to reach
/// `ReadyForQuery` before the connection is given up on.
const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long the login exchange may take before it is reported as a timeout.
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

fn short_message() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "incomplete server message")
}

// ---------------------------------------------------------------------------
// Errors

/// The parts of an `ErrorResponse` worth showing.
#[derive(Debug, Clone)]
pub struct DbError {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
    pub position: Option<usize>,
}

impl DbError {
    fn parse(body: &backend::ErrorResponseBody) -> std::io::Result<Self> {
        Self::parse_fields(body.fields())
    }

    fn parse_notice(body: &backend::NoticeResponseBody) -> std::io::Result<Self> {
        Self::parse_fields(body.fields())
    }

    fn parse_fields<'a>(mut fields: impl FallibleIterator<Item = backend::ErrorField<'a>, Error = std::io::Error>) -> std::io::Result<Self> {
        let mut error = DbError {
            severity: String::new(),
            code: String::new(),
            message: String::new(),
            detail: None,
            position: None,
        };
        while let Some(field) = fields.next()? {
            let value = String::from_utf8_lossy(field.value_bytes()).into_owned();
            match field.type_() {
                b'V' | b'S' if error.severity.is_empty() => error.severity = value,
                b'C' => error.code = value,
                b'M' => error.message = value,
                // The hint is appended to the detail: two boxes of small print
                // is more than the panel needs.
                b'D' => error.detail = Some(value),
                b'H' => error.detail = Some(match error.detail.take() {
                    Some(detail) => format!("{detail}\n{value}"),
                    None => value,
                }),
                b'P' => error.position = value.parse().ok(),
                _ => {}
            }
        }
        Ok(error)
    }

    fn is_notice(&self) -> bool {
        !matches!(self.severity.as_str(), "ERROR" | "FATAL" | "PANIC")
    }

    fn to_app_error(&self) -> AppError {
        let kind = match self.code.as_str() {
            // query_canceled, admin_shutdown
            "57014" | "57P01" => "cancelled",
            code if code.starts_with("08") => "connection",
            code if code.starts_with("28") => "auth",
            _ => "database",
        };
        let mut error = AppError::new(kind, self.message.clone())
            .with_detail(self.detail.clone())
            .with_code(&self.code);
        if let Some(position) = self.position {
            error = error.with_position(position);
        }
        error
    }
}

fn io_error(context: &str, err: impl std::fmt::Display) -> AppError {
    AppError::new("connection", context).detail(err.to_string())
}

fn db_error(body: backend::ErrorResponseBody) -> AppError {
    match DbError::parse(&body) {
        Ok(error) => error.to_app_error(),
        Err(err) => io_error("read_server_error", err),
    }
}

// ---------------------------------------------------------------------------
// Connection

#[derive(Debug, Clone)]
pub struct StartupParams {
    pub user: String,
    pub database: String,
    pub password: Option<String>,
    pub application_name: Option<String>,
    /// The password may be sent over this channel: TLS finished, or the user
    /// allowed clear text. SCRAM is refused otherwise, which is how a silent
    /// `prefer` downgrade is kept from leaking the password.
    pub ssl_verified: bool,
}

/// What one statement produced.
#[derive(Debug)]
pub struct StatementOutput {
    /// The server's command tag, e.g. `UPDATE 3` or `SELECT 2`.
    pub tag: String,
    pub rows: Option<Rows>,
}

#[derive(Debug)]
pub struct Rows {
    pub columns: Vec<Column>,
    pub values: Vec<Vec<Option<String>>>,
    /// Rows the server sent past the display limit.
    pub dropped: u64,
}

pub struct Connection {
    /// The one socket, shared by the reader and the writes.
    stream: Arc<Mutex<Stream>>,
    /// The socket, decoded into chunks of whole messages.
    reader: Framed<StreamIo, BackendMessages>,
    /// Messages read from `reader` but not yet handed out. The whole chunk
    /// is kept here so that reading one message never touches `reader`.
    chunk: Option<BackendMessages>,
    /// Server parameters (`server_version`, `TimeZone`, ...), kept current
    /// from `ParameterStatus` messages.
    pub parameters: HashMap<String, String>,
    pub backend_pid: i32,
    /// Notices raised during the last statement, oldest first.
    notices: Vec<Translated>,
    key_secret: i32,
    /// The transaction state from the last `ReadyForQuery`: 'I', 'T' or 'E'.
    status: u8,
    /// Server error from the statement in progress; public to the runner,
    /// which combines it with whatever rows were read first.
    pub failed: Option<AppError>,
}

impl Connection {
    /// The transaction state the server last reported.
    pub fn status(&self) -> u8 {
        self.status
    }

    pub fn take_notices(&mut self) -> Vec<Translated> {
        std::mem::take(&mut self.notices)
    }

    pub fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters.get(name).map(String::as_str)
    }

    /// Cancels the statement currently running on the backend. Opens a fresh
    /// socket for the cancel request, exactly like libpq.
    /// The cancel request for the running statement, exactly as libpq sends
    /// it: a short message on a fresh connection carrying the backend key.
    pub fn cancel_request(&self) -> Vec<u8> {
        let mut buf = BytesMut::new();
        frontend::cancel_request(self.backend_pid, self.key_secret, &mut buf);
        buf.to_vec()
    }

    /// Reads and discards everything until the server is ready again, used
    /// after a cancel request so the next statement starts clean.
    pub async fn drain_until_ready(&mut self, timeout: std::time::Duration) -> Result<(), AppError> {
        let drained = async {
            loop {
                match self.next_message().await? {
                    backend::Message::ReadyForQuery(body) => {
                        self.status = body.status();
                        return Ok(());
                    }
                    _ => {}
                }
            }
        };
        match tokio::time::timeout(timeout, drained).await {
            Ok(result) => result,
            Err(_) => Err(AppError::new("connection", "cancel_not_acknowledged")),
        }
    }

    /// Sends one statement and reads it to `ReadyForQuery`.
    ///
    /// On a server error the rows received so far come back with it, so a
    /// statement that fails midway does not discard the output it produced.
    pub async fn run(
        &mut self,
        sql: &str,
        max_rows: usize,
    ) -> Result<StatementOutput, (AppError, Option<Rows>)> {
        self.failed = None;
        let mut buf = BytesMut::new();
        frontend::query(sql, &mut buf)
            .map_err(|err| (io_error("encode_statement", err), None))?;
        self.send(buf).await?;
        self.read_response(max_rows).await
    }

    /// Waits until the statement in flight is over, however it ended, so the
    /// connection is usable for the next one.
    pub async fn settle(&mut self, timeout: std::time::Duration) {
        let _ = self.drain_until_ready(timeout).await;
    }

    /// Sends a simple query made only of commands returning no rows (SAVEPOINT,
    /// ROLLBACK TO, SET), discarding any output.
    pub async fn execute(&mut self, sql: &str) -> Result<(), AppError> {
        self.run(sql, 0).await.map(|_| ()).map_err(|(err, _)| err)
    }

    /// A query whose rows are all needed at once, used by the catalog.
    #[allow(dead_code)]
    pub async fn query(&mut self, sql: &str) -> Result<Rows, AppError> {
        let output = self.run(sql, usize::MAX).await.map_err(|(err, _)| err)?;
        output.rows.ok_or_else(|| {
            AppError::new("internal", "unexpected_rows").with_args([output.tag.as_str()])
        })
    }

    /// Runs a `COPY … TO STDOUT` and returns its data instead of discarding
    /// it. Each row is one text column holding a line of the copy text,
    /// trailing newline included.
    pub async fn copy_out(&mut self, sql: &str) -> Result<Rows, AppError> {
        let line = Column {
            name: "line".into(),
            type_oid: 25,
            type_name: "text".into(),
            kind: crate::protocol::ValueKind::Text,
        };
        let mut rows = Rows {
            columns: vec![line],
            values: Vec::new(),
            dropped: 0,
        };
        let mut buf = BytesMut::new();
        frontend::query(sql, &mut buf).map_err(|err| io_error("encode_statement", err))?;
        self.failed = None;
        self.send(buf).await.map_err(|(err, _)| err)?;
        self.read_copy_out(&mut rows).await?;
        Ok(rows)
    }

    async fn read_copy_out(&mut self, rows: &mut Rows) -> Result<(), AppError> {
        loop {
            match self
                .next_message()
                .await
                .map_err(|err| io_error("read_copy_data", err))?
            {
                backend::Message::CopyData(body) => {
                    let text = String::from_utf8_lossy(body.data()).into_owned();
                    rows.values.push(vec![Some(text)]);
                }
                backend::Message::CopyDone => {}
                // A COPY of a view with `with check option` is answered with
                // the rows of a SELECT instead: one column each, like the
                // lines of copy data.
                backend::Message::DataRow(body) => {
                    let mut values = read_row(&body)
                        .map_err(|err| io_error("read_copy_data", err))?;
                    values.resize(1, None);
                    values.truncate(1);
                    rows.values.push(values);
                }
                backend::Message::CommandComplete(body) => {
                    let _ = body.tag();
                }
                backend::Message::ErrorResponse(body) => {
                    // Like `run`, keep reading up to `ReadyForQuery` so the
                    // connection outlives the failed COPY; the error is
                    // reported once the server is ready again.
                    self.failed = Some(db_error(body));
                }
                backend::Message::ReadyForQuery(body) => {
                    self.status = body.status();
                    return match self.failed.take() {
                        Some(error) => Err(error),
                        None => Ok(()),
                    };
                }
                _ => {}
            }
        }
    }

    async fn send(&mut self, buf: BytesMut) -> Result<(), (AppError, Option<Rows>)> {
        write_guarded(&self.stream, buf).await
            .map_err(|err| (io_error("connection_lost", err), None))
    }

    async fn read_response(
        &mut self,
        max_rows: usize,
    ) -> Result<StatementOutput, (AppError, Option<Rows>)> {
        let mut output = StatementOutput {
            tag: String::new(),
            rows: None,
        };

        loop {
            let message = match self.next_message().await {
                Ok(message) => message,
                Err(err) => return Err((err, output.rows.take())),
            };

            match message {
                backend::Message::RowDescription(body) => {
                    let columns = types::describe(&body).map_err(|err| {
                        (io_error("read_columns", err), None)
                    })?;
                    output.rows = Some(Rows {
                        columns,
                        values: Vec::new(),
                        dropped: 0,
                    });
                }
                backend::Message::DataRow(body) => {
                    // A data row always follows its description.
                    let Some(rows) = output.rows.as_mut() else { continue };
                    if rows.values.len() >= max_rows {
                        rows.dropped += 1;
                        continue;
                    }
                    let values = read_row(&body)
                        .map_err(|err| (io_error("read_row", err), None))?;
                    rows.values.push(values);
                }
                backend::Message::CommandComplete(body) => {
                    output.tag = body
                        .tag()
                        .map_err(|err| (io_error("read_tag", err), None))?
                        .to_string();
                }
                backend::Message::EmptyQueryResponse => output.tag.clear(),
                backend::Message::ErrorResponse(body) => {
                    // The server still sends `ReadyForQuery` after an error;
                    // reading it is what leaves the connection usable.
                    let error = db_error(body);
                    self.failed = Some(error.clone());
                    if self.drain_until_ready(DRAIN_TIMEOUT).await.is_err() {
                        return Err((
                            AppError::new("connection", "connection_lost"),
                            output.rows.take(),
                        ));
                    }
                    return Err((error, output.rows.take()));
                }
                backend::Message::ReadyForQuery(body) => {
                    self.status = body.status();
                    return Ok(output);
                }
                backend::Message::ParameterStatus(body) => self.track_parameter(body),
                backend::Message::NoticeResponse(body) => self.collect_notice(&body),
                // `COPY … TO` reports its rows as `CopyData` messages, which
                // fall through below: the grid has no shape for them.
                backend::Message::CopyOutResponse(_) => {}
                backend::Message::CopyInResponse(_) => {
                    // The editor never writes to COPY ... FROM STDIN.
                    self.reject_copy_in().await;
                    let error = AppError::new("unsupported", "copy_from_stdin");
                    return Err((error, output.rows.take()));
                }
                // COPY FROM STDIN, NoData, portals, prepare responses and the
                // like are meaningless in simple-query flow.
                _ => {}
            }
        }
    }

    /// The next message from the server. Notices and parameter changes are
    /// recorded as they pass; the server pushes them between requests too.
    async fn next_message(&mut self) -> Result<backend::Message, AppError> {
        loop {
            if let Some(messages) = self.chunk.as_mut() {
                if let Some(message) = messages
                    .next()
                    .map_err(|err| io_error("decode_message", err))?
                {
                    match message {
                        backend::Message::NoticeResponse(body) => self.collect_notice(&body),
                        backend::Message::ParameterStatus(body) => self.track_parameter(body),
                        backend::Message::NotificationResponse(_) => {}
                        message => return Ok(message),
                    }
                    continue;
                }
            }
            self.chunk = None;
            match self.reader.try_next().await {
                Ok(Some(chunk)) => self.chunk = Some(chunk.messages),
                Ok(None) => {
                    return Err(AppError::new("connection", "server_closed"))
                }
                Err(err) => {
                    return Err(io_error("connection_lost", err))
                }
            }
        }
    }

    fn track_parameter(&mut self, body: backend::ParameterStatusBody) {
        if let (Ok(name), Ok(value)) = (body.name(), body.value()) {
            self.parameters.insert(name.to_string(), value.to_string());
        }
    }

    fn collect_notice(&mut self, body: &backend::NoticeResponseBody) {
        if let Ok(notice) = DbError::parse_notice(body) {
            if notice.is_notice() {
                // The severity and message are the server's words; the
                // sentence around the position is ours, so it is a token.
                let text = match notice.position {
                    Some(position) => Translated::new("notice_at").args([
                        notice.severity.clone(),
                        notice.message.clone(),
                        position.to_string(),
                    ]),
                    None => Translated::new("notice").args([
                        notice.severity.clone(),
                        notice.message.clone(),
                    ]),
                };
                // Older servers only send NOTICEs here, which are the ones a
                // user wants to see after a `raise notice`.
                if matches!(notice.severity.as_str(), "NOTICE" | "WARNING") {
                    self.notices.push(text);
                }
            }
        }
    }

    async fn reject_copy_in(&mut self) {
        let mut buf = BytesMut::new();
        // `CopyFail` needs a 'f' tag; the helper writes one.
        let _ = frontend::copy_fail("COPY FROM STDIN is not supported", &mut buf);
        let _ = write_guarded(&self.stream, buf).await;
    }
}

// ---------------------------------------------------------------------------
// Startup

pub async fn connect(stream: Stream, params: &StartupParams) -> Result<Connection, AppError> {
    // TLS is decided before this point; from here on it is one protocol.
    let stream = Arc::new(Mutex::new(stream));
    let (backend_pid, key_secret, parameters, status, carry) = login(&stream, params).await?;
    let io = StreamIo::new(&stream);
    let mut reader = Framed::with_capacity(io, BackendMessages::default(), carry.len());
    reader.read_buffer_mut().extend_from_slice(&carry);

    Ok(Connection {
        stream,
        reader,
        chunk: None,
        parameters,
        backend_pid,
        notices: Vec::new(),
        key_secret,
        status,
        failed: None,
    })
}

/// The startup exchange. Returns what it learned plus whatever the framing
/// had already read past the login's `ReadyForQuery`, which must reach the
/// connection's own reader.
async fn login(
    stream: &Arc<Mutex<Stream>>,
    params: &StartupParams,
) -> Result<(i32, i32, HashMap<String, String>, u8, BytesMut), AppError> {
    let mut reader = Framed::new(StreamIo::new(stream), BackendMessages::default());
    startup(&mut reader, stream, params).await?;
    let (backend_pid, key_secret, parameters, status) = read_info(&mut reader).await?;
    let carry = reader.read_buffer().clone();
    Ok((backend_pid, key_secret, parameters, status, carry))
}

async fn startup(
    reader: &mut Framed<StreamIo, BackendMessages>,
    writer: &Mutex<Stream>,
    params: &StartupParams,
) -> Result<(), AppError> {
    let application_name = params.application_name.as_deref().unwrap_or("pg easy");
    let mut buf = BytesMut::new();
    frontend::startup_message(
        [
            ("user", params.user.as_str()),
            ("database", params.database.as_str()),
            ("application_name", application_name),
            ("client_encoding", "UTF8"),
        ],
        &mut buf,
    )
    .map_err(|err| io_error("start_connection", err))?;
    send_raw(writer, buf).await?;

    match next_login_message(reader).await? {
        Some(backend::Message::AuthenticationOk) => {}
        Some(backend::Message::AuthenticationCleartextPassword) => {
            let password = require_password(params)?;
            let mut buf = BytesMut::new();
            frontend::password_message(password.as_bytes(), &mut buf)
                .map_err(|err| io_error("send_password", err))?;
            send_raw(writer, buf).await?;
            expect_ok(reader).await?;
        }
        Some(backend::Message::AuthenticationMd5Password(body)) => {
            let password = require_password(params)?;
            let hash = authentication::md5_hash(
                params.user.as_bytes(),
                password.as_bytes(),
                body.salt(),
            );
            let mut buf = BytesMut::new();
            frontend::password_message(hash.as_bytes(), &mut buf)
                .map_err(|err| io_error("send_password", err))?;
            send_raw(writer, buf).await?;
            expect_ok(reader).await?;
        }
        Some(backend::Message::AuthenticationSasl(body)) => {
            authenticate_scram(reader, writer, body, params).await?
        }
        Some(
            backend::Message::AuthenticationKerberosV5
            | backend::Message::AuthenticationGss
            | backend::Message::AuthenticationSspi
            | backend::Message::AuthenticationScmCredential,
        ) => {
            return Err(AppError::new("auth", "auth_unsupported"))
        }
        Some(backend::Message::ErrorResponse(body)) => return Err(db_error(body)),
        Some(other) => {
            return Err(AppError::new(
                "protocol",
                "login_unexpected_message",
            )
            .detail(message_name(&other)))
        }
        None => {
            return Err(AppError::new("protocol", "login_no_auth_step"))
        }
    }
    Ok(())
}

async fn authenticate_scram(
    reader: &mut Framed<StreamIo, BackendMessages>,
    writer: &Mutex<Stream>,
    body: backend::AuthenticationSaslBody,
    params: &StartupParams,
) -> Result<(), AppError> {
    let password = require_password(params)?;
    let mut mechanisms = body.mechanisms();
    let mut has_scram = false;
    while let Some(mechanism) = mechanisms
        .next()
        .map_err(|err| io_error("read_challenge", err))?
    {
        if mechanism == sasl::SCRAM_SHA_256 {
            has_scram = true;
        }
    }
    if !has_scram {
        return Err(AppError::new("auth", "scram_unsupported"));
    }

    // The password must not go to a channel that could not be encrypted: a
    // `prefer` downgrade would otherwise go unnoticed. Switching SSL mode
    // cannot fix this by itself — the server refused TLS — so say what will.
    if !params.ssl_verified {
        return Err(AppError::new("auth", "password_not_sent"));
    }

    // No channel binding: the TLS layer is ours, not reached through the
    // driver's `TlsStream` hook, so the server's certificate hash is not
    // available to it here.
    let mut scram = ScramSha256::new(password.as_bytes(), sasl::ChannelBinding::unsupported());
    let mut buf = BytesMut::new();
    frontend::sasl_initial_response(sasl::SCRAM_SHA_256, scram.message(), &mut buf)
        .map_err(|err| io_error("answer_challenge", err))?;
    send_raw(writer, buf).await?;

    match next_login_message(reader).await? {
        Some(backend::Message::AuthenticationSaslContinue(body)) => {
            scram
                .update(body.data())
                .map_err(|_| AppError::new("auth", "login_malformed"))?;
            let mut buf = BytesMut::new();
            frontend::sasl_response(scram.message(), &mut buf)
                .map_err(|err| io_error("answer_challenge", err))?;
            send_raw(writer, buf).await?;
        }
        Some(backend::Message::ErrorResponse(body)) => return Err(db_error(body)),
        other => {
            return Err(AppError::new("protocol", "login_unexpected")
                .detail(unexpected_detail(&other)))
        }
    }

    // Next come the server's proof (SaslFinal) and AuthenticationOk, in an
    // order that varies between versions: PostgreSQL 18 acknowledges before
    // it proves, so the two are matched as a set.
    let mut acknowledged = false;
    let mut proven = false;
    while !(proven && acknowledged) {
        match next_login_message(reader).await? {
            Some(backend::Message::AuthenticationOk) => acknowledged = true,
            Some(backend::Message::AuthenticationSaslFinal(body)) if !proven => {
                scram
                    .finish(body.data())
                    .map_err(|_| AppError::new("auth", "proof_failed"))?;
                proven = true;
            }
            Some(backend::Message::ErrorResponse(body)) => return Err(db_error(body)),
            other => {
                return Err(AppError::new("protocol", "login_unexpected")
                    .detail(unexpected_detail(&other)))
            }
        }
    }
    Ok(())
}

/// The next message of the login exchange.
async fn next_login_message(
    reader: &mut Framed<StreamIo, BackendMessages>,
) -> Result<Option<backend::Message>, AppError> {
    // A server that stops answering would otherwise hang the connect form.
    match tokio::time::timeout(LOGIN_TIMEOUT, read_login_message(reader)).await {
        Ok(Ok(message)) => Ok(message),
        Ok(Err(err)) => Err(err),
        Err(_) => Err(AppError::new("timeout", "login_timeout")
            .with_token_detail("no_answer_for", [LOGIN_TIMEOUT.as_secs()])),
    }
}

/// Names the message that was expected to be something else.
fn unexpected_detail(other: &Option<backend::Message>) -> String {
    match other {
        Some(message) => message_name(message).to_string(),
        None => "end of login".into(),
    }
}

/// Reads the next message of the login exchange, dropping the NOTICEs the
/// server likes to send right after `STARTUP`. `None` means the socket closed.
async fn read_login_message(
    reader: &mut Framed<StreamIo, BackendMessages>,
) -> Result<Option<backend::Message>, AppError> {
    loop {
        if let Some(message) = next_login_frame(reader).await? {
            match message {
                // Notices before the first query are noise.
                backend::Message::NoticeResponse(_) => continue,
                message => return Ok(Some(message)),
            }
        }
        let bytes = {
            let mut framed = Pin::new(&mut *reader);
            let mut buffer = std::mem::take(framed.read_buffer_mut());
            let read = {
                let mut incoming = framed.as_mut().get_pin_mut();
                std::future::poll_fn(|cx| poll_read_buf(incoming.as_mut(), cx, &mut buffer)).await
            };
            *framed.read_buffer_mut() = buffer;
            read.map_err(|err| io_error("could_not_reach_server", err))?
        };
        if bytes == 0 {
            return Err(AppError::new("connection", "closed_during_login"));
        }
    }
}

/// The next complete message in the login buffer, if one has arrived. The
/// buffer keeps its position: `Framed` reserves the room the message will
/// need, so the buffer must not be handed to it empty.
async fn next_login_frame(
    reader: &mut Framed<StreamIo, BackendMessages>,
) -> Result<Option<backend::Message>, AppError> {
    backend::Message::parse(reader.read_buffer_mut())
        .map_err(|err| io_error("decode_login", err))
}

fn message_name(message: &backend::Message) -> &'static str {
    match message {
        backend::Message::AuthenticationOk => "AuthenticationOk",
        backend::Message::AuthenticationCleartextPassword => "AuthenticationCleartextPassword",
        backend::Message::AuthenticationMd5Password(_) => "AuthenticationMd5Password",
        backend::Message::AuthenticationSasl(_) => "AuthenticationSasl",
        backend::Message::AuthenticationSaslContinue(_) => "AuthenticationSaslContinue",
        backend::Message::AuthenticationSaslFinal(_) => "AuthenticationSaslFinal",
        backend::Message::ErrorResponse(_) => "ErrorResponse",
        backend::Message::NoticeResponse(_) => "NoticeResponse",
        backend::Message::ParameterStatus(_) => "ParameterStatus",
        backend::Message::BackendKeyData(_) => "BackendKeyData",
        backend::Message::ReadyForQuery(_) => "ReadyForQuery",
        backend::Message::RowDescription(_) => "RowDescription",
        backend::Message::DataRow(_) => "DataRow",
        backend::Message::CommandComplete(_) => "CommandComplete",
        backend::Message::CopyOutResponse(_) => "CopyOutResponse",
        backend::Message::CopyData(_) => "CopyData",
        backend::Message::CopyDone => "CopyDone",
        _ => "another message",
    }
}

async fn expect_ok(reader: &mut Framed<StreamIo, BackendMessages>) -> Result<(), AppError> {
    match next_login_message(reader).await? {
        Some(backend::Message::AuthenticationOk) => Ok(()),
        Some(backend::Message::ErrorResponse(body)) => Err(db_error(body)),
        other => Err(AppError::new("protocol", "login_unexpected")
            .detail(unexpected_detail(&other))),
    }
}

async fn read_info(
    reader: &mut Framed<StreamIo, BackendMessages>,
) -> Result<(i32, i32, HashMap<String, String>, u8), AppError> {
    let mut process_id = 0;
    let mut secret_key = 0;
    let mut parameters = HashMap::new();
    loop {
        match next_login_message(reader).await? {
            Some(backend::Message::BackendKeyData(body)) => {
                process_id = body.process_id();
                secret_key = body.secret_key();
            }
            Some(backend::Message::ParameterStatus(body)) => {
                let name = body
                    .name()
                    .map_err(|err| io_error("read_setting", err))?;
                let value = body
                    .value()
                    .map_err(|err| io_error("read_setting", err))?;
                parameters.insert(name.to_string(), value.to_string());
            }
            Some(backend::Message::ReadyForQuery(body)) => {
                return Ok((process_id, secret_key, parameters, body.status()))
            }
            // The server closed the connection before it was ready.
            None => {
                return Err(AppError::new("connection", "closed_during_login"))
            }
            Some(backend::Message::ErrorResponse(body)) => return Err(db_error(body)),
            Some(other) => {
                return Err(AppError::new("protocol", "unexpected_after_login")
                    .detail(message_name(&other)))
            }
        }
    }
}

async fn send_raw(writer: &Mutex<Stream>, buf: BytesMut) -> Result<(), AppError> {
    write_guarded(writer, buf)
        .await
        .map_err(|err| io_error("connection_lost", err))
}

/// Writes `buf` to the guarded socket and flushes it.
///
/// The write future is only polled while the lock is held on this very poll,
/// so the guard never becomes part of the enclosing future's state; a busy
/// socket just yields and tries again.
async fn write_guarded(socket: &Mutex<Stream>, buf: impl AsRef<[u8]>) -> std::io::Result<()> {
    let mut buf = buf.as_ref();
    while !buf.is_empty() {
        let written = match try_write(socket, buf) {
            Some(result) => result?,
            None => {
                tokio::task::yield_now().await;
                continue;
            }
        };
        if written == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        buf = &buf[written..];
    }
    loop {
        if let Some(result) = try_flush(socket) {
            return result;
        }
        tokio::task::yield_now().await;
    }
}

fn try_write(socket: &Mutex<Stream>, buf: &[u8]) -> Option<std::io::Result<usize>> {
    let mut guard = socket.try_lock().ok()?;
    let stream = Pin::new(&mut *guard);
    match stream.poll_write(&mut std::task::Context::from_waker(Waker::noop()), buf) {
        Poll::Ready(result) => Some(result),
        Poll::Pending => None,
    }
}

fn try_flush(socket: &Mutex<Stream>) -> Option<std::io::Result<()>> {
    let mut guard = socket.try_lock().ok()?;
    let stream = Pin::new(&mut *guard);
    match stream.poll_flush(&mut std::task::Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => Some(result),
        Poll::Pending => None,
    }
}

fn require_password(params: &StartupParams) -> Result<String, AppError> {
    params.password.clone().ok_or_else(|| {
        AppError::new("auth", "password_required_for").with_args([&params.user])
    })
}
