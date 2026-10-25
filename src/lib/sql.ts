/** 给一条语句套上 `EXPLAIN`。已经是 EXPLAIN 的不再套第二层。
 *  选中的文本优先；否则用第一条语句。只有注释或空白时返回空串。 */
export function explainSql(script: string) {
  const statement = firstStatement(script).trim().replace(/;+\s*$/, "");
  if (!statement || !hasCode(statement)) return "";
  if (/^explain\b/i.test(statement)) return statement;
  return `explain (analyze, buffers, format text)\n${statement}`;
}

/** 去掉注释和字符串之后还剩没有代码。用来拒绝「只有一行注释」的编辑器。 */
function hasCode(statement: string) {
  return statement
    .replace(/--[^\n]*/g, "")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/'(?:''|[^'])*'/g, "''")
    .replace(/\$[A-Za-z0-9_]*\$[\s\S]*?\$[A-Za-z0-9_]*\$/g, "")
    .trim().length > 0;
}

/** 脚本里的第一条语句，去掉首尾空白。分号藏在引号、美元引用或注释里时不算结束。 */
export function firstStatement(script: string) {
  let i = 0;
  const text = script;
  while (i < text.length && /\s/.test(text[i])) i += 1;
  const start = i;
  while (i < text.length) {
    const ch = text[i];
    if (ch === "-" && text[i + 1] === "-") {
      const next = text.indexOf("\n", i);
      i = next === -1 ? text.length : next + 1;
      continue;
    }
    if (ch === "/" && text[i + 1] === "*") {
      const next = text.indexOf("*/", i + 2);
      i = next === -1 ? text.length : next + 2;
      continue;
    }
    if (ch === "'" || ch === '"') {
      i += 1;
      while (i < text.length) {
        if (text[i] === ch) {
          if (text[i + 1] === ch) {
            i += 2;
            continue;
          }
          i += 1;
          break;
        }
        i += 1;
      }
      continue;
    }
    if (ch === "$") {
      const tag = text.slice(i).match(/^\$[A-Za-z0-9_]*\$/);
      if (tag) {
        const close = text.indexOf(tag[0], i + tag[0].length);
        i = close === -1 ? text.length : close + tag[0].length;
        continue;
      }
    }
    if (ch === ";") return text.slice(start, i);
    i += 1;
  }
  return text.slice(start);
}

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
 *  保持稳定。`where` 是已经拼好的条件，原样接在 FROM 后面。 */
export function tablePageSql(
  schema: string,
  relation: string,
  primaryKey: string[],
  limit: number,
  offset: number,
  where = "",
) {
  const order = primaryKey.length > 0 ? ` order by ${primaryKey.map(quoteIdent).join(", ")}` : "";
  const filter = where ? ` where ${where}` : "";
  return `select * from ${qualifiedName(schema, relation)}${filter}${order} limit ${limit} offset ${offset}`;
}

/** 把筛选框里的文字收成一条 WHERE。空串表示不过滤。
 *
 * 含空格或比较符时按用户写的 SQL 条件用（`status = 'open'`）；否则对每一列
 * 做不区分大小写的包含匹配。列名一律加引号，文字本身走字面量。 */
export function filterWhere(text: string, columns: string[]): string {
  const trimmed = text.trim();
  if (!trimmed) return "";
  if (/[=<>()]|\s/.test(trimmed)) return `(${trimmed})`;
  if (columns.length === 0) return "";
  const pattern = quoteLiteral(`%${trimmed}%`);
  return columns.map((column) => `${quoteIdent(column)}::text ilike ${pattern}`).join(" or ");
}

/** 标准 SQL 字面量：`'` 加倍，不使用 `E''`，这样反斜杠保持原样。 */
export function quoteLiteral(value: string) {
  return `'${value.replace(/'/g, "''")}'`;
}

export type RowChange = { column: string; value: string | null };

/** 用主键定位一行。没有主键时不能安全地改，调用方应先拦住。 */
export function rowPredicate(primaryKey: string[], keyValues: (string | null)[]) {
  return primaryKey
    .map((column, index) => {
      const value = keyValues[index];
      return value === null
        ? `${quoteIdent(column)} is null`
        : `${quoteIdent(column)} = ${quoteLiteral(value)}`;
    })
    .join(" and ");
}

export function updateRowSql(
  schema: string,
  relation: string,
  primaryKey: string[],
  keyValues: (string | null)[],
  changes: RowChange[],
) {
  const assignments = changes
    .map((change) => `${quoteIdent(change.column)} = ${change.value === null ? "null" : quoteLiteral(change.value)}`)
    .join(", ");
  return `update ${qualifiedName(schema, relation)} set ${assignments} where ${rowPredicate(primaryKey, keyValues)}`;
}

export function deleteRowSql(
  schema: string,
  relation: string,
  primaryKey: string[],
  keyValues: (string | null)[],
) {
  return `delete from ${qualifiedName(schema, relation)} where ${rowPredicate(primaryKey, keyValues)}`;
}

export function insertRowSql(schema: string, relation: string, changes: RowChange[]) {
  const columns = changes.map((change) => quoteIdent(change.column)).join(", ");
  const values = changes
    .map((change) => (change.value === null ? "null" : quoteLiteral(change.value)))
    .join(", ");
  return `insert into ${qualifiedName(schema, relation)} (${columns}) values (${values})`;
}

export type Tabular = {
  columns: { name: string }[];
  rows: (string | null)[][];
};

/** 结果导出成 CSV。NULL 变成空字段，含逗号、引号或换行的值加引号。 */
export function toCsv({ columns, rows }: Tabular) {
  const head = columns.map((column) => csvField(column.name)).join(",");
  const body = rows.map((row) => row.map((value) => (value === null ? "" : csvField(value))).join(","));
  return [head, ...body].join("\n");
}

function csvField(value: string) {
  if (/[",\n\r]/.test(value)) return `"${value.replace(/"/g, '""')}"`;
  return value;
}

/** 结果导出成 JSON 数组。NULL 保留为 null，其余值保持服务器返回的文本。 */
export function toJson({ columns, rows }: Tabular) {
  const objects = rows.map((row) => {
    const record: Record<string, string | null> = {};
    columns.forEach((column, index) => {
      record[column.name] = row[index] ?? null;
    });
    return record;
  });
  return JSON.stringify(objects, null, 2);
}

/** 把若干行收成可直接执行的 INSERT。NULL 写成 null。 */
export function toInserts(schema: string, relation: string, table: Tabular) {
  const columnList = table.columns.map((column) => quoteIdent(column.name)).join(", ");
  const target = qualifiedName(schema, relation);
  return table.rows
    .map((row) => {
      const values = row.map((value) => (value === null ? "null" : quoteLiteral(value))).join(", ");
      return `insert into ${target} (${columnList}) values (${values});`;
    })
    .join("\n");
}
