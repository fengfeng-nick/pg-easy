import { useEffect, useRef, useState } from "react";
import { FileCode2, LoaderCircle, Upload } from "lucide-react";
import { api, type PickedSqlFile } from "@/lib/ipc";
import { importSqlFile } from "@/lib/actions";
import { describeError } from "@/lib/errors";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { Toggle } from "@/components/ui/Field";

/** 一次导入的进度：已经跑到第几条语句、写入了多少行。 */
type Progress = { statements: number; total: number; rows: number };

/** 导入结束后留在对话框里的结果，成功时对话框直接关闭。 */
type Result = {
  tone: "error" | "warn";
  title: string;
  detail?: string;
};

/** 把 .sql 文件回放到当前连接：选文件、确认选项、看着它跑完。 */
export function ImportDialog() {
  const request = useApp((state) => state.importRequest);
  const openImport = useApp((state) => state.openImport);
  const connection = useApp((state) =>
    state.connections.find((item) => item.id === state.importRequest?.connectionId),
  );
  const pushToast = useApp((state) => state.pushToast);
  const t = useT();

  const [file, setFile] = useState<PickedSqlFile | null>(null);
  const [inTransaction, setInTransaction] = useState(true);
  const [picking, setPicking] = useState(false);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [result, setResult] = useState<Result | null>(null);
  /** 回调里读到的进度不能依赖渲染时的闭包：事件不会触发重新渲染。 */
  const latest = useRef<Progress | null>(null);

  const track = (next: Progress) => {
    latest.current = next;
    setProgress(next);
  };

  // 每次打开都从干净状态开始，上一次的选择和报错不跟过来。
  useEffect(() => {
    if (!request) return;
    setFile(null);
    setInTransaction(true);
    setPicking(false);
    setRunning(false);
    setProgress(null);
    setResult(null);
    latest.current = null;
  }, [request]);

  if (!request || !connection) return null;

  const close = () => {
    if (!running) openImport(null);
  };

  const pick = async () => {
    setPicking(true);
    try {
      const picked = await api.pickSqlFile(t("import.pickTitle"));
      if (picked) {
        setFile(picked);
        setResult(null);
      }
    } catch (failure) {
      const { message, detail } = describeError(failure);
      setResult({
        tone: "error",
        title: t("import.unreadable"),
        detail: [message, detail].filter(Boolean).join("\n"),
      });
    } finally {
      setPicking(false);
    }
  };

  const start = async () => {
    if (!file) return;
    setRunning(true);
    setResult(null);
    track({ statements: 0, total: 0, rows: 0 });

    const run = await importSqlFile({
      connectionId: connection.id,
      path: file.path,
      inTransaction,
      onProgress: track,
    });
    setRunning(false);

    if (run.outcome) {
      const { statements, rows } = run.outcome;
      pushToast({
        tone: "success",
        retitle: (t) => ({
          title: t("import.doneTitle"),
          detail: t("import.doneDetail", { statements, rows }),
        }),
      });
      openImport(null);
      return;
    }

    if (run.cancelled) {
      setResult({
        tone: "warn",
        title: t("import.stoppedTitle"),
        detail: stopDetail(t, latest.current?.statements, inTransaction),
      });
      return;
    }

    setResult({
      tone: "error",
      title: t("import.failedTitle"),
      detail: [failureLine(t, run.error, inTransaction), run.error?.message, run.error?.detail]
        .filter(Boolean)
        .join("\n"),
    });
  };

  const stop = () => {
    void api.cancelQuery(connection.id);
  };

  const sharing = progress !== null && progress.total > 0;
  const percent =
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.statements / progress.total) * 100))
      : 0;

  return (
    <Dialog
      open
      onClose={close}
      dismissible={!running}
      title={t("import.title")}
      description={
        <span className="block truncate">
          {connection.name}
          <span className="text-ink-faint">
            {" · "}
            {connection.user}@{connection.host}/{connection.database}
          </span>
        </span>
      }
      width={520}
      footer={
        <>
          <span className="mr-auto text-[11.5px] text-ink-muted">
            {running
              ? progress
                ? t("import.progress", {
                    statements: progress.statements,
                    total: progress.total,
                    rows: progress.rows,
                  })
                : t("import.running")
              : file
                ? t(inTransaction ? "import.inTransactionHint" : "import.noTransactionHint")
                : t("import.noFile")}
          </span>
          <Button variant="ghost" onClick={close} disabled={running}>
            {result ? t("import.close") : t("common.cancel")}
          </Button>
          {running ? (
            <Button variant="secondary" onClick={stop} icon={<StopIcon />}>
              {t("import.stop")}
            </Button>
          ) : (
            <Button
              data-autofocus
              variant="primary"
              onClick={() => void start()}
              disabled={!file || picking}
              icon={picking ? <LoaderCircle className="animate-spin" /> : undefined}
            >
              {t("import.run")}
            </Button>
          )}
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="rounded-[var(--radius-md)] border border-line bg-raised px-3.5 py-3">
          <div className="flex items-start gap-2.5">
            <FileCode2 className="mt-px size-4 shrink-0 text-accent" />
            <div className="min-w-0 flex-1">
              <p className="truncate text-[12.5px] font-medium text-ink">
                {file ? file.name : t("import.noFile")}
              </p>
              <p className="mt-1 text-[11.5px] leading-snug text-ink-muted">
                {file ? (
                  <>
                    <span className="tabular">{formatBytes(file.bytes)}</span>
                    <span className="mx-1.5 text-ink-faint">·</span>
                    <span className="break-all text-ink-faint">{file.path}</span>
                  </>
                ) : (
                  t("import.fileHint")
                )}
              </p>
            </div>
            <Button
              size="sm"
              variant="secondary"
              onClick={() => void pick()}
              disabled={running || picking}
              loading={picking}
              icon={<Upload />}
            >
              {file ? t("import.changeFile") : t("import.chooseFile")}
            </Button>
          </div>

          {running && (
            <div className="mt-3 h-[3px] overflow-hidden rounded-full bg-active">
              <div
                className="h-full rounded-full bg-accent transition-[width] duration-200 ease-out"
                style={{ width: sharing ? `${Math.max(3, percent)}%` : "30%" }}
              />
            </div>
          )}
        </div>

        <Toggle
          checked={inTransaction}
          onChange={setInTransaction}
          label={t("import.inTransaction")}
          description={t(inTransaction ? "import.inTransactionHint" : "import.noTransactionHint")}
        />

        {result && (
          <div
            className={
              result.tone === "error"
                ? "rounded-[var(--radius-md)] border border-danger/40 bg-danger-wash px-3.5 py-2.5"
                : "rounded-[var(--radius-md)] border border-warn/40 bg-warn-wash px-3.5 py-2.5"
            }
          >
            <p
              className={
                result.tone === "error"
                  ? "text-[12.5px] font-medium text-danger-ink"
                  : "text-[12.5px] font-medium text-warn"
              }
            >
              {result.title}
            </p>
            {result.detail && (
              <pre className="mt-1.5 max-h-[140px] overflow-auto whitespace-pre-wrap break-words font-sans text-[11.5px] leading-relaxed text-ink-soft">
                {result.detail}
              </pre>
            )}
          </div>
        )}
      </div>
    </Dialog>
  );
}

/** 停止后说明停在哪条语句、之前的改动是否还在。 */
function stopDetail(
  t: ReturnType<typeof useT>,
  statements: number | undefined,
  inTransaction: boolean,
) {
  const index = statements ?? 0;
  return t(inTransaction ? "import.stoppedAtomic" : "import.stoppedPartial", {
    index: index + 1,
  });
}

/** 失败时先说明是哪一条语句，再说服务器自己的话。 */
function failureLine(
  t: ReturnType<typeof useT>,
  error: { statementIndex?: number } | undefined,
  inTransaction: boolean,
) {
  if (!error) return undefined;
  if (error.statementIndex === undefined) return t("import.failedNoIndex");
  return t(inTransaction ? "import.failedAtomic" : "import.failedPartial", {
    index: error.statementIndex + 1,
  });
}

function formatBytes(bytes: number) {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} B`;
}

/** 停止图标：方框，和工具栏里的停止按钮同一套语言。 */
function StopIcon() {
  return (
    <svg viewBox="0 0 14 14" className="size-[13px]" fill="none" aria-hidden>
      <rect x="3.5" y="3.5" width="7" height="7" rx="1.2" fill="currentColor" />
    </svg>
  );
}
