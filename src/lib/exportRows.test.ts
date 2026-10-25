import { describe, expect, it } from "vitest";
import { encodeBase64, rowsForCopy } from "./exportRows";
import type { Column } from "./ipc";

const columns: Column[] = [
  { name: "id", typeName: "int4", typeOid: 23, kind: "number" },
  { name: "name", typeName: "text", typeOid: 25, kind: "text" },
];

describe("row export helpers", () => {
  it("encodes unicode as utf-8 base64", () => {
    expect(encodeBase64("你好")).toBe("5L2g5aW9");
  });

  it("copies the selected row, or every loaded row when nothing is selected", () => {
    const table = { columns, rows: [["1", "a"], ["2", "b"]] };
    expect(rowsForCopy(table, 1).rows).toEqual([["2", "b"]]);
    expect(rowsForCopy(table, null).rows).toHaveLength(2);
  });
});