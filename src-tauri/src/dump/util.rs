//! 转储各段共用的小工具：行取值、名字拆分、SQL 字面量。

use std::time::{SystemTime, UNIX_EPOCH};

use crate::catalog::quote_ident;

pub(crate) fn text(row: &[Option<String>], index: usize) -> String {
    row.get(index).cloned().flatten().unwrap_or_default()
}

pub(crate) fn int(row: &[Option<String>], index: usize) -> i64 {
    text(row, index).parse().unwrap_or_default()
}

pub(crate) fn key_of(name: &str) -> (String, String) {
    let (schema, name) = split_name(name);
    (schema.unwrap_or_default(), name)
}

pub(crate) fn kind_of(relkind: &str) -> String {
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

pub(crate) fn oid_list(oids: &[&str]) -> String {
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

pub(crate) fn lit(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// 关于 `schema.relname` 的语句是否需要给名字加引号——`to_regclass` 接受
/// 加引号的形式，裸的类型名则不行。
pub(crate) fn needs_quotes(qualified: &str) -> bool {
    relation_name(qualified) != qualified
}

/// `schema.name`，以及它的索引会写成的裸 `name`。
pub(crate) fn key_name_sql(qualified: &str, name: &str) -> String {
    let (schema, _) = split_name(qualified);
    format!("{}{}", schema_prefix(&schema), quote_ident(name))
}

/// 由未加引号的 `schema.relname` 组对得到 `public.orders`，只在 PostgreSQL
/// 需要时给各部分加引号。
pub(crate) fn relation_name(qualified: &str) -> String {
    let (schema, name) = split_name(qualified);
    format!("{}{}", schema_prefix(&schema), quote_ident(&name))
}

pub(crate) fn schema_prefix(schema: &Option<String>) -> String {
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

pub(crate) fn unquote(name: &str) -> String {
    match name.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => name.to_string(),
    }
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub(crate) fn section(out: &mut String, title: &str) {
    out.push_str(&format!("\n--\n-- {title}\n--\n\n"));
}
