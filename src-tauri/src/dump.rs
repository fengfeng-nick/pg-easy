//! SQL 转储。
//!
//! 这里不调用 `pg_dump`，而是从目录里拼出脚本：表结构来自 `pg_catalog`
//! （列、默认值、约束、索引、触发器、分区、外键、序列），数据通过
//! `COPY ... TO STDOUT` 取出，线协议客户端本来就认识它。结果是可被 `psql -f`
//! 直接回放的纯 SQL，顺序保证每个对象的依赖都在它之前存在。
//!
//! 整个转储在一个只读的 `repeatable read` 事务里读取，所以即使有人在写库，
//! 结构和行描述的也是同一时刻；无论转储完成、失败还是被取消，事务都会结束。
//!
//! 输出是流式的：结构一次产出（它整体上必须有序），随后每张表输出一个
//! `copy ... from stdin` 块，大库因此不会在内存里存两份。
//!
//! 同一批材料产出两种形态：一个包含全部内容的 SQL 脚本，或一个分别装
//! `schema.sql` 与 `data.sql` 的 zip。

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::catalog::quote_ident;
use crate::error::AppError;
use crate::protocol::{DumpFormat, DumpOptions};
use crate::state::Session;

const SYSTEM_SCHEMA_FILTER: &str =
    "n.nspname not in ('pg_catalog', 'information_schema') and n.nspname not like 'pg_toast%'";

/// 被取消的导出等待服务器平静下来要多久。
const SETTLE: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// 规划

/// 转储覆盖的一个关系。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRelation {
    /// `pg_class.oid`，以文本形式保留，因为它会随 SQL 列表一起传递。
    pub oid: String,
    pub schema: String,
    pub name: String,
    /// table、partitioned、view、matview、foreign
    pub kind: String,
    /// 是否导出它的行。视图和物化视图为 false，分区也为 false——分区的行随
    /// 父表一起走。
    pub has_data: bool,
    /// 该关系是分区时，写 `schema.parent`。
    pub partition_of: Option<String>,
    /// 本表外键指向的表，采用 PostgreSQL 自己的写法。
    pub foreign_keys: Vec<Reference>,
}

/// 外键指向哪里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// 约束定义里写成的 `schema.table`。
    pub references: String,
}

impl PlanRelation {
    /// `schema.table`，只在 PostgreSQL 需要时加引号。
    fn qualified(&self) -> String {
        format!("{}.{}", quote_ident(&self.schema), quote_ident(&self.name))
    }

    /// `schema.name`，`partition_of` 使用的形式。
    pub fn key_name(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }

    fn key(&self) -> (String, String) {
        (self.schema.clone(), self.name.clone())
    }
}

/// 算出转储覆盖哪些关系。
///
/// `None` 表示会话可用的所有 schema。传入列表则按用户输入的写法命名表——
/// `public.orders` 或裸的 `orders`——裸名可能匹配多个 schema 里的同名表。
pub async fn plan(
    session: &mut Session,
    scope: Option<&[String]>,
    include_data: bool,
) -> Result<Vec<PlanRelation>, AppError> {
    let mut relations = exportable_relations(session).await?;
    if let Some(wanted) = scope {
        if wanted.is_empty() {
            return Err(AppError::new("empty", "no_tables_selected"));
        }
        let patterns: Vec<(Option<String>, String)> =
            wanted.iter().map(|name| split_name(name)).collect();
        relations.retain(|relation| {
            patterns.iter().any(|(schema, name)| {
                *name == relation.name
                    && schema
                        .as_ref()
                        .map(|schema| *schema == relation.schema)
                        .unwrap_or(true)
            })
        });
        missing_names(wanted, &relations)?;

        // 分区离开父表结构就无法创建，而父表本身可能又是几层之上的分区。
        let mut index = 0;
        while index < relations.len() {
            let parent = relations[index].partition_of.clone();
            index += 1;
            let Some(parent) = parent else { continue };
            if relations.iter().any(|item| item.key() == key_of(&parent)) {
                continue;
            }
            if let Some(ancestor) = load_relation(session, &key_of(&parent)).await? {
                relations.push(ancestor);
            }
        }
    }

    relations.sort_by(|a, b| a.key().cmp(&b.key()));
    for relation in &mut relations {
        relation.has_data = include_data
            && relation.partition_of.is_none()
            && matches!(relation.kind.as_str(), "table" | "partitioned");
    }
    Ok(relations)
}

fn missing_names(wanted: &[String], found: &[PlanRelation]) -> Result<(), AppError> {
    let missing: Vec<&str> = wanted
        .iter()
        .filter(|name| {
            let (schema, relation) = split_name(name);
            !found.iter().any(|item| {
                item.name == relation && schema.as_ref().map(|s| *s == item.schema).unwrap_or(true)
            })
        })
        .map(String::as_str)
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let error = if missing.len() == 1 {
        AppError::new("notfound", "relation_not_found").with_args([missing[0]])
    } else {
        let list = missing.join(", ");
        AppError::new("notfound", "tables_not_found")
            .with_args([missing.len().to_string(), list])
    };
    Err(error.detail("table_not_found_hint"))
}

const RELATION_QUERY: &str = "
    select c.oid::text,
           n.nspname,
           c.relname,
           c.relkind,
           (pg_catalog.pg_get_partkeydef(c.oid) is not null
            or pg_catalog.pg_get_expr(c.relpartbound, c.oid) is not null) as is_partition,
           (select pn.nspname || '.' || pc.relname
              from pg_catalog.pg_inherits i
              join pg_catalog.pg_class pc on pc.oid = i.inhparent
              join pg_catalog.pg_namespace pn on pn.oid = pc.relnamespace
             where i.inhrelid = c.oid
             limit 1) as parent
      from pg_catalog.pg_class c
      join pg_catalog.pg_namespace n on n.oid = c.relnamespace";

async fn exportable_relations(session: &mut Session) -> Result<Vec<PlanRelation>, AppError> {
    let rows = session
        .conn
        .query(&format!(
            "{RELATION_QUERY}
              where c.relkind in ('r', 'p', 'v', 'm', 'f')
                and {SYSTEM_SCHEMA_FILTER}
                and (n.nspname = current_schema()
                     or pg_catalog.has_schema_privilege(n.oid, 'USAGE'))
              order by n.nspname, c.relname"
        ))
        .await?;
    Ok(rows
        .values
        .iter()
        .filter_map(|row| relation_from_row(row))
        .collect())
}

