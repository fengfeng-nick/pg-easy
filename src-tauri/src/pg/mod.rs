//! 线协议客户端。
//!
//! `tokio-postgres` 返回的 `Connection`，其 TLS 流类型取决于连接器，明文和
//! 加密连接因此无法共用一种类型；它只按 OID 给出的列类型要通过目录往返来
//! 解析；而且它既不暴露后端的事务状态，也不暴露命令标签。
//!
//! 本模块在装箱的流上完成同样的启动握手——一种类型覆盖所有 sslmode——然后
//! 借助驱动自身公开的编解码器和消息类型驱动协议。认证用的是驱动自己的
//! `postgres-protocol` 实现；这里没有手写的加密代码。

use std::collections::HashMap;

use std::io::Read;

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

/// 一条 `COPY … FROM STDIN` 语句，随脚本执行一起发出：先语句，再数据，最后
/// 结束标记。
pub use self::copy_in::CopyInScript;
pub(crate) use self::copy_in::{find_end_of_data, reads_from_stdin};

/// 失败语句错误面板的 id。导入会把多条语句合成一条，服务器按语句在其中的
/// 位置指出失败的那条，所以执行器给每条语句打上唯一标记。
pub const STATEMENT_MARKER: &str = "pg_easy/statement/";

use crate::error::{AppError, Translated};

use self::types::{read_row, Column};

mod copy_in;
pub mod types;

/// TLS 决定之后的 socket：明文或加密，藏在同一种类型后面。
pub type Stream = tokio::io::BufStream<Box<dyn Conn>>;

pub trait Conn: AsyncRead + AsyncWrite + Send + Unpin {}

// ---------------------------------------------------------------------------
// 分帧

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

/// 出错之后到 `ReadyForQuery` 之前，消息流最多可以拖多久，超过就放弃这条
/// 连接。
const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 登录交互最多可以花多久，超过就报超时。
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

fn short_message() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "incomplete server message")
}

// ---------------------------------------------------------------------------
// 错误

