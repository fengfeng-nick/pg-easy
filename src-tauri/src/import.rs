//! 把 SQL 文件回放进数据库。
//!
//! 文件整体读入，用编辑器同一个词法器切分成语句（`sql.rs`），每个
//! `COPY … FROM STDIN` 块与其后的数据行分开。随后语句按批次通过简单查询协议
//! 发出——psql 回放转储用的正是这套交互——所以混用 DDL、DML 和 copy 数据的
//! 脚本，行为与在 psql 里一致。
//!
//! 分批而不是整个脚本：一批能让进度（以及界面上的计数）持续前进，取消一批
//! 只会结束那次 copy，不会把连接丢掉。失败会在出错的那条语句处停止导入，
//! 与 `ON_ERROR_STOP=1` 完全一致。

use std::ops::Range;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::connection;
use crate::error::AppError;
use crate::pg::{self, CopyInScript, CopyScript};
use crate::protocol::{ImportOptions, ImportOutcome};
use crate::sql;
use crate::state::Session;

/// 一次交互携带多少条语句。小到长文件上进度也能推进，大到往返开销可以忽略。
const CHUNK_STATEMENTS: usize = 200;

/// 一次交互携带多少语句文本。由几条超长语句组成的文件则改按大小切分。
const CHUNK_BYTES: usize = 1024 * 1024;

/// 被取消的导入等待服务器确认最多多久。
const CANCEL_ACK: Duration = Duration::from_secs(5);

/// 计划里的一条语句，连同它 `COPY` 块的数据行（如果有）。
struct Planned {
    /// 语句文本，不含终止符。
    sql: String,
    /// zlib 压缩的 copy 数据，由发送它的那一批取走。
    data: Option<Vec<u8>>,
}

/// 一个将被回放的文件。
pub struct Plan {
    statements: Vec<Planned>,
}

impl Plan {
    pub fn total(&self) -> usize {
        self.statements.len()
    }
}

/// 把 `text` 切成语句，并把每个 `COPY … FROM STDIN` 与它的数据行配对。
///
/// 拆分器会把 copy 块的数据行当成 SQL：数据里出现的引号能让它把后面的
/// 语句一并吞掉，分号则会凭空切出语句。所以每跳过一个数据块都从标记行
/// 之后重新开始扫描——那正是 psql 的边界，也是数据之外唯一的干净位置。
pub fn plan(text: &str) -> Result<Plan, AppError> {
    let mut statements = Vec::new();
    let mut offset = 0usize;

    while let Some(statement) = sql::next_statement(text, offset) {
        let body = text[statement.start..statement.end].trim();
        if body.is_empty() {
            offset = statement.end + 1;
            continue;
        }

        if !pg::reads_from_stdin(body) {
            statements.push(Planned {
                sql: body.to_string(),
                data: None,
            });
            offset = statement.end + 1;
            continue;
        }

        // 数据行从语句的下一行开始，块到只含 `\.` 的那一行结束——这个标记
        // 服务器永远看不到：数据的结束是靠 `CopyDone` 告知的。
        let index = statements.len();
        let data_start = next_line(text, statement.end).ok_or_else(|| unterminated(index))?;
        let Some((end, after)) = pg::find_end_of_data(&text[data_start..]) else {
            return Err(unterminated(index));
        };
        let data = &text[data_start..data_start + end];
        let script = CopyInScript::split(body, data)
            .ok_or_else(|| AppError::new("internal", "something_went_wrong"))?;
        statements.push(Planned {
            sql: script.sql,
            data: Some(script.data),
        });
        offset = data_start + after;
    }

    if statements.is_empty() {
        return Err(AppError::new("empty", "nothing_to_import"));
    }
    Ok(Plan { statements })
}

fn unterminated(index: usize) -> AppError {
    AppError::new("sql", "copy_block_unterminated").with_statement(index)
}

/// 结束包含 `at` 的那一行的换行之后一位的偏移。
fn next_line(text: &str, at: usize) -> Option<usize> {
    text[at..].find('\n').map(|offset| at + offset + 1)
}

