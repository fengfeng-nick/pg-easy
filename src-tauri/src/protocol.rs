//! Data returned to (and accepted from) the UI. Field names are camelCase so
//! the TypeScript side needs no mapping layer.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio_postgres::types::Type;

use crate::connection::ConnectParams;
use crate::error::Translated;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueKind {
    Null,
    Bool,
    Number,
    Text,
    /// json/jsonb — rendered as text but with inline expansion available.
    Json,
    /// bytea — rendered as a hex preview, never as raw bytes.
    Binary,
    Time,
    Uuid,
    Array,
    /// A type the server reported only by OID, e.g. a domain over json.
    Other,
}

impl ValueKind {
    pub fn of(ty: &Type) -> Self {
        // pg_type.typtype for array types is 'b' on the element type, but the
        // name convention (leading underscore) plus the driver's built-in
        // lookup is reliable enough here.
        if ty.name().starts_with('_') || matches!(ty.kind(), tokio_postgres::types::Kind::Array(_))
        {
            return ValueKind::Array;
        }
        match *ty {
            Type::BOOL => ValueKind::Bool,
            Type::INT2 | Type::INT4 | Type::INT8 | Type::FLOAT4 | Type::FLOAT8 | Type::NUMERIC
            | Type::OID => ValueKind::Number,
            Type::JSON | Type::JSONB => ValueKind::Json,
            Type::BYTEA => ValueKind::Binary,
            Type::DATE | Type::TIME | Type::TIMETZ | Type::TIMESTAMP | Type::TIMESTAMPTZ => {
                ValueKind::Time
            }
            Type::UUID => ValueKind::Uuid,
            Type::JSON_ARRAY | Type::JSONB_ARRAY | Type::TEXT_ARRAY | Type::INT4_ARRAY
            | Type::INT8_ARRAY | Type::UUID_ARRAY => ValueKind::Array,
            _ => ValueKind::Text,
        }
    }

    /// For OIDs outside the driver's built-in table, which is all a simple
    /// query result tells us. Guessing by OID would be wrong on a server with
    /// custom types, so anything unusual stays plain text.
    pub fn of_oid(oid: u32) -> Self {
        match oid {
            16 => ValueKind::Bool,
            20 | 21 | 23 | 26 | 700 | 701 | 1700 => ValueKind::Number,
            25 | 1043 | 19 => ValueKind::Text,
            114 | 3802 => ValueKind::Json,
            17 => ValueKind::Binary,
            1082 | 1083 | 1266 | 1114 | 1184 => ValueKind::Time,
            2950 => ValueKind::Uuid,
            1000 | 1005 | 1007 | 1009 | 1015 | 1016 | 1021 | 1022 | 199 | 3807 | 2951 => {
                ValueKind::Array
            }
            _ => ValueKind::Other,
        }
    }
}

/// Names for the OIDs a typical query returns; used for the column header's
/// type label when the driver cannot map the OID to a `Type`.
pub fn type_name_of_oid(oid: u32) -> String {
    let name = match oid {
        16 => "bool",
        17 => "bytea",
        18 => "char",
        19 => "name",
        20 => "int8",
        21 => "int2",
        23 => "int4",
        25 => "text",
        26 => "oid",
        114 => "json",
        142 => "xml",
        700 => "float4",
        701 => "float8",
        790 => "money",
        829 => "macaddr",
        1042 => "bpchar",
        1043 => "varchar",
        1082 => "date",
        1083 => "time",
        1114 => "timestamp",
        1184 => "timestamptz",
        1266 => "timetz",
        1700 => "numeric",
        2950 => "uuid",
        3802 => "jsonb",
        1000 => "bool[]",
        1007 => "int4[]",
        1009 => "text[]",
        1015 => "varchar[]",
        1016 => "int8[]",
        1021 => "float4[]",
        1022 => "float8[]",
        199 => "json[]",
        3807 => "jsonb[]",
        2951 => "uuid[]",
        _ => return format!("oid {oid}"),
    };
    name.to_string()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    pub type_name: String,
    pub type_oid: u32,
    pub kind: ValueKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementResult {
    /// Position of the statement in the script, 0-based.
    pub index: usize,
    pub sql: String,
    /// Byte offset of the statement in the script; the UI maps errors and
    /// result tabs back onto the editor with it.
    pub start: usize,
    pub has_rows: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<Column>>,
    /// Row-major values encoded as strings. `null` means SQL NULL, an empty
    /// string means an empty value — the two must not be conflated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<Vec<Option<String>>>>,
    /// Rows the statement modified (INSERT/UPDATE/DELETE/MOVE...).
    pub affected: u64,
    /// Rows a SELECT returned.
    pub returned: u64,
    /// The command tag PostgreSQL reports ("UPDATE 3"), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    pub duration_ms: f64,
    /// More rows arrived than the display limit, so the grid holds a prefix.
    pub truncated: bool,
    /// RAISE NOTICE / WARNING messages raised by this statement, as tokens
    /// the UI formats (`notice`, `notice_at`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<Translated>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    pub results: Vec<StatementResult>,
    pub total_duration_ms: f64,
    /// Transaction state when the script finished: idle, intransaction, inerror.
    pub tx_status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    /// Rows already held by the grid when this page was requested.
    pub offset: usize,
    pub limit: usize,
    pub rows: Vec<Vec<Option<String>>>,
    pub exhausted: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub tx_status: String,
    pub server_version: String,
    pub fingerprint: String,
    pub backend_pid: i32,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub id: String,
    pub tx_status: String,
    pub backend_pid: i32,
    pub server_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedConnection {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub params: ConnectParams,
    #[serde(default)]
    pub save_password: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseResult {
    pub server_version: String,
    pub current_database: String,
    pub current_user: String,
    pub databases: Vec<DatabaseInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
    pub name: String,
    pub owner: String,
    pub encoding: String,
    pub size: String,
    pub allow_connections: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaInfo {
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationInfo {
    pub oid: u32,
    pub schema: String,
    pub name: String,
    /// table, view, matview, partitioned, foreign, sequence
    pub kind: String,
    pub columns: i32,
    pub est_rows: i64,
    pub size: String,
    /// 100 is "owned by the database"; 0 means unanalyzed.
    pub last_autoanalyze: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub default_value: Option<String>,
    pub position: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineInfo {
    pub schema: String,
    pub name: String,
    /// function, procedure, or aggregate
    pub kind: String,
    pub arguments: String,
    pub returns: String,
}

/// How a dump is packaged: one SQL script, or a zip with `schema.sql` and
/// `data.sql`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DumpFormat {
    Sql,
    Zip,
}

/// What the export dialog asks for.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DumpOptions {
    /// Database being exported; names the generated file and appears in the
    /// script header.
    pub database: String,
    pub format: DumpFormat,
    /// `false` exports the structure only.
    pub include_data: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub schemas: Vec<SchemaInfo>,
    /// Keyed by schema name.
    pub relations: HashMap<String, Vec<RelationInfo>>,
    pub routines: HashMap<String, Vec<RoutineInfo>>,
    pub fetched_at: u64,
}
