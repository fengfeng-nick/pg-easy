import { PostgreSQL, type SQLConfig, type SQLNamespace } from "@codemirror/lang-sql";
import { api, type Catalog } from "@/lib/ipc";

/** Builds the completion config. `columns` is keyed `schema.relation`; an
 *  entry may be absent (columns not loaded yet) or a promise placeholder. */
export function makeSqlConfig(
  catalog: Catalog | null,
  columns: ReadonlyMap<string, string[]>,
): SQLConfig {
  if (!catalog || catalog.schemas.length === 0) {
    return { dialect: PostgreSQL, upperCaseKeywords: true };
  }

  const schema: Record<string, Record<string, readonly string[]>> = {};
  for (const [schemaName, relations] of Object.entries(catalog.relations)) {
    const tables: Record<string, readonly string[]> = {};
    for (const relation of relations) {
      if (relation.kind === "sequence") continue;
      tables[relation.name] = columns.get(`${schemaName}.${relation.name}`) ?? [];
    }
    schema[schemaName] = tables;
  }

  return {
    dialect: PostgreSQL,
    upperCaseKeywords: true,
    schema: schema as unknown as SQLNamespace,
    defaultSchema: catalog.schemas[0]?.name,
  };
}

// Shared across editor instances: a column list is the same for every tab on
// a connection, and duplicate in-flight requests would double the round trips.
const inflight = new Map<string, Promise<string[]>>();

/**
 * Loads a relation's columns into `cache`. Safe to call repeatedly; only the
 * first call per relation issues a query. Returns whether anything changed.
 */
export async function loadRelationColumns(
  connectionId: string,
  schema: string,
  relation: string,
  cache: Map<string, string[]>,
): Promise<boolean> {
  const key = `${schema}.${relation}`;
  if (cache.has(key)) return false;

  const scope = `${connectionId}:${key}`;
  let request = inflight.get(scope);
  if (!request) {
    request = api
      .loadColumns(connectionId, schema, relation)
      .then((list) => list.map((column) => column.name));
    inflight.set(scope, request);
  }

  try {
    const names = await request;
    if (cache.has(key)) return false;
    cache.set(key, names);
    return true;
  } catch {
    // Completion silently stays at table level for this relation; the user's
    // query is unaffected.
    return false;
  } finally {
    inflight.delete(scope);
  }
}

export type TableEntry = {
  oid: number;
  schema: string;
  name: string;
  kind: string;
  columns: number;
  estRows: number;
  size: string;
  qualified: string;
};

/** Every relation in the connection, ordered like the sidebar, for the
 *  command palette's table jump. */
export function listTables(catalog: Catalog | null): TableEntry[] {
  if (!catalog) return [];
  return Object.entries(catalog.relations)
    .flatMap(([schema, relations]) =>
      relations.map((relation) => ({
        ...relation,
        schema,
        qualified: `${schema}.${relation.name}`,
      })),
    )
    .sort((a, b) => a.schema.localeCompare(b.schema) || a.name.localeCompare(b.name));
}
