import { describe, expect, it } from "vitest";
import { en } from "./en";
import { fill } from "./fill";
import { zh } from "./zh";

/** Every leaf path in a message catalogue: `a.b.c` -> value. */
function leaves(node: unknown, prefix = "", out = new Map<string, string>()) {
  for (const [key, value] of Object.entries(node as Record<string, unknown>)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") out.set(path, value);
    else leaves(value, path, out);
  }
  return out;
}

const placeholders = (text: string) =>
  [...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort();

describe("message catalogues", () => {
  const english = leaves(en);
  const chinese = leaves(zh);

  it("translate every English message", () => {
    expect([...english.keys()].filter((key) => !chinese.has(key))).toEqual([]);
  });

  it("add no messages English does not have", () => {
    expect([...chinese.keys()].filter((key) => !english.has(key))).toEqual([]);
  });

  it("use the same placeholders as English", () => {
    const mismatched = [...chinese].filter(
      ([key, text]) => placeholders(text).join() !== placeholders(english.get(key)!).join(),
    );
    expect(mismatched.map(([key]) => key)).toEqual([]);
  });

  it("leave no Chinese label as untranslated English", () => {
    // Short technical terms (TLS, pid, …) are the same on purpose.
    const same = [...chinese]
      .filter(([key, text]) => text === english.get(key))
      .map(([key]) => key)
      .sort();
    expect(same).toEqual([
      "app.name",
      "common.notice",
      "common.null",
      "connection.namePlaceholder",
      "status.pid",
      "table.headerIndex",
    ]);
  });

  it("fill placeholders and leave unknown ones alone", () => {
    expect(fill("Hello {name}, {count} rows", { name: "Ada", count: 3 })).toBe("Hello Ada, 3 rows");
    expect(fill("{a} {b}", { a: "x" })).toBe("x {b}");
  });
});
