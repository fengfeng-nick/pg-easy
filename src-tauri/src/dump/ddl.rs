//! 从 pg_catalog 读出结构，并按依赖顺序渲染成 DDL。

use std::collections::{HashMap, HashSet};

use crate::catalog::quote_ident;
use crate::error::AppError;
use crate::state::Session;

use super::order::ordered_data_relations;
use super::plan::PlanRelation;

use super::script::{is_key, push_steps};
use super::util::{
    key_of, key_name_sql, lit, oid_list, relation_name, schema_prefix, section, split_name, text,
};

// ---------------------------------------------------------------------------
// 结构

/// 结构段需要的全部内容，预先一次读好，脚本才能按顺序一遍写出。
pub(crate) struct SchemaDump {
    schemas: Vec<String>,
    sequences: Vec<SequenceInfo>,
    pub(crate) tables: Vec<TableDdl>,
    views: Vec<ViewDdl>,
    pub(crate) constraints: Vec<Constraint>,
    indexes: Vec<Simple>,
    triggers: Vec<Simple>,
    /// 数据段要读其行的表，父表在子表之前。
    pub(crate) data_relations: Vec<PlanRelation>,
    /// 每张表的继承父表，用于普通（非分区）继承。
    inheritants: HashMap<(String, String), String>,
}

pub(crate) struct TableDdl {
    pub(crate) relation: PlanRelation,
    pub(crate) columns: Vec<ColumnDdl>,
    pub(crate) partition_key: String,
    pub(crate) partition_bound: String,
    /// 非数据库默认表空间的表空间名。
    pub(crate) tablespace: Option<String>,
    pub(crate) unlogged: bool,
    pub(crate) parent: Option<Parent>,
}

pub(crate) enum Parent {
    /// 通过 `for values <bound>` 挂到父表上。
    Partition(String),
    /// 普通的 `inherits (parent)`。
    Inherit(String),
}

pub(crate) struct ColumnDdl {
    pub(crate) name: String,
    pub(crate) data_type: String,
    pub(crate) default: Option<String>,
    pub(crate) not_null: bool,
    /// `a`（generated always）、`d`（by default）或为空。
    pub(crate) identity: String,
    pub(crate) generated: bool,
    pub(crate) collation: Option<String>,
}

/// 一行 `pg_constraint`，渲染成 `alter table ... add constraint`。
pub(crate) struct Constraint {
    /// `schema.table`，只在 PostgreSQL 需要时加引号。
    pub(crate) relation: String,
    /// PostgreSQL 是否不得不给 schema 或表名加引号；只有这类表需要重新查找
    /// 其键的索引（见 `render`）。
    pub(crate) quoted_relation: bool,
    /// 承载表隐式序列的列（也就是它的 id），前提是键正好只覆盖这一列。
    pub(crate) id_column: Option<String>,
    pub(crate) name: String,
    /// p、u、c、x、f 或 t
    pub(crate) kind: String,
    pub(crate) definition: String,
    /// 外键所指的 `schema.table`。
    pub(crate) references: Option<String>,
    /// 分区自身的键所对应的父表 `schema.table`。这样的键不带行，但它的索引
    /// 挂在父表的索引上，从而保持索引名与约束名一致。
    pub(crate) parent: Option<String>,
}

pub(crate) struct Simple {
    pub(crate) sql: String,
}

pub(crate) struct ViewDdl {
    pub(crate) relation: PlanRelation,
    pub(crate) materialized: bool,
    pub(crate) definition: String,
}

pub(crate) struct SequenceInfo {
    pub(crate) schema: String,
    pub(crate) name: String,
    pub(crate) data_type: String,
    pub(crate) increment: i64,
    pub(crate) min_value: i64,
    pub(crate) max_value: i64,
    pub(crate) start: i64,
    pub(crate) cache: i64,
    pub(crate) cycle: bool,
}

impl SchemaDump {
    pub(crate) async fn load(
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
            if let Some(parent) = super::load::inheritance_parent(session, relation).await? {
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
                    tables.push(super::load::load_table(session, relation, &table_keys, &inheritants).await?)
                }
                "view" | "matview" => views.push(super::load::load_view(session, relation).await?),
                _ => {}
            }
        }

        Ok(Self {
            schemas,
            sequences: super::load::load_sequences(session).await?,
            tables,
            views,
            constraints: super::load::load_constraints(session, &oids).await?,
            indexes: super::load::load_indexes(session, &oids).await?,
            triggers: super::load::load_triggers(session, &oids).await?,
            data_relations,
            inheritants,
        })
    }

    /// 回放顺序的 DDL：schema、序列、表（父表总在分区之前）、键、检查约束、
    /// 索引、视图、触发器、待所有表都建好后的外键、继承关系，以及防止后续
    /// 插入与导入 id 冲突的标识列重置。
    pub(crate) fn render(&self) -> String {
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
    pub(crate) async fn reconcile_constraints(
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
    pub(crate) fn identity_resets(&self) -> String {
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
    pub(crate) fn column_list(&self) -> String {
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
    pub(crate) fn qualified(&self) -> String {
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

/// 把每个标识列的隐式序列推到已导入的 id 之后。
pub(crate) fn reset_identity(table: &TableDdl) -> String {
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
