//! SQL dumps.
//!
//! Rather than shelling out to `pg_dump`, the script is assembled from the
//! catalog: table structure comes from `pg_catalog` (columns, defaults,
//! constraints, indexes, triggers, partitions, foreign keys, sequences) and
//! data comes out through `COPY ... TO STDOUT`, which the wire client already
//! understands. The result is plain SQL that replays with `psql -f`, ordered
//! so every object's dependencies exist before it is used.
//!
//! The whole dump is read inside one read-only `repeatable read` transaction,
//! so the structure and the rows describe the same moment even while the
//! database is written to; the transaction is ended again whether the dump
//! finishes, fails or is cancelled.
//!
//! The output streams: the structure is produced in one piece (it has to be
//! ordered as a whole), then the rows go out one `copy ... from stdin` block
//! per table, so a large database never sits in memory twice.
//!
//! Two shapes are produced from the same pieces: one SQL script holding
//! everything, or a zip holding `schema.sql` and `data.sql` separately.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::catalog::quote_ident;
use crate::error::AppError;
use crate::protocol::{DumpFormat, DumpOptions};
use crate::state::Session;

const SYSTEM_SCHEMA_FILTER: &str =
    "n.nspname not in ('pg_catalog', 'information_schema') and n.nspname not like 'pg_toast%'";

/// How long a cancelled export waits for the server to settle.
const SETTLE: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Planning

/// One relation the dump covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRelation {
    /// `pg_class.oid`, kept as text because it rides along in SQL lists.
    pub oid: String,
    pub schema: String,
    pub name: String,
    /// table, partitioned, view, matview, foreign
    pub kind: String,
    /// Whether its rows are exported. False for views and materialized views,
    /// and for partitions, whose rows travel with their parent.
    pub has_data: bool,
    /// `schema.parent` when this relation is a partition.
    pub partition_of: Option<String>,
    /// Tables named by this table's foreign keys, as PostgreSQL spells them.
    pub foreign_keys: Vec<Reference>,
}

/// Where a foreign key points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// `schema.table` as written in the constraint's definition.
    pub references: String,
}

impl PlanRelation {
    /// `schema.table`, quoting only what PostgreSQL needs.
    fn qualified(&self) -> String {
        format!("{}.{}", quote_ident(&self.schema), quote_ident(&self.name))
    }

    /// `schema.name`, the form `partition_of` uses.
    pub fn key_name(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }

    fn key(&self) -> (String, String) {
        (self.schema.clone(), self.name.clone())
    }
}

/// Works out which relations the dump covers.
///
/// `None` means every schema the session can use. A list names tables as the
/// user typed them — `public.orders`, or bare `orders` — and a bare name may
/// match the same table in several schemas.
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

        // A partition cannot be created without its parent's structure, and
        // the parent may itself be a partition several levels up.
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

/// One relation by `schema.name`, used for partition ancestors that were not
/// part of the chosen list.
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
// Running a dump

/// Where the script is written: a file on disk, or a buffer in tests.
pub trait DumpSink: Send {
    fn write(&mut self, bytes: &[u8]) -> Result<(), AppError>;
}

impl DumpSink for Vec<u8> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), AppError> {
        self.extend_from_slice(bytes);
        Ok(())
    }
}

