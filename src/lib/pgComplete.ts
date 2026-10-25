import { syntaxTree } from "@codemirror/language";
import type { Completion, CompletionContext, CompletionResult } from "@codemirror/autocomplete";
import type { SyntaxNode } from "@lezer/common";
import { api, type Catalog, type ColumnInfo, type ForeignKeyInfo, type RelationInfo } from "@/lib/ipc";

/**
 * PostgreSQL 补全。
 *
 * 建议按语法位置给，而不是把关键词、表、列、函数堆成一份。位置由
 * `@codemirror/lang-sql` 的 Lezer 语法树决定（它就是 PostgreSQL 方言），
 * 对象来自当前连接的目录。这是 Navicat / pgcli 那一类客户端的做法：
 * `FROM` 后面是表，`WHERE` 后面是本语句已经引入的列，`JOIN` 的 `ON`
 * 优先给出外键条件。
 *
 * 列按关系懒加载。上千张表的库如果在连接时把列全部拉下来，会比打开
 * 会话本身还慢。
 */

type Clause =
  | "select"
  | "from"
  | "join"
  | "on"
  | "where"
  | "group"
  | "order"
  | "having"
  | "insert-table"
  | "insert-columns"
  | "insert-values"
  | "update-table"
  | "set"
  | "delete"
  | "call"
  | "expression";

type RelationRef = {
  schema?: string;
  name: string;
  alias?: string;
};

type ColumnRecord = {
  name: string;
  dataType: string;
  isPrimaryKey: boolean;
  isNullable: boolean;
  foreignKeys: ForeignKeyInfo[];
};

const CLAUSE_WORDS = new Set([
  "select", "from", "where", "join", "inner", "left", "right", "full", "cross",
  "natural", "on", "using", "group", "order", "having", "limit", "offset",
  "union", "intersect", "except", "window", "returning", "set", "values",
  "into", "update", "insert", "delete", "with", "as",
]);

const JOIN_KEYWORDS = [
  "INNER JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN", "CROSS JOIN", "ON", "USING",
];

const SELECT_KEYWORDS = [
  "FROM", "WHERE", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "OFFSET",
  "UNION", "UNION ALL", "INTERSECT", "EXCEPT", "WINDOW",
];

const DML_KEYWORDS = ["WHERE", "RETURNING", "FROM", "SET", "VALUES"];

/** 未连上库时仍然有用的类型和常用函数。连上之后由目录里的真实函数接替。 */
const PG_TYPES = [
  "bigint", "bigserial", "boolean", "bytea", "char", "date", "decimal", "double precision",
  "float", "int", "integer", "interval", "json", "jsonb", "money", "numeric", "real",
  "serial", "smallint", "text", "time", "timestamp", "timestamptz", "uuid", "varchar", "xml",
];