/// 按 `schema.name` 取一个关系，用于不在选定列表里的分区祖先。
async fn load_relation(
    session: &mut Session,
    key: &(String, String),
) -> Result<Option<PlanRelation>, AppError> {
    let rows = session
        .conn
        .query(&format!(
            "{RELATION_QUERY}
              where n.nspname = {} and c.relname = {} and c.relkind in ('r', 'p')",
            lit(&key.0),
            lit(&key.1)
        ))
        .await?;
    Ok(rows.values.first().and_then(|row| relation_from_row(row)))
}

fn relation_from_row(row: &[Option<String>]) -> Option<PlanRelation> {
    let oid = text(row, 0);
    if oid.is_empty() {
        return None;
    }
    Some(PlanRelation {
        oid,
        schema: text(row, 1),
        name: text(row, 2),
        kind: kind_of(&text(row, 3)),
        has_data: false,
        partition_of: if text(row, 4) == "t" {
            row.get(5).cloned().flatten()
        } else {
            None
        },
        foreign_keys: Vec::new(),
    })
}

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

fn is_key(kind: &str) -> bool {
    matches!(kind, "p" | "u" | "x")
}

/// 一串键约束，作为 `do` 块的步骤：每条语句运行在 `wrapped` 打开的块内，
/// 收尾的 `end` 由调用方负责。
///
/// 语句执行时，服务器可能已经不认识前面某条语句创建的对象（给表加上主键后
/// 再从目录里重读这张表，就是实际出现过的情况）；这取决于对象上次解析之后
/// 脚本增长了多少，无法预判。失败的语句会再试一次，试之前先让服务器重新读
/// 一遍目录。
fn push_steps(out: &mut String, statements: &[String]) {
    for (index, statement) in statements.iter().enumerate() {
        let retry = if index + 1 == statements.len() {
            "raise"
        } else {
            "select 1 from pg_catalog.pg_class limit 0"
        };
        let close = if statement.starts_with("do $") { " end" } else { "" };
        out.push_str(&format!(
            "begin\n    {statement}\n    exception when others then\n    {retry}{close};\nend;\n"
        ));
    }
}

/// 把 `body`（一串以 `;` 结尾的语句）放进自己的 `do` 块里，失败时先让服务器
/// 重读目录再重试一次（原因见 `run_steps`）。该块在单个事务中运行：一旦失败
/// 整段回滚，psql 随后停在重新抛出的错误上，效果与停在原语句上相同。
/// `body` 必须以自身的 `;` 结尾语句开头，或者为空。
fn wrapped(header: String, body: &str) -> String {
    if !body.contains(';') {
        return header + body;
    }
    format!("{header}do $steps$\nbegin\n{body}exception when others then\nraise;\nend $steps$;\n\n")
}

/// 脚本开头：让回放能精确复现数据行的设置，以及转储发生的时刻。
fn header(title: &str) -> String {
    format!(
        "--\n-- {title} — exported by pg easy at {} (unix time)\n--\n\n\
         SET statement_timeout = 0;\n\
         SET lock_timeout = 0;\n\
         SET idle_in_transaction_session_timeout = 0;\n\
         SET client_encoding = 'UTF8';\n\
         SET standard_conforming_strings = on;\n\
         SET check_function_bodies = false;\n\
         SET client_min_messages = warning;\n\
         SET row_security = off;\n\n",
        unix_now()
    )
}

// ---------------------------------------------------------------------------
// 数据

