//! 从 pg_catalog 读出表、视图、序列、约束、索引和触发器。

use std::collections::{HashMap, HashSet};

use crate::error::AppError;
use crate::state::Session;

use super::ddl::{
    ColumnDdl, Constraint, Parent, SequenceInfo, Simple, TableDdl, ViewDdl,
};
use super::plan::PlanRelation;
use super::util::{int, key_of, lit, needs_quotes, oid_list, relation_name, text};
use super::plan::SYSTEM_SCHEMA_FILTER;

pub(crate) async fn load_table(
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

pub(crate) async fn load_view(session: &mut Session, relation: &PlanRelation) -> Result<ViewDdl, AppError> {
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
pub(crate) async fn load_sequences(session: &mut Session) -> Result<Vec<SequenceInfo>, AppError> {
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
pub(crate) async fn load_constraints(session: &mut Session, oids: &[&str]) -> Result<Vec<Constraint>, AppError> {
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

pub(crate) async fn load_indexes(session: &mut Session, oids: &[&str]) -> Result<Vec<Simple>, AppError> {
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

pub(crate) async fn load_triggers(session: &mut Session, oids: &[&str]) -> Result<Vec<Simple>, AppError> {
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
pub(crate) async fn inheritance_parent(
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

