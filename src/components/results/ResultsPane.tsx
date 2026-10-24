import { useEffect, useState } from "react";
import { Check, Copy, OctagonX, TriangleAlert } from "lucide-react";
import { api, type StatementResult } from "@/lib/ipc";
import { cn } from "@/lib/cn";
import {
  describeError,
  errorDetail,
  errorMessage,
  translateToken,
  type Translated,
} from "@/lib/errors";
import { useT } from "@/lib/i18n";
import { formatCount, formatDuration } from "@/lib/platform";
import type { Connection, ResultState, Tab } from "@/lib/store";
import { useApp } from "@/lib/store";
import { ResultGrid } from "@/components/results/ResultGrid";

const DEFAULT_PAGE = 1000;

export function ResultsPane({ tab }: { tab: Tab }) {
  const connection = useApp((state) =>
    state.connections.find((item) => item.id === tab.connectionId),
  );
  const updateTab = useApp((state) => state.updateTab);
  const pageSize = useApp((state) => state.settings?.pageSize ?? DEFAULT_PAGE);
  const pushToast = useApp((state) => state.pushToast);
  const t = useT();
  const result = tab.result;

  const [loadingMore, setLoadingMore] = useState(false);
  const [copiedError, setCopiedError] = useState(false);

  if (!result) return <EmptyResults connection={connection ?? null} />;
  if (result.status === "running") return <RunningState startedAt={result.startedAt} />;

  const activeIndex = Math.min(
    Math.max(tab.activeResult, 0),
    Math.max(result.results.length - 1, 0),
  );
  const active = result.results[activeIndex];
  const error = result.error;

  // 只有确实经本客户端执行过的语句才能分页；返回行数不足一页，
  // 说明没有更多可取了。
  const canLoadMore =
    !!active?.hasRows &&
    !active.truncated &&
    !active.exhausted &&
    (active.rows?.length ?? 0) >= pageSize;

  const loadMore = async () => {
    if (!connection || !active || loadingMore) return;
    const startedAt = Date.now();
    const offset = active.rows?.length ?? 0;
    setLoadingMore(true);
    try {
      const page = await api.fetchMore(connection.id, active.sql, offset, pageSize);
      const current = useApp.getState().tabs.find((item) => item.id === tab.id)?.result;
      if (!current) return;
      updateTab(tab.id, {
        result: {
          ...current,
          results: current.results.map((item, index) =>
            index === activeIndex
              ? {
                  ...item,
                  rows: [...(item.rows ?? []), ...page.rows],
                  // 服务器说这一页就是最后一页。
                  exhausted: page.exhausted,
                  // 后续这条语句也要花时间，保持可见。
                  durationMs: item.durationMs + (Date.now() - startedAt),
                }
              : item,
          ),
        },
      });
      if (page.rows.length === 0) {
        pushToast({ tone: "info", retitle: (t) => ({ title: t("results.thatIsAllRows") }) });
      }
    } catch (failure) {
      pushToast({
        tone: "error",
        retitle: () => ({ title: t("results.couldNotLoadMore"), detail: describeError(failure).message }),
      });
    } finally {
      setLoadingMore(false);
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col bg-surface">
      {(result.results.length > 1 || (error && result.results.length > 0)) && (
        <div className="flex h-[30px] shrink-0 items-stretch overflow-x-auto border-b border-line bg-chrome px-2">
          {result.results.map((statement, index) => (
            <ResultTab
              key={index}
              index={index}
              statement={statement}
              active={index === activeIndex}
              onSelect={() => updateTab(tab.id, { activeResult: index })}
            />
          ))}
          {error && (
            <div
              className={cn(
                "flex shrink-0 items-center gap-1.5 px-2.5 text-[11.5px]",
                error.kind === "cancelled" ? "text-ink-soft" : "text-danger-ink",
              )}
            >
              <OctagonX className="size-3" />
              {t(error.kind === "cancelled" ? "results.cancelledAt" : "results.stoppedAt", {
                index: (error.statementIndex ?? result.results.length) + 1,
              })}
            </div>
          )}
          <div className="ml-auto flex shrink-0 items-center pr-2 text-[11px] text-ink-muted">
            <span className="tabular">
              {formatDuration(
                result.totalDurationMs ??
                  (result.finishedAt ?? Date.now()) - result.startedAt,
              )}
            </span>
          </div>
        </div>
      )}

      <div className="flex min-h-0 flex-1 flex-col">
        {error ? (
          <ErrorPanel
            error={error}
            copied={copiedError}
            onCopy={() => {
              const text = [
                errorMessage({ ...error, args: error.args }),
                errorDetail({ ...error, args: error.args }),
                error.code ? `SQLSTATE ${error.code}` : "",
              ]
                .filter(Boolean)
                .join("\n");
              void navigator.clipboard.writeText(text);
              setCopiedError(true);
              window.setTimeout(() => setCopiedError(false), 1200);
            }}
          />
        ) : active?.hasRows && active.columns ? (
          <>
            {active.notices && active.notices.length > 0 && (
              <Notices notices={active.notices} />
            )}
            <div className="h-full min-h-0">
              <ResultGrid
                key={activeIndex}
                columns={active.columns}
                rows={active.rows ?? []}
                truncated={active.truncated}
                canLoadMore={canLoadMore}
                loadingMore={loadingMore}
                loadedCount={pageSize}
                onLoadMore={loadMore}
              />
            </div>
          </>
        ) : active ? (
          <CommandResult statement={active} />
        ) : (
          <div className="flex h-full items-center justify-center text-[12.5px] text-ink-muted">
            {t("results.nothingReturned")}
          </div>
        )}
      </div>
    </div>
  );
}

function ResultTab({
  index,
  statement,
  active,
  onSelect,
}: {
  index: number;
  statement: StatementResult;
  active: boolean;
  onSelect: () => void;
}) {
  const t = useT();
  const label = statement.hasRows
    ? t("common.rows", {
        count: `${formatCount(statement.rows?.length ?? 0)}${statement.truncated ? "+" : ""}`,
      })
    : (statement.tag ?? t("results.result"));
  return (
    <button
      onClick={onSelect}
      className={cn(
        "relative flex shrink-0 items-center gap-1.5 px-2.5 text-[11.5px] transition-colors",
        active ? "text-ink" : "text-ink-muted hover:text-ink-soft",
      )}
    >
      <span className="text-ink-faint tabular">{index + 1}</span>
      <span className="tabular">{label}</span>
      {active && <span className="absolute inset-x-1.5 bottom-0 h-[2px] rounded-t bg-accent" />}
    </button>
  );
}

function CommandResult({ statement }: { statement: StatementResult }) {
  const t = useT();
  const tag = statement.tag ?? t("results.commandCompleted");
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2">
      <div className="flex items-center gap-2 text-[13px] text-ink">
        <span className="flex size-5 items-center justify-center rounded-full bg-accent-wash text-accent-ink">
          <Check className="size-3" strokeWidth={2.5} />
        </span>
        <span className="font-medium">{tag}</span>
      </div>
      <p className="text-[11.5px] text-ink-muted">{formatDuration(statement.durationMs)}</p>
    </div>
  );
}