const PG_FUNCTIONS: { name: string; args: string; returns: string }[] = [
  { name: "count", args: "*", returns: "bigint" },
  { name: "sum", args: "numeric", returns: "numeric" },
  { name: "avg", args: "numeric", returns: "numeric" },
  { name: "min", args: "anyelement", returns: "anyelement" },
  { name: "max", args: "anyelement", returns: "anyelement" },
  { name: "coalesce", args: "anyelement, anyelement", returns: "anyelement" },
  { name: "nullif", args: "anyelement, anyelement", returns: "anyelement" },
  { name: "greatest", args: "anyelement, anyelement", returns: "anyelement" },
  { name: "least", args: "anyelement, anyelement", returns: "anyelement" },
  { name: "now", args: "", returns: "timestamptz" },
  { name: "current_date", args: "", returns: "date" },
  { name: "current_timestamp", args: "", returns: "timestamptz" },
  { name: "date_trunc", args: "text, timestamp", returns: "timestamp" },
  { name: "date_part", args: "text, timestamp", returns: "double precision" },
  { name: "age", args: "timestamp, timestamp", returns: "interval" },
  { name: "lower", args: "text", returns: "text" },
  { name: "upper", args: "text", returns: "text" },
  { name: "trim", args: "text", returns: "text" },
  { name: "btrim", args: "text", returns: "text" },
  { name: "length", args: "text", returns: "integer" },
  { name: "substring", args: "text, integer, integer", returns: "text" },
  { name: "replace", args: "text, text, text", returns: "text" },
  { name: "concat", args: "text, text", returns: "text" },
  { name: "concat_ws", args: "text, text, text", returns: "text" },
  { name: "left", args: "text, integer", returns: "text" },
  { name: "right", args: "text, integer", returns: "text" },
  { name: "split_part", args: "text, text, integer", returns: "text" },
  { name: "regexp_replace", args: "text, text, text", returns: "text" },
  { name: "round", args: "numeric, integer", returns: "numeric" },
  { name: "abs", args: "numeric", returns: "numeric" },
  { name: "ceil", args: "numeric", returns: "numeric" },
  { name: "floor", args: "numeric", returns: "numeric" },
  { name: "generate_series", args: "integer, integer", returns: "setof integer" },
  { name: "jsonb_build_object", args: "text, anyelement", returns: "jsonb" },
  { name: "jsonb_agg", args: "anyelement", returns: "jsonb" },
  { name: "to_jsonb", args: "anyelement", returns: "jsonb" },
  { name: "row_number", args: "", returns: "bigint" },
  { name: "rank", args: "", returns: "bigint" },
  { name: "dense_rank", args: "", returns: "bigint" },
  { name: "lag", args: "anyelement", returns: "anyelement" },
  { name: "lead", args: "anyelement", returns: "anyelement" },
  { name: "string_agg", args: "text, text", returns: "text" },
  { name: "array_agg", args: "anyelement", returns: "anyarray" },
  { name: "unnest", args: "anyarray", returns: "setof anyelement" },
];

export class PgCompletion {
  private columns = new Map<string, ColumnRecord[]>();
  private inflight = new Set<string>();
  private listeners = new Set<() => void>();

  constructor(
    readonly catalog: Catalog | null,
    private readonly connectionId: string | null,
  ) {}

  /** 列到达后通知编辑器，让还开着的补全用新数据再问一次。 */
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  completionSource = (context: CompletionContext): CompletionResult | null | Promise<CompletionResult | null> => {
    const pos = context.pos;
    const tree = syntaxTree(context.state);
    const node = tree.resolveInner(pos, -1);
    if (insideLiteral(node)) return null;

    const token = readToken(context);
    const statement = statementOf(node);
    const before = context.state.sliceDoc(0, pos);
    const clause = clauseAt(context.state.sliceDoc(statement.from, pos));
    // 整条语句的关系都算在范围内，这样 `select | from users` 也能补 users 的列。
    const scope = relationsIn(context.state.sliceDoc(statement.from, statement.to));
    this.warmScope(scope);
    // 没有正在输入的词时保持安静，除非光标刚落在一个该给建议的位置
    // （FROM 后面、点号后面、INSERT 的列清单里）。显式触发不受这个限制。
    if (!token && !context.explicit && !endsWithDot(context) && !awaitingSuggestion(before)) return null;
    if (clause === "insert-columns") {
      const target = insertTarget(before);
      if (target?.schema) this.warm(target.schema, target.name);
      else if (target) {
        const resolved = this.findRelation(undefined, target.name);
        if (resolved?.schema) this.warm(resolved.schema, resolved.name);
      }
    }

    const dotted = qualifierAt(context);
    if (dotted) return this.qualified(dotted, scope);

    const options = this.optionsFor(clause, scope, before, token, context.explicit);
    if (options.length === 0) return null;
    return {
      from: token ? token.from : pos,
      options,
      validFor: /^[\w$]*$/,
    };
  };

