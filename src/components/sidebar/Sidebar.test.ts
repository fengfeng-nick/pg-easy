import { describe, expect, it } from "vitest";
import type { Catalog } from "@/lib/ipc";
import { treeGroups } from "./Sidebar";

const relation = (
  schema: string,
  name: string,
  kind: Catalog["relations"][string][number]["kind"],
) => ({ oid: name.length, schema, name, kind, columns: 3, estRows: 10, size: "8 kB" });

const catalog: Catalog = {
  schemas: [{ name: "public" }, { name: "sales" }],
  relations: {
    public: [
      relation("public", "orders", "table"),
      relation("public", "orders_report", "view"),
      relation("public", "orders_id_seq", "sequence"),
    ],
    sales: [relation("sales", "invoices", "table")],
  },
  routines: {},
  fetchedAt: Date.now(),
};

describe("the tree under a connection", () => {
  it("lists every schema's tables, the schema's name spelt as the server has it", () => {
    const groups = treeGroups(catalog, "");
    expect(groups.map((group) => group.schema)).toEqual(["public", "sales"]);
    expect(groups[0].tables.map((table) => table.name)).toEqual(["orders", "orders_report"]);
    expect(groups[1].tables.map((table) => table.name)).toEqual(["invoices"]);
  });

  it("keeps sequences out of the way until they are searched for", () => {
    expect(treeGroups(catalog, "").flatMap((group) => group.tables.map((t) => t.name))).not.toContain(
      "orders_id_seq",
    );
    expect(treeGroups(catalog, "seq")[0].tables.map((table) => table.name)).toEqual([
      "orders_id_seq",
    ]);
  });

  it("matches on the table name or the schema-qualified one, ignoring case", () => {
    expect(treeGroups(catalog, "REPORT")[0].tables.map((table) => table.name)).toEqual([
      "orders_report",
    ]);
    expect(treeGroups(catalog, "sales.inv")[1].tables.map((table) => table.name)).toEqual([
      "invoices",
    ]);
    expect(treeGroups(catalog, "nothing").every((group) => group.tables.length === 0)).toBe(true);
  });

  it("counts the schema's routines", () => {
    const withRoutine: Catalog = {
      ...catalog,
      routines: {
        public: [
          {
            schema: "public",
            name: "total",
            kind: "function",
            arguments: "",
            returns: "numeric",
          },
        ],
      },
    };
    expect(treeGroups(withRoutine, "").map((group) => group.routineCount)).toEqual([1, 0]);
  });

  it("has nothing to show before the catalog has loaded", () => {
    expect(treeGroups(null, "")).toEqual([]);
  });
});
