//! 算出这次转储覆盖哪些关系。

use crate::error::AppError;
use crate::catalog::quote_ident;
use crate::state::Session;

use super::util::{key_of, kind_of, lit, split_name, text};

pub(crate) const SYSTEM_SCHEMA_FILTER: &str =
    "n.nspname not in ('pg_catalog', 'information_schema') and n.nspname not like 'pg_toast%'";

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
    pub(crate) fn qualified(&self) -> String {
        format!("{}.{}", quote_ident(&self.schema), quote_ident(&self.name))
    }

    pub(crate) fn key(&self) -> (String, String) {
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
