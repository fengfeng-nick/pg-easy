import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { ExportDialog } from "@/components/workbench/ExportDialog";
import { useApp, type ExportRequest } from "@/lib/store";
import "@fontsource-variable/inter";
import "@/styles/tokens.css";
import "@/styles/base.css";

useApp.setState({
  booted: true,
  connections: [
    {
      id: "c1",
      savedId: null,
      name: "Local dev",
      user: "root",
      database: "shop",
      host: "localhost",
      port: 5432,
      serverVersion: "18.6",
      backendPid: 1,
      txStatus: "idle",
      browsing: false,
      open: true,
      catalogError: null,
      exporting: { done: 3, total: 7 },
      catalog: {
        serverVersion: "18.6",
        currentDatabase: "shop",
        currentUser: "root",
        databases: [],
        fetchedAt: Date.now(),
        routines: { public: [] },
        relations: {
          public: [
            { oid: 1, schema: "public", name: "orders", kind: "table", columns: 5, estRows: 2100, size: "1.2 MB" },
            { oid: 2, schema: "public", name: "users", kind: "table", columns: 7, estRows: 300, size: "96 kB" },
            { oid: 3, schema: "public", name: "rich", kind: "view", columns: 2, estRows: 0, size: "0 bytes" },
          ],
          audit: [
            { oid: 4, schema: "audit", name: "events", kind: "partitioned", columns: 4, estRows: 90000, size: "40 MB" },
            { oid: 5, schema: "audit", name: "free_seq", kind: "sequence", columns: 1, estRows: 0, size: "8 kB" },
          ],
        },
      },
    },
  ] as never,
});

const mode = new URLSearchParams(location.search).get("mode") ?? "db";
const requests: Record<string, ExportRequest | null> = {
  db: { connectionId: "c1", schema: null, scope: null },
  schema: { connectionId: "c1", schema: "public", scope: null },
  picked: { connectionId: "c1", schema: null, scope: ["audit.events", "public.users", "audit.free_seq"] },
  one: { connectionId: "c1", schema: null, scope: ["public.orders"] },
};

function Preview() {
  const [request, setRequest] = useState(requests[mode] ?? null);
  useEffect(() => {
    useApp.setState({ openExport: (next) => setRequest(next) });
  }, []);
  useEffect(() => {
    useApp.setState({ exportRequest: request });
  }, [request]);
  return (
    <div className="flex h-full flex-col gap-3 bg-surface p-10 text-[12.5px] text-ink-muted">
      <div className="flex gap-2">
        {Object.keys(requests).map((key) => (
          <a key={key} href={`?mode=${key}`} className="text-accent-ink underline">
            {key}
          </a>
        ))}
        <button onClick={() => setRequest(null)} className="text-accent-ink underline">
          closed
        </button>
      </div>
      <ExportDialog />
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Preview />);
