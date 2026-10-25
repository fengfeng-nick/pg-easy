import { describe, expect, it } from "vitest";
import {
  deleteRowSql,
  explainSql,
  filterWhere,
  firstPageSql,
  insertRowSql,
  quoteIdent,
  tablePageSql,
  toCsv,
  toInserts,
  toJson,
  updateRowSql,
} from "./sql";

describe("explain", () => {
  it("wraps the first statement and ignores a later one", () => {
    expect(explainSql("select 1; select 2")).toBe("explain (analyze, buffers, format text)\nselect 1");
  });

  it("keeps a semicolon that sits inside a string", () => {
    expect(explainSql("select ';'; select 2")).toBe(
      "explain (analyze, buffers, format text)\nselect ';'",
    );
  });

  it("does not wrap a statement that is already an explain", () => {
    expect(explainSql("EXPLAIN select 1")).toBe("EXPLAIN select 1");
  });

  it("returns nothing for an empty editor", () => {
    expect(explainSql("  \n-- only a comment")).toBe("");
  });
});

describe("table browsing SQL", () => {
  it("quotes identifiers, doubling embedded quotes", () => {
    expect(quoteIdent("public")).toBe('"public"');
    expect(quoteIdent('we"ird')).toBe('"we""ird"');
  });

  it("reads the first page of a table", () => {
    expect(firstPageSql("public", "orders", 500)).toBe(
      'select * from "public"."orders" limit 500',
    );
  });

  it("orders by the primary key and pages with an offset", () => {
    expect(tablePageSql("sales", "orders", ["id", "region"], 500, 1000)).toBe(
      'select * from "sales"."orders" order by "id", "region" limit 500 offset 1000',
    );
  });

  it("leaves out the order by when there is no primary key", () => {
    expect(tablePageSql("public", "no_pk", [], 500, 0)).toBe(
      'select * from "public"."no_pk" limit 500 offset 0',
    );
  });

  it("appends a ready-made filter before the order", () => {
    expect(tablePageSql("public", "orders", ["id"], 100, 0, `"status" = 'open'`)).toBe(
      `select * from "public"."orders" where "status" = 'open' order by "id" limit 100 offset 0`,
    );
  });
});

describe("table filters", () => {
  it("treats a plain word as a case-insensitive contains match", () => {
    expect(filterWhere("ada", ["name", "email"])).toBe(
      `"name"::text ilike '%ada%' or "email"::text ilike '%ada%'`,
    );
  });

  it("keeps a written condition as the user typed it", () => {
    expect(filterWhere("status = 'open'", ["status"])).toBe(`(status = 'open')`);
  });

  it("returns nothing for a blank filter", () => {
    expect(filterWhere("   ", ["name"])).toBe("");
  });
});

describe("row edits", () => {
  it("updates only the changed columns, matched on the primary key", () => {
    expect(
      updateRowSql("public", "users", ["id"], ["7"], [{ column: "email", value: "a@b.c" }]),
    ).toBe(`update "public"."users" set "email" = 'a@b.c' where "id" = '7'`);
  });

  it("writes null and escapes quotes in the value", () => {
    expect(
      updateRowSql("public", "users", ["id"], ["7"], [{ column: "note", value: "it's" }]),
    ).toBe(`update "public"."users" set "note" = 'it''s' where "id" = '7'`);
    expect(deleteRowSql("public", "users", ["id"], [null])).toBe(
      `delete from "public"."users" where "id" is null`,
    );
  });

  it("inserts the columns the user actually filled in", () => {
    expect(
      insertRowSql("public", "users", [
        { column: "email", value: "a@b.c" },
        { column: "note", value: null },
      ]),
    ).toBe(`insert into "public"."users" ("email", "note") values ('a@b.c', null)`);
  });
});

describe("result export", () => {
  const table = {
    columns: [{ name: "id" }, { name: "note" }],
    rows: [
      ["1", "plain"],
      ["2", 'say "hi",\nok'],
      ["3", null],
    ],
  };

  it("quotes csv fields that contain a comma, quote or newline", () => {
    expect(toCsv(table)).toBe(`id,note\n1,plain\n2,"say ""hi"",\nok"\n3,`);
  });

  it("keeps nulls as json null and text as text", () => {
    expect(JSON.parse(toJson(table))).toEqual([
      { id: "1", note: "plain" },
      { id: "2", note: 'say "hi",\nok' },
      { id: "3", note: null },
    ]);
  });

  it("renders one insert per row", () => {
    expect(toInserts("public", "notes", table)).toBe(
      `insert into "public"."notes" ("id", "note") values ('1', 'plain');\n` +
        `insert into "public"."notes" ("id", "note") values ('2', 'say "hi",\nok');\n` +
        `insert into "public"."notes" ("id", "note") values ('3', null);`,
    );
  });
});
