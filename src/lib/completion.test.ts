import { PostgreSQL, sql } from "@codemirror/lang-sql";
import { EditorState } from "@codemirror/state";
import { CompletionContext } from "@codemirror/autocomplete";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => ({
  api: {
    loadColumns: vi.fn(),
  },
}));

import { api, type Catalog, type ColumnInfo } from "./ipc";
import { PgCompletion, clauseAt, relationsIn } from "./pgComplete";

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
        columns: 2,
        estRows: 3,
        size: "8 kB",
      },
    ],
  },
  routines: {
    public: [
      { schema: "public", name: "slugify", kind: "function", arguments: "text", returns: "text" },
    ],
    pg_catalog: [
      { schema: "pg_catalog", name: "generate_series", kind: "function", arguments: "integer, integer", returns: "setof integer" },
    ],
    sales: [],
  },
  fetchedAt: 0,
};

const userColumns: ColumnInfo[] = [
  { name: "id", dataType: "integer", isNullable: false, isPrimaryKey: true, defaultValue: null, position: 1, foreignKeys: [] },
  { name: "email", dataType: "text", isNullable: true, isPrimaryKey: false, defaultValue: null, position: 2, foreignKeys: [] },
];

const orderColumns: ColumnInfo[] = [
  { name: "id", dataType: "integer", isNullable: false, isPrimaryKey: true, defaultValue: null, position: 1, foreignKeys: [] },
  {
    name: "user_id",
    dataType: "integer",
    isNullable: false,
    isPrimaryKey: false,
    defaultValue: null,
    position: 2,
    foreignKeys: [{ columns: ["user_id"], foreignSchema: "public", foreignTable: "users", foreignColumns: ["id"] }],
  },
];

function contextAt(sqlText: string, pos = sqlText.length) {
  const state = EditorState.create({
    doc: sqlText,
    extensions: [sql({ dialect: PostgreSQL }).language],
  });
  return new CompletionContext(state, pos, false);
}

function labelsOf(result: { options: readonly { label: string }[] } | null | undefined) {
  return result?.options.map((option) => option.label) ?? [];
}

describe("clause detection", () => {
  it("keeps a subquery from leaking into the outer clause", () => {
    expect(clauseAt("select ")).toBe("select");
    expect(clauseAt("select id from ")).toBe("from");
    expect(clauseAt("select id from users where ")).toBe("where");
    expect(clauseAt("select id from users u join ")).toBe("join");
    expect(clauseAt("select id from users u join orders o on ")).toBe("on");
    expect(clauseAt("select id from users group by ")).toBe("group");
    expect(clauseAt("select id from users order by ")).toBe("order");
    expect(clauseAt("insert into ")).toBe("insert-table");
    expect(clauseAt("insert into users (")).toBe("insert-columns");
    expect(clauseAt("insert into users (id) values (")).toBe("insert-values");
    expect(clauseAt("update users set ")).toBe("set");
    expect(clauseAt("select (select id from ")).toBe("from");
  });

  it("reads schema-qualified names and aliases", () => {
    expect(relationsIn('select * from sales.orders o join "Users" u')).toEqual([
      { schema: "sales", name: "orders", alias: "o" },
      { schema: undefined, name: "Users", alias: "u" },
    ]);
  });
});

describe("PostgreSQL completion", () => {
  beforeEach(() => {
    vi.mocked(api.loadColumns).mockReset();
    vi.mocked(api.loadColumns).mockImplementation(async (_id, _schema, relation) =>
      relation === "orders" ? orderColumns : userColumns,
    );
  });

  it("offers tables rather than a dump of keywords after FROM", async () => {
    const data = new PgCompletion(catalog, "conn");
    const result = await data.completionSource(contextAt("select * from "));
    const labels = labelsOf(result);
    expect(labels).toContain("users");
    expect(labels).toContain("orders");
    expect(labels).not.toContain("generate_series");
    expect(labels).not.toContain("email");
  });

  it("offers a schema's relations after the schema dot", async () => {
    const data = new PgCompletion(catalog, "conn");
    const result = await data.completionSource(contextAt("select * from sales."));
    expect(labelsOf(result)).toEqual(["orders"]);
  });

  it("completes an alias's columns, including when the alias is typed later", async () => {
    const data = new PgCompletion(catalog, "conn");
    const sqlText = "select u.\nfrom users u";
    const atDot = sqlText.indexOf(".") + 1;
    expect(await data.completionSource(contextAt(sqlText, atDot))).toBeNull();
    await vi.waitFor(() => expect(api.loadColumns).toHaveBeenCalled());

    const result = await data.completionSource(contextAt(sqlText, atDot));
    expect(labelsOf(result)).toEqual(["id", "email"]);
    expect(api.loadColumns).toHaveBeenCalledTimes(1);
  });

  it("offers columns of tables already in the statement after WHERE", async () => {
    const data = new PgCompletion(catalog, "conn");
    data.completionSource(contextAt("select * from users where "));
    await vi.waitFor(() => expect(api.loadColumns).toHaveBeenCalled());

    const result = await data.completionSource(contextAt("select * from users where "));
    const labels = labelsOf(result);
    expect(labels).toContain("id");
    expect(labels).toContain("email");
    expect(labels).not.toContain("orders");
  });

  it("suggests the foreign-key join before loose columns", async () => {
    const data = new PgCompletion(catalog, "conn");
    const sqlText = "select * from users u join orders o on ";
    data.completionSource(contextAt(sqlText));
    await vi.waitFor(() => expect(api.loadColumns).toHaveBeenCalledTimes(2));

    const result = await data.completionSource(contextAt(sqlText));
    expect(result && "options" in result ? result.options[0]?.label : "").toBe("u.id = o.user_id");
  });

  it("lists the target table's columns inside an INSERT column list", async () => {
    const data = new PgCompletion(catalog, "conn");
    data.completionSource(contextAt("insert into users ("));
    await vi.waitFor(() => expect(api.loadColumns).toHaveBeenCalled());
    const result = await data.completionSource(contextAt("insert into users ("));
    expect(labelsOf(result)).toEqual(expect.arrayContaining(["id", "email"]));
    expect(labelsOf(result)).not.toContain("orders");
  });

  it("filters functions by the typed prefix", async () => {
    const data = new PgCompletion(catalog, "conn");
    const result = await data.completionSource(contextAt("select gen"));
    const labels = labelsOf(result);
    expect(labels).toContain("generate_series");
    expect(labels).not.toContain("slugify");
  });
});
