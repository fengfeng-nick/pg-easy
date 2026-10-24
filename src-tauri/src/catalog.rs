//! 为侧边栏和 SQL 编辑器的补全列表读取 `pg_catalog`。
//! 每个库只做几次往返，几百个关系也能保持侧边栏响应迅速；所有内容都按当前
//! 用户的权限过滤，树里永远不会列出打不开的对象。

use std::collections::HashMap;

use crate::error::AppError;
use crate::protocol::{Catalog, ColumnInfo, DatabaseInfo, RelationInfo, RoutineInfo, SchemaInfo};
use crate::state::Session;

const SYSTEM_SCHEMA_FILTER: &str =
    "n.nspname not in ('pg_catalog', 'information_schema') and n.nspname not like 'pg_toast%'";

pub async fn load_overview(session: &mut Session) -> Result<Vec<DatabaseInfo>, AppError> {
    let rows = session
        .conn
        .query(
            "select d.datname,
                    pg_catalog.pg_get_userbyid(d.datdba) as owner,
                    pg_catalog.pg_encoding_to_char(d.encoding) as encoding,
                    pg_catalog.pg_size_pretty(pg_catalog.pg_database_size(d.datname)) as size,
                    d.datallowconn
               from pg_catalog.pg_database d
              where not d.datistemplate
              order by d.datname",
        )
        .await?;

    Ok(rows
        .values
        .iter()
        .map(|row| DatabaseInfo {
            name: text(row, 0),
            owner: text(row, 1),
            encoding: text(row, 2),
            size: text(row, 3),
            allow_connections: text(row, 4) == "t",
        })
        .collect())
}

pub async fn load_catalog(session: &mut Session) -> Result<Catalog, AppError> {
    let schemas = session
        .conn
        .query(&format!(
            "select n.nspname
               from pg_catalog.pg_namespace n
              where {SYSTEM_SCHEMA_FILTER}
                and (n.nspname = current_schema()
                     or pg_catalog.has_schema_privilege(n.oid, 'USAGE'))
              order by n.nspname = 'public' desc, n.nspname"
        ))
        .await?;

    let schema_rows: Vec<SchemaInfo> = schemas
        .values
        .iter()
        .map(|row| SchemaInfo { name: text(row, 0) })
        .collect();
    let schema_list = sql_list(&schema_rows.iter().map(|s| s.name.as_str()).collect::<Vec<_>>());

    let relations = session
        .conn
        .query(&format!(
            "select c.oid,
                    n.nspname,
                    c.relname,
                    case c.relkind
                      when 'r' then 'table'
                      when 'p' then 'partitioned'
                      when 'v' then 'view'
                      when 'm' then 'matview'
                      when 'f' then 'foreign'
                      when 'S' then 'sequence'
                      else 'other'
                    end as kind,
                    (select count(*) from pg_catalog.pg_attribute a
                      where a.attrelid = c.oid and a.attnum > 0 and not a.attisdropped)::int as columns,
                    greatest(c.reltuples, 0)::bigint as est_rows,
                    pg_catalog.pg_size_pretty(pg_catalog.pg_total_relation_size(c.oid)) as size
               from pg_catalog.pg_class c
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
              where c.relkind in ('r','p','v','m','f','S')
                and {SYSTEM_SCHEMA_FILTER}
                and n.nspname in ({schema_list})
              order by n.nspname, c.relname"
        ))
        .await?;

    let mut grouped: HashMap<String, Vec<RelationInfo>> = HashMap::new();
    for row in &relations.values {
        let schema = text(row, 1);
        grouped.entry(schema.clone()).or_default().push(RelationInfo {
            oid: int(row, 0) as u32,
            schema,
            name: text(row, 2),
            kind: text(row, 3),
            columns: int(row, 4) as i32,
            est_rows: int(row, 5),
            size: text(row, 6),
            last_autoanalyze: None,
        });
    }

    let routines = session
        .conn
        .query(&format!(
            "select n.nspname,
                    p.proname,
                    case p.prokind when 'p' then 'procedure' when 'a' then 'aggregate' else 'function' end,
                    pg_catalog.pg_get_function_arguments(p.oid),
                    pg_catalog.pg_get_function_result(p.oid)
               from pg_catalog.pg_proc p
               join pg_catalog.pg_namespace n on n.oid = p.pronamespace
              where {SYSTEM_SCHEMA_FILTER}
                and n.nspname in ({schema_list})
              order by n.nspname, p.proname"
        ))
        .await?;

    let mut routines_grouped: HashMap<String, Vec<RoutineInfo>> = HashMap::new();
    for row in &routines.values {
        let schema = text(row, 0);
        routines_grouped.entry(schema.clone()).or_default().push(RoutineInfo {
            schema,
            name: text(row, 1),
            kind: text(row, 2),
            arguments: text(row, 3),
            returns: text(row, 4),
        });
    }

    Ok(Catalog {
        schemas: schema_rows,
        relations: grouped,
        routines: routines_grouped,
        fetched_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_default(),
    })
}

