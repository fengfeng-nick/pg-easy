//! 返回给界面（以及从界面接受）的数据。字段名是 camelCase，TypeScript 侧
//! 因此不需要映射层。

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
    /// json/jsonb —— 按文本渲染，但可以就地展开。
    Json,
    /// bytea —— 显示十六进制预览，绝不显示原始字节。
    Binary,
    Time,
    Uuid,
    Array,
    /// 服务器只按 OID 报告的类型，例如基于 json 的域。
    Other,
}

impl ValueKind {
    pub fn of(ty: &Type) -> Self {
        // 数组类型在 pg_type.typtype 里是元素类型的 'b'，但命名约定（前导
        // 下划线）加上驱动的内置查找在这里已经足够可靠。
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

    /// 用于驱动内置表之外的 OID，而简单查询的结果只能提供这么多信息。按
    /// OID 猜测在有自定义类型的服务器上会出错，所以任何不常见的东西都按
    /// 纯文本处理。
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

/// 普通查询会返回的 OID 对应的名字；当驱动无法把 OID 映射成 `Type` 时，
/// 用于列表头的类型标签。
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
    /// 语句在脚本中的位置，从 0 开始。
    pub index: usize,
    pub sql: String,
    /// 语句在脚本中的字节偏移；界面用它把错误和结果标签页映射回编辑器。
    pub start: usize,
    pub has_rows: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<Column>>,
    /// 以字符串编码的行主序值。`null` 表示 SQL NULL，空字符串表示空值
    /// ——两者不能混为一谈。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<Vec<Option<String>>>>,
    /// 语句影响的行数（INSERT/UPDATE/DELETE/MOVE……）。
    pub affected: u64,
    /// SELECT 返回的行数。
    pub returned: u64,
    /// PostgreSQL 报告的命令标签（"UPDATE 3"），已知时给出。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    pub duration_ms: f64,
    /// 到达的行数超过显示上限，表格只持有前缀部分。
    pub truncated: bool,
    /// 这条语句抛出的 RAISE NOTICE / WARNING 消息，以界面会格式化的 token
    /// 形式给出（`notice`、`notice_at`）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<Translated>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    pub results: Vec<StatementResult>,
    pub total_duration_ms: f64,
    /// 脚本结束时的事务状态：idle、intransaction、inerror。
    pub tx_status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    /// 请求这一页时表格里已经持有的行数。
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
    /// table、view、matview、partitioned、foreign、sequence
    pub kind: String,
    pub columns: i32,
    pub est_rows: i64,
    pub size: String,
    /// 100 表示“被数据库拥有”；0 表示未分析。
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
    /// function、procedure 或 aggregate
    pub kind: String,
    pub arguments: String,
    pub returns: String,
}

/// 转储如何打包：单个 SQL 脚本，或一个含 `schema.sql` 与 `data.sql` 的
/// zip。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DumpFormat {
    Sql,
    Zip,
}

/// 导出对话框询问的内容。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DumpOptions {
    /// 正在导出的数据库；它给生成的文件命名，并出现在脚本头部。
    pub database: String,
    pub format: DumpFormat,
    /// `false` 时只导出结构。
    pub include_data: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub schemas: Vec<SchemaInfo>,
    /// 以 schema 名为键。
    pub relations: HashMap<String, Vec<RelationInfo>>,
    pub routines: HashMap<String, Vec<RoutineInfo>>,
    pub fetched_at: u64,
}

/// 导入对话框询问的内容。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOptions {
    /// 把整个脚本包在一个事务里，失败时不留下任何残留。对于 PostgreSQL 拒绝
    /// 在事务中执行的语句（例如 `create index concurrently`）应关闭它。
    #[serde(default = "yes")]
    pub in_transaction: bool,
}

fn yes() -> bool {
    true
}

/// 用户选中的 SQL 文件，在读取之前先描述出来。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedSqlFile {
    pub path: String,
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    /// 文件包含的语句数，不含注释和空语句。
    pub statements: usize,
    /// `COPY` 块以及 INSERT、UPDATE、DELETE 写入的行数。
    pub rows: u64,
    /// 经 `COPY` 到达的行数，是 `rows` 的一部分。
    pub copied_rows: u64,
    /// 读取的文件体积。
    pub bytes: u64,
    pub duration_ms: f64,
}
