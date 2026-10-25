//! 关系的写出顺序：父表在分区之前，被引用的表在外键之前。

use std::collections::HashMap;

use super::ddl::{Constraint, SchemaDump};
use super::plan::{PlanRelation, Reference};
use super::util::{key_of, relation_name, split_name};

// ---------------------------------------------------------------------------
// 小工具

/// 数据段要读其行的表，父表在分区之前：COPY 块依赖这个顺序，截断则依赖它的
/// 逆向。复制计划时带上每张表的外键，行的排序需要它，而计划本身不需要。
pub(crate) fn with_foreign_keys(plan: &[PlanRelation], constraints: &[Constraint]) -> Vec<PlanRelation> {
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
pub(crate) fn parent_subtree_contains(
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

pub(crate) fn ordered_data_relations(plan: &[PlanRelation], include_data: bool) -> Vec<PlanRelation> {
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
pub(crate) fn order_by_keys(tables: &[PlanRelation]) -> Vec<PlanRelation> {
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
