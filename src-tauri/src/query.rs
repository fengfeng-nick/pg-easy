//! 语句执行。
//!
//! 脚本在客户端切分（见 `sql.rs`），每个结果因此能对应到产生它的那条语句，
//! 每条语句都走简单查询协议。正是这个协议让脚本能像 psql 那样混用 DDL、
//! DML 和查询，而且每个值都以服务器自己的文本到达：送往表格的路上不做任何
//! 重新编码。
//!
//! 一个会话一次只跑一条语句：语句执行期间锁住会话槽位，而会话表本身不锁，
//! 所以一条连接上的长查询永远不会挡住另一条。

use std::time::{Duration, Instant};

use tokio::select;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::connection::{self, ConnectParams};
use crate::error::AppError;
use crate::pg::{self, Rows, StatementOutput};
use crate::protocol::{Column, ExecResult, Page, SessionInfo, StatementResult};
use crate::sql::SplitStatement;
use crate::state::{AppState, Session, SessionHandle};

/// 被取消的语句等服务器确认最多多久。
const CANCEL_ACK: Duration = Duration::from_secs(5);

pub async fn connect(
    state: &AppState,
    mut params: ConnectParams,
    timeout_ms: u64,
) -> Result<SessionInfo, AppError> {
    params.connect_timeout_ms = Some(timeout_ms);

    let (stream, encrypted) = connection::open_stream(&params).await?;
    let startup = pg::StartupParams {
        user: params.user.clone(),
        database: params.database.clone(),
        password: params.password.clone(),
        application_name: params.application_name.clone(),
        ssl_verified: encrypted,
    };
    let conn = match tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        pg::connect(stream, &startup),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            return Err(AppError::new("timeout", "login_accepted_no_finish"))
        }
    };

    let id = Uuid::new_v4().to_string();
    let server_version = conn
        .parameter("server_version")
        .unwrap_or_default()
        .to_string();
    let backend_pid = conn.backend_pid;
    let info = SessionInfo {
        id: id.clone(),
        tx_status: "idle".into(),
        server_version: server_version.clone(),
        fingerprint: fingerprint_of(&params),
        backend_pid,
    };

    let host = connection::effective_host(&params.host).to_string();
    let port = params.port;
    let session = Session {
        id,
        database: params.database.clone(),
        user: params.user.clone(),
        server_version,
        conn,
        busy: false,
        backend_pid,
        params,
        idle_since: Instant::now(),
    };
    state.insert(session, (host, port)).await;
    Ok(info)
}

/// 同一台服务器、同一份凭据上换一个库。PostgreSQL 没有“切库”的协议消息，
/// 所以关掉当前会话，用同样的参数重新登录到 `database`，并沿用原来的
/// 会话 id：界面上的标签页和打开的表都还指着它。
pub async fn switch_database(
    state: &AppState,
    id: &str,
    database: &str,
) -> Result<SessionInfo, AppError> {
    let handle = state.handle(id).await?;
    let (mut params, target) = {
        let slot = handle.slot.lock().await;
        if slot.session.busy {
            return Err(AppError::new("busy", "connection_busy_switch"));
        }
        (slot.session.params.clone(), handle.target)
    };
    let database = database.trim();
    if database.is_empty() {
        return Err(AppError::new("config", "database_required"));
    }
    if params.database == database {
        let slot = handle.slot.lock().await;
        return Ok(SessionInfo {
            id: id.to_string(),
            tx_status: match slot.session.conn.status() {
                b'T' => "intransaction".into(),
                b'E' => "inerror".into(),
                _ => "idle".into(),
            },
            server_version: slot.session.server_version.clone(),
            fingerprint: fingerprint_of(&params),
            backend_pid: slot.session.backend_pid,
        });
    }

    params.database = database.to_string();
    // 先登录成功再丢掉旧会话：新库拒绝连接时，原来的会话还在。
    let (stream, encrypted) = connection::open_stream(&params).await?;
    let startup = pg::StartupParams {
        user: params.user.clone(),
        database: params.database.clone(),
        password: params.password.clone(),
        application_name: params.application_name.clone(),
        ssl_verified: encrypted,
    };
    let timeout_ms = params.connect_timeout_ms.unwrap_or(15_000);
    let conn = match tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        pg::connect(stream, &startup),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => return Err(AppError::new("timeout", "login_accepted_no_finish")),
    };

    let server_version = conn
        .parameter("server_version")
        .unwrap_or_default()
        .to_string();
    let backend_pid = conn.backend_pid;
    let info = SessionInfo {
        id: id.to_string(),
        tx_status: "idle".into(),
        server_version: server_version.clone(),
        fingerprint: fingerprint_of(&params),
        backend_pid,
    };
    let session = Session {
        id: id.to_string(),
        database: params.database.clone(),
        user: params.user.clone(),
        server_version,
        conn,
        busy: false,
        backend_pid,
        params,
        idle_since: Instant::now(),
    };

    // 替换而不是先删后插：失败的话旧会话还在。`remove` 会取消在飞的语句并
    // 丢掉 socket，服务器立刻看到断开。
    state.remove(id).await;
    state.insert(session, target).await;
    Ok(info)
}