/// 导入已到达的语句数和行数，用于对话框的进度行。
#[derive(Debug, Clone, Copy, Default)]
pub struct Progress {
    pub statements: usize,
    pub total: usize,
    pub rows: u64,
    pub copied_rows: u64,
}

/// 在会话上执行 `plan`，每批之后上报一次。
///
/// 会话由调用方独占（命令层持有会话槽），协议上不会有别的东西插入。
/// `cancel` 在批次之间轮询；批次内部，它发出的取消请求会让服务器结束当前
/// 语句，错误也从那里返回。
pub async fn run<F>(
    session: &mut Session,
    mut plan: Plan,
    options: &ImportOptions,
    cancel: &CancellationToken,
    file_bytes: u64,
    mut on_progress: F,
) -> Result<ImportOutcome, AppError>
where
    F: FnMut(Progress),
{
    let started = Instant::now();
    let total = plan.total();

    // 开始之前先记下来：取消请求需要后端的密钥，而脚本一上路 socket 就忙了。
    let cancel_request = session.conn.cancel_request();
    let host = connection::effective_host(&session.params.host).to_string();
    let port = session.params.port;
    let stopper = tokio::spawn({
        let cancel = cancel.clone();
        async move {
            cancel.cancelled().await;
            let _ = tokio::time::timeout(CANCEL_ACK, send_cancel(&host, port, cancel_request)).await;
        }
    });

    let mut progress = Progress {
        total,
        ..Default::default()
    };

    let mut failure = None;
    if options.in_transaction {
        if let Err(error) = session.conn.execute("begin").await {
            failure = Some(error);
        }
    }

    while failure.is_none() && progress.statements < total {
        if cancel.is_cancelled() {
            failure = Some(
                AppError::new("cancelled", "import_cancelled").with_statement(progress.statements),
            );
            break;
        }
        let range = batch_range(&plan, progress.statements);
        let script = build_script(&mut plan, range.clone());
        match session.conn.run_copy_script(&script).await {
            Ok(outcome) => {
                progress.rows += outcome.rows;
                progress.copied_rows += outcome.copied_rows;
                progress.statements = range.end;
                on_progress(progress);
            }
            Err(error) => failure = Some(remap(error, range.start)),
        }
    }

    // 从未提交的事务不能留在打开状态：下一个使用这条会话的标签页会悄悄
    // 加入它。
    let result = match (failure, options.in_transaction) {
        (None, true) => session.conn.execute("commit").await,
        (Some(error), true) => {
            let _ = session.conn.execute("rollback").await;
            Err(error)
        }
        (outcome, false) => outcome.map_or(Ok(()), Err),
    };

    stopper.abort();
    result?;

    Ok(ImportOutcome {
        statements: total,
        rows: progress.rows,
        copied_rows: progress.copied_rows,
        bytes: file_bytes,
        duration_ms: elapsed_ms(started),
    })
}

/// 下一批：一串语句，按数量或按大小切分。
fn batch_range(plan: &Plan, from: usize) -> Range<usize> {
    let mut end = from;
    let mut bytes = 0usize;
    while end < plan.statements.len() && end - from < CHUNK_STATEMENTS {
        bytes += plan.statements[end].sql.len();
        end += 1;
        if bytes >= CHUNK_BYTES {
            break;
        }
    }
    from..end
}

/// 把一批语句拼成连接要发出的交互，过程中把 copy 数据从计划里取走。
fn build_script(plan: &mut Plan, range: Range<usize>) -> CopyScript {
    let statements = &mut plan.statements[range];
    let mut blocks = Vec::new();
    for (index, statement) in statements.iter_mut().enumerate() {
        if let Some(data) = statement.data.take() {
            blocks.push((index, data));
        }
    }
    let sql: Vec<&str> = statements
        .iter()
        .map(|statement| statement.sql.as_str())
        .collect();
    CopyScript::new(&sql, blocks)
}