/** 语句执行期间服务器发出的 NOTICE 与 WARNING，按服务器顺序排列。 */
function Notices({ notices }: { notices: Translated[] }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const shown = open ? notices : notices.slice(0, 2);
  return (
    <div className="shrink-0 border-b border-line bg-warn-wash/60 px-3 py-1.5">
      {shown.map((notice, index) => (
        <p
          key={index}
          className="flex items-start gap-1.5 py-[1px] font-mono text-[11px] leading-relaxed text-ink-soft"
        >
          <TriangleAlert className="mt-[3px] size-3 shrink-0 text-warn" />
          <span className="min-w-0 break-words">{translateToken(notice)}</span>
        </p>
      ))}
      {notices.length > 2 && (
        <button
          onClick={() => setOpen((value) => !value)}
          className="mt-0.5 text-[11px] text-accent-ink hover:underline"
        >
          {open
            ? t("results.showFewer")
            : t("results.showMoreNotices", { count: notices.length - 2 })}
        </button>
      )}
    </div>
  );
}

function ErrorPanel({
  error,
  partial,
  copied,
  onCopy,
}: {
  error: NonNullable<ResultState["error"]>;
  /** 失败的那条语句此前已经收到的行。 */
  partial?: React.ReactNode;
  copied: boolean;
  onCopy: () => void;
}) {
  const t = useT();
  const muted = error.kind === "cancelled";
  const message = errorMessage(error);
  const detail = errorDetail(error);
  return (
    <div className="h-full overflow-auto px-4 py-4">
      <div
        className={cn(
          "mx-auto max-w-[720px] rounded-[var(--radius-md)] border",
          muted
            ? "border-line bg-chrome"
            : "border-[color-mix(in_oklab,var(--danger)_28%,var(--line))] bg-danger-wash",
        )}
      >
        <div className="flex items-start gap-2.5 px-3.5 py-3">
          <TriangleAlert
            className={cn(
              "mt-[2px] size-[15px] shrink-0",
              muted ? "text-ink-muted" : "text-danger-ink",
            )}
          />
          <div className="min-w-0 flex-1">
            <p className="min-w-0 break-words text-[13px] font-medium leading-snug text-ink">{message}</p>
            {detail && (
              <p className="mt-1.5 whitespace-pre-wrap break-words font-mono text-[11.5px] leading-relaxed text-ink-soft">
                {detail}
              </p>
            )}
            <div className="mt-2.5 flex items-center gap-3 text-[11px] text-ink-muted">
              {error.code && (
                <span className="rounded border border-line-strong bg-raised px-1.5 py-[1px] font-mono text-[10.5px]">
                  {error.code}
                </span>
              )}
              {error.position !== undefined && <span>{t("results.position", { position: error.position })}</span>}
              <button
                onClick={onCopy}
                className="ml-auto flex items-center gap-1 rounded px-1.5 py-0.5 hover:bg-hover hover:text-ink"
              >
                {copied ? <Check className="size-3 text-ok" /> : <Copy className="size-3" />}
                {copied ? t("common.copied") : t("common.copy")}
              </button>
            </div>
          </div>
        </div>
      </div>
      {partial}
    </div>
  );
}

function RunningState({ startedAt }: { startedAt: number }) {
  const t = useT();
  const [elapsed, setElapsed] = useState(Date.now() - startedAt);
  useEffect(() => {
    const timer = window.setInterval(() => setElapsed(Date.now() - startedAt), 100);
    return () => window.clearInterval(timer);
  }, [startedAt]);
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2.5">
      <div className="flex items-center gap-2 text-[12.5px] text-ink-soft">
        <span className="size-3 animate-spin rounded-full border-[1.5px] border-ink-faint border-r-transparent" />
        <span className="tabular">{formatDuration(elapsed)}</span>
      </div>
      <p className="text-[11.5px] text-ink-faint">{t("results.pressEscToCancel")}</p>
    </div>
  );
}

function EmptyResults({ connection }: { connection: Connection | null }) {
  const t = useT();
  return (
    <div className="flex h-full flex-col items-center justify-center gap-1 bg-surface text-center">
      <p className="text-[12.5px] text-ink-soft">
        {connection
          ? t("results.connectedTo", { database: connection.database })
          : t("results.noConnection")}
      </p>
      <p className="text-[11.5px] text-ink-faint">{t("results.willAppearHere")}</p>
    </div>
  );
}
