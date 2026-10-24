import { EditorState } from "@codemirror/state";
import { CompletionContext } from "@codemirror/autocomplete";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => ({
  api: {
    loadColumns: vi.fn(),
  },
}));

import { api, type Catalog, type ColumnInfo } from "./ipc";
import { CompletionData } from "./completion";

const catalog: Catalog = {
  schemas: [{ name: "public" }, { name: "sales" }],
  relations: {
    public: [
      {
        oid: 1,
        schema: "public",
        name: "users",
        kind: "table",
        columns: 2,
        estRows: 10,
        size: "8 kB",
      },
    ],
    sales: [
      {
        oid: 2,
        schema: "sales",
        name: "orders",
        kind: "table",
        columns: 1,
        estRows: 3,
        size: "8 kB",
      },
    ],
  },
  routines: {
    public: [
      {
        schema: "public",
        name: "slugify",
        kind: "function",
        arguments: "text",
        returns: "text",
      },
    ],
    sales: [],
  },
  fetchedAt: 0,
};

const columns: ColumnInfo[] = [
  { name: "id", dataType: "int4", isNullable: false, isPrimaryKey: true, defaultValue: null, position: 1 },
  { name: "email", dataType: "text", isNullable: true, isPrimaryKey: false, defaultValue: null, position: 2 },
];

function contextAt(sql: string, pos = sql.length) {
  const state = EditorState.create({ doc: sql });
  return new CompletionContext(state, pos, false);
}

describe("SQL completion", () => {
  beforeEach(() => {
    vi.mocked(api.loadColumns).mockReset();
    vi.mocked(api.loadColumns).mockResolvedValue(columns);
  });

  it("offers schemas and functions while typing an identifier", () => {
    const data = new CompletionData(catalog, "conn");
    const result = data.identifierSource(contextAt("select slu"));
    const labels = result?.options.map((option) => option.label) ?? [];
    expect(labels).toContain("public");
    expect(labels).toContain("sales");
    expect(labels).toContain("slugify");
  });

  it("does not repeat identifiers after a qualifier dot", () => {
    const data = new CompletionData(catalog, "conn");
    expect(data.identifierSource(contextAt("select public."))).toBeNull();
  });

  it("lists a schema's tables and functions after the dot", () => {
    const data = new CompletionData(catalog, "conn");
    const result = data.completionSource(contextAt("select sales."));
    expect(result?.options.map((option) => option.label)).toEqual(["orders"]);

    const fns = data.completionSource(contextAt("select public."));
    expect(fns?.options.map((option) => option.label)).toEqual(["users", "slugify"]);
  });

  it("completes an alias's columns once they have loaded", async () => {
    const data = new CompletionData(catalog, "conn");
    const notified = vi.fn();
    data.subscribe(notified);

    const sql = "select u.\nfrom users u";
    const afterDot = sql.indexOf(".") + 1;
    // 列还在路上：点号后面先补不出东西，但 FROM 已经把 users 预热了。
    expect(data.completionSource(contextAt(sql, afterDot))).toBeNull();
    await vi.waitFor(() => expect(notified).toHaveBeenCalled());

    const result = data.completionSource(contextAt(sql, afterDot));
    expect(result?.options.map((option) => option.label)).toEqual(["id", "email"]);
    expect(api.loadColumns).toHaveBeenCalledTimes(1);
  });

  it("puts loaded columns into the schema namespace", async () => {
    const data = new CompletionData(catalog, "conn");
    data.completionSource(contextAt("from users "));
    await vi.waitFor(() => {
      const tables = data.namespace() as Record<string, Record<string, string[]>>;
      expect(tables.public.users).toEqual(["id", "email"]);
    });
  });
});