  private optionsFor(
    clause: Clause,
    scope: RelationRef[],
    before: string,
    token: { text: string } | null,
    explicit: boolean,
  ): Completion[] {
    const options: Completion[] = [];
    const seen = new Set<string>();
    const push = (option: Completion) => {
      const key = `${option.type}:${option.label}`;
      if (seen.has(key)) return;
      seen.add(key);
      options.push(option);
    };

    const wantColumns = clause === "select" || clause === "where" || clause === "group"
      || clause === "order" || clause === "having" || clause === "on" || clause === "set"
      || clause === "expression" || clause === "insert-values";
    const wantTables = clause === "from" || clause === "join" || clause === "insert-table"
      || clause === "update-table" || clause === "delete";
    const wantFunctions = clause === "select" || clause === "where" || clause === "having"
      || clause === "on" || clause === "group" || clause === "order" || clause === "set"
      || clause === "expression" || clause === "call" || clause === "insert-values";

    if (clause === "on") {
      for (const join of joinConditions(scope, (ref) => this.columnsOf(ref))) {
        push(join);
      }
    }

    if (wantColumns) {
      for (const column of columnsInScope(scope, (ref) => this.columnsOf(ref))) push(column);
    }
    if (wantTables || clause === "expression") {
      for (const table of this.tableOptions(clause === "expression")) push(table);
    }
    if (clause === "from" || clause === "join" || clause === "expression" || clause === "delete") {
      for (const schema of this.schemaOptions()) push(schema);
    }
    // 内置函数有几千个。没打出两个字符就全列出来，列表里就看不见表和列了。
    const prefix = token?.text ?? "";
    if (wantFunctions && (explicit || prefix.length >= 2)) {
      for (const fn of this.functionOptions(clause === "call")) push(fn);
    }
    if (clause === "insert-columns") {
      const table = insertTarget(before);
      if (table) {
        for (const column of this.columnsOf(table)) push(columnOption(column, undefined, 2));
      }
    }

    for (const keyword of keywordsFor(clause)) push(keywordOption(keyword));
    if (clause === "expression" || clause === "select") {
      for (const typeName of PG_TYPES) push({ label: typeName, type: "type", boost: -8 });
    }

    // 正在输入时，没匹配上的项不进列表，避免 FROM 后面冒出一排无关关键词。
    if (token && token.text.length > 0) {
      const prefix = token.text.toLowerCase();
      return options.filter((option) => option.label.toLowerCase().includes(prefix));
    }
    return options;
  }

  private qualified(
    dotted: { qualifier: string[]; from: number; partial: string },
    scope: RelationRef[],
  ): CompletionResult | null {
    const [first, second] = dotted.qualifier;
    const options: Completion[] = [];

    if (dotted.qualifier.length === 1) {
      const schema = this.schemaByName(first);
      if (schema) {
        for (const relation of this.relationsOf(schema)) {
          options.push(relationOption(relation));
        }
        for (const routine of this.catalog?.routines[schema] ?? []) {
          options.push(routineOption(routine.name, routine.arguments, routine.returns, routine.schema));
        }
      }
      const target = resolveRelation(first, scope, (name) => this.findRelation(undefined, name));
      if (target) {
        const resolved = target.schema ? target : this.findRelation(undefined, target.name) ?? target;
        if (resolved.schema) this.warm(resolved.schema, resolved.name);
        for (const column of this.columnsOf(resolved)) options.push(columnOption(column));
      }
    } else if (second) {
      const relation = this.findRelation(first, second);
      if (relation?.schema) {
        this.warm(relation.schema, relation.name);
        for (const column of this.columnsOf(relation)) options.push(columnOption(column));
      }
    }

    if (options.length === 0) return null;
    const partial = dotted.partial.toLowerCase();
    return {
      from: dotted.from,
      options: partial ? options.filter((option) => option.label.toLowerCase().includes(partial)) : options,
      validFor: /^[\w$]*$/,
    };
  }

  private tableOptions(demoted: boolean): Completion[] {
    if (!this.catalog) return [];
    const options: Completion[] = [];
    for (const [schema, relations] of Object.entries(this.catalog.relations)) {
      for (const relation of relations) {
        if (relation.kind === "sequence") continue;
        options.push(relationOption(relation, schema === this.defaultSchema() && !demoted ? 1 : demoted ? -4 : 0));
      }
    }
    return options;
  }

  private schemaOptions(): Completion[] {
    return (this.catalog?.schemas ?? []).map((schema) => ({
      label: schema.name,
      type: "namespace",
      detail: "schema",
      boost: schema.name === "public" ? 0 : -1,
    }));
  }

