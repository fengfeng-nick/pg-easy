//! SCRAM-SHA-256 登录。密码只在已经加密、或用户明确允许明文的信道上发送。

use std::sync::Mutex;

use bytes::BytesMut;
use postgres_protocol::authentication::sasl;
use postgres_protocol::authentication::sasl::ScramSha256;
use postgres_protocol::message::backend;
use postgres_protocol::message::frontend;
use tokio_util::codec::Framed;

use crate::error::AppError;

use super::error::{db_error, io_error};
use super::frame::{BackendMessages, StreamIo};
use fallible_iterator::FallibleIterator;

use super::startup::{next_login_message, require_password, send_raw, unexpected_detail};
use super::{StartupParams, Stream};

pub(crate) async fn authenticate_scram(
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

    // 密码不能发到一个无法加密的信道上：否则 `prefer` 的降级会毫无察觉。
    // 切换 SSL 模式本身解决不了问题——服务器已经拒绝了 TLS——所以要把接下来
    // 能做什么说清楚。
    if !params.ssl_verified {
        return Err(AppError::new("auth", "password_not_sent"));
    }

    // 没有通道绑定：TLS 层是我们自己做的，不经过驱动的 `TlsStream` 钩子，
    // 服务器证书的哈希在这里拿不到。
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

    // 接下来是服务器的证明（SaslFinal）和 AuthenticationOk，两者的先后顺序
    // 随版本而异：PostgreSQL 18 先确认再证明，所以把两者当作一个集合来匹配。
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

