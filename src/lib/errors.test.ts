import { describe, expect, it } from "vitest";
import { isOwnMessage, translateToken } from "./errors";

describe("backend error tokens", () => {
  it("formats a token with its arguments in order", () => {
    expect(translateToken({ token: "connect_timed_out", args: ["db.local", 5432] }, "en")).toBe(
      "Connecting to db.local:5432 timed out",
    );
  });

  it("translates the same token into Chinese", () => {
    expect(translateToken({ token: "connect_timed_out", args: ["db.local", 5432] }, "zh")).toMatch(
      /db\.local:5432/,
    );
  });

  it("passes server text through untouched", () => {
    const serverText = 'relation "nope" does not exist';
    expect(isOwnMessage(serverText)).toBe(false);
    expect(translateToken({ token: serverText }, "zh")).toBe(serverText);
  });

  it("knows which messages the app composed", () => {
    expect(isOwnMessage("connect_timed_out")).toBe(true);
    expect(isOwnMessage("something not a token")).toBe(false);
  });
});
