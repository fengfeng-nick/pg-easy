import { describe, expect, it } from "vitest";
import { firstPageSql, tablePageSql, quoteIdent } from "./sql";

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
});