/// 把一批里抛出的错误映射回它来自的文件。
fn remap(mut error: AppError, offset: usize) -> AppError {
    if let Some(index) = error.statement_index {
        error = error.with_statement(offset + index);
    }
    if error.kind == "cancelled" {
        // 服务器报告的是它自己的取消代码；对话框应该说导入已停止，而不是
        // 一条查询被取消。
        let mut cancelled = AppError::new("cancelled", "import_cancelled");
        if let Some(index) = error.statement_index {
            cancelled = cancelled.with_statement(index);
        }
        return cancelled;
    }
    error
}

/// 请服务器放弃这条会话上正在执行的语句。
async fn send_cancel(host: &str, port: u16, request: Vec<u8>) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut socket = tokio::net::TcpStream::connect((host, port)).await?;
    socket.write_all(&request).await?;
    socket.shutdown().await
}

fn elapsed_ms(start: Instant) -> f64 {
    (start.elapsed().as_micros() as f64 / 100.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn inflate(data: &[u8]) -> String {
        let mut out = String::new();
        flate2::read::ZlibDecoder::new(data)
            .read_to_string(&mut out)
            .expect("inflate");
        out
    }

    /// 计划中每条语句的 `(语句, 行数)`。
    fn planned(text: &str) -> Vec<(String, Option<String>)> {
        plan(text)
            .expect("a plan")
            .statements
            .into_iter()
            .map(|statement| (statement.sql, statement.data.as_deref().map(inflate)))
            .collect()
    }

    #[test]
    fn splits_plain_statements() {
        let statements = planned("create table t (a int);\ninsert into t values (1);\n");
        assert_eq!(
            statements,
            vec![
                ("create table t (a int)".to_string(), None),
                ("insert into t values (1)".to_string(), None),
            ]
        );
    }

    #[test]
    fn pairs_a_copy_block_with_its_rows() {
        let script = "\
-- header
--
COPY public.t (a, b) FROM stdin;
1	x
2	y
\\.
create index t_a on public.t (a);
";
        // 文件里的注释块属于它所引导的那条语句，所以留在语句文本中。
        assert_eq!(
            planned(script),
            vec![
                (
                    "-- header\n--\nCOPY public.t (a, b) FROM stdin".to_string(),
                    Some("1\tx\n2\ty\n".to_string())
                ),
                ("create index t_a on public.t (a)".to_string(), None),
            ]
        );
    }

    #[test]
    fn keeps_a_statement_that_follows_the_data_on_the_same_line() {
        // 拆分器会把数据行并进下一条语句的文本；计划必须从标记之后继续，
        // 而不是从那条语句的开头继续。
        let script = "copy t from stdin;\n1\n\\.\nselect 1;\n";
        assert_eq!(
            planned(script),
            vec![
                ("copy t from stdin".to_string(), Some("1\n".to_string())),
                ("select 1".to_string(), None),
            ]
        );
    }

    #[test]
    fn keeps_an_empty_copy_block() {
        let script = "copy t from stdin;\n\\.\nselect 1;\n";
        assert_eq!(
            planned(script),
            vec![
                ("copy t from stdin".to_string(), Some(String::new())),
                ("select 1".to_string(), None),
            ]
        );
    }

    #[test]
    fn refuses_a_copy_block_without_a_marker() {
        let error = plan("copy t from stdin;\n1\n2\n").err().expect("an error");
        assert_eq!(error.message, "copy_block_unterminated");
        assert_eq!(error.statement_index, Some(0));
    }

    #[test]
    fn refuses_an_empty_script() {
        assert!(plan("\n-- nothing but comments\n").is_err());
    }

    #[test]
    fn batches_by_count_and_size() {
        let many = plan(
            &(0..CHUNK_STATEMENTS + 5)
                .map(|index| format!("select {index};"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .expect("a plan");
        assert_eq!(batch_range(&many, 0), 0..CHUNK_STATEMENTS);
        assert_eq!(
            batch_range(&many, CHUNK_STATEMENTS),
            CHUNK_STATEMENTS..CHUNK_STATEMENTS + 5
        );

        let big = format!("select '{}';", "x".repeat(CHUNK_BYTES));
        let huge = plan(&format!("{big}\nselect 1;")).expect("a plan");
        assert_eq!(batch_range(&huge, 0), 0..1);
    }
}