  private cachedFunctions: Completion[] | null = null;

  private functionOptions(proceduresOnly: boolean): Completion[] {
    if (proceduresOnly) {
      return (this.catalog ? Object.values(this.catalog.routines).flat() : [])
        .filter((routine) => routine.kind !== "function")
        .map((routine) => routineOption(routine.name, routine.arguments, routine.returns, routine.schema));
    }
    if (this.cachedFunctions) return this.cachedFunctions;
    const options: Completion[] = [];
    const seen = new Set<string>();
    const push = (name: string, args: string, returns: string, schema?: string) => {
      const key = `${schema ?? ""}:${name}:${args}`;
      if (seen.has(key)) return;
      seen.add(key);
      options.push(routineOption(name, args, returns, schema));
    };
    if (this.catalog) {
      for (const routines of Object.values(this.catalog.routines)) {
        for (const routine of routines) push(routine.name, routine.arguments, routine.returns, routine.schema);
      }
    }
    if (options.length === 0) {
      for (const fn of PG_FUNCTIONS) push(fn.name, fn.args, fn.returns);
    }
    this.cachedFunctions = options;
    return options;
  }

  private relationsOf(schema: string): RelationInfo[] {
    return (this.catalog?.relations[schema] ?? []).filter((relation) => relation.kind !== "sequence");
  }

  private schemaByName(name: string) {
    return this.catalog?.schemas.find((schema) => schema.name.toLowerCase() === name.toLowerCase())?.name;
  }

  private defaultSchema() {
    if (this.catalog?.schemas.some((schema) => schema.name === "public")) return "public";
    return this.catalog?.schemas[0]?.name;
  }

  private findRelation(schema: string | undefined, name: string): RelationRef | null {
    if (!this.catalog) return null;
    const schemas = schema ? [schema] : this.catalog.schemas.map((item) => item.name);
    const folded = name.toLowerCase();
    for (const schemaName of schemas) {
      const found = this.catalog.relations[schemaName]?.find(
        (relation) => relation.name === name || relation.name.toLowerCase() === folded,
      );
      if (found && found.kind !== "sequence") return { schema: schemaName, name: found.name };
    }
    return null;
  }

  private columnsOf(ref: RelationRef): ColumnRecord[] {
    const resolved = ref.schema
      ? this.findRelation(ref.schema, ref.name) ?? ref
      : this.findRelation(undefined, ref.name) ?? ref;
    if (!resolved.schema) return [];
    return this.columns.get(keyOf(resolved.schema, resolved.name)) ?? [];
  }

  private warmScope(scope: RelationRef[]) {
    for (const ref of scope) {
      const resolved = this.findRelation(ref.schema, ref.name);
      if (resolved?.schema) this.warm(resolved.schema, resolved.name);
    }
  }

  private warm(schema: string, relation: string) {
    if (!this.connectionId) return;
    const key = keyOf(schema, relation);
    if (this.columns.has(key) || this.inflight.has(key)) return;
    this.inflight.add(key);
    api
      .loadColumns(this.connectionId, schema, relation)
      .then((list) => {
        this.columns.set(key, list.map(toRecord));
        this.listeners.forEach((listener) => listener());
      })
      .catch(() => {
        // 这个关系退回到表名补全，查询本身不受影响。
      })
      .finally(() => this.inflight.delete(key));
  }
}

function toRecord(column: ColumnInfo): ColumnRecord {
  return {
    name: column.name,
    dataType: column.dataType,
    isPrimaryKey: column.isPrimaryKey,
    isNullable: column.isNullable,
    foreignKeys: column.foreignKeys ?? [],
  };
}

function keyOf(schema: string, relation: string) {
  return `${schema}.${relation}`;
}

function columnOption(column: ColumnRecord, qualify?: string, boost = 0): Completion {
  const label = qualify ? `${qualify}.${column.name}` : column.name;
  const marks = [
    column.dataType,
    column.isPrimaryKey ? "pk" : "",
    column.foreignKeys.length > 0 ? "fk" : "",
    column.isNullable ? "" : "not null",
  ].filter(Boolean);
  return {
    label,
    apply: quotedForm(label),
    type: "property",
    detail: marks.join(" · "),
    boost: boost + (column.isPrimaryKey ? 1 : 0),
  };
}

