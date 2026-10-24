/** 给 SQL 标识符加引号：`we"ird` 变成 `"we""ird"`。 */
export function quoteIdent(name: string) {
  return `"${name.replace(/"/g, '""')}"`;
}

export function qualifiedName(schema: string, relation: string) {
  return `${quoteIdent(schema)}.${quoteIdent(relation)}`;
}

/** 表的第一页。后续页会加上主键排序（见 `tablePageSql`），这不改变
 *  第一页的内容。 */
export function firstPageSql(schema: string, relation: string, limit: number) {
  return `select * from ${qualifiedName(schema, relation)} limit ${limit}`;
}

/** 表的一页，按主键排序——也就是表格将采用的顺序——这样滚动时 offset
 *  保持稳定。 */
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
