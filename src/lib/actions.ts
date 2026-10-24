import {
  api,
  asAppError,
  type AppError,
  type ConnectParams,
  type DumpOptions,
  type SavedConnection,
} from "@/lib/ipc";
import { describeError } from "@/lib/errors";
import { translateTo, useApp, type Connection } from "@/lib/store";

/** Workspaces opened this launch. Re-saving a password should reconnect with
 *  it rather than keep a stale session around. */
export function workspaceFor(params: ConnectParams) {
  return useApp
    .getState()
    .connections.find(
      (connection) =>
        connection.user === params.user &&
        connection.database === params.database &&
        connection.host === params.host.trim() &&
        connection.port === params.port,
    );
}

export async function openConnection(
  params: ConnectParams,
  saved: SavedConnection | null,
): Promise<string | null> {
  const state = useApp.getState();
  const existing = workspaceFor(params);
  if (existing) {
    if (saved) state.updateConnection(existing.id, { savedId: saved.id });
    return existing.id;
  }

  state.setConnectingTo(params.host || "localhost");
  try {
    const info = await api.connect(params);
    state.addConnection({
      id: info.id,
      savedId: saved?.id ?? null,
      name: saved?.name || params.database,
      user: params.user,
      database: params.database,
      host: params.host,
      port: params.port,
      serverVersion: info.serverVersion,
      backendPid: info.backendPid,
      txStatus: info.txStatus,
      catalog: null,
      catalogError: null,
      browsing: false,
      open: true,
    });
    void refreshCatalog(info.id);
    const firstAlive = state.tabs.find((tab) => !tab.connectionId);
    if (!state.tabs.some((tab) => tab.connectionId === info.id) && !firstAlive) {
      state.addTab({ connectionId: info.id });
    } else if (firstAlive) {
      state.updateTab(firstAlive.id, { connectionId: info.id });
      state.activateTab(firstAlive.id);
    }
    return info.id;
  } catch (failure) {
    const error = asAppError(failure);
    state.setConnectionError(params.host, error.message);
    const { message, detail } = describeError(error);
    state.pushToast({
      tone: "error",
      retitle: (t) => ({
        // A generic headline alone gives nothing to act on; the detail carries the
        // reason (wrong password, server closed, TLS refused).
        title:
          error.kind === "connection" || error.kind === "tls"
            ? t("notices.couldNotReach")
            : error.kind === "database"
              ? t("notices.refused")
              : t("notices.failed"),
        detail: [message, detail].filter(Boolean).join("\n"),
      }),
      detail: [message, detail].filter(Boolean).join("\n"),
    });
    return null;
  } finally {
    state.setConnectingTo(null);
  }
}

export async function refreshCatalog(connectionId: string) {
  const state = useApp.getState();
  state.updateConnection(connectionId, { browsing: true, catalogError: null });
  try {
    const catalog = await api.loadCatalog(connectionId);
    state.updateConnection(connectionId, { catalog, browsing: false });
  } catch (failure) {
    const error = asAppError(failure);
    state.updateConnection(connectionId, {
      browsing: false,
      catalogError: error.message,
    });
  }
}

export async function closeConnection(connection: Connection) {
  const state = useApp.getState();
  try {
    await api.disconnect(connection.id);
  } catch {
    // The session may already be gone (server restarted); removing it from the
    // UI is still correct.
  }
  state.removeConnection(connection.id);
}

/** Ends the workspace that is using this saved connection; the saved entry
 *  itself (and its stored password) stays. */
export async function disconnectSaved(connection: Connection | null, savedId: string) {
  if (connection) await closeConnection(connection);
  // A workspace opened by hand before the entry was saved is the same
  // connection but carries no `savedId`.
  for (const item of useApp.getState().connections) {
    if (item.savedId === savedId) await closeConnection(item);
  }
}

/** Deletes a saved connection and its stored password. A session that is
 *  already open stays open, detached from the entry that no longer exists.
 *  Resolves to whether the list still holds the entry: a delete that landed
 *  after a stale dialog was opened for the same id would otherwise remove
 *  two servers. */
