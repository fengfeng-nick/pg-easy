//! 后端消息分帧，以及读写共用的 socket。

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::codec::Decoder;

use postgres_protocol::message::backend;

use super::Stream;

/// 服务器在一次 socket 读取中发来的全部完整消息。
///
/// 刻意不实现 `Debug` 和 `Deref`：里面装的是服务器数据，需要看内容的代码
/// 应该走 `next`。
pub struct BackendMessages(bytes::BytesMut);

impl BackendMessages {
    /// 下一条消息，缓冲区读空后为 `None`。
    pub fn next(&mut self) -> std::io::Result<Option<backend::Message>> {
        let Some(header) = backend::Header::parse(&self.0)? else {
            return Ok(None);
        };
        let len = header.len() as usize + 1;
        if self.0.len() < len {
            return Err(short_message());
        }
        // `Message::parse` 会从给定的缓冲区里消费，还会把后面跟随的消息一起
        // 吞掉，所以每条消息都要单独给一段切片。
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

/// 一次读取中到达的所有消息，按顺序排列。
pub struct BackendMessage {
    pub messages: BackendMessages,
    /// 这一轮在 `ReadyForQuery` 结束，说明请求已处理完。
    #[allow(dead_code)]
    pub complete: bool,
}

/// 读端持有的 socket，同时写操作也通过连接到达同一个 socket。两者任何时候
/// 都只借用一个：一条语句先完整写完，才开始读它的任何应答。
pub struct StreamIo(Arc<Mutex<Stream>>);

impl StreamIo {
    pub(crate) fn new(stream: &Arc<Mutex<Stream>>) -> Self {
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
            // 共享的 socket 只在写入期间被持有，而写入不会跨越等待，所以这
            // 只是短暂停顿，不会死锁。
            Err(_) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}

impl AsyncWrite for StreamIo {
    // 只有 `Framed` 会在这里写：连接的空关键字和启动消息，而且它要求两半
    // 都就绪；语句则走 `write_guarded`。
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

pub(crate) fn short_message() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "incomplete server message")
}
