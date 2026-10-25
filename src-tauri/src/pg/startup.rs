//! 启动握手：StartupMessage、明文 / MD5 / SCRAM，直到 ReadyForQuery。

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

use bytes::BytesMut;
use postgres_protocol::authentication;
use postgres_protocol::message::backend;
use postgres_protocol::message::frontend;
use tokio::io::AsyncWrite;
use tokio_util::codec::Framed;
use tokio_util::io::poll_read_buf;

use crate::error::AppError;

use super::error::{db_error, io_error};
use super::frame::{BackendMessages, StreamIo};
use super::{Connection, LOGIN_TIMEOUT, StartupParams, Stream};

// ---------------------------------------------------------------------------
// 启动

pub async fn connect(stream: Stream, params: &StartupParams) -> Result<Connection, AppError> {
    // TLS 在这之前就已决定；从这里开始只有一种协议。
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

/// 启动交互。返回它了解到的东西，以及分帧层在登录的 `ReadyForQuery` 之后
/// 就已多读的部分——这些必须送到连接自己的读端。
pub(crate) async fn login(
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
            super::auth::authenticate_scram(reader, writer, body, params).await?
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

/// 登录交互的下一条消息。
pub(crate) async fn next_login_message(
    reader: &mut Framed<StreamIo, BackendMessages>,
) -> Result<Option<backend::Message>, AppError> {
    // 否则一台不再应答的服务器会让连接表单一直卡住。
    match tokio::time::timeout(LOGIN_TIMEOUT, read_login_message(reader)).await {
        Ok(Ok(message)) => Ok(message),
        Ok(Err(err)) => Err(err),
        Err(_) => Err(AppError::new("timeout", "login_timeout")
            .with_token_detail("no_answer_for", [LOGIN_TIMEOUT.as_secs()])),
    }
}

/// 把期望是别的东西的消息命名出来。
pub(crate) fn unexpected_detail(other: &Option<backend::Message>) -> String {
    match other {
        Some(message) => message_name(message).to_string(),
        None => "end of login".into(),
    }
}

/// 读取登录交互的下一条消息，丢掉服务器喜欢在 `STARTUP` 之后立刻发送的
/// NOTICE。`None` 表示 socket 已关闭。
async fn read_login_message(
    reader: &mut Framed<StreamIo, BackendMessages>,
) -> Result<Option<backend::Message>, AppError> {
    loop {
        if let Some(message) = next_login_frame(reader).await? {
            match message {
                // 第一条查询之前的通知都是噪音。
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

/// 登录缓冲区里的下一条完整消息（如果已经到达）。缓冲区保留其位置：
/// `Framed` 会预留这条消息需要的空间，所以不能把空缓冲区交给它。
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
            // 服务器在就绪之前就关闭了连接。
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

pub(crate) async fn send_raw(writer: &Mutex<Stream>, buf: BytesMut) -> Result<(), AppError> {
    write_guarded(writer, buf)
        .await
        .map_err(|err| io_error("connection_lost", err))
}

/// 把 `buf` 写到受保护的 socket 上并刷新。
///
/// 写 future 只在本次轮询持有锁时被轮询，因此守卫不会进入外层 future 的
/// 状态；socket 忙时只需让出并重试。
pub(super) async fn write_guarded(socket: &Mutex<Stream>, buf: impl AsRef<[u8]>) -> std::io::Result<()> {
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

pub(crate) fn require_password(params: &StartupParams) -> Result<String, AppError> {
    params.password.clone().ok_or_else(|| {
        AppError::new("auth", "password_required_for").with_args([&params.user])
    })
}
