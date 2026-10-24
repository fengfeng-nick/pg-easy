//! Column types as the UI presents them.
//!
//! The simple protocol delivers every value as text, so the type only has to
//! say how to *show* a cell (align numbers, colour JSON, italicise NULLs) and
//! what to call the column. The mapping is restricted to built-in OIDs;
//! anything else is text, which is never wrong about the bytes on screen.

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

/// Uses `tokio-postgres`'s table of built-in types for the name, which is
/// complete for everything the server reports without a catalog lookup.
fn describe_oid(oid: u32) -> (String, ValueKind) {
    match tokio_postgres::types::Type::from_oid(oid) {
        Some(ty) => (ty.name().to_string(), ValueKind::of(&ty)),
        None => (crate::protocol::type_name_of_oid(oid), ValueKind::of_oid(oid)),
    }
}

/// Iterates the row's values in column order.
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
