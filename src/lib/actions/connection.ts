import {
  api,
  asAppError,
  type ConnectParams,
  type DatabaseInfo,
  type SavedConnection,
} from "@/lib/ipc";
import { describeError } from "@/lib/errors";
import { useApp, type Connection } from "@/lib/store";

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

export async function disconnectAll() {
  const state = useApp.getState();
  await Promise.allSettled(state.connections.map((connection) => api.disconnect(connection.id)));
}