pub async fn load_columns(
    session: &mut Session,
    schema: &str,
    relation: &str,
) -> Result<Vec<ColumnInfo>, AppError> {
    let rows = session
        .conn
        .query(&format!(
            "select a.attname,
                    pg_catalog.format_type(a.atttypid, a.atttypmod),
                    not a.attnotnull,
                    coalesce(pg_catalog.pg_get_expr(d.adbin, d.adrelid), ''),
                    a.attnum,
                    exists (
                      select 1 from pg_catalog.pg_index i
                       where i.indrelid = a.attrelid and i.indisprimary and a.attnum = any(i.indkey)
                    ) as is_pk
               from pg_catalog.pg_attribute a
               join pg_catalog.pg_class c on c.oid = a.attrelid
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
               left join pg_catalog.pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
              where n.nspname = {} and c.relname = {} and a.attnum > 0 and not a.attisdropped
              order by a.attnum",
            lit(schema),
            lit(relation)
        ))
        .await?;

    Ok(rows
        .values
        .iter()
        .map(|row| ColumnInfo {
            name: text(row, 0),
            data_type: text(row, 1),
            is_nullable: text(row, 2) == "t",
            default_value: Some(text(row, 3)).filter(|value| !value.is_empty()),
            position: int(row, 4) as i32,
            is_primary_key: text(row, 5) == "t",
        })
        .collect())
}

fn text(row: &[Option<String>], index: usize) -> String {
    row.get(index).cloned().flatten().unwrap_or_default()
}

fn int(row: &[Option<String>], index: usize) -> i64 {
    text(row, index).parse().unwrap_or_default()
}

/// `'public', 'sales'`——简单协议下的目录查询不接受参数，所以名字以字面量
/// 形式加引号。
fn sql_list(names: &[&str]) -> String {
    if names.is_empty() {
        return "''".into();
    }
    names.iter().map(|name| lit(name)).collect::<Vec<_>>().join(", ")
}

fn lit(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// 只在 PostgreSQL 否则会折叠大小写或拒收时才给标识符加引号，这样生成的 SQL
/// 读起来像手写的 SQL。
pub fn quote_ident(ident: &str) -> String {
    if ident
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !ident.starts_with(|c: char| c.is_ascii_digit())
    {
        ident.to_string()
    } else {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(quote_ident("users"), "users");
        assert_eq!(quote_ident("user_data2"), "user_data2");
        assert_eq!(quote_ident("User"), "\"User\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(quote_ident("2fast"), "\"2fast\"");
    }

    #[test]
    fn escapes_literals() {
        assert_eq!(lit("o'brien"), "'o''brien'");
        assert_eq!(sql_list(&["public", "a b"]), "'public', 'a b'");
    }
}