export async function removeSavedConnection(id: string): Promise<boolean> {
  // The backend's list (not the store's) is the truth after a second delete.
  const remaining = await api.deleteConnection(id);
  const state = useApp.getState();
  state.setSavedConnections(remaining);
  for (const connection of state.connections) {
    if (connection.savedId === id) state.updateConnection(connection.id, { savedId: null });
  }
  return remaining.some((entry) => entry.id === id);
}

export type RunOutcome = {
  ok: boolean;
  cancelled?: boolean;
};

/**
 * Runs `sql` on a tab's connection and writes the outcome onto the tab.
 * Partial results are preserved when a later statement fails, which is what
 * makes "run script" feel safe.
 */
export async function runSql(tabId: string, sql: string): Promise<RunOutcome> {
  const state = useApp.getState();
  const tab = state.tabs.find((item) => item.id === tabId);
  if (!tab) return { ok: false };
  if (!tab.connectionId) {
    state.pushToast({
      tone: "warn",
      retitle: (t) => ({
        title: t("notices.notConnectedTitle"),
        detail: t("notices.notConnectedDetail"),
      }),
    });
    return { ok: false };
  }
  if (sql.trim().length === 0) {
    state.pushToast({
      tone: "info",
      retitle: (t) => ({ title: t("notices.nothingToRun") }),
    });
    return { ok: false };
  }

  // A statement already running on this session is refused by the backend; the
  // toast explains why instead of silently doing nothing.
  const otherRunning = state.tabs.some(
    (item) => item.connectionId === tab.connectionId && item.result?.status === "running",
  );
  if (otherRunning) {
    state.pushToast({
      tone: "warn",
      retitle: (t) => ({
        title: t("notices.busyTitle"),
        detail: t("notices.busyDetail"),
      }),
    });
    return { ok: false };
  }

  const startedAt = Date.now();
  state.updateTab(tabId, {
    result: { status: "running", results: [], error: null, startedAt },
    activeResult: 0,
  });

  try {
    const result = await api.execute(tab.connectionId, sql);
    const current = useApp.getState().tabs.find((item) => item.id === tabId);
    // The tab may have been closed while the query ran.
    if (!current) return { ok: true };
    useApp.getState().updateTab(tabId, {
      result: {
        status: "done",
        results: result.results,
        error: null,
        startedAt,
        finishedAt: Date.now(),
        totalDurationMs: result.totalDurationMs,
      },
    });
    useApp.getState().updateConnection(tab.connectionId, {
      txStatus: result.txStatus,
    });
    return { ok: true };
  } catch (failure) {
    const error = asAppError(failure);
    const { message, detail } = describeError(error);
    const cancelled = error.kind === "cancelled";
    const current = useApp.getState().tabs.find((item) => item.id === tabId);
    if (!current) return { ok: false, cancelled };
    const connectionId = current.connectionId;
    // The panel under the grid shows the error; a toast makes sure the user
    // notices it even when the editor still has the focus.
    state.pushToast({
      tone: "error",
      retitle: (t) => ({
        title:
          error.kind === "cancelled"
            ? t("error.cancelled")
            : t("notices.failedWhy", { reason: message }),
      }),
      detail: error.kind === "cancelled" ? undefined : detail,
    });
    useApp.getState().updateTab(tabId, {
      result: {
        status: cancelled ? "cancelled" : "error",
        results: error.results ?? [],
        error: {
          message: error.message,
          detail: typeof error.detail === "string" ? error.detail : undefined,
          code: error.code,
          position: error.position,
          statementIndex: error.statementIndex,
          rows: error.rows ? { ...error.rows, exhausted: true } : undefined,
          /** Placeholder values, so the error panel can translate the message. */
          args: (error as AppError).args,
          kind: error.kind,
        },
        startedAt,
        finishedAt: Date.now(),
        totalDurationMs: error.resultsDurationMs,
      },
    });
    // A statement that fails or is cancelled can still change the session:
    // an aborted transaction, an open one, or a dropped socket.
    if (connectionId) void refreshStatus(connectionId);
    return { ok: false, cancelled };
  }
}

