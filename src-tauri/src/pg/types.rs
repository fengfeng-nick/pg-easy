//! 界面呈现列类型的方式。
//!
//! 简单协议把每个值都作为文本送达，所以类型只需要说明如何*显示*一个单元格
//! （数字对齐、JSON 着色、NULL 用斜体）以及列该怎么称呼。映射只覆盖内置
//! OID；其他一切都是文本，而这在屏幕上显示字节这件事上永远不会错。

use fallible_iterator::FallibleIterator;
use postgres_protocol::message::backend::{DataRowBody, RowDescriptionBody};

use crate::protocol::ValueKind;

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub type_oid: u32,
    pub type_name: String,
    pub kind: ValueKind,
}

pub fn describe(body: &RowDescriptionBody) -> Result<Vec<Column>, std::io::Error> {
    let mut columns = Vec::new();
    let mut fields = body.fields();
    while let Some(field) = fields.next()? {
        let type_oid = field.type_oid();
        let (type_name, kind) = describe_oid(type_oid);
        columns.push(Column {
            name: field.name().to_string(),
            type_oid,
            type_name,
            kind,
        });
    }
    Ok(columns)
}

/// 名称取自 `tokio-postgres` 内置类型表，对服务器不经目录查询就报告的一切
/// 都足够完整。
fn describe_oid(oid: u32) -> (String, ValueKind) {
    match tokio_postgres::types::Type::from_oid(oid) {
        Some(ty) => (ty.name().to_string(), ValueKind::of(&ty)),
        None => (crate::protocol::type_name_of_oid(oid), ValueKind::of_oid(oid)),
    }
}

/// 按列顺序遍历该行的值。
pub fn read_row(body: &DataRowBody) -> Result<Vec<Option<String>>, std::io::Error> {
    let buffer = body.buffer();
    let mut out = Vec::new();
    let mut ranges = body.ranges();
    while let Some(range) = ranges.next()? {
        out.push(match range {
            None => None,
            Some(range) => Some(String::from_utf8_lossy(&buffer[range]).into_owned()),
        });
    }
    Ok(out)
}
