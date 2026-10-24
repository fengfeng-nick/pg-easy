import { PostgreSQL, type SQLConfig, type SQLNamespace } from "@codemirror/lang-sql";
import { api, type Catalog } from "@/lib/ipc";

/** 构建补全配置。`columns` 以 `schema.relation` 为键；某一项可能缺失
 *  （列还没加载）或是一个占位的 promise。 */
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

// 跨编辑器实例共享：同一条连接上每个标签页的列清单都一样，
// 重复的并发请求只会让往返次数翻倍。
const inflight = new Map<string, Promise<string[]>>();

/**
 * 把某个关系的列加载进 `cache`。可以重复调用，每个关系只有第一次调用
 * 会真正发起查询。返回是否有内容发生了变化。
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
    // 这个关系上的补全就停在表级别；用户的查询本身不受影响。
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

/** 连接里的所有关系，顺序与侧边栏一致，供命令面板的表跳转使用。 */
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
