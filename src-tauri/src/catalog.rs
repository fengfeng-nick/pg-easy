//! 为侧边栏和 SQL 编辑器的补全列表读取 `pg_catalog`。
//! 每个库只做几次往返，几百个关系也能保持侧边栏响应迅速；所有内容都按当前
//! 用户的权限过滤，树里永远不会列出打不开的对象。

use std::collections::HashMap;

use crate::error::AppError;
use crate::protocol::{
    Catalog, ColumnInfo, DatabaseInfo, ForeignKeyConstraint, IndexInfo, RelationInfo,
    RelationStructure, RoutineInfo, SchemaInfo, TriggerInfo,
};
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

    // 用户自己的函数，再加上 pg_catalog 里当前用户真正能执行的内置函数。
    // 内置函数数量很大，但补全列表需要它们：`select gen_` 应该能落到
    // generate_series，而不是只剩用户自己建的那几个。
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
              where (({SYSTEM_SCHEMA_FILTER} and n.nspname in ({schema_list}))
                     or n.nspname = 'pg_catalog')
                and pg_catalog.has_function_privilege(p.oid, 'EXECUTE')
                and p.prokind in ('f', 'p', 'a', 'w')
              order by n.nspname = 'pg_catalog', n.nspname, p.proname"
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
                    ) as is_pk,
                    coalesce((
                      select string_agg(
                               concat_ws(
                                 '|',
                                 (select string_agg(sa.attname, ',' order by u.ord)
                                    from unnest(con.conkey) with ordinality as u(attnum, ord)
                                    join pg_catalog.pg_attribute sa
                                      on sa.attrelid = con.conrelid and sa.attnum = u.attnum),
                                 fn.nspname,
                                 fc.relname,
                                 (select string_agg(fa.attname, ',' order by u.ord)
                                    from unnest(con.confkey) with ordinality as u(attnum, ord)
                                    join pg_catalog.pg_attribute fa
                                      on fa.attrelid = con.confrelid and fa.attnum = u.attnum)
                               ),
                               E'\\n' order by con.oid
                             )
                        from pg_catalog.pg_constraint con
                        join pg_catalog.pg_class fc on fc.oid = con.confrelid
                        join pg_catalog.pg_namespace fn on fn.oid = fc.relnamespace
                       where con.conrelid = a.attrelid
                         and con.contype = 'f'
                         and a.attnum = any(con.conkey)
                    ), '') as foreign_keys
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
        .map(|row| column_from_row(row))
        .collect())
}

/// 这个对象的 `CREATE` 语句：表带列、默认值、约束、索引和触发器；视图和物化视图
/// 用 `pg_get_viewdef`。序列只给出它自己的 `CREATE SEQUENCE`。
pub async fn load_ddl(
    session: &mut Session,
    schema: &str,
    relation: &str,
) -> Result<String, AppError> {
    let rows = session
        .conn
        .query(&format!(
            "select c.relkind::text,
                    pg_catalog.pg_get_viewdef(c.oid, true)
               from pg_catalog.pg_class c
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
              where n.nspname = {} and c.relname = {}",
            lit(schema),
            lit(relation)
        ))
        .await?;
    let Some(row) = rows.values.first() else {
        return Err(AppError::new("notfound", "relation_not_found").with_args([format!("{schema}.{relation}")]));
    };
    let kind = text(row, 0);
    let qualified = format!("{}.{}", quote_ident(schema), quote_ident(relation));
    let sql = match kind.as_str() {
        "v" | "m" => {
            let verb = if kind == "m" { "materialized view" } else { "view" };
            format!("create {verb} {qualified} as\n{};\n", text(row, 1).trim_end())
        }
        "S" => format!("create sequence {qualified};\n"),
        _ => table_ddl(session, schema, relation, &qualified).await?,
    };
    Ok(sql)
}