/// `ErrorResponse` 里值得显示的部分。
#[derive(Debug, Clone)]
pub struct DbError {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
    pub position: Option<usize>,
    /// 失败语句的位置，当错误是在挂到某条语句的函数（它的触发器、规则或
    /// 约束）里抛出时由服务器设置。从 1 开始。
    pub internal_position: Option<usize>,
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
            internal_position: None,
        };
        while let Some(field) = fields.next()? {
            let value = String::from_utf8_lossy(field.value_bytes()).into_owned();
            match field.type_() {
                b'V' | b'S' if error.severity.is_empty() => error.severity = value,
                b'C' => error.code = value,
                b'M' => error.message = value,
                // 提示追加到细节后面：两段小字说明已经超出了这个面板的
                // 需要。
                b'D' => error.detail = Some(value),
                b'H' => error.detail = Some(match error.detail.take() {
                    Some(detail) => format!("{detail}\n{value}"),
                    None => value,
                }),
                b'P' => error.position = value.parse().ok(),
                b'p' => error.internal_position = value.parse().ok(),
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
            // query_canceled、admin_shutdown
            "57014" | "57P01" => "cancelled",
            code if code.starts_with("08") => "connection",
            code if code.starts_with("28") => "auth",
            _ => "database",
        };
        let mut error = AppError::new(kind, self.message.clone())
            .with_detail(self.detail.clone())
            .with_code(&self.code);
        // 执行器会把合并脚本里的位置映射到具体语句（并在那里丢弃它）；
        // 单独执行的语句保留位置，编辑器因此能指向出错的 token。函数内部
        // 抛出的错误两者都报：语句本身，以及语句里的那次调用。
        if self.internal_position.is_none() {
            if let Some(position) = self.position {
                error = error.with_position(position);
            }
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
// 连接

#[derive(Debug, Clone)]
pub struct StartupParams {
    pub user: String,
    pub database: String,
    pub password: Option<String>,
    pub application_name: Option<String>,
    /// 密码可以在这个信道上发送：TLS 已完成，或用户允许明文。否则拒绝
    /// SCRAM，这样 `prefer` 悄悄降级时也不会泄漏密码。
    pub ssl_verified: bool,
}

/// 一条语句产出的结果。
#[derive(Debug)]
pub struct StatementOutput {
    /// 服务器的命令标签，例如 `UPDATE 3` 或 `SELECT 2`。
    pub tag: String,
    pub rows: Option<Rows>,
}

#[derive(Debug)]
pub struct Rows {
    pub columns: Vec<Column>,
    pub values: Vec<Vec<Option<String>>>,
    /// 超过显示上限后服务器仍发来的行数。
    pub dropped: u64,
}

/// 一条 `CopyData` 消息承载多少数据。
const COPY_CHUNK: usize = 256 * 1024;

/// 一批一起发出的语句，含其中任何 `COPY … FROM STDIN` 的数据。
#[derive(Debug, Default)]
pub struct CopyScript {
    /// 查询文本：各语句按顺序排列，每条前面有一行标明其索引的注释
    /// （`STATEMENT_MARKER`），`COPY` 后紧跟它的数据。
    pub sql: String,
    /// 每条语句的 `STATEMENT_MARKER` 行在 `sql` 中的字节偏移。
    pub blocks: Vec<Vec<u8>>,
    /// 每条语句的 `STATEMENT_MARKER` 行在 `sql` 中的字节偏移。
    markers: Vec<usize>,
}

impl CopyScript {
    /// 构造查询。`statements` 是拆分器切出的脚本；`blocks` 把语句索引映射
    /// 到它的 copy 数据（如果有）。
    pub fn new(
        statements: &[&str],
        blocks: impl IntoIterator<Item = (usize, Vec<u8>)>,
    ) -> Self {
        let mut script = Self::default();
        let mut blocks: Vec<(usize, Vec<u8>)> = blocks.into_iter().collect();
        blocks.sort_by_key(|(index, _)| *index);
        let mut next_block = blocks.into_iter().peekable();
        for (index, statement) in statements.iter().enumerate() {
            script.markers.push(script.sql.len());
            script
                .sql
                .push_str(&format!("-- {STATEMENT_MARKER}{index}\n"));
            script.sql.push_str(statement.trim_end());
            if let Some((_, data)) = next_block.next_if(|(owner, _)| *owner == index) {
                // 数据后面从不跟分号；标记行就是下一条语句。
                script.sql.push_str(";\n");
                script.blocks.push(data);
                continue;
            }
            // 最后一行是 `--` 注释的语句会把追加在该行的终止符吞掉，所以这里
            // 单独给它一行。
            script.sql.push_str("\n;\n");
        }
        script
    }

    /// 服务器的 `position`（字符数，从 1 开始，相对查询开头）落在哪条语句
    /// 里。
    pub fn statement_index(&self, position: Option<usize>) -> Option<usize> {
        let position = position?;
        // 把字符计数换算成字节偏移：标记本身是纯 ASCII，但它们之前的 SQL
        // 不一定。
        let target = self
            .sql
            .char_indices()
            .nth(position.saturating_sub(1))
            .map(|(offset, _)| offset)
            .unwrap_or(self.sql.len());
        self.markers
            .iter()
            .rposition(|start| *start <= target)
    }
}

    /// 各块包含的行数，按换行数计算。
#[derive(Debug, Default)]
pub struct CopyOutcome {
    /// `COPY` 块送出、以及 INSERT、UPDATE、DELETE、MERGE 接收的行数。
    pub rows: u64,
    /// 各块包含的行数，按换行数计算。
    pub copied_rows: u64,
    /// 服务器索要并已收到的 `COPY` 块数。
    pub blocks: usize,
}

/// `INSERT 0 3`、`UPDATE 3`、`DELETE 3` 或 `MERGE 3` 标签里的行数。
fn rows_in_tag(tag: &str) -> u64 {
    let mut parts = tag.split_whitespace();
    let command = parts.next().unwrap_or_default();
    let numbers: Vec<u64> = parts.filter_map(|part| part.parse().ok()).collect();
    match command {
        "INSERT" => numbers.get(1).copied().unwrap_or(0),
        "UPDATE" | "DELETE" | "MERGE" => numbers.first().copied().unwrap_or(0),
        _ => 0,
    }
}

pub struct Connection {
    /// 唯一的 socket，由读端和写操作共享。
    stream: Arc<Mutex<Stream>>,
    /// 服务器参数（`server_version`、`TimeZone`……），随 `ParameterStatus`
    /// 消息保持最新。
    reader: Framed<StreamIo, BackendMessages>,
    /// 已从 `reader` 读出但尚未交出的消息。整块留在这里，读一条消息就永远
    /// 不会去碰 `reader`。
    chunk: Option<BackendMessages>,
    /// 服务器参数（`server_version`、`TimeZone`……），随 `ParameterStatus`
    /// 消息保持最新。
    pub parameters: HashMap<String, String>,
    pub backend_pid: i32,
    /// 进行中语句的服务器错误；对执行器公开，好把先读到的行与它合并。
    notices: Vec<Translated>,
    key_secret: i32,
    /// 最近一次 `ReadyForQuery` 里的事务状态：'I'、'T' 或 'E'。
    status: u8,
    /// 进行中语句的服务器错误；对执行器公开，好把先读到的行与它合并。
    pub failed: Option<AppError>,
}

impl Connection {
    /// 服务器最近一次报告的事务状态。
    pub fn status(&self) -> u8 {
        self.status
    }

    pub fn take_notices(&mut self) -> Vec<Translated> {
        std::mem::take(&mut self.notices)
    }

    pub fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters.get(name).map(String::as_str)
    }

    /// 取消后端当前正在执行的语句。与 libpq 完全一样，为这个取消请求新开
    /// 一个 socket。
    /// 针对运行中语句的取消请求，与 libpq 发出的完全一致：在一条新连接上带
    /// 后端密钥的短消息。
    pub fn cancel_request(&self) -> Vec<u8> {
        let mut buf = BytesMut::new();
        frontend::cancel_request(self.backend_pid, self.key_secret, &mut buf);
        buf.to_vec()
    }

    /// 一直读到服务器再次就绪为止并丢弃内容，用在取消请求之后，好让下一条
    /// 语句从干净状态开始。
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

    /// 发送一条语句并读到 `ReadyForQuery`。
    ///
    /// 服务器报错时，已收到的行会随错误一起返回，中途失败的语句不会丢掉它
    /// 已经产出的内容。
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

    /// 等到在飞的语句结束，无论它是怎么结束的，连接可以用于下一条语句。
    pub async fn settle(&mut self, timeout: std::time::Duration) {
        let _ = self.drain_until_ready(timeout).await;
    }

    /// 发送一条只由不返回行的命令（SAVEPOINT、ROLLBACK TO、SET）组成的简单
    /// 查询，丢弃任何输出。
    pub async fn execute(&mut self, sql: &str) -> Result<(), AppError> {
        self.run(sql, 0).await.map(|_| ()).map_err(|(err, _)| err)
    }

    /// 执行可能包含 `COPY … FROM STDIN` 块的语句，就像转储那样。不是每条
    /// 语句一次交互，而是全部作为一条简单查询发出——psql 回放转储用的正是
    /// 这个协议——`COPY` 的数据只在服务器索要后才发送。某条语句失败会中断
    /// 其余部分，效果等同 `ON_ERROR_STOP=1`。
    ///
    /// 服务器返回的行会被计数后丢弃：导入显示的是进度行，不是表格。
    pub async fn run_copy_script(
        &mut self,
        script: &CopyScript,
    ) -> Result<CopyOutcome, AppError> {
        let mut buf = BytesMut::new();
        frontend::query(&script.sql, &mut buf).map_err(|err| io_error("encode_statement", err))?;
        self.failed = None;
        if let Err((error, _)) = self.send(buf).await {
            return Err(error);
        }

        let mut outcome = CopyOutcome::default();
        // 下一个 `CopyInResponse` 索要的块。按计数取而不是按语句文本查找：
        // 服务器是按顺序索要的。
        let mut blocks = 0usize;
        loop {
            match self
                .next_message()
                .await
                .map_err(|err| io_error("read_copy_data", err))?
            {
                backend::Message::CopyInResponse(_) => {
                    let Some(data) = script.blocks.get(blocks) else {
                        // 服务器索要了本脚本没有的块；用自己的错误结束那个
                        // COPY。
                        self.fail_copy_in("no data for this COPY").await?;
                        continue;
                    };
                    blocks += 1;
                    self.stream_copy_data(data, &mut outcome).await?;
                }
                backend::Message::CopyData(_) | backend::Message::CopyDone => {}
                backend::Message::CommandComplete(body) => {
                    // COPY 的行在到达时计数（见下文）；其他标签报告的是
                    // INSERT、UPDATE 或 DELETE 接收的行数。
                    if let Ok(tag) = body.tag() {
                        if !tag.starts_with("COPY ") {
                            outcome.rows += rows_in_tag(tag);
                        }
                    }
                }
                backend::Message::DataRow(_) => outcome.rows += 1,
                backend::Message::EmptyQueryResponse
                | backend::Message::RowDescription(_) => {}
                backend::Message::ErrorResponse(body) => {
                    if self.failed.is_none() {
                        self.failed = Some(db_error(body));
                    }
                }
                backend::Message::ReadyForQuery(body) => {
                    self.status = body.status();
                    return match self.failed.take() {
                        Some(error) => {
                            let index = script.statement_index(error.position);
                            Err(match index {
                                Some(index) => error.with_statement(index),
                                None => error,
                            })
                        }
                        None => {
                            outcome.blocks = blocks;
                            Ok(outcome)
                        }
                    };
                }
                _ => {}
            }
        }
    }

    /// 把某个块的 zlib 压缩数据按 socket 大小的片段发出，然后发 `CopyDone`。
    /// 流是推过去的，而不是等到有空间：COPY 执行期间连接的读端闲着，所以在
    /// socket 上等待的只有它。
    async fn stream_copy_data(
        &mut self,
        data: &[u8],
        outcome: &mut CopyOutcome,
    ) -> Result<(), AppError> {
        let mut decoder = flate2::read::ZlibDecoder::new(data);
        let mut chunk = vec![0u8; COPY_CHUNK];
        let mut buf = BytesMut::with_capacity(COPY_CHUNK + 5);
        let mut rows = 0u64;
        loop {
            match decoder.read(&mut chunk) {
                Ok(0) => break,
                Ok(len) => {
                    rows += chunk[..len].iter().filter(|byte| **byte == b'\n').count() as u64;
                    buf.clear();
                    frontend::CopyData::new(&chunk[..len])
                        .map_err(|err| io_error("encode_statement", err))?
                        .write(&mut buf);
                    write_guarded(&self.stream, buf.split())
                        .await
                        .map_err(|err| io_error("connection_lost", err))?;
                }
                Err(err) => {
                    return Err(io_error("read_copy_data", err));
                }
            }
        }
        outcome.copied_rows += rows;
        outcome.rows += rows;
        let mut done = BytesMut::new();
        frontend::copy_done(&mut done);
        write_guarded(&self.stream, done)
            .await
            .map_err(|err| io_error("connection_lost", err))
    }

    /// 手边没有数据时，结束服务器正在等待的 `COPY`。
    async fn fail_copy_in(&mut self, reason: &str) -> Result<(), AppError> {
        let mut buf = BytesMut::new();
        frontend::copy_fail(reason, &mut buf).map_err(|err| io_error("encode_statement", err))?;
        write_guarded(&self.stream, buf)
            .await
            .map_err(|err| io_error("connection_lost", err))
    }

    /// 一次取回全部行的查询，目录读取用它。
    #[allow(dead_code)]
    pub async fn query(&mut self, sql: &str) -> Result<Rows, AppError> {
        let output = self.run(sql, usize::MAX).await.map_err(|(err, _)| err)?;
        output.rows.ok_or_else(|| {
            AppError::new("internal", "unexpected_rows").with_args([output.tag.as_str()])
        })
    }

    /// 执行 `COPY … TO STDOUT` 并返回它的数据而不是丢弃。每行是一个文本列，
    /// 装着 copy 文本的一行，含行尾换行。
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
        self.read_copy_out(&mut rows)
            .await
            .map_err(|(error, _)| error)?;
        Ok(rows)
    }

    async fn read_copy_out(&mut self, rows: &mut Rows) -> Result<(), (AppError, Option<Rows>)> {
        loop {
            match self.next_message().await {
                Ok(backend::Message::CopyData(body)) => {
                    let text = String::from_utf8_lossy(body.data()).into_owned();
                    rows.values.push(vec![Some(text)]);
                }
                Ok(backend::Message::CopyDone) => {}
                // 带 `with check option` 的视图做 COPY 时，服务器改为返回一个
                // SELECT 的行：每行一列，与 copy 数据的行一样。
                Ok(backend::Message::DataRow(body)) => {
                    let mut values = read_row(&body).map_err(|err| (io_error("read_copy_data", err), None))?;
                    values.resize(1, None);
                    values.truncate(1);
                    rows.values.push(values);
                }
                Ok(backend::Message::CommandComplete(body)) => {
                    let _ = body.tag();
                }
                Ok(backend::Message::ErrorResponse(body)) => {
                    // 像 `run` 一样，继续读到 `ReadyForQuery`，好让连接在
                    // COPY 失败后仍然可用；错误等服务器再次就绪后上报。
                    self.failed = Some(db_error(body));
                }
                Ok(backend::Message::ReadyForQuery(body)) => {
                    self.status = body.status();
                    return match self.failed.take() {
                        Some(error) => Err((error, None)),
                        None => Ok(()),
                    };
                }
                Ok(_) => {}
                Err(err) => return Err((err, None)),
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
                    // 数据行总是跟在它的描述之后。
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
                    // 出错之后服务器仍会发 `ReadyForQuery`；读掉它，连接才
                    // 能继续使用。
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
                // `COPY … TO` 以 `CopyData` 消息报告其行，这些会落到下面的
                // 分支：表格没有对应的形状。
                backend::Message::CopyOutResponse(_) => {}
                backend::Message::CopyInResponse(_) => {
                // COPY FROM STDIN、NoData、portal、prepare 响应之类在简单
                // 查询流程里没有意义。
                    self.reject_copy_in().await;
                    let error = AppError::new("unsupported", "copy_from_stdin");
                    return Err((error, output.rows.take()));
                }
                // COPY FROM STDIN、NoData、portal、prepare 响应之类在简单
                // 查询流程里没有意义。
                _ => {}
            }
        }
    }

    /// 服务器的下一条消息。通知和参数变化经过时被记录下来；服务器在请求
    /// 之间也会推送它们。
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
                // 级别和消息是服务器的话；围绕位置的那句话是我们写的，所以
                // 它是 token。
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
                // 较老的服务器在这里只发 NOTICE，正是用户执行 `raise notice`
                // 之后想看到的那些。
                if matches!(notice.severity.as_str(), "NOTICE" | "WARNING") {
                    self.notices.push(text);
                }
            }
        }
    }

    async fn reject_copy_in(&mut self) {
        let mut buf = BytesMut::new();
        // `CopyFail` 需要一个 'f' 标签；辅助函数会写上一个。
        let _ = frontend::copy_fail("COPY FROM STDIN is not supported", &mut buf);
        let _ = write_guarded(&self.stream, buf).await;
    }
}

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

/// 登录交互的下一条消息。
async fn next_login_message(
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
fn unexpected_detail(other: &Option<backend::Message>) -> String {
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

async fn send_raw(writer: &Mutex<Stream>, buf: BytesMut) -> Result<(), AppError> {
    write_guarded(writer, buf)
        .await
        .map_err(|err| io_error("connection_lost", err))
}

/// 把 `buf` 写到受保护的 socket 上并刷新。
///
/// 写 future 只在本次轮询持有锁时被轮询，因此守卫不会进入外层 future 的
/// 状态；socket 忙时只需让出并重试。
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