/// 数据行部分，边读边流式输出：先截断（子表在前、父表在后，父表就不会在
/// 子表还需要它的时候被清空），然后每张表一个 COPY 块，最后是序列值。
async fn write_data(
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
async fn sequence_values(session: &mut Session) -> Result<Option<String>, AppError> {
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

/// 把每个标识列的隐式序列推到已导入的 id 之后。
fn reset_identity(table: &TableDdl) -> String {
    let qualified = table.relation.qualified();
    let mut out = String::new();
    for column in table.columns.iter().filter(|c| !c.identity.is_empty()) {
        let name = quote_ident(&column.name);
        out.push_str(&format!(
            "select pg_catalog.setval(pg_catalog.pg_get_serial_sequence({}, {}), \
             coalesce(max({name}), 1), max({name}) is not null) from {qualified};\n",
            lit(&qualified),
            lit(&name)
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// 结构

/// 结构段需要的全部内容，预先一次读好，脚本才能按顺序一遍写出。
struct SchemaDump {
    schemas: Vec<String>,
    sequences: Vec<SequenceInfo>,
    tables: Vec<TableDdl>,
    views: Vec<ViewDdl>,
    constraints: Vec<Constraint>,
    indexes: Vec<Simple>,
    triggers: Vec<Simple>,
    /// 数据段要读其行的表，父表在子表之前。
    data_relations: Vec<PlanRelation>,
    /// 每张表的继承父表，用于普通（非分区）继承。
    inheritants: HashMap<(String, String), String>,
}

struct TableDdl {
    relation: PlanRelation,
    columns: Vec<ColumnDdl>,
    partition_key: String,
    partition_bound: String,
    /// 非数据库默认表空间的表空间名。
    tablespace: Option<String>,
    unlogged: bool,
    parent: Option<Parent>,
}

enum Parent {
    /// 通过 `for values <bound>` 挂到父表上。
    Partition(String),
    /// 普通的 `inherits (parent)`。
    Inherit(String),
}

struct ColumnDdl {
    name: String,
    data_type: String,
    default: Option<String>,
    not_null: bool,
    /// `a`（generated always）、`d`（by default）或为空。
    identity: String,
    generated: bool,
    collation: Option<String>,
}

/// 一行 `pg_constraint`，渲染成 `alter table ... add constraint`。
struct Constraint {
    /// `schema.table`，只在 PostgreSQL 需要时加引号。
    relation: String,
    /// PostgreSQL 是否不得不给 schema 或表名加引号；只有这类表需要重新查找
    /// 其键的索引（见 `render`）。
    quoted_relation: bool,
    /// 承载表隐式序列的列（也就是它的 id），前提是键正好只覆盖这一列。
    id_column: Option<String>,
    name: String,
    /// p、u、c、x、f 或 t
    kind: String,
    definition: String,
    /// 外键所指的 `schema.table`。
    references: Option<String>,
    /// 分区自身的键所对应的父表 `schema.table`。这样的键不带行，但它的索引
    /// 挂在父表的索引上，从而保持索引名与约束名一致。
    parent: Option<String>,
}

struct Simple {
    sql: String,
}

struct ViewDdl {
    relation: PlanRelation,
    materialized: bool,
    definition: String,
}

struct SequenceInfo {
    schema: String,
    name: String,
    data_type: String,
    increment: i64,
    min_value: i64,
    max_value: i64,
    start: i64,
    cache: i64,
    cycle: bool,
}

impl SchemaDump {
    async fn load(
        session: &mut Session,
        plan: &[PlanRelation],
        include_data: bool,
        stop: &(dyn Fn() -> Option<AppError> + Send + Sync),
    ) -> Result<Self, AppError> {
        let mut schemas: Vec<String> = plan.iter().map(|r| r.schema.clone()).collect();
        schemas.sort();
        schemas.dedup();

        let data_relations = ordered_data_relations(plan, include_data);
        let oids: Vec<&str> = data_relations
            .iter()
            .map(|relation| relation.oid.as_str())
            .collect();
        let table_keys: HashSet<(String, String)> = plan
            .iter()
            .filter(|relation| matches!(relation.kind.as_str(), "table" | "partitioned"))
            .map(PlanRelation::key)
            .collect();

        // 分区是按边界挂接的，不走 `inherits`，所以只需要为其余表查找父表。
        let mut inheritants = HashMap::new();
        for relation in &data_relations {
            if let Some(error) = stop() {
                return Err(error);
            }
            if relation.partition_of.is_some() {
                continue;
            }
            if let Some(parent) = inheritance_parent(session, relation).await? {
                inheritants.insert(relation.key(), parent);
            }
        }

        let mut tables = Vec::new();
        let mut views = Vec::new();
        for relation in plan {
            if let Some(error) = stop() {
                return Err(error);
            }
            match relation.kind.as_str() {
                "table" | "partitioned" => {
                    tables.push(load_table(session, relation, &table_keys, &inheritants).await?)
                }
                "view" | "matview" => views.push(load_view(session, relation).await?),
                _ => {}
            }
        }

        Ok(Self {
            schemas,
            sequences: load_sequences(session).await?,
            tables,
            views,
            constraints: load_constraints(session, &oids).await?,
            indexes: load_indexes(session, &oids).await?,
            triggers: load_triggers(session, &oids).await?,
            data_relations,
            inheritants,
        })
    }

    /// 回放顺序的 DDL：schema、序列、表（父表总在分区之前）、键、检查约束、
    /// 索引、视图、触发器、待所有表都建好后的外键、继承关系，以及防止后续
    /// 插入与导入 id 冲突的标识列重置。
    fn render(&self) -> String {
        let mut out = String::new();

        if !self.schemas.is_empty() {
            section(&mut out, "Schemas");
            for schema in &self.schemas {
                out.push_str(&format!(
                    "create schema if not exists {};\n",
                    quote_ident(schema)
                ));
            }
        }

        if !self.sequences.is_empty() {
            section(&mut out, "Sequences");
            for sequence in &self.sequences {
                out.push_str(&sequence.create_sql());
            }
        }

        if !self.tables.is_empty() {
            section(&mut out, "Tables");
            for table in self.ordered_tables() {
                out.push_str(&table.render());
            }
        }

        let keys: Vec<&Constraint> = self
            .constraints
            .iter()
            .filter(|c| matches!(c.kind.as_str(), "p" | "u" | "x"))
            .collect();
        if !keys.is_empty() {
            section(&mut out, "Primary keys and unique constraints");
            let statements = keys.iter().map(|constraint| constraint.render()).collect::<Vec<_>>();
            push_steps(&mut out, &statements);
        }

        let checks: Vec<&Constraint> = self
            .constraints
            .iter()
            .filter(|c| matches!(c.kind.as_str(), "c" | "t"))
            .collect();
        if !checks.is_empty() {
            section(&mut out, "Check constraints");
            for constraint in checks {
                out.push_str(&constraint.render());
            }
        }

        if !self.indexes.is_empty() {
            section(&mut out, "Indexes");
            for index in &self.indexes {
                out.push_str(&format!("{};\n", index.sql));
            }
        }

        if !self.views.is_empty() {
            section(&mut out, "Views");
            for view in &self.views {
                out.push_str(&view.render());
            }
        }

        if !self.triggers.is_empty() {
            section(&mut out, "Triggers");
            for trigger in &self.triggers {
                out.push_str(&format!("{};\n", trigger.sql));
            }
        }

        let foreign: Vec<&Constraint> = self
            .constraints
            .iter()
            .filter(|c| c.kind == "f")
            .collect();
        if !foreign.is_empty() {
            section(&mut out, "Foreign keys");
            for constraint in foreign {
                out.push_str(&constraint.render());
            }
        }

        // 导出范围之外的继承父表在目标库里已经存在，所以它的子表放到最后
        // 再挂上去。
        let inheritants: Vec<(&PlanRelation, &String)> = self
            .data_relations
            .iter()
            .filter(|relation| {
                self.tables
                    .iter()
                    .any(|table| table.relation.key() == relation.key())
            })
            .filter_map(|relation| {
                self.inheritants
                    .get(&relation.key())
                    .filter(|parent| {
                        !self
                            .tables
                            .iter()
                            .any(|table| table.relation.key() == key_of(parent))
                    })
                    .map(|parent| (relation, parent))
            })
            .collect();
        if !inheritants.is_empty() {
            section(&mut out, "Inheritance");
            for (relation, parent) in inheritants {
                let (schema, name) = split_name(parent);
                out.push_str(&format!(
                    "alter table {} inherit {}{};\n",
                    relation.qualified(),
                    schema_prefix(&schema),
                    quote_ident(&name)
                ));
            }
        }

        out
    }

    /// 找出父表不在导出范围内的分区的键。这些索引保留着它们当初挂在父表旁边
    /// 时的名字，单独的 `add constraint` 做不到这一点，所以索引经由父表找回，
    /// 之后用 `attach partition` 挂接。
    async fn reconcile_constraints(
        &mut self,
        session: &mut Session,
        relations: &[PlanRelation],
        stop: &(dyn Fn() -> Option<AppError> + Send + Sync),
    ) -> Result<(), AppError> {
        let loose: Vec<&Constraint> = self
            .constraints
            .iter()
            .filter(|constraint| constraint.parent.is_none() && is_key(&constraint.kind))
            .collect();
        if loose.is_empty() {
            return Ok(());
        }
        let oids: Vec<&str> = relations.iter().map(|relation| relation.oid.as_str()).collect();
        let rows = session
            .conn
            .query(&format!(
                "select n.nspname || '.' || c.relname,
                        con.conname,
                        pn.nspname || '.' || pc.relname,
                        (select pcon.conname
                           from pg_catalog.pg_constraint pcon
                          where pcon.conindid = con.conindid
                            and pcon.conrelid::int8 = con.conparentid::int8)
                   from pg_catalog.pg_constraint con
                   join pg_catalog.pg_class c on c.oid = con.conrelid
                   join pg_catalog.pg_namespace n on n.oid = c.relnamespace
                   join pg_catalog.pg_class pc on pc.oid::int8 = con.conparentid::int8
                   join pg_catalog.pg_namespace pn on pn.oid = pc.relnamespace
                  where con.conrelid in ({})
                    and con.conparentid::int8 <> 0",
                oid_list(&oids)
            ))
            .await?;
        for row in &rows.values {
            let relation = relation_name(&text(row, 0));
            let parent = text(row, 2);
            // 父表在导出范围内时，已在 `load_constraints` 里把键还给了子表，
            // 用的是它在目标库中将拥有的名字。
            let (parent_schema, parent_name) = split_name(&parent);
            let inside = relations.iter().any(|other| {
                other.schema == parent_schema.as_deref().unwrap_or("public")
                    && other.name == parent_name
            });
            if inside {
                continue;
            }
            if let Some(error) = stop() {
                return Err(error);
            }
            let Some(constraint) = self
                .constraints
                .iter_mut()
                .find(|constraint| constraint.relation == relation && constraint.name == text(row, 1))
            else {
                continue;
            };
            constraint.parent = Some(parent);
            constraint.name = text(row, 3);
        }
        Ok(())
    }

    /// 标识序列要跟在导入的行之后，所以 `setval` 必须排在它们后面；脚本的
    /// 数据段会调用这里。
    fn identity_resets(&self) -> String {
        if self.data_relations.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        for table in &self.tables {
            if table
                .columns
                .iter()
                .any(|column| !column.identity.is_empty())
            {
                out.push_str(&reset_identity(table));
            }
        }
        out
    }

    /// 父表在子表之前，除此之外按名称字母序。表先创建再挂到父表上，所以父表
    /// 只需在挂接之前建好；两条语句都在这里按这个顺序输出。
    fn ordered_tables(&self) -> Vec<&TableDdl> {
        let mut ordered: Vec<&TableDdl> = Vec::new();
        let mut pending: Vec<&TableDdl> = self.tables.iter().collect();
        while !pending.is_empty() {
            let before = ordered.len();
            let mut index = 0;
            while index < pending.len() {
                let parent = pending[index].parent.as_ref().map(|parent| match parent {
                    Parent::Partition(parent) | Parent::Inherit(parent) => key_of(parent),
                });
                if match parent {
                    Some(key) => ordered.iter().any(|table| table.relation.key() == key),
                    None => true,
                } {
                    ordered.push(pending.remove(index));
                } else {
                    index += 1;
                }
            }
            if ordered.len() == before {
                // 没找到父表（加了引号的名字写法不同，或继承无法形成的
                // 依赖）；继续输出其余部分，让脚本保持完整，而不是悄悄变短。
                ordered.extend(pending.drain(..));
            }
        }
        ordered
    }
}

impl TableDdl {
    /// COPY 读写的列，按表内顺序。生成列被排除：它们的值由计算得出，而
    /// COPY 不接受生成列。
    fn column_list(&self) -> String {
        self.columns
            .iter()
            .filter(|column| !column.generated)
            .map(|column| quote_ident(&column.name))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn render(&self) -> String {
        let qualified = self.relation.qualified();
        let columns: Vec<String> = self.columns.iter().map(ColumnDdl::render).collect();
        let body = if columns.is_empty() {
            String::new()
        } else {
            format!("\n{}\n", columns.join(",\n"))
        };
        let mut sql = format!("create table {qualified} ({body})");
        // 只有独立的被分区表才声明自己的键；分区从父表继承键。
        if !self.partition_key.is_empty() && !matches!(self.parent, Some(Parent::Partition(_))) {
            sql.push_str(&format!(" partition by {}", self.partition_key));
        }
        if let Some(Parent::Inherit(parent)) = &self.parent {
            let (schema, name) = split_name(parent);
            sql.push_str(&format!(
                " inherits ({}{})",
                schema_prefix(&schema),
                quote_ident(&name)
            ));
        }
        sql.push(';');
        // 分区单独创建再挂接，因此它得到父表的形状，而不必在这里重复一遍。
        if let Some(Parent::Partition(parent)) = &self.parent {
            let (schema, name) = split_name(parent);
            sql.push_str(&format!(
                "\nalter table {}{} attach partition {qualified} {};",
                schema_prefix(&schema),
                quote_ident(&name),
                self.partition_bound
            ));
        }
        if self.unlogged {
            sql.push_str(&format!("\nalter table {qualified} set unlogged;"));
        }
        if let Some(tablespace) = &self.tablespace {
            sql.push_str(&format!("\nalter table {qualified} set tablespace {tablespace};"));
        }
        sql.push('\n');
        sql
    }
}

impl ColumnDdl {
    fn render(&self) -> String {
        let mut definition = format!("    {} {}", quote_ident(&self.name), self.data_type);
        if let Some(collation) = &self.collation {
            definition.push_str(&format!(" collate {collation}"));
        }
        match self.identity.as_str() {
            "a" => definition.push_str(" generated always as identity"),
            "d" => definition.push_str(" generated by default as identity"),
            // 生成列把表达式存在普通列放默认值的位置；两者不会同时出现。
            _ if self.generated => {
                if let Some(expression) = &self.default {
                    // `pg_get_expr` 已经给表达式加好了括号。
                    definition.push_str(&format!(" generated always as {expression} stored"));
                }
            }
            _ => {
                if let Some(default) = &self.default {
                    definition.push_str(&format!(" default {default}"));
                }
            }
        }
        if self.not_null {
            definition.push_str(" not null");
        }
        definition
    }
}

impl Constraint {
    /// 键约束当初所用的索引，在名字表露出这一点时返回。
    fn index_name(&self) -> Option<String> {
        let key = match self.kind.as_str() {
            "p" => "pkey",
            "u" => "key",
            "x" => "excl",
            _ => return None,
        };
        let table = split_name(&self.relation).1;
        let prefix = format!("{table}_{key}");
        (self.name == prefix || self.name.starts_with(&format!("{prefix}_")))
            .then_some(self.name.clone())
    }

    fn render(&self) -> String {
        if let (Some(parent), Some(index)) = (&self.parent, self.index_name()) {
            let index = index.as_str();
            // 给分区加键会以分区自己的名字创建索引（`big_high_pkey` 而不是
            // `big_pkey`）；改为挂接父表的索引，名字就会和原来一致。
            return format!(
                "alter index {} attach partition {};\n",
                key_name_sql(parent, index),
                key_name_sql(&self.relation, index)
            );
        }
        let add = format!(
            "alter table only {} add constraint {} {};\n",
            self.relation,
            quote_ident(&self.name),
            self.definition
        );
        let Some(id) = self.id_column.as_ref().filter(|_| self.quoted_relation) else {
            return add;
        };
        // 表名需要加引号时，键的索引会按列名命名（`"U"."id"` 得到
        // `U_id_not_null`），而不是 `create table "U" (id bigint primary key)`
        // 产生的 `U_pkey`。这类表的索引通过它的 id 序列找回并复用，名字因此
        // 保持一致。该查找只对序列所属的那张表作答，其他情况直接加键。
        format!(
            "--\n-- Keys on names that need quoting reuse the index the column\n\
             -- definitions created, so index names stay the same.\n--\n\
             do $keys$\n\
             declare index_of regclass;\n\
             begin\n\
                 index_of := {lookup};\n\
                 if index_of is not null\n\
                    and (select count(*) from pg_catalog.pg_attribute\n\
                          where attrelid = index_of and attnum > 0) = 1 then\n\
                     execute format('alter table only %s add constraint %s %s using index %s',\n\
                         {table}, {name}, {def}, index_of);\n\
                 else\n\
                     {add}\n\
                 end if;\n\
             end $keys$;\n",
            lookup = format!(
                "pg_catalog.pg_get_serial_sequence({}, {})",
                lit(&self.relation),
                lit(id)
            ),
            table = lit(&self.relation),
            name = lit(&self.name),
            def = lit(&self.definition),
            add = add,
        )
    }
}

impl ViewDdl {
    fn render(&self) -> String {
        let qualified = self.relation.qualified();
        let definition = self.definition.trim().trim_end_matches(';');
        if self.materialized {
            format!("create materialized view {qualified} as\n{definition}\nwith no data;\n")
        } else {
            format!("create view {qualified} as\n{definition};\n")
        }
    }
}

impl SequenceInfo {
    fn qualified(&self) -> String {
        format!("{}.{}", quote_ident(&self.schema), quote_ident(&self.name))
    }

    fn create_sql(&self) -> String {
        format!(
            "create sequence {} as {}\n    increment by {}\n    minvalue {}\n    maxvalue {}\n    start with {}\n    cache {}\n    {};\n",
            self.qualified(),
            self.data_type,
            self.increment,
            self.min_value,
            self.max_value,
            self.start,
            self.cache,
            if self.cycle { "cycle" } else { "no cycle" }
        )
    }
}

async fn load_table(
    session: &mut Session,
    relation: &PlanRelation,
    tables: &HashSet<(String, String)>,
    inheritants: &HashMap<(String, String), String>,
) -> Result<TableDdl, AppError> {
    let rows = session
        .conn
        .query(&format!(
            "select a.attname,
                    pg_catalog.format_type(a.atttypid, a.atttypmod),
                    /* serial 列的默认值就是它的序列，直接引用序列要求它已经
                       声明过；列本身存在之后这个查找才能作答。 */
                    case when sequence_of.oid IS NOT NULL then format(
                             'pg_catalog.pg_get_serial_sequence(%L, %L)::regclass',
                             a.attrelid::regclass::text, a.attname)
                         else pg_catalog.pg_get_expr(d.adbin, d.adrelid) end,
                    a.attnotnull,
                    a.attidentity,
                    a.attgenerated,
                    case when a.attcollation <> 0 and a.attcollation <> t.typcollation
                         then pg_catalog.quote_ident(co.collname) end,
                    case when c.relkind = 'p' then pg_catalog.pg_get_partkeydef(c.oid) end,
                    pg_catalog.pg_get_expr(c.relpartbound, c.oid),
                    c.relpersistence,
                    case when ts.spcname is distinct from
                              (select dts.spcname from pg_catalog.pg_database d
                                 join pg_catalog.pg_tablespace dts
                                   on dts.oid = d.dattablespace
                                where d.datname = current_database())
                         then pg_catalog.quote_ident(ts.spcname) end
               from pg_catalog.pg_class c
               left join pg_catalog.pg_attribute a
                      on a.attrelid = c.oid and a.attnum > 0 and not a.attisdropped
               left join pg_catalog.pg_type t on t.oid = a.atttypid
               left join pg_catalog.pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
               left join pg_catalog.pg_collation co on co.oid = a.attcollation
               left join lateral (select seq.oid
                                    from pg_catalog.pg_depend dp
                                    join pg_catalog.pg_class seq on seq.oid = dp.objid
                                   where dp.classid = 'pg_class'::regclass
                                     and dp.refclassid = 'pg_class'::regclass
                                     and dp.refobjid = a.attrelid
                                     and dp.refobjsubid = a.attnum
                                     and dp.deptype = 'a'
                                     and seq.relkind = 'S') sequence_of on true
               left join pg_catalog.pg_tablespace ts on ts.oid = c.reltablespace
              where c.oid = {}
              order by a.attnum",
            lit(&relation.oid)
        ))
        .await?;

    let mut table = TableDdl {
        relation: relation.clone(),
        columns: Vec::new(),
        partition_key: String::new(),
        partition_bound: String::new(),
        tablespace: None,
        unlogged: false,
        parent: match relation.partition_of.as_ref() {
            Some(parent) => Some(Parent::Partition(parent.clone())),
            // 普通 `inherits (parent)` 紧跟在建表之后声明，而且仅当那张父表
            // 也由本次导出创建时。
            None => inheritants
                .get(&relation.key())
                .filter(|parent| tables.contains(&key_of(parent)))
                .map(|parent| Parent::Inherit(parent.clone())),
        },
    };
    let mut first = true;
    for row in &rows.values {
        if first {
            first = false;
            table.partition_key = text(row, 7);
            table.partition_bound = text(row, 8);
            table.unlogged = text(row, 9) == "u";
            table.tablespace = row[10].clone().filter(|value| !value.is_empty());
        }
        if text(row, 0).is_empty() {
            continue;
        }
        table.columns.push(ColumnDdl {
            name: text(row, 0),
            data_type: text(row, 1),
            default: row[2].clone().filter(|value| !value.is_empty()),
            not_null: text(row, 3) == "t",
            identity: text(row, 4),
            generated: text(row, 5) == "s",
            collation: row[6].clone().filter(|value| !value.is_empty()),
        });
    }
    Ok(table)
}

async fn load_view(session: &mut Session, relation: &PlanRelation) -> Result<ViewDdl, AppError> {
    let rows = session
        .conn
        .query(&format!(
            "select pg_catalog.pg_get_viewdef(c.oid, true), c.relkind = 'm'
               from pg_catalog.pg_class c
              where c.oid = {}",
            lit(&relation.oid)
        ))
        .await?;
    let row = rows.values.first();
    Ok(ViewDdl {
        relation: relation.clone(),
        materialized: row.map(|row| text(row, 1) == "t").unwrap_or_default(),
        definition: row.map(|row| text(row, 0)).unwrap_or_default(),
    })
}

/// 独立序列。被列拥有的序列（serial 和标识列）由该列的定义重建，不在这里
/// 声明——包括每个序列对自身的那条依赖。
async fn load_sequences(session: &mut Session) -> Result<Vec<SequenceInfo>, AppError> {
    let rows = session
        .conn
        .query(&format!(
            "select n.nspname, c.relname,
                    pg_catalog.format_type(s.seqtypid, null) as data_type,
                    s.seqincrement::text, s.seqmin::text, s.seqmax::text,
                    s.seqstart::text, s.seqcache::text, s.seqcycle::text
               from pg_catalog.pg_class c
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
               join pg_catalog.pg_sequence s on s.seqrelid = c.oid
              where c.relkind = 'S' and {SYSTEM_SCHEMA_FILTER}
                and (n.nspname = current_schema()
                     or pg_catalog.has_schema_privilege(n.oid, 'USAGE'))
                and not exists (
                      select 1 from pg_catalog.pg_depend dp
                       where dp.classid = 'pg_class'::regclass
                         and dp.objid = c.oid
                         and dp.refclassid = 'pg_class'::regclass
                         and dp.deptype in ('a', 'i')
                    )
              order by n.nspname, c.relname"
        ))
        .await?;
    Ok(rows
        .values
        .iter()
        .map(|row| SequenceInfo {
            schema: text(row, 0),
            name: text(row, 1),
            data_type: text(row, 2),
            increment: int(row, 3),
            min_value: int(row, 4),
            max_value: int(row, 5),
            start: int(row, 6),
            cache: int(row, 7),
            cycle: text(row, 8) == "t",
        })
        .collect())
}

/// 隐式序列（`serial`、标识列）所属的列。
async fn load_constraints(session: &mut Session, oids: &[&str]) -> Result<Vec<Constraint>, AppError> {
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = session
        .conn
        .query(&format!(
            "select n.nspname || '.' || c.relname,
                    con.conname,
                    con.contype,
                    pg_catalog.pg_get_constraintdef(con.oid, true),
                    con.convalidated,
                    /* 外键指向的表，写法与定义中一致。 */
                    case when con.contype = 'f' then
                        (select nt.nspname || '.' || tc.relname
                           from pg_catalog.pg_class tc
                           join pg_catalog.pg_namespace nt on nt.oid = tc.relnamespace
                          where tc.oid = con.confrelid)
                    end,
                    /* 键正好覆盖一个带隐式序列的列：也就是表的 id，
                       不管它叫什么名字。 */
                    case when con.contype = 'p' and array_length(con.conkey, 1) = 1
                          then (select a.attname from pg_catalog.pg_attribute a
                                 where a.attrelid = con.conrelid and a.attnum = con.conkey[1]
                                   and exists (select 1 from pg_catalog.pg_depend dp
                                                where dp.classid = 'pg_class'::regclass
                                                  and dp.refclassid = 'pg_class'::regclass
                                                  and dp.refobjid = con.conrelid
                                                  and dp.refobjsubid = a.attnum
                                                  and dp.deptype in ('a', 'i')))
                    end,
                    /* 分区自己的键属于父表的键。 */
                    case when con.conparentid <> 0 then
                        (select pn.nspname || '.' || pc.relname
                           from pg_catalog.pg_constraint par
                           join pg_catalog.pg_class pc on pc.oid = par.conrelid
                           join pg_catalog.pg_namespace pn on pn.oid = pc.relnamespace
                          where par.oid = con.conparentid)
                    end
               from pg_catalog.pg_constraint con
               join pg_catalog.pg_class c on c.oid = con.conrelid
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
              where con.conrelid in ({})
              order by n.nspname, c.relname, con.contype = 'p' desc, con.conname",
            oid_list(oids)
        ))
        .await?;
    Ok(rows
        .values
        .iter()
        .map(|row| {
            let mut definition = text(row, 3);
            if text(row, 4) != "t" {
                definition.push_str(" not valid");
            }
            Constraint {
                relation: relation_name(&text(row, 0)),
                quoted_relation: needs_quotes(&text(row, 0)),
                references: row[5].clone().filter(|name| !name.is_empty()),
                id_column: row[6].clone().filter(|name| !name.is_empty()),
                name: text(row, 1),
                kind: text(row, 2),
                definition,
                // 转储中将写出的父表名，`render` 写 `alter index … attach`
                // 时需要它。
                parent: row.get(7).cloned().flatten().filter(|name| !name.is_empty()),
            }
        })
        .collect())
}

async fn load_indexes(session: &mut Session, oids: &[&str]) -> Result<Vec<Simple>, AppError> {
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    // 支撑约束的索引随约束一起重建。
    let rows = session
        .conn
        .query(&format!(
            "select pg_catalog.pg_get_indexdef(i.indexrelid)
               from pg_catalog.pg_index i
               join pg_catalog.pg_class c on c.oid = i.indrelid
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
              where i.indrelid in ({})
                and not exists (
                      select 1 from pg_catalog.pg_constraint con
                       where con.conindid = i.indexrelid and con.conrelid = i.indrelid
                    )
              order by n.nspname, c.relname, i.indexrelid",
            oid_list(oids)
        ))
        .await?;
    Ok(rows
        .values
        .iter()
        .map(|row| Simple {
            sql: text(row, 0),
        })
        .collect())
}

async fn load_triggers(session: &mut Session, oids: &[&str]) -> Result<Vec<Simple>, AppError> {
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    // 内部触发器属于约束（外键），不单独重建。
    let rows = session
        .conn
        .query(&format!(
            "select pg_catalog.pg_get_triggerdef(t.oid, true)
               from pg_catalog.pg_trigger t
               join pg_catalog.pg_class c on c.oid = t.tgrelid
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
              where t.tgrelid in ({}) and not t.tgisinternal
              order by n.nspname, c.relname, t.tgname",
            oid_list(oids)
        ))
        .await?;
    Ok(rows
        .values
        .iter()
        .map(|row| Simple {
            sql: text(row, 0),
        })
        .collect())
}

/// 不是分区的表的直接继承父表。
async fn inheritance_parent(
    session: &mut Session,
    relation: &PlanRelation,
) -> Result<Option<String>, AppError> {
    let rows = session
        .conn
        .query(&format!(
            "select pn.nspname || '.' || pc.relname
               from pg_catalog.pg_inherits i
               join pg_catalog.pg_class pc on pc.oid = i.inhparent
               join pg_catalog.pg_namespace pn on pn.oid = pc.relnamespace
              where i.inhrelid = {}
              order by i.inhseqno
              limit 1",
            lit(&relation.oid)
        ))
        .await?;
    Ok(rows
        .values
        .first()
        .map(|row| text(row, 0))
        .filter(|parent| !parent.is_empty()))
}

// ---------------------------------------------------------------------------
// 组装文件

fn section(out: &mut String, title: &str) {
    out.push_str(&format!("\n--\n-- {title}\n--\n\n"));
}

/// 一个未压缩（stored）的 zip，内含 `schema.sql` 和 `data.sql`。
fn build_zip(schema: &str, data: &str) -> Result<Vec<u8>, AppError> {
    let files = [
        ("schema.sql".to_string(), schema.as_bytes().to_vec()),
        ("data.sql".to_string(), data.as_bytes().to_vec()),
    ];
    let mut out = Vec::new();
    write_zip(&mut out, &files).map_err(|err| {
        AppError::new("io", "archive_failed").detail(err.to_string())
    })?;
    Ok(out)
}

fn write_zip(out: &mut Vec<u8>, files: &[(String, Vec<u8>)]) -> std::io::Result<()> {
    let mut central: Vec<u8> = Vec::new();
    let (dos_time, dos_date) = dos_now();
    for (name, content) in files {
        let crc = crc32(content);
        let offset = out.len() as u32;
        let name_bytes = name.as_bytes();

        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // names are UTF-8
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&dos_time.to_le_bytes());
        out.extend_from_slice(&dos_date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(content.len() as u32).to_le_bytes());
        out.extend_from_slice(&(content.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra field
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(content);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&dos_time.to_le_bytes());
        central.extend_from_slice(&dos_date.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(content.len() as u32).to_le_bytes());
        central.extend_from_slice(&(content.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let count = files.len() as u16;
    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(())
}

fn dos_now() -> (u16, u16) {
    let seconds = unix_now();
    let days = seconds / 86_400;
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let date = (((year.clamp(1980, 2107) - 1980) as u16) << 9)
        | ((month as u16) << 5)
        | day as u16;
    let dos_time = ((time / 3600) as u16) << 11
        | (((time % 3600) / 60) as u16) << 5
        | ((time % 60) / 2) as u16;
    (dos_time, date)
}

/// 1970-01-01 以来的天数转成公历日期（Howard Hinnant 算法）。
fn civil_from_days(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// 小工具

/// 数据段要读其行的表，父表在分区之前：COPY 块依赖这个顺序，截断则依赖它的
/// 逆向。复制计划时带上每张表的外键，行的排序需要它，而计划本身不需要。
fn with_foreign_keys(plan: &[PlanRelation], constraints: &[Constraint]) -> Vec<PlanRelation> {
    let mut plan = plan.to_vec();
    for constraint in constraints.iter().filter(|c| c.kind == "f") {
        let Some(target) = &constraint.references else {
            continue;
        };
        let child = key_of(&relation_name(&constraint.relation));
        if let Some(relation) = plan.iter_mut().find(|relation| relation.key() == child) {
            relation.foreign_keys.push(Reference {
                references: target.clone(),
            });
        }
    }
    plan
}

/// `relation` 是否为 `key` 的父表或计划表里的祖先，嵌套分区就是这样找到的。
fn parent_subtree_contains(
    schema: &SchemaDump,
    ancestor: &PlanRelation,
    key: &(String, String),
) -> bool {
    let mut current = Some(key.clone());
    while let Some(key) = current {
        if key == ancestor.key() {
            return true;
        }
        current = schema
            .tables
            .iter()
            .map(|table| &table.relation)
            .find(|item| item.key() == key)
            .and_then(|item| item.partition_of.as_deref())
            .map(key_of);
    }
    false
}

fn ordered_data_relations(plan: &[PlanRelation], include_data: bool) -> Vec<PlanRelation> {
    if !include_data {
        return Vec::new();
    }
    let mut relations: Vec<PlanRelation> = plan
        .iter()
        .filter(|relation| relation.has_data)
        .cloned()
        .collect();
    relations.sort_by(|a, b| {
        a.partition_of
            .is_some()
            .cmp(&b.partition_of.is_some())
            .then_with(|| a.key().cmp(&b.key()))
    });
    relations
}

/// 把表排在其数据行所引用的表之后，这样外键在行到达时已经满足。遇到环时剩下
/// 的表按字母序保留，而不是死循环。
fn order_by_keys(tables: &[PlanRelation]) -> Vec<PlanRelation> {
    // 一张表排在它外键指向的表之后，也排在它的父分区之后。
    let before: HashMap<(String, String), Vec<(String, String)>> = tables
        .iter()
        .map(|relation| {
            let mut deps = Vec::new();
            if let Some(parent) = &relation.partition_of {
                deps.push(key_of(parent));
            }
            for constraint in &relation.foreign_keys {
                let (schema, name) = split_name(&constraint.references);
                let key = (schema.unwrap_or_default(), name);
                deps.push(
                    tables
                        .iter()
                        .find(|table| table.key() == key)
                        .map(|table| table.key())
                        .unwrap_or(key),
                );
            }
            (relation.key(), deps)
        })
        .collect();

    let mut ordered: Vec<PlanRelation> = Vec::new();
    let mut pending: Vec<PlanRelation> = tables.to_vec();
    while !pending.is_empty() {
        let before_count = ordered.len();
        let mut index = 0;
        while index < pending.len() {
            let key = pending[index].key();
            let ready = before
                .get(&key)
                .map(|deps| {
                    deps.iter()
                        .all(|dep| ordered.iter().any(|table| table.key() == *dep))
                })
                .unwrap_or(true);
            if ready {
                ordered.push(pending.remove(index));
            } else {
                index += 1;
            }
        }
        if ordered.len() == before_count {
            // 外键成环：剩下的按计划顺序保留，而不是死循环，让回放去报告
            // 服务器不接受的行。
            ordered.extend(pending.drain(..));
        }
    }
    ordered
}

fn text(row: &[Option<String>], index: usize) -> String {
    row.get(index).cloned().flatten().unwrap_or_default()
}

fn int(row: &[Option<String>], index: usize) -> i64 {
    text(row, index).parse().unwrap_or_default()
}

fn key_of(name: &str) -> (String, String) {
    let (schema, name) = split_name(name);
    (schema.unwrap_or_default(), name)
}

fn kind_of(relkind: &str) -> String {
    match relkind {
        "r" => "table",
        "p" => "partitioned",
        "v" => "view",
        "m" => "matview",
        "f" => "foreign",
        _ => "other",
    }
    .to_string()
}

fn oid_list(oids: &[&str]) -> String {
    let oids: Vec<String> = oids
        .iter()
        .filter_map(|oid| oid.parse::<u32>().ok())
        .map(|oid| oid.to_string())
        .collect();
    if oids.is_empty() {
        "0".into()
    } else {
        oids.join(", ")
    }
}

fn lit(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// 关于 `schema.relname` 的语句是否需要给名字加引号——`to_regclass` 接受
/// 加引号的形式，裸的类型名则不行。
fn needs_quotes(qualified: &str) -> bool {
    relation_name(qualified) != qualified
}

/// `schema.name`，以及它的索引会写成的裸 `name`。
fn key_name_sql(qualified: &str, name: &str) -> String {
    let (schema, _) = split_name(qualified);
    format!("{}{}", schema_prefix(&schema), quote_ident(name))
}

/// 由未加引号的 `schema.relname` 组对得到 `public.orders`，只在 PostgreSQL
/// 需要时给各部分加引号。
fn relation_name(qualified: &str) -> String {
    let (schema, name) = split_name(qualified);
    format!("{}{}", schema_prefix(&schema), quote_ident(&name))
}

fn schema_prefix(schema: &Option<String>) -> String {
    match schema {
        Some(schema) if !schema.is_empty() => format!("{}.", quote_ident(schema)),
        _ => String::new(),
    }
}

/// `public.orders` → (`Some("public")`, `orders`)。加引号的名字可能含点，
/// 所以只拆分未加引号的名字。
pub fn split_name(name: &str) -> (Option<String>, String) {
    let name = name.trim();
    match name.split_once('.') {
        Some((schema, relation)) => {
            let schema = if schema.starts_with('"') {
                unquote(schema)
            } else {
                schema.to_string()
            };
            (Some(schema), unquote(relation))
        }
        None => (None, name.to_string()),
    }
}

fn unquote(name: &str) -> String {
    match name.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => name.to_string(),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_qualified_names() {
        assert_eq!(
            split_name("public.orders"),
            (Some("public".into()), "orders".into())
        );
        assert_eq!(split_name("orders"), (None, "orders".into()));
        assert_eq!(
            split_name("\"my schema\".\"My.Table\""),
            (Some("my schema".into()), "My.Table".into())
        );
    }

    #[test]
    fn quotes_relation_names_only_where_needed() {
        assert_eq!(relation_name("public.users"), "public.users");
        assert_eq!(relation_name("public.User"), "public.\"User\"");
    }

    #[test]
    fn sorts_tables_parent_first() {
        let plan = vec![
            PlanRelation {
                oid: "1".into(),
                schema: "public".into(),
                name: "events_2024".into(),
                kind: "table".into(),
                has_data: true,
                partition_of: Some("public.events".into()),
                foreign_keys: Vec::new(),
            },
            PlanRelation {
                oid: "2".into(),
                schema: "public".into(),
                name: "events".into(),
                kind: "partitioned".into(),
                has_data: true,
                partition_of: None,
                foreign_keys: Vec::new(),
            },
        ];
        let ordered = ordered_data_relations(&plan, true);
        assert_eq!(ordered[0].name, "events");
        assert_eq!(ordered[1].name, "events_2024");
    }

    #[test]
    fn writes_a_readable_zip_container() {
        // 容器是手写的，所以它的结构值得固定下来：本地文件头、各文件，然后
        // 是中央目录结束标记。
        let zip = build_zip("schema", "data").unwrap();
        assert_eq!(&zip[0..4], b"PK\x03\x04");
        assert_eq!(&zip[zip.len() - 22..zip.len() - 18], b"PK\x05\x06");
        let names = String::from_utf8_lossy(&zip);
        assert!(names.contains("schema.sql") && names.contains("data.sql"));
    }

    #[test]
    fn computes_crc32() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn renders_dos_dates_in_range() {
        let (_, date) = dos_now();
        let year = ((date >> 9) as i64) + 1980;
        assert!((2024..2108).contains(&year), "unexpected DOS year {year}");
    }
}
