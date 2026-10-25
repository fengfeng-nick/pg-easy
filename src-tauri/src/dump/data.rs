//! 数据段：按依赖顺序 TRUNCATE，再为每张表写一个 COPY 块。

use std::collections::HashMap;

use crate::error::AppError;
use crate::state::Session;

use super::ddl::SchemaDump;
use super::load::load_sequences;
use super::order::{order_by_keys, parent_subtree_contains};
use super::plan::PlanRelation;
use super::run::DumpSink;
use super::util::{key_of, lit, text};

// ---------------------------------------------------------------------------
// 数据

/// 数据行部分，边读边流式输出：先截断（子表在前、父表在后，父表就不会在
/// 子表还需要它的时候被清空），然后每张表一个 COPY 块，最后是序列值。
pub(crate) async fn write_data(
    session: &mut Session,
    schema: &SchemaDump,
    sink: &mut (dyn DumpSink + Send),
    stop: &(dyn Fn() -> Option<AppError> + Send + Sync),
) -> Result<(), AppError> {
    // 父表在前，指向了外键目标的表排在其后。
    let data_relations = order_by_keys(&schema.data_relations);
    let tables: Vec<&PlanRelation> = data_relations.iter().collect();
    if tables.is_empty() {
        return Ok(());
    }

    let mut clears = String::from("--\n-- Clear existing rows\n--\n\n");
    for relation in tables.iter().rev() {
        clears.push_str(&format!(
            "truncate table {} cascade;\n",
            relation.qualified()
        ));
    }
    clears.push('\n');
    sink.write(clears.as_bytes())?;

    // 分区自身不携带数据行；父表的行要经由它们读取，无论嵌套多深。
    let columns: HashMap<(String, String), String> = schema
        .tables
        .iter()
        .map(|table| (table.relation.key(), table.column_list()))
        .collect();

    for relation in &tables {
        if let Some(error) = stop() {
            return Err(error);
        }
        // COPY 不接受生成列，所以两边都明确列出列名。
        let column_list = columns.get(&relation.key()).cloned().unwrap_or_default();
        // 用 `copy (select …)` 而不是 `copy table`：这样它仍是客户端能读的
        // 普通 COPY，也不受行级策略影响。
        let source = if relation.kind == "partitioned" {
            // 分区父表自身不存数据行，也拒绝普通 COPY，所以改读它的叶子
            // 分区——但只读计划覆盖到的那些，被排除的分区必须保持为空。
            let leaves: Vec<String> = schema
                .tables
                .iter()
                .map(|table| &table.relation)
                .filter(|other| {
                    other
                        .partition_of
                        .as_deref()
                        .map(key_of)
                        .is_some_and(|parent| {
                            parent == relation.key() ||
                                parent_subtree_contains(schema, relation, &parent)
                        })
                })
                .map(|other| format!("'{}'::regclass", other.qualified()))
                .collect();
            if leaves.is_empty() {
                continue;
            }
            format!(
                "select {column_list} from {} where tableoid in ({})",
                relation.qualified(),
                leaves.join(", ")
            )
        } else {
            format!("select {column_list} from {}", relation.qualified())
        };
        let rows = session
            .conn
            .copy_out(&format!("copy ({source}) to stdout"))
            .await?;
        let mut payload = String::new();
        for value in rows
            .values
            .iter()
            .filter_map(|row| row.first().cloned().flatten())
        {
            payload.push_str(value.trim_end_matches('\n'));
            payload.push('\n');
        }
        if payload.is_empty() {
            continue;
        }
        sink.write(
            format!(
                "copy {} ({column_list}) from stdin;\n{payload}\\.\n\n",
                relation.qualified()
            )
            .as_bytes(),
        )?;
    }

    sink.write(schema.identity_resets().as_bytes())?;
    if let Some(values) = sequence_values(session).await? {
        sink.write(b"--\n-- Sequence values\n--\n\n")?;
        sink.write(values.as_bytes())?;
    }
    Ok(())
}

/// 为每个独立序列执行 `setval`，下一次插入就不会复用随数据行导入的值。
/// 用户读不到的序列、以及从未被调用过的序列，保留其声明的起始值。
pub(crate) async fn sequence_values(session: &mut Session) -> Result<Option<String>, AppError> {
    let sequences = load_sequences(session).await?;
    let mut out = String::new();
    for sequence in &sequences {
        let Ok(rows) = session
            .conn
            .query(&format!(
                "select last_value::text, is_called::text from {}",
                sequence.qualified()
            ))
            .await
        else {
            continue;
        };
        let Some(row) = rows.values.first() else {
            continue;
        };
        if text(row, 1) != "t" {
            continue;
        }
        out.push_str(&format!(
            "select pg_catalog.setval({}, {}, true);\n",
            lit(&sequence.qualified()),
            lit(&text(row, 0))
        ));
    }
    Ok((!out.is_empty()).then_some(out))
}

