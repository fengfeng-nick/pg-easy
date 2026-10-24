import {
  api,
  asAppError,
  type AppError,
  type ConnectParams,
  type DatabaseInfo,
  type DumpOptions,
  type ImportOutcome,
  type SavedConnection,
} from "@/lib/ipc";
import { describeError } from "@/lib/errors";
import { translateTo, useApp, type Connection } from "@/lib/store";

/** 本次启动中打开的会话。重新保存密码时应该用它重连，而不是留下一个
 *  已经过期的会话。 */
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
        // 只有一句笼统的标题没法采取行动；详细说明才给出原因
        //（密码错误、服务器关闭、TLS 被拒）。
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

/** 列出这台服务器上的数据库，供侧边栏切换。失败时把错误交给调用方显示。 */
export async function listDatabases(connectionId: string): Promise<DatabaseInfo[]> {
  const overview = await api.browse(connectionId);
  return overview.databases;
}

/**
 * 把已打开的会话换到另一个库。会话 id 不变，所以标签页还挂在它上面；
 * 换过去之后重新读目录，树里显示新库的表。
 */
export async function switchDatabase(connectionId: string, database: string): Promise<boolean> {
  const state = useApp.getState();
  const current = state.connections.find((connection) => connection.id === connectionId);
  if (!current || current.database === database) return false;

  // 旧库的表在新库里不存在。先收起打开着的表视图，再换库。
  if (state.view.kind === "table" && state.view.connectionId === connectionId) {
    state.setView({ kind: "editor" });
  }
  state.updateConnection(connectionId, {
    database,
    browsing: true,
    catalog: null,
    catalogError: null,
  });
  try {
    const info = await api.switchDatabase(connectionId, database);
    useApp.getState().updateConnection(connectionId, {
      database,
      serverVersion: info.serverVersion,
      backendPid: info.backendPid,
      txStatus: info.txStatus,
    });
    void refreshCatalog(connectionId);
    return true;
  } catch (failure) {
    const error = asAppError(failure);
    const { message, detail } = describeError(error);
    // 登录被拒时后端还留着旧会话，把界面上的库名改回去。
    // 登录被拒时后端还留着旧会话。库名改回去，再把刚才清掉的目录读回来，
    // 否则侧边栏会一直空着，连重试的入口都没有。
    useApp.getState().updateConnection(connectionId, { database: current.database });
    void refreshCatalog(connectionId);
    useApp.getState().pushToast({
      tone: "error",
      retitle: (t) => ({
        title: t("sidebar.switchFailed", { database }),
        detail: [message, detail].filter(Boolean).join("\n"),
      }),
      detail: [message, detail].filter(Boolean).join("\n"),
    });
    return false;
  }
}

export async function closeConnection(connection: Connection) {
  const state = useApp.getState();
  try {
    await api.disconnect(connection.id);
  } catch {
    // 会话可能已经不在了（服务器重启过）；把它从界面上移除仍然是对的。
  }
  state.removeConnection(connection.id);
}

/** 结束正在使用这条已保存连接的会话；保存的条目本身（以及保存的密码）
 *  仍然保留。 */
export async function disconnectSaved(connection: Connection | null, savedId: string) {
  if (connection) await closeConnection(connection);
  // 在条目保存之前手动打开的会话是同一条连接，但没有 `savedId`。
  for (const item of useApp.getState().connections) {
    if (item.savedId === savedId) await closeConnection(item);
  }
}

/** 删除一条已保存连接及其保存的密码。已经打开的会话保持打开，只是与那个
 *  不再存在的条目脱钩。返回列表里是否还持有该条目：否则对一个同 id 的
 *  过期对话框执行删除，会顺带删掉两台服务器。 */
