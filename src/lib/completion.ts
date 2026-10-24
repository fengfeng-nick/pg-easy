import type { CompletionContext, CompletionResult } from "@codemirror/autocomplete";
import type { SQLNamespace } from "@codemirror/lang-sql";
import { api, type Catalog } from "@/lib/ipc";

/**
 * SQL completion for one connection.
 *
 * Tables come from the catalog up front; columns are fetched lazily per
 * relation and cached for the connection's lifetime. On a database with a
 * thousand tables, loading every column at connect would cost more than
 * opening the session itself.
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
   * Notifies when lazily loaded columns change the suggestions. The editor
   * subscribes and rebuilds its namespace, so the first `alias.` completes
   * columns on the second attempt rather than the tenth.
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
        // Falls back to table-level completion; the query is unaffected.
      })
      .finally(() => this.inflight.delete(key));
  }

  /** The `{ schema: { table: [columns] } }` shape lang-sql takes. */
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
   * Layered after lang-sql's own sources. It warms any relation named after
   * FROM/JOIN so its columns arrive in the background, and answers
   * `qualifier.` for aliases and table names.
   */
  completionSource = (context: CompletionContext): CompletionResult | null => {
    if (!this.catalog) return null;
    const line = context.state.doc.lineAt(context.pos);
    const before = line.text.slice(0, context.pos - line.from);

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

    if (this.catalog.relations[qualifier]) {
      return {
        from,
        options: this.catalog.relations[qualifier]
          .filter((relation) => relation.kind !== "sequence")
          .map((relation) => ({ label: relation.name, type: "type" })),
        validFor: /^[\w$]*$/,
      };
    }

    const doc = context.state.doc.toString();
    const escaped = qualifier.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const aliasMatch = doc.match(
      new RegExp(
        `(?:from|join|update|into)\\s+"?([\\w$]+)"?\\s+(?:as\\s+)?"?${escaped}"?(?![\\w$])`,
        "i",
      ),
    );
    const target = this.findRelation(undefined, aliasMatch?.[1] ?? qualifier);
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
}