async fn table_ddl(
    session: &mut Session,
    schema: &str,
    relation: &str,
    qualified: &str,
) -> Result<String, AppError> {
    let columns = session
        .conn
        .query(&format!(
            "select a.attname,
                    pg_catalog.format_type(a.atttypid, a.atttypmod),
                    a.attnotnull,
                    pg_catalog.pg_get_expr(d.adbin, d.adrelid),
                    a.attidentity::text,
                    a.attgenerated::text
               from pg_catalog.pg_attribute a
               join pg_catalog.pg_class c on c.oid = a.attrelid
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
               left join pg_catalog.pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
              where n.nspname = {} and c.relname = {}
                and a.attnum > 0 and not a.attisdropped
              order by a.attnum",
            lit(schema),
            lit(relation)
        ))
        .await?;

    let mut lines = Vec::new();
    for row in &columns.values {
        let mut line = format!("  {} {}", quote_ident(&text(row, 0)), text(row, 1));
        let identity = text(row, 4);
        let generated = text(row, 5);
        if identity == "a" || identity == "d" {
            let always = if identity == "a" { "always" } else { "by default" };
            line.push_str(&format!(" generated {always} as identity"));
        } else if generated == "s" {
            if let Some(expr) = row.get(3).cloned().flatten().filter(|value| !value.is_empty()) {
                line.push_str(&format!(" generated always as ({expr}) stored"));
            }
        } else if let Some(expr) = row.get(3).cloned().flatten().filter(|value| !value.is_empty()) {
            line.push_str(&format!(" default {expr}"));
        }
        if text(row, 2) == "t" {
            line.push_str(" not null");
        }
        lines.push(line);
    }

    let constraints = session
        .conn
        .query(&format!(
            "select '  ' || pg_catalog.pg_get_constraintdef(con.oid, true)
               from pg_catalog.pg_constraint con
               join pg_catalog.pg_class c on c.oid = con.conrelid
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
              where n.nspname = {} and c.relname = {}
                and con.contype in ('p', 'u', 'c', 'f')
              order by con.contype, con.conname",
            lit(schema),
            lit(relation)
        ))
        .await?;
    for row in &constraints.values {
        lines.push(text(row, 0));
    }

    let mut sql = format!("create table {qualified} (\n{}\n);\n", lines.join(",\n"));

    let indexes = session
        .conn
        .query(&format!(
            "select pg_catalog.pg_get_indexdef(x.indexrelid)
               from pg_catalog.pg_index x
               join pg_catalog.pg_class c on c.oid = x.indrelid
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
              where n.nspname = {} and c.relname = {}
                and not x.indisprimary and not x.indisunique
              order by x.indexrelid",
            lit(schema),
            lit(relation)
        ))
        .await?;
    for row in &indexes.values {
        let definition = text(row, 0);
        if !definition.is_empty() {
            sql.push_str(&format!("{definition};\n"));
        }
    }

    let triggers = session
        .conn
        .query(&format!(
            "select pg_catalog.pg_get_triggerdef(t.oid, true)
               from pg_catalog.pg_trigger t
               join pg_catalog.pg_class c on c.oid = t.tgrelid
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
              where not t.tgisinternal
                and n.nspname = {} and c.relname = {}
              order by t.tgname",
            lit(schema),
            lit(relation)
        ))
        .await?;
    for row in &triggers.values {
        let definition = text(row, 0);
        if !definition.is_empty() {
            sql.push_str(&format!("{definition};\n"));
        }
    }
    Ok(sql)
}

/// 列、索引、外键和触发器。四条查询共用同一个连接，避免结构页自己再拼 SQL。
pub async fn load_structure(
    session: &mut Session,
    schema: &str,
    relation: &str,
) -> Result<RelationStructure, AppError> {
    let columns = load_columns(session, schema, relation).await?;
    let indexes = load_indexes(session, schema, relation).await?;
    let foreign_keys = load_foreign_keys(session, schema, relation).await?;
    let triggers = load_triggers(session, schema, relation).await?;
    Ok(RelationStructure {
        columns,
        indexes,
        foreign_keys,
        triggers,
    })
}

async fn load_indexes(
    session: &mut Session,
    schema: &str,
    relation: &str,
) -> Result<Vec<IndexInfo>, AppError> {
    let rows = session
        .conn
        .query(&format!(
            "select i.relname,
                    coalesce((
                      select string_agg(a.attname, ',' order by u.ord)
                        from unnest(x.indkey) with ordinality as u(attnum, ord)
                        join pg_catalog.pg_attribute a
                          on a.attrelid = x.indrelid and a.attnum = u.attnum
                       where u.attnum > 0
                    ), ''),
                    x.indisunique,
                    x.indisprimary,
                    am.amname,
                    pg_catalog.pg_get_indexdef(x.indexrelid)
               from pg_catalog.pg_index x
               join pg_catalog.pg_class c on c.oid = x.indrelid
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
               join pg_catalog.pg_class i on i.oid = x.indexrelid
               join pg_catalog.pg_am am on am.oid = i.relam
              where n.nspname = {} and c.relname = {}
              order by x.indisprimary desc, x.indisunique desc, i.relname",
            lit(schema),
            lit(relation)
        ))
        .await?;

    Ok(rows
        .values
        .iter()
        .map(|row| IndexInfo {
            name: text(row, 0),
            columns: split_names(&text(row, 1)),
            is_unique: text(row, 2) == "t",
            is_primary: text(row, 3) == "t",
            method: text(row, 4),
            definition: text(row, 5),
        })
        .collect())
}

