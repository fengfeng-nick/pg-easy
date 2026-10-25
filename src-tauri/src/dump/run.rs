//! 在快照事务里执行转储，并把字节写进调用方给的 sink。

use std::time::Duration;

use crate::error::AppError;
use crate::protocol::{DumpFormat, DumpOptions};
use crate::state::Session;

use super::data::write_data;
use super::ddl::SchemaDump;
use super::order::{ordered_data_relations, with_foreign_keys};
use super::plan::PlanRelation;
use super::script::{header, wrapped};
use super::zip::build_zip;

/// 被取消的导出等待服务器平静下来要多久。
const SETTLE: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// 执行转储

/// 脚本写到哪里：磁盘上的文件，或测试里的缓冲区。
pub trait DumpSink: Send {
    fn write(&mut self, bytes: &[u8]) -> Result<(), AppError>;
}

impl DumpSink for Vec<u8> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), AppError> {
        self.extend_from_slice(bytes);
        Ok(())
    }
}

/// 在快照事务里执行转储，边产出边直接写入 `sink`，并返回有多少张表贡献了
/// 数据行。
///
/// 每张表之间检查一次 `cancelled`，对话框的停止按钮因此能在最后一个关系被
/// 读完之前生效。
pub async fn run(
    session: &mut Session,
    relations: &[PlanRelation],
    options: &DumpOptions,
    sink: &mut (dyn DumpSink + Send),
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> Result<usize, AppError> {
    let stop = || {
        if cancelled() {
            Some(AppError::new("cancelled", "export_cancelled"))
        } else {
            None
        }
    };
    session.conn.settle(SETTLE).await;
    session
        .conn
        .execute("begin isolation level repeatable read read only")
        .await?;
    let outcome = write_dump(session, relations, options, sink, &stop).await;
    // 成功、失败、连接断开或取消请求都在这里收尾：把会话交还编辑器之前，
    // 快照事务先结束。
    session.conn.settle(SETTLE).await;
    let _ = session.conn.execute("rollback").await;
    outcome
}

async fn write_dump(
    session: &mut Session,
    relations: &[PlanRelation],
    options: &DumpOptions,
    sink: &mut (dyn DumpSink + Send),
    stop: &(dyn Fn() -> Option<AppError> + Send + Sync),
) -> Result<usize, AppError> {
    let mut schema = SchemaDump::load(session, relations, options.include_data, stop).await?;
    schema.reconcile_constraints(session, relations, stop).await?;
    let relations = with_foreign_keys(relations, &schema.constraints);
    // 数据段按外键排序，而加载结构时还不知道这些外键。
    schema.data_relations = ordered_data_relations(&relations, options.include_data);
    let tables = ordered_data_relations(&relations, options.include_data);

    let structure = schema.render();
    match options.format {
        DumpFormat::Zip => {
            // zip 需要先知道每个条目的大小才能写它的目录，所以这种形态在
            // 内存里组装。
            let data = if options.include_data {
                let mut buffer: Vec<u8> = Vec::new();
                write_data(session, &schema, &mut buffer, stop).await?;
                String::from_utf8_lossy(&buffer).into_owned()
            } else {
                "-- Structure only: this dump holds no table rows.\n\n".to_string()
            };
            // 只有在确认存在一个能干净回放的 schema 之后才读取行，所以
            // schema 决定了脚本的形状。
            let schema_file = wrapped(header("Schema"), &structure);
            let data_file = header("Data") + &data;
            sink.write(&build_zip(&schema_file, &data_file)?)?;
        }
        DumpFormat::Sql => {
            sink.write(wrapped(header("Database dump"), &structure).as_bytes())?;
            sink.write(b"\n--\n-- Data\n--\n\n")?;
            write_data(session, &schema, sink, stop).await?;
        }
    }
    Ok(tables.len())
}
