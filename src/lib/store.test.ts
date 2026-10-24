import { beforeEach, describe, expect, it } from "vitest";
import { useApp, type Connection } from "./store";

const connection = (id: string): Connection => ({
  id,
  savedId: null,
  name: id,
  user: "postgres",
  database: "postgres",
  host: "localhost",
  port: 5432,
  serverVersion: "17",
  backendPid: 1,
  txStatus: "idle",
  catalog: null,
  catalogError: null,
  browsing: false,
  open: true,
});

beforeEach(() => {
  useApp.setState({ connections: [], tabs: [], activeTabId: null, view: { kind: "welcome" } });
  useApp.getState().addConnection(connection("keep"));
  useApp.getState().addConnection(connection("drop"));
  useApp.getState().addTab({ connectionId: "drop" });
  useApp.getState().addTab({ connectionId: "keep" });
});

describe("removing a connection", () => {
  it("drops the connection, its tabs and the table view it was showing", () => {
    const stored = useApp.getState();
    stored.setView({ kind: "table", connectionId: "drop", schema: "public", relation: "orders" });
    stored.removeConnection("drop");

    const after = useApp.getState();
    expect(after.connections.map((item) => item.id)).toEqual(["keep"]);
    expect(after.tabs.every((tab) => tab.connectionId === "keep")).toBe(true);
    expect(after.tabs).toHaveLength(1);
    expect(after.view).toEqual({ kind: "editor" });
    expect(after.tabs.find((tab) => tab.id === after.activeTabId)?.connectionId).toBe("keep");
  });

  it("falls back to the welcome screen when no tabs are left", () => {
    useApp.getState().removeConnection("keep");
    useApp.getState().removeConnection("drop");

    const after = useApp.getState();
    expect(after.connections).toEqual([]);
    expect(after.tabs).toEqual([]);
    expect(after.activeTabId).toBeNull();
    expect(after.view).toEqual({ kind: "welcome" });
  });
});
