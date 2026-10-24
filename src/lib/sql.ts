/** Quotes an SQL identifier: `we"ird` becomes `"we""ird"`. */
export function quoteIdent(name: string) {
  return `"${name.replace(/"/g, '""')}"`;
}

export function qualifiedName(schema: string, relation: string) {
  return `${quoteIdent(schema)}.${quoteIdent(relation)}`;
}

/** The first page of a table. The follow-up pages add the primary-key order
 *  (see `tablePageSql`), which does not change the first page's contents. */
export function firstPageSql(schema: string, relation: string, limit: number) {
  return `select * from ${qualifiedName(schema, relation)} limit ${limit}`;
}

/** A page of a table ordered by its primary key — the order the grid will use —
 *  so offsets stay stable while the user scrolls. */
export function tablePageSql(
  schema: string,
  relation: string,
  primaryKey: string[],
  limit: number,
  offset: number,
) {
  const order = primaryKey.length > 0 ? ` order by ${primaryKey.map(quoteIdent).join(", ")}` : "";
  return `select * from ${qualifiedName(schema, relation)}${order} limit ${limit} offset ${offset}`;
}