/// Runs the dump inside a snapshot transaction, writing the script straight
/// to `sink` as it is produced, and reports how many tables contributed rows.
///
/// `cancelled` is polled between tables so the dialog's stop button can act
/// before the last relation has been read.
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
    // Success, failure, a lost socket or a cancel request all land here: the
    // snapshot ends before the session is handed back to the editor.
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
    // The rows section is ordered by foreign keys, which the structure load
    // does not know about yet.
    schema.data_relations = ordered_data_relations(&relations, options.include_data);
    let tables = ordered_data_relations(&relations, options.include_data);

    let structure = schema.render();
    match options.format {
        DumpFormat::Zip => {
            // A zip needs every entry's size before its directory can be
            // written, so this shape is assembled in memory.
            let data = if options.include_data {
                let mut buffer: Vec<u8> = Vec::new();
                write_data(session, &schema, &mut buffer, stop).await?;
                String::from_utf8_lossy(&buffer).into_owned()
            } else {
                "-- Structure only: this dump holds no table rows.\n\n".to_string()
            };
            // The rows are read only after a schema that replays cleanly is
            // known to exist, so the schema decides how the script is shaped.
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

/// A list of key constraints as the steps of a `do` block: each statement runs
/// inside the block `wrapped` opens, and the closing `end` is the caller's.
///
/// By the time a statement runs, the server may no longer know an object an
/// earlier one created (a table re-read from the catalog after its key was
/// added is the case that showed up); whether that happens depends on how much
/// the script has grown since the object was last parsed, so it cannot be
/// predicted. A failed statement gets one more try, after the server is made
/// to read its catalogs again.
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

/// Puts `body`, a run of `;`-terminated statements, inside a `do` block of its
/// own that retries a failed statement once after making the server read its
/// catalogs again (see `run_steps` for why). The block runs in one
/// transaction: a failure rolls the whole run back, and psql then stops on
/// the re-raised error as it would have on the statement itself. `body` must
/// start with a `;`-terminated statement of its own or be empty.
fn wrapped(header: String, body: &str) -> String {
    if !body.contains(';') {
        return header + body;
    }
    format!("{header}do $steps$\nbegin\n{body}exception when others then\nraise;\nend $steps$;\n\n")
}

/// The script preamble: settings that make a replay reproduce the rows
/// exactly, and the moment the dump was taken.
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
// Data

/// The rows section, streamed as it is read: truncations first (children
/// before parents, so a parent is never cleared while a child still needs
/// it), then one COPY block per table, then sequence values.
async fn write_data(
    session: &mut Session,
    schema: &SchemaDump,
    sink: &mut (dyn DumpSink + Send),
    stop: &(dyn Fn() -> Option<AppError> + Send + Sync),
) -> Result<(), AppError> {
    // Parents first, and a table after everything it has foreign keys to.
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

    // Partitions carry no rows of their own; a parent's rows are read
    // through them, however deeply they are nested.
    let columns: HashMap<(String, String), String> = schema
        .tables
        .iter()
        .map(|table| (table.relation.key(), table.column_list()))
        .collect();

    for relation in &tables {
        if let Some(error) = stop() {
            return Err(error);
        }
        // COPY refuses generated columns, so both sides name their columns.
        let column_list = columns.get(&relation.key()).cloned().unwrap_or_default();
        // `copy (select …)` rather than `copy table`: it keeps the statement
        // a plain COPY, which the client can read, whatever policies apply.
        let source = if relation.kind == "partitioned" {
            // A partitioned parent holds no rows of its own and refuses a
            // plain COPY, so its leaves are read instead — but only the ones
            // the dump covers, since a partition left out of the plan must
            // stay empty.
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

/// `setval` for every free-standing sequence, so the next insert does not
/// reuse a value that came in with the rows. Sequences the user cannot read,
/// or that were never called, keep their declared start.
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

/// Moves each identity column's implicit sequence past the imported ids.
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
// Structure

/// Everything the structure section needs, read up front so the script is
/// written in one ordered pass.
struct SchemaDump {
    schemas: Vec<String>,
    sequences: Vec<SequenceInfo>,
    tables: Vec<TableDdl>,
    views: Vec<ViewDdl>,
    constraints: Vec<Constraint>,
    indexes: Vec<Simple>,
    triggers: Vec<Simple>,
    /// Tables whose rows the data section reads, parents before children.
    data_relations: Vec<PlanRelation>,
    /// Inheritance parent per table, for plain (non-partition) inheritance.
    inheritants: HashMap<(String, String), String>,
}

struct TableDdl {
    relation: PlanRelation,
    columns: Vec<ColumnDdl>,
    partition_key: String,
    partition_bound: String,
    /// A tablespace name other than the database's default.
    tablespace: Option<String>,
    unlogged: bool,
    parent: Option<Parent>,
}

enum Parent {
    /// Attached to its parent with `for values <bound>`.
    Partition(String),
    /// A plain `inherits (parent)`.
    Inherit(String),
}

struct ColumnDdl {
    name: String,
    data_type: String,
    default: Option<String>,
    not_null: bool,
    /// `a` (generated always), `d` (by default) or empty.
    identity: String,
    generated: bool,
    collation: Option<String>,
}

/// One `pg_constraint` row, rendered as an `alter table ... add constraint`.
struct Constraint {
    /// `schema.table`, quoting only what PostgreSQL needs.
    relation: String,
    /// Whether PostgreSQL had to quote the schema or table name; only such
    /// tables need their key's index looked up again (see `render`).
    quoted_relation: bool,
    /// The column carrying the table's implicit sequence (its id), when the
    /// key covers exactly that column.
    id_column: Option<String>,
    name: String,
    /// p, u, c, x, f or t
    kind: String,
    definition: String,
    /// For a foreign key, the `schema.table` it refers to.
    references: Option<String>,
    /// For a partition's own key, the `schema.table` of its parent. Such a
    /// key carries no row, but its index is attached to the parent's, which
    /// keeps index and constraint names identical.
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

        // Partitions are attached by their bound, not through `inherits`, so
        // only the remaining tables need their parent looked up.
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

    /// DDL in replay order: schemas, sequences, tables (a parent always
    /// before its partitions), keys, checks, indexes, views, triggers,
    /// foreign keys once every table exists, inheritance links, and the
    /// identity resets that keep later inserts from colliding with imported
    /// ids.
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

        // An inherited parent outside the export already exists in the
        // target, so its children are linked to it at the end instead.
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

    /// Finds the keys of partitions whose parent is not part of the export.
    /// Their indexes keep the names they had next to that parent, which a
    /// standalone `add constraint` would not, so the index is recovered
    /// through the parent and `attach partition` links it afterwards.
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
            // A parent inside the export already gave the child its key back
            // in `load_constraints`, under the name it will have there.
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

    /// Identity sequences follow the imported rows, so their `setval` has to
    /// come after them; the data section of the script calls this.
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

    /// Parents before children, in alphabetical order otherwise. A table is
    /// created first and attached to its parent afterwards, so the parent
    /// only has to be created before the attach; both statements are emitted
    /// here, in that order.
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
                // The parent was not found (a quoted name spelled differently,
                // or a dependency inheritance cannot form); emitting the rest
                // keeps the script complete rather than silently shorter.
                ordered.extend(pending.drain(..));
            }
        }
        ordered
    }
}

impl TableDdl {
    /// The columns COPY reads and writes, in table order. Generated columns
    /// are left out: their values are computed, and COPY rejects them.
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
        // Only a standalone partitioned table declares its key; a partition
        // inherits the key from its parent.
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
        // A partition is created on its own and then attached, so it gets the
        // parent's shape without repeating it here.
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
            // A generated column stores its expression where a plain column
            // stores its default; the two never appear together.
            _ if self.generated => {
                if let Some(expression) = &self.default {
                    // `pg_get_expr` already parenthesises the expression.
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
    /// The index a key constraint was made from, when its name says so.
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
            // Adding a key to a partition would create the index under the
            // partition's own name (`big_high_pkey` rather than `big_pkey`);
            // attaching the parent's index instead names them as they are.
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
        // A key on a table whose name needs quoting gets an index named
        // after its column (`"U"."id"` gives `U_id_not_null`) instead of the
        // `U_pkey` that `create table "U" (id bigint primary key)` makes.
        // Such a table's index is recovered through its id sequence and
        // reused, which keeps the name the same. The lookup only answers for
        // the sequence's own table, so anything else adds the key instead.
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
                    /* A serial column's default is its sequence, and naming
                       the sequence directly needs it already declared; the
                       lookup answers once the column itself exists. */
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
            // A plain `inherits (parent)` is declared right after the table,
            // but only when that parent is created by this export too.
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

/// Free-standing sequences. Ones owned by a column (serial and identity) are
/// recreated by that column's own definition and are not declared here —
/// including the sequence's own dependency on itself, which every sequence
/// has.
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

/// Columns an implicit sequence (`serial`, identity) belongs to.
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
                    /* The table a foreign key refers to, spelled the way
                       the definition spells it. */
                    case when con.contype = 'f' then
                        (select nt.nspname || '.' || tc.relname
                           from pg_catalog.pg_class tc
                           join pg_catalog.pg_namespace nt on nt.oid = tc.relnamespace
                          where tc.oid = con.confrelid)
                    end,
                    /* A key covers exactly one column with an implicit
                       sequence: the table's id, whichever name that has. */
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
                    /* A partition's own key belongs to its parent's. */
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
                // The parent's name as the dump will spell it, which is
                // what `render` needs to write `alter index … attach`.
                parent: row.get(7).cloned().flatten().filter(|name| !name.is_empty()),
            }
        })
        .collect())
}