export async function removeSavedConnection(id: string): Promise<boolean> {
  // 第二次删除之后，以后端返回的列表（而不是 store 里的）为准。
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
 * 在标签页的连接上执行 `sql`，并把结果写回标签页。后面某条语句失败时，
 * 前面已经拿到的部分结果会保留，这正是“执行脚本”让人放心的原因。
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

  // 这条会话上已有语句在跑时后端会拒绝；用 toast 说明原因，
  // 而不是默默什么都不做。
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
    // 查询执行期间标签页可能已经被关闭。
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
    // 表格下面的面板会显示错误；再加一个 toast，保证编辑器仍有焦点时
    // 用户也能注意到。
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
          /** 占位值，好让错误面板能翻译这条消息。 */
          args: (error as AppError).args,
          kind: error.kind,
        },
        startedAt,
        finishedAt: Date.now(),
        totalDurationMs: error.resultsDurationMs,
      },
    });
    // 失败或被取消的语句同样会改变会话状态：事务被中止、事务处于打开
    // 状态，或者 socket 已经断开。
    if (connectionId) void refreshStatus(connectionId);
    return { ok: false, cancelled };
  }
}

/** 语句改变会话状态后，重新读取它的事务状态。 */
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
    // 已关闭的会话由下一条语句来报告。
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

/** `schema.table`，后端用于匹配的作用域写法。 */
export function tableKey(schema: string, relation: string) {
  return `${schema}.${relation}`;
}

/** `public."order items"` → `public.order_items`：用作文件名，点保留。 */
function sanitize(qualified: string) {
  return qualified.replace(/[^A-Za-z0-9_.-]/g, "_").replace(/\.{2,}/g, ".");
}

/**
 * 把转储写到用户选定的文件。`scope` 为 null 表示整个数据库；否则只导出
 * 这些表（以及它们所需的结构）。返回写入的路径；保存面板被取消或导出
 * 失败（会有 toast 说明原因）时返回 null。
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
    // 一个表用表名命名文件；多个表则退回用数量。
    name = sanitize(scope[0]);
  } else if (scope) {
    name = `${sanitize(database)}_${scope.length}_tables`;
  }
  const fileName = `${name}${suffix}.${extension}`;

  const exportId = `export-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  let unlisten: (() => void) | null = null;
  try {
    // 各表流式导出期间对话框显示计数；调用要等所有表都读完才返回。
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

/** 导入的结局：成功时带统计，失败时带可展示的错误。 */
export type ImportRun = {
  outcome?: ImportOutcome;
  error?: {
    kind: AppError["kind"];
    message: string;
    detail?: string;
    /** 出错语句在文件中的序号（从 0 开始）。 */
    statementIndex?: number;
  };
  /** 用户按了停止。 */
  cancelled?: boolean;
};

/**
 * 把 `path` 指向的 .sql 文件回放到连接里。
 *
 * 进度通过 `onProgress` 回调逐批上报。成功后刷新目录：导入的多半是新表。
 * 失败时把结构化错误交回对话框，由它展示是第几条语句出的问题。
 */
export async function importSqlFile({
  connectionId,
  path,
  inTransaction,
  onProgress,
}: {
  connectionId: string;
  path: string;
  inTransaction: boolean;
  onProgress?: (progress: { statements: number; total: number; rows: number }) => void;
}): Promise<ImportRun> {
  const importId = `import-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  const unlisten = await api.onImportProgress((progress) => {
    if (progress.importId !== importId) return;
    onProgress?.({
      statements: progress.statements,
      total: progress.total,
      rows: progress.rows,
    });
  });
  try {
    const outcome = await api.importSql(connectionId, importId, path, { inTransaction });
    // 新表、新列都要出现在侧边栏里。
    void refreshCatalog(connectionId);
    return { outcome };
  } catch (failure) {
    const error = asAppError(failure);
    const { message, detail } = describeError(error);
    return {
      cancelled: error.kind === "cancelled",
      error: {
        kind: error.kind,
        message,
        detail,
        statementIndex: error.statementIndex,
      },
    };
  } finally {
    unlisten();
  }
}