/// 在不编码密码的前提下标识共享同一份凭据的服务器。用于查找已保存的密码；
/// 它自身从不存储或记录。
pub fn fingerprint_of(params: &ConnectParams) -> String {
    format!(
        "{}@{}:{}/{}",
        params.user,
        connection::effective_host(&params.host),
        params.port,
        params.database
    )
}

pub async fn execute_script(
    handle: &SessionHandle,
    script: &str,
    max_rows: usize,
) -> Result<ExecResult, AppError> {
    let statements = crate::sql::split(script);
    if statements.is_empty() {
        return Err(AppError::new("empty", "nothing_to_run"));
    }

    let started = Instant::now();
    let mut results = Vec::with_capacity(statements.len());
    let mut failure: Option<Failure> = None;

    for statement in &statements {
        // 每条语句一个新 token：在这条语句开始之前按下停止，不能取消用户还
        // 没看到的工作。
        let cancel = CancellationToken::new();
        *handle.cancel.lock().await = cancel.clone();

        let outcome = select! {
            biased;
            _ = stop(handle, &cancel) => Err(Failure::Cancelled),
            outcome = run_one(handle, statement, max_rows, &cancel) => outcome,
        };

        match outcome {
            Ok(result) => results.push(result),
            Err(failure_kind) => {
                failure = Some(failure_kind);
                break;
            }
        }
    }

    let total_duration_ms = elapsed_ms(started);
    let tx_status: &'static str = {
        let slot = handle.slot.lock().await;
        match slot.session.status().tx_status.as_str() {
            "intransaction" => "intransaction",
            "inerror" => "inerror",
            _ => "idle",
        }
    };

    match failure {
        Some(Failure::Cancelled) => Err(AppError::new("cancelled", "query_cancelled")
            .with_statement(results.len())
            .with_results(results, total_duration_ms)
            .with_tx_status(tx_status)),
        Some(Failure::Failed {
            mut error,
            index,
            rows,
        }) => {
            // 失败语句之前的语句产出了真实输出；把它们交回去，用户能看到
            // 部分进展，而不是把它丢掉。
            error = error
                .with_statement(index)
                .with_results(results, total_duration_ms)
                .with_tx_status(tx_status);
            if let Some(rows) = rows {
                // 失败语句自己的输出，显示在它的错误下面。
                error = error.with_rows(build_result(
                    &statements[index],
                    StatementOutput {
                        tag: String::new(),
                        rows: Some(rows),
                    },
                    Vec::new(),
                    started,
                ));
            }
            Err(error)
        }
        None => Ok(ExecResult {
            results,
            total_duration_ms,
            tx_status: tx_status.to_string(),
        }),
    }
}

enum Failure {
    Cancelled,
    Failed {
        error: AppError,
        index: usize,
        rows: Option<Rows>,
    },
}

/// 一直等到用户要求停止，然后让服务器停掉这条语句。
///
/// 请求在语句 future 被丢弃之前发出，界面说停之后后端不会继续干活。短暂地
/// 等一下槽位，语句恰好同时结束时按下的停止也不会丢。
async fn stop(handle: &SessionHandle, cancel: &CancellationToken) {
    cancel.cancelled().await;
    let Ok(slot) = tokio::time::timeout(Duration::from_millis(100), handle.slot.lock()).await
    else {
        return;
    };
    let request = slot.session.conn.cancel_request();
    let (host, port) = handle.target.clone();
    drop(slot);
    let _ = tokio::time::timeout(CANCEL_ACK, send_cancel(&host, port, request)).await;
}

async fn send_cancel(host: &str, port: u16, request: Vec<u8>) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut socket = tokio::net::TcpStream::connect((host, port)).await?;
    socket.write_all(&request).await?;
    socket.shutdown().await
}