function relationOption(relation: RelationInfo, boost = 0): Completion {
  const kind = relation.kind === "matview" ? "materialized view" : relation.kind;
  return {
    label: relation.name,
    apply: quotedForm(relation.name),
    type: relation.kind === "view" || relation.kind === "matview" ? "interface" : "type",
    detail: relation.schema === "public" ? kind : `${relation.schema} · ${kind}`,
    boost,
  };
}

/** PostgreSQL 会把未加引号的标识符折叠成小写，所以只有安全的名字原样插入。 */
function quotedForm(label: string): string | undefined {
  const rendered = label
    .split(".")
    .map((part) => (/^[a-z_][a-z0-9_$]*$/.test(part) ? part : `"${part.replace(/"/g, '""')}"`))
    .join(".");
  return rendered === label ? undefined : rendered;
}

function routineOption(name: string, args: string, returns: string, schema?: string): Completion {
  const signature = args ? `${name}(${args})` : `${name}()`;
  return {
    label: name,
    type: "function",
    detail: schema && schema !== "pg_catalog" && schema !== "public" ? `${schema} · ${returns}` : returns || undefined,
    apply: (view, _completion, from, to) => {
      const insert = `${name}()`;
      view.dispatch({
        changes: { from, to, insert },
        selection: { anchor: from + name.length + 1 },
      });
    },
    boost: schema === "pg_catalog" ? -2 : 0,
    info: signature,
  };
}

function keywordOption(label: string): Completion {
  return { label, type: "keyword", boost: -6 };
}