async fn load_indexes(session: &mut Session, oids: &[&str]) -> Result<Vec<Simple>, AppError> {
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    // Indexes that back a constraint are recreated with it.
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
    // Internal triggers belong to constraints (foreign keys) and are not
    // recreated on their own.
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

/// The direct inheritance parent of a table that is not a partition.
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
// File assembly

fn section(out: &mut String, title: &str) {
    out.push_str(&format!("\n--\n-- {title}\n--\n\n"));
}

/// A stored (no compression) zip holding `schema.sql` and `data.sql`.
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

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
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
// Small helpers

/// The tables whose rows the data section reads, parents before partitions:
/// copy blocks rely on that order, and the truncations rely on its reverse.
/// Copies the plan with each table's foreign keys, which the ordering of
/// rows needs and the plan itself does not.
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

/// Whether `relation` is `key`'s parent or an ancestor of it in the planned
/// tables, which is how nested partitions are found.
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

/// Puts tables after the tables their rows reference, so a foreign key is
/// already satisfied when its rows arrive. A cycle keeps the remaining
/// tables in alphabetical order rather than looping.
fn order_by_keys(tables: &[PlanRelation]) -> Vec<PlanRelation> {
    // A table follows the tables it has a foreign key to, and its parent
    // partition.
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
            // A cycle of foreign keys: keep what is left in plan order
            // rather than looping, and let the replay report any row the
            // server will not take.
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

/// Whether a statement about `schema.relname` will need the name quoted,
/// which `to_regclass` accepts and a plain type name would not.
fn needs_quotes(qualified: &str) -> bool {
    relation_name(qualified) != qualified
}

/// `schema.name` with the bare `name` as its index would be spelled.
fn key_name_sql(qualified: &str, name: &str) -> String {
    let (schema, _) = split_name(qualified);
    format!("{}{}", schema_prefix(&schema), quote_ident(name))
}

/// `public.orders` from an unquoted `schema.relname` pair, quoting each part
/// only when PostgreSQL would need it.
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

/// `public.orders` → (`Some("public")`, `orders`). A quoted name may carry a
/// dot, so only unquoted names are split.
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
        // The container is hand-rolled, so its shape is worth pinning down:
        // a local header, the files, then the end of central directory.
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
