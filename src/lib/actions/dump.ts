import { api, asAppError, type AppError, type DumpOptions, type ImportOutcome } from "@/lib/ipc";
import { describeError } from "@/lib/errors";
import { translateTo, useApp } from "@/lib/store";

import { refreshCatalog } from "./connection";

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
