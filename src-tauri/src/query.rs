//! Statement execution.
//!
//! Scripts are split client-side (see `sql.rs`) so each result is attached to
//! the exact statement that produced it, and each statement runs through the
//! simple query protocol. That protocol is what lets a script mix DDL, DML and
//! queries the way psql does, and every value arrives as the server's own
//! text: nothing is re-encoded on the way to the grid.
//!
//! A session runs one statement at a time: the session slot is locked for the
//! duration of a statement while the table of sessions is not, so a long query
//! on one connection never holds up another.

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

/// How long a cancelled statement waits for the server to acknowledge it.
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

/// Identifies servers sharing a credential without encoding the password.
/// Used to look up a saved password; never stored or logged itself.
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
        // A fresh token per statement: pressing stop before this statement
        // began must not cancel work the user has not seen yet.
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
            // Statements before the failing one produced real output; hand it
            // back so the user sees partial progress instead of losing it.
            error = error
                .with_statement(index)
                .with_results(results, total_duration_ms)
                .with_tx_status(tx_status);
            if let Some(rows) = rows {
                // The failing statement's own output, shown under its error.
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

/// Waits until the user asks to stop, then stops the statement on the server.
///
/// The request is sent before the statement future is dropped, so the backend
/// does not keep working after the UI says it stopped. A short wait for the
/// slot keeps a stop press from being lost when the statement finishes at the
/// same moment.
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
            // The cancel request is on its way; the statement future is
            // dropped mid-read, so the rest of its response must be
            // consumed before the connection can be used again.
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

/// `UPDATE 3`, `SELECT 12`, `INSERT 0 1`, `COPY 4`, `BEGIN`.
fn parse_tag(tag: &str) -> (u64, u64) {
    let mut parts = tag.split_whitespace();
    let command = parts.next().unwrap_or_default();
    let numbers: Vec<u64> = parts.filter_map(|part| part.parse().ok()).collect();
    match command {
        "SELECT" => (0, numbers.first().copied().unwrap_or(0)),
        // INSERT tags read `INSERT <oid> <rows>`.
        "INSERT" => (numbers.get(1).copied().unwrap_or(0), 0),
        _ => (numbers.first().copied().unwrap_or(0), 0),
    }
}

fn elapsed_ms(start: Instant) -> f64 {
    (start.elapsed().as_micros() as f64 / 100.0).round() / 10.0
}

/// Reads the next `limit` rows of a statement already shown in the grid, of
/// which the caller already holds `offset`.
///
/// The simple protocol cannot resume a result set, so the statement is re-run
/// inside a read-only transaction with a server-side cursor. `read only` makes
/// the re-run incapable of repeating side effects; skipping with `fetch
/// forward` keeps memory proportional to one page.
pub async fn fetch_page(
    handle: &SessionHandle,
    sql: &str,
    offset: usize,
    limit: usize,
) -> Result<Page, AppError> {
    let mut slot = handle.slot.lock().await;
    // Named so the cursor is recognisable in pg_stat_activity.
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
    // Whatever happened, the transaction ends here; a failed statement has
    // already aborted it, and rollback then simply closes it.
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
    // Stop on an idle session is a no-op the UI may race into.
    handle.try_cancel();
    Ok(())
}