function keywordsFor(clause: Clause): string[] {
  switch (clause) {
    case "select":
      return ["DISTINCT", "AS", "*", ...SELECT_KEYWORDS];
    case "from":
    case "join":
      return [...JOIN_KEYWORDS, "WHERE", "AS"];
    case "on":
      return ["AND", "OR", "USING", "WHERE"];
    case "where":
    case "having":
      return ["AND", "OR", "NOT", "IN", "EXISTS", "BETWEEN", "LIKE", "ILIKE", "IS", "NULL", "GROUP BY", "ORDER BY", "LIMIT"];
    case "group":
      return ["ORDER BY", "HAVING", "LIMIT"];
    case "order":
      return ["ASC", "DESC", "NULLS FIRST", "NULLS LAST", "LIMIT", "OFFSET"];
    case "set":
      return ["WHERE", "FROM", "RETURNING"];
    case "insert-table":
    case "update-table":
    case "delete":
      return ["WHERE", "RETURNING", "VALUES"];
    case "insert-columns":
      return ["VALUES", "SELECT"];
    default:
      return ["SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "EXPLAIN", ...DML_KEYWORDS];
  }
}

function insideLiteral(node: SyntaxNode) {
  for (let cursor: SyntaxNode | null = node; cursor; cursor = cursor.parent) {
    if (
      cursor.name === "String" ||
      cursor.name === "QuotedIdentifier" ||
      cursor.name === "LineComment" ||
      cursor.name === "BlockComment"
    ) {
      return cursor.name !== "QuotedIdentifier";
    }
  }
  return false;
}

function statementOf(node: SyntaxNode): SyntaxNode {
  let cursor: SyntaxNode | null = node;
  let statement = node;
  while (cursor) {
    if (cursor.name === "Statement") statement = cursor;
    cursor = cursor.parent;
  }
  return statement;
}

function readToken(context: CompletionContext) {
  const word = context.matchBefore(/[\w$]+/);
  if (!word || word.from === word.to) return null;
  return { from: word.from, text: word.text };
}

function endsWithDot(context: CompletionContext) {
  return context.state.sliceDoc(Math.max(0, context.pos - 1), context.pos) === ".";
}

/** 光标停在子句关键词、逗号或左括号后面，即使还没开始打下一个词也该给建议。 */
export function awaitingSuggestion(sql: string): boolean {
  if (/[.(,]\s*$/.test(sql)) return true;
  const trimmed = sql.trimEnd();
  if (sql === trimmed) return false;
  return /\b(select|distinct|from|join|where|on|using|set|into|update|delete|having|by|and|or|values)\s*$/i.test(trimmed);
}

/** `schema.table.` 或 `alias.` 光标前的限定名。 */
function qualifierAt(context: CompletionContext): { qualifier: string[]; from: number; partial: string } | null {
  const before = context.state.sliceDoc(Math.max(0, context.pos - 200), context.pos);
  const match = before.match(/(?:^|[^.\w$"])((?:"[^"]+"|[\w$]+)(?:\s*\.\s*(?:"[^"]+"|[\w$]+))*)\s*\.\s*([\w$]*)$/);
  if (!match) return null;
  const qualifier = match[1].split(".").map((part) => unquote(part.trim()));
  return {
    qualifier,
    from: context.pos - match[2].length,
    partial: match[2],
  };
}

function unquote(value: string) {
  return value.replace(/^"|"$/g, "").replace(/""/g, '"');
}

type SqlToken = { value: string; kind: "ident" | "punct" | "other" };

/** 丢掉字符串、注释和美元引号，只留下补全关心的词。 */
function tokenize(sql: string): SqlToken[] {
  const tokens: SqlToken[] = [];
  const pattern =
    /--[^\n]*|\/\*[\s\S]*?\*\/|'(?:''|[^'])*'|"(?:""|[^"])*"|\$[\w$]*\$[\s\S]*?\$[\w$]*\$|\b[\w$]+\b|[().]/g;
  for (const match of sql.matchAll(pattern)) {
    const value = match[0];
    if (value.startsWith("--") || value.startsWith("/*") || value.startsWith("'") || value.startsWith("$")) {
      continue;
    }
    if (value.startsWith('"')) tokens.push({ value: unquote(value), kind: "ident" });
    else if (/^[\w$]+$/.test(value)) tokens.push({ value, kind: "ident" });
    else tokens.push({ value, kind: "punct" });
  }
  return tokens;
}

/**
 * 光标所在子句。半截 SQL 的语法树经常是平的，所以按 token 看当前括号层里
 * 最后一个子句词。子查询有自己的一层，不会把外层 WHERE 当成内层 SELECT。
 */
export function clauseAt(sql: string): Clause {
  const tokens = tokenize(sql);
  const first = tokens.find((token) => token.kind === "ident")?.value.toLowerCase() ?? "";
  const open: Clause = first === "insert"
    ? "insert-table"
    : first === "update"
      ? "update-table"
      : first === "delete"
        ? "delete"
        : first === "call"
          ? "call"
          : first === "select" || first === "with"
            ? "select"
            : "expression";

  const frames: Clause[] = [open];
  for (const token of tokens) {
    if (token.value === "(") {
      const here = frames[frames.length - 1];
      // INSERT 的第一对括号是列清单；VALUES 里的括号仍然是值。
      frames.push(here === "insert-table" ? "insert-columns" : here === "insert-values" ? "insert-values" : "expression");
      continue;
    }
    if (token.value === ")") {
      if (frames.length > 1) frames.pop();
      continue;
    }
    if (token.kind !== "ident") continue;
    const word = token.value.toLowerCase();
    const frame = frames.length - 1;
    if (word === "and" || word === "or" || word === "not") continue;
    if (!CLAUSE_WORDS.has(word)) continue;
    frames[frame] = clauseFromWord(word, frames[frame]);
  }
  return frames[frames.length - 1] ?? open;
}

function clauseFromWord(word: string, current: Clause): Clause {
  switch (word) {
    case "select":
      return current === "insert-columns" || current === "insert-table" ? "insert-values" : "select";
    case "from":
      return "from";
    case "join":
    case "inner":
    case "left":
    case "right":
    case "full":
    case "cross":
      return "join";
    case "on":
    case "using":
      return "on";
    case "where":
      return "where";
    case "group":
      return "group";
    case "order":
      return "order";
    case "having":
      return "having";
    case "set":
      return "set";
    case "into":
      return "insert-table";
    case "values":
      return "insert-values";
    case "update":
      return "update-table";
    case "delete":
      return "delete";
    default:
      return current;
  }
}

/** FROM / JOIN / UPDATE / INTO 引入的关系，保留别名和 schema。 */
export function relationsIn(sql: string): RelationRef[] {
  const tokens = tokenize(sql);
  const refs: RelationRef[] = [];
  for (let index = 0; index < tokens.length; index += 1) {
    const word = tokens[index].value.toLowerCase();
    if (word !== "from" && word !== "join" && word !== "update" && word !== "into") continue;
    let cursor = index + 1;
    if (tokens[cursor]?.value.toLowerCase() === "only") cursor += 1;
    const first = tokens[cursor];
    if (!first || first.kind !== "ident") continue;
    cursor += 1;
    let schema: string | undefined;
    let name = first.value;
    if (tokens[cursor]?.value === "." && tokens[cursor + 1]?.kind === "ident") {
      schema = first.value;
      name = tokens[cursor + 1].value;
      cursor += 2;
    }
    let alias: string | undefined;
    if (tokens[cursor]?.value.toLowerCase() === "as" && tokens[cursor + 1]?.kind === "ident") {
      alias = tokens[cursor + 1].value;
    } else if (
      tokens[cursor]?.kind === "ident"
      && !CLAUSE_WORDS.has(tokens[cursor].value.toLowerCase())
    ) {
      alias = tokens[cursor].value;
    }
    refs.push({ schema, name, alias });
  }
  return refs;
}

function resolveRelation(
  qualifier: string,
  scope: RelationRef[],
  find: (name: string) => RelationRef | null,
): RelationRef | null {
  const folded = qualifier.toLowerCase();
  const aliased = scope.find((ref) => ref.alias?.toLowerCase() === folded);
  if (aliased) return aliased;
  const inScope = scope.find((ref) => ref.name.toLowerCase() === folded);
  if (inScope) return inScope;
  return find(qualifier);
}

function columnsInScope(scope: RelationRef[], columnsOf: (ref: RelationRef) => ColumnRecord[]): Completion[] {
  const options: Completion[] = [];
  const seen = new Set<string>();
  const many = scope.length > 1;
  for (const ref of scope) {
    const qualify = many ? ref.alias ?? ref.name : undefined;
    for (const column of columnsOf(ref)) {
      const option = columnOption(column, qualify);
      if (seen.has(option.label)) continue;
      seen.add(option.label);
      options.push(option);
    }
  }
  return options;
}

/** `ON` 后面优先给外键能直接写完的等值条件。 */
function joinConditions(scope: RelationRef[], columnsOf: (ref: RelationRef) => ColumnRecord[]): Completion[] {
  if (scope.length < 2) return [];
  const right = scope[scope.length - 1];
  const options: Completion[] = [];
  const rightColumns = columnsOf(right);
  for (const column of rightColumns) {
    for (const fk of column.foreignKeys) {
      const left = scope.slice(0, -1).find(
        (ref) => ref.name.toLowerCase() === fk.foreignTable.toLowerCase()
          && (!fk.foreignSchema || !ref.schema || ref.schema.toLowerCase() === fk.foreignSchema.toLowerCase()),
      );
      if (!left) continue;
      const index = fk.columns.findIndex((name) => name.toLowerCase() === column.name.toLowerCase());
      const foreignColumn = fk.foreignColumns[index] ?? fk.foreignColumns[0];
      if (!foreignColumn) continue;
      const leftSide = `${left.alias ?? left.name}.${foreignColumn}`;
      const rightSide = `${right.alias ?? right.name}.${column.name}`;
      options.push({
        label: `${leftSide} = ${rightSide}`,
        type: "keyword",
        detail: "foreign key",
        boost: 8,
      });
    }
  }
  return options;
}

function insertTarget(before: string): RelationRef | null {
  const refs = relationsIn(before).filter((ref) => ref.name);
  return refs.length > 0 ? refs[refs.length - 1] : null;
}
