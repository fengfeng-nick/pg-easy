import type { Completion, CompletionContext, CompletionResult } from "@codemirror/autocomplete";
import type { SQLNamespace } from "@codemirror/lang-sql";
import { api, type Catalog } from "@/lib/ipc";

/**
 * 单条连接的 SQL 补全。
 *
 * 表在连接时就从目录一次取好；列按关系懒加载，并在连接的整个生命周期内
 * 缓存。在有一千张表的库上，连接时加载所有列的开销会比打开会话本身还大。
 */
export class CompletionData {
  private columns = new Map<string, string[]>();
  private inflight = new Set<string>();
  private listeners = new Set<() => void>();

  constructor(
    readonly catalog: Catalog | null,
    private readonly connectionId: string | null,
  ) {}

  /**
   * 懒加载的列发生变化时发出通知。编辑器订阅它并重建命名空间，这样第一次
   * 敲 `alias.` 时也许补不出列，第二次就能补出来，而不必等到第十次。
   */
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  private key(schema: string, relation: string) {
    return `${schema}.${relation}`;
  }

  private warm(schema: string, relation: string) {
    if (!this.connectionId) return;
    const key = this.key(schema, relation);
    if (this.columns.has(key) || this.inflight.has(key)) return;
    this.inflight.add(key);
    api
      .loadColumns(this.connectionId, schema, relation)
      .then((list) => {
        this.columns.set(key, list.map((column) => column.name));
        this.listeners.forEach((listener) => listener());
      })
      .catch(() => {
        // 退回到表级别补全；查询本身不受影响。
      })
      .finally(() => this.inflight.delete(key));
  }

  /** lang-sql 接收的 `{ schema: { table: [columns] } }` 结构。 */
  namespace(): SQLNamespace | undefined {
    if (!this.catalog) return undefined;
    const namespace: Record<string, Record<string, readonly string[]>> = {};
    for (const [schema, relations] of Object.entries(this.catalog.relations)) {
      const tables: Record<string, readonly string[]> = {};
      for (const relation of relations) {
        if (relation.kind === "sequence") continue;
        tables[relation.name] = this.columns.get(this.key(schema, relation.name)) ?? [];
      }
      namespace[schema] = tables;
    }
    return namespace as unknown as SQLNamespace;
  }

  private findRelation(schema: string | undefined, name: string) {
    if (!this.catalog) return null;
    const schemas = schema ? [schema] : this.catalog.schemas.map((item) => item.name);
    for (const schemaName of schemas) {
      const found = this.catalog.relations[schemaName]?.find((relation) => relation.name === name);
      if (found) return { schema: schemaName, relation: found.name };
    }
    return null;
  }

  /**
   * 叠在 lang-sql 自带的数据源之后。它会把 FROM/JOIN 后面提到的关系预热，
   * 让列在后台到达；对别名和表名响应 `qualifier.`。
   */
  completionSource = (context: CompletionContext): CompletionResult | null => {
    if (!this.catalog) return null;
    const doc = context.state.doc.toString();
    // 别名和 FROM 往往不在同一行，预热和解析都看光标之前的全文。
    const before = doc.slice(0, context.pos);

    for (const match of before.matchAll(
      /(?:from|join|update|into)\s+"?([\w$]+)"?(?:\s*\.\s*"?([\w$]+)"?)?/gi,
    )) {
      const [, first, qualifiedTable] = match;
      if (qualifiedTable) this.warm(first, qualifiedTable);
      else {
        const resolved = this.findRelation(undefined, first);
        if (resolved) this.warm(resolved.schema, resolved.relation);
      }
    }

    const dotted = before.match(/([\w$"]+)\.([\w$"]*)$/);
    if (!dotted) return null;
    const qualifier = dotted[1].replace(/"/g, "");
    const from = context.pos - dotted[2].length;
    // 别名可能写在光标后面（`select u.\nfrom users u`），所以用全文。
    const aliases = aliasMap(doc);

    if (this.catalog.relations[qualifier]) {
      const relations = this.catalog.relations[qualifier]
        .filter((relation) => relation.kind !== "sequence")
        .map((relation) => ({ label: relation.name, type: "type" }));
      const routines = (this.catalog.routines[qualifier] ?? []).map((routine) => ({
        label: routine.name,
        type: "function",
        detail: routine.arguments,
      }));
      return {
        from,
        options: [...relations, ...routines],
        validFor: /^[\w$]*$/,
      };
    }

    const target = this.findRelation(undefined, aliases.get(qualifier.toLowerCase()) ?? qualifier);
    if (!target) return null;

    const columns = this.columns.get(this.key(target.schema, target.relation)) ?? [];
    this.warm(target.schema, target.relation);
    if (columns.length === 0) return null;
    return {
      from,
      options: columns.map((name) => ({ label: name, type: "property" })),
      validFor: /^[\w$]*$/,
    };
  };

  /**
   * 补 schema 名、函数名和过程名。表和列交给 lang-sql 的 schema 补全，
   * 这里只补它没有的那一层，避免同一项出现两次。
   */
  identifierSource = (context: CompletionContext): CompletionResult | null => {
    if (!this.catalog) return null;
    const word = context.matchBefore(/[\w$]+/);
    if (!word) return null;
    if (word.from === word.to && !context.explicit) return null;

    const line = context.state.doc.lineAt(context.pos);
    const before = line.text.slice(0, word.from - line.from);
    // `qualifier.` 由 completionSource 负责，这里不再插一脚。
    if (/[\w$"]\s*\.\s*$/.test(before)) return null;

    const options: Completion[] = [];
    const seen = new Set<string>();
    const push = (option: Completion) => {
      const key = `${option.type}:${option.label}`;
      if (seen.has(key)) return;
      seen.add(key);
      options.push(option);
    };

    for (const schema of this.catalog.schemas) {
      push({ label: schema.name, type: "namespace" });
    }
    for (const routines of Object.values(this.catalog.routines)) {
      for (const routine of routines) {
        push({
          label: routine.name,
          type: "function",
          detail: routine.arguments || undefined,
          boost: -1,
        });
      }
    }
    if (options.length === 0) return null;
    return { from: word.from, options, validFor: /^[\w$]*$/ };
  };
}

/** `别名 → 表名`。FROM/JOIN 后面的限定名只取表那一段。 */
function aliasMap(doc: string) {
  const aliases = new Map<string, string>();
  const pattern =
    /(?:from|join|update|into)\s+(?:only\s+)?(?:"?[\w$]+"?\s*\.\s*)?"?([\w$]+)"?\s+(?:as\s+)?"?([\w$]+)"?/gi;
  const reserved = new Set([
    "where", "join", "inner", "left", "right", "full", "cross", "on", "using",
    "group", "order", "limit", "offset", "union", "set", "returning", "window",
  ]);
  for (const match of doc.matchAll(pattern)) {
    const table = match[1];
    const alias = match[2];
    if (reserved.has(alias.toLowerCase())) continue;
    aliases.set(alias.toLowerCase(), table);
  }
  return aliases;
}