async fn load_foreign_keys(
    session: &mut Session,
    schema: &str,
    relation: &str,
) -> Result<Vec<ForeignKeyConstraint>, AppError> {
    let rows = session
        .conn
        .query(&format!(
            "select con.conname,
                    coalesce((
                      select string_agg(a.attname, ',' order by u.ord)
                        from unnest(con.conkey) with ordinality as u(attnum, ord)
                        join pg_catalog.pg_attribute a
                          on a.attrelid = con.conrelid and a.attnum = u.attnum
                    ), ''),
                    fn.nspname,
                    fc.relname,
                    coalesce((
                      select string_agg(a.attname, ',' order by u.ord)
                        from unnest(con.confkey) with ordinality as u(attnum, ord)
                        join pg_catalog.pg_attribute a
                          on a.attrelid = con.confrelid and a.attnum = u.attnum
                    ), ''),
                    case con.confupdtype
                      when 'a' then 'NO ACTION' when 'r' then 'RESTRICT'
                      when 'c' then 'CASCADE' when 'n' then 'SET NULL'
                      when 'd' then 'SET DEFAULT' else '' end,
                    case con.confdeltype
                      when 'a' then 'NO ACTION' when 'r' then 'RESTRICT'
                      when 'c' then 'CASCADE' when 'n' then 'SET NULL'
                      when 'd' then 'SET DEFAULT' else '' end
               from pg_catalog.pg_constraint con
               join pg_catalog.pg_class c on c.oid = con.conrelid
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
               join pg_catalog.pg_class fc on fc.oid = con.confrelid
               join pg_catalog.pg_namespace fn on fn.oid = fc.relnamespace
              where con.contype = 'f'
                and n.nspname = {} and c.relname = {}
              order by con.conname",
            lit(schema),
            lit(relation)
        ))
        .await?;

    Ok(rows
        .values
        .iter()
        .map(|row| ForeignKeyConstraint {
            name: text(row, 0),
            columns: split_names(&text(row, 1)),
            foreign_schema: text(row, 2),
            foreign_table: text(row, 3),
            foreign_columns: split_names(&text(row, 4)),
            on_update: text(row, 5),
            on_delete: text(row, 6),
        })
        .collect())
}

async fn load_triggers(
    session: &mut Session,
    schema: &str,
    relation: &str,
) -> Result<Vec<TriggerInfo>, AppError> {
    let rows = session
        .conn
        .query(&format!(
            "select t.tgname,
                    case
                      when (t.tgtype & 2) <> 0 then 'BEFORE'
                      when (t.tgtype & 64) <> 0 then 'INSTEAD OF'
                      else 'AFTER'
                    end,
                    concat_ws(', ',
                      case when (t.tgtype & 4) <> 0 then 'INSERT' end,
                      case when (t.tgtype & 8) <> 0 then 'DELETE' end,
                      case when (t.tgtype & 16) <> 0 then 'UPDATE' end,
                      case when (t.tgtype & 32) <> 0 then 'TRUNCATE' end
                    ),
                    pg_catalog.pg_get_triggerdef(t.oid, true),
                    t.tgenabled <> 'D'
               from pg_catalog.pg_trigger t
               join pg_catalog.pg_class c on c.oid = t.tgrelid
               join pg_catalog.pg_namespace n on n.oid = c.relnamespace
              where not t.tgisinternal
                and n.nspname = {} and c.relname = {}
              order by t.tgname",
            lit(schema),
            lit(relation)
        ))
        .await?;

    Ok(rows
        .values
        .iter()
        .map(|row| TriggerInfo {
            name: text(row, 0),
            timing: text(row, 1),
            events: text(row, 2),
            definition: text(row, 3),
            enabled: text(row, 4) == "t",
        })
        .collect())
}

fn column_from_row(row: &[Option<String>]) -> ColumnInfo {
    ColumnInfo {
        name: text(row, 0),
        data_type: text(row, 1),
        is_nullable: text(row, 2) == "t",
        default_value: Some(text(row, 3)).filter(|value| !value.is_empty()),
        position: int(row, 4) as i32,
        is_primary_key: text(row, 5) == "t",
        foreign_keys: parse_foreign_keys(&text(row, 6)),
    }
}

/// `列,列|schema|表|列,列`，多条约束用换行隔开。列顺序与约束定义一致。
fn parse_foreign_keys(raw: &str) -> Vec<crate::protocol::ForeignKeyInfo> {
    if raw.is_empty() {
        return Vec::new();
    }
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '|');
            let columns = parts.next()?;
            let foreign_schema = parts.next()?.to_string();
            let foreign_table = parts.next()?.to_string();
            let foreign_columns = parts.next()?;
            if foreign_schema.is_empty() || foreign_table.is_empty() {
                return None;
            }
            Some(crate::protocol::ForeignKeyInfo {
                columns: split_names(columns),
                foreign_schema,
                foreign_table,
                foreign_columns: split_names(foreign_columns),
            })
        })
        .collect()
}

fn split_names(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
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

    #[test]
    fn parses_foreign_keys() {
        let keys = parse_foreign_keys("user_id|public|users|id\ntenant_id,user_id|sales|accounts|tenant_id,id");
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].columns, vec!["user_id"]);
        assert_eq!(keys[0].foreign_schema, "public");
        assert_eq!(keys[0].foreign_table, "users");
        assert_eq!(keys[0].foreign_columns, vec!["id"]);
        assert_eq!(keys[1].columns, vec!["tenant_id", "user_id"]);
        assert_eq!(keys[1].foreign_columns, vec!["tenant_id", "id"]);
        assert!(parse_foreign_keys("").is_empty());
    }

    #[test]
    fn splits_index_columns() {
        assert!(split_names("").is_empty());
        assert_eq!(split_names("id,email"), vec!["id", "email"]);
    }
}
