import { api, asAppError, type AppError } from "@/lib/ipc";
import { describeError } from "@/lib/errors";
import { useApp } from "@/lib/store";

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