async fn run_one(
    handle: &SessionHandle,
    statement: &SplitStatement,
    max_rows: usize,
    cancel: &CancellationToken,
) -> Result<StatementResult, Failure> {
    let started = Instant::now();
    let mut slot = handle.slot.lock().await;
    slot.session.busy = true;

    let outcome = select! {
        biased;
        _ = cancel.cancelled() => {
            // 取消请求已经在路上；语句 future 在读取中途被丢弃，所以它响应
            // 的剩余部分必须先消费掉，连接才能再次使用。
            slot.session.conn.settle(CANCEL_ACK).await;
            Err(Failure::Cancelled)
        }
        outcome = slot.session.conn.run(&statement.sql, max_rows) => {
            outcome.map_err(|(error, rows)| Failure::Failed {
                error,
                index: statement.index,
                rows,
            })
        },
    };
    let notices = slot.session.conn.take_notices();
    slot.session.busy = false;
    slot.session.idle_since = Instant::now();

    match outcome {
        Ok(output) => Ok(build_result(statement, output, notices, started)),
        Err(failure) => Err(failure),
    }
}

fn build_result(
    statement: &SplitStatement,
    output: StatementOutput,
    notices: Vec<crate::error::Translated>,
    started: Instant,
) -> StatementResult {
    let tag = output.tag;
    let (columns, values, dropped) = match output.rows {
        Some(rows) => (
            Some(
                rows.columns
                    .into_iter()
                    .map(|column| Column {
                        name: column.name,
                        type_name: column.type_name.to_string(),
                        type_oid: column.type_oid,
                        kind: column.kind,
                    })
                    .collect::<Vec<_>>(),
            ),
            Some(rows.values),
            rows.dropped,
        ),
        None => (None, None, 0),
    };

    let (affected, returned) = parse_tag(&tag);
    StatementResult {
        index: statement.index,
        sql: statement.sql.clone(),
        start: statement.start,
        has_rows: columns.is_some(),
        columns,
        rows: values,
        affected,
        returned,
        tag: (!tag.is_empty()).then_some(tag),
        duration_ms: elapsed_ms(started),
        truncated: dropped > 0,
        notices,
    }
}

/// `UPDATE 3`、`SELECT 12`、`INSERT 0 1`、`COPY 4`、`BEGIN`。
fn parse_tag(tag: &str) -> (u64, u64) {
    let mut parts = tag.split_whitespace();
    let command = parts.next().unwrap_or_default();
    let numbers: Vec<u64> = parts.filter_map(|part| part.parse().ok()).collect();
    match command {
        "SELECT" => (0, numbers.first().copied().unwrap_or(0)),
        // INSERT 标签读作 `INSERT <oid> <rows>`。
        "INSERT" => (numbers.get(1).copied().unwrap_or(0), 0),
        _ => (numbers.first().copied().unwrap_or(0), 0),
    }
}

fn elapsed_ms(start: Instant) -> f64 {
    (start.elapsed().as_micros() as f64 / 100.0).round() / 10.0
}

/// 读取一条已显示在表格里的语句接下来的 `limit` 行，调用方已持有 `offset`。
///
/// 简单协议无法续读结果集，所以在只读事务里用服务端游标重跑这条语句。
/// `read only` 让重跑不可能重复副作用；用 `fetch forward` 跳过前面的行，
/// 让内存占用与一页成正比。
pub async fn fetch_page(
    handle: &SessionHandle,
    sql: &str,
    offset: usize,
    limit: usize,
) -> Result<Page, AppError> {
    let mut slot = handle.slot.lock().await;
    // 起了名字，方便在 pg_stat_activity 里认出这个游标。
    let cursor = format!("pg_easy_{}", Uuid::new_v4().simple());

    slot.session.conn.execute("begin read only").await?;
    let outcome = async {
        let conn = &mut slot.session.conn;
        conn.execute(&format!("declare {cursor} cursor for {sql}"))
            .await?;
        if offset > 0 {
            conn.execute(&format!("fetch forward {offset} from {cursor}"))
                .await?;
        }
        conn.query(&format!("fetch forward {limit} from {cursor}"))
            .await
    }
    .await;
    // 无论发生了什么，事务都在这里结束；失败的语句已经中止了它，回滚只是把
    // 它关掉。
    let _ = slot.session.conn.execute("rollback").await;

    let rows = outcome?;
    Ok(Page {
        offset,
        limit,
        exhausted: rows.values.len() < limit,
        rows: rows.values,
    })
}

pub fn request_cancel(handle: &SessionHandle) -> Result<(), AppError> {
    // 在空闲会话上按停止是个空操作，界面可能恰好赶在这个时机上。
    handle.try_cancel();
    Ok(())
}