/** Re-reads a session's transaction state after a statement changed it. */
export async function refreshStatus(connectionId: string) {
  try {
    const status = await api.connectionStatus(connectionId);
    const state = useApp.getState();
    if (!status) {
      state.removeConnection(connectionId);
      return;
    }
    state.updateConnection(connectionId, {
      txStatus: status.txStatus,
      backendPid: status.backendPid,
    });
  } catch {
    // A closed session is reported by the next statement instead.
  }
}

export async function cancelQuery(connectionId: string) {
  try {
    await api.cancelQuery(connectionId);
  } catch (failure) {
    const error = asAppError(failure);
    if (error.kind !== "noop") {
      useApp.getState().pushToast({
        tone: "error",
        retitle: (t) => ({ title: t("notices.couldNotCancel") }),
        detail: error.message,
      });
    }
  }
}

/** `schema.table`, the scope spelling the backend matches against. */
export function tableKey(schema: string, relation: string) {
  return `${schema}.${relation}`;
}

/** `public."order items"` → `public.order_items`: a file name, dot kept. */
function sanitize(qualified: string) {
  return qualified.replace(/[^A-Za-z0-9_.-]/g, "_").replace(/\.{2,}/g, ".");
}

/**
 * Writes a dump to a file the user picks. `scope` is null for the whole
 * database; otherwise only those tables (and the structure they need) are
 * exported. Resolves to the path written, or null when the save panel was
 * dismissed or the export failed (a toast says why).
 */
export async function exportDump({
  connectionId,
  scope,
  includeData,
  format,
}: {
  connectionId: string;
  scope: string[] | null;
  includeData: boolean;
  format: DumpOptions["format"];
}): Promise<string | null> {
  const state = useApp.getState();
  const connection = state.connections.find((item) => item.id === connectionId);
  if (!connection) return null;

  const database = connection.database;
  let name = sanitize(database);
  const suffix = includeData ? "" : "_schema";
  const extension = format === "zip" ? "zip" : "sql";
  if (scope && scope.length === 1) {
    // One table names the file after itself; several fall back to the count.
    name = sanitize(scope[0]);
  } else if (scope) {
    name = `${sanitize(database)}_${scope.length}_tables`;
  }
  const fileName = `${name}${suffix}.${extension}`;

  const exportId = `export-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  let unlisten: (() => void) | null = null;
  try {
    // The dialog shows the count while tables stream; the call resolves only
    // once every table has been read.
    unlisten = await api.onDumpProgress((progress) => {
      if (progress.exportId !== exportId) return;
      state.updateConnection(connectionId, {
        exporting: { done: progress.done, total: progress.total },
      });
    });
    state.updateConnection(connectionId, { exporting: { done: 0, total: 0 } });
    const dump = await api.exportSql(connectionId, exportId, scope, {
      database,
      format,
      includeData,
    });
    unlisten?.();
    unlisten = null;
    state.updateConnection(connectionId, { exporting: null });
    const t = translateTo(state.language);
    const path = await api.saveFile(
      format === "zip" ? t("export.saveZipTitle") : t("export.saveSqlTitle"),
      fileName,
      dump.content,
    );
    if (!path) return null;
    const size =
      dump.bytes >= 1024 * 1024
        ? `${(dump.bytes / (1024 * 1024)).toFixed(1)} MB`
        : `${Math.max(1, Math.round(dump.bytes / 1024))} KB`;
    state.pushToast({
      tone: "success",
      retitle: (t) => ({
        title: t("export.savedTitle"),
        detail: `${path}\n${size}${
          dump.tables > 0
            ? ` · ${t("export.tablesOfData", { count: dump.tables })}`
            : ` · ${t("export.structureOnly")}`
        }`,
      }),
    });
    return path;
  } catch (failure) {
    const error = asAppError(failure);
    if (error.kind !== "cancelled") {
      state.pushToast({
        tone: "error",
        retitle: (t) => ({
          title: t("export.failed"),
          detail: [error.message, typeof error.detail === "string" ? error.detail : undefined]
            .filter(Boolean)
            .join("\n"),
        }),
      });
    }
    return null;
  } finally {
    unlisten?.();
    useApp.getState().updateConnection(connectionId, { exporting: null });
  }
}

export async function disconnectAll() {
  const state = useApp.getState();
  await Promise.allSettled(state.connections.map((connection) => api.disconnect(connection.id)));
}
