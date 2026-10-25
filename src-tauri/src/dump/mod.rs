//! SQL 转储。
//!
//! 这里不调用 `pg_dump`，而是从目录里拼出脚本：表结构来自 `pg_catalog`
//! （列、默认值、约束、索引、触发器、分区、外键、序列），数据通过
//! `COPY ... TO STDOUT` 取出，线协议客户端本来就认识它。结果是可被 `psql -f`
//! 直接回放的纯 SQL，顺序保证每个对象的依赖都在它之前存在。
//!
//! 整个转储在一个只读的 `repeatable read` 事务里读取，所以即使有人在写库，
//! 结构和行描述的也是同一时刻；无论转储完成、失败还是被取消，事务都会结束。
//!
//! 输出是流式的：结构一次产出（它整体上必须有序），随后每张表输出一个
//! `copy ... from stdin` 块，大库因此不会在内存里存两份。
//!
//! 同一批材料产出两种形态：一个包含全部内容的 SQL 脚本，或一个分别装
//! `schema.sql` 与 `data.sql` 的 zip。

mod data;
mod ddl;
mod load;
mod order;
mod plan;
mod run;
mod script;
mod util;
mod zip;

pub use plan::{plan, PlanRelation};
pub use run::{run, DumpSink};

#[cfg(test)]
mod tests {
    use super::order::ordered_data_relations;
    use super::plan::PlanRelation;
    use super::util::{relation_name, split_name};
    use super::zip::{build_zip, crc32, dos_now};

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
        // 容器是手写的，所以它的结构值得固定下来：本地文件头、各文件，然后
        // 是中央目录结束标记。
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
