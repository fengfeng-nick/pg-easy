import { useEffect, useMemo, useState } from "react";
import { ArrowUpRight, Columns3, Copy, Download, KeyRound, LoaderCircle, RefreshCw } from "lucide-react";
import { api, type ColumnInfo, type StatementResult } from "@/lib/ipc";
import { describeError } from "@/lib/errors";
import { useT } from "@/lib/i18n";
import { tableKey } from "@/lib/actions";
import { cn } from "@/lib/cn";
import { formatCount, formatDuration } from "@/lib/platform";
import { useApp, type View } from "@/lib/store";
import { firstPageSql, quoteIdent, tablePageSql } from "@/lib/sql";
import { ResultGrid } from "@/components/results/ResultGrid";
import { Button, IconButton } from "@/components/ui/Button";

const PAGE = 500;

export function TableView({ view }: { view: Extract<View, { kind: "table" }> }) {
  const connection = useApp((state) =>
    state.connections.find((item) => item.id === view.connectionId),
  );
  const addTab = useApp((state) => state.addTab);
  const setView = useApp((state) => state.setView);
  const pushToast = useApp((state) => state.pushToast);
  const openExport = useApp((state) => state.openExport);
  const t = useT();

  const [columns, setColumns] = useState<ColumnInfo[] | null>(null);
  const [data, setData] = useState<StatementResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [showStructure, setShowStructure] = useState(false);
  const [reloadKey, setReloadKey] = useState(0);

  const relation = connection?.catalog?.relations[view.schema]?.find(
    (item) => item.name === view.relation,
  );
  const qualified = `${quoteIdent(view.schema)}.${quoteIdent(view.relation)}`;

  useEffect(() => {
    if (!connection) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    setData(null);
    Promise.all([
      api.loadColumns(connection.id, view.schema, view.relation),
      api.execute(connection.id, firstPageSql(view.schema, view.relation, PAGE)),
    ])
      .then(([loadedColumns, result]) => {
        if (cancelled) return;
        setColumns(loadedColumns);
        setData(result.results[0] ?? null);
      })
      .catch((failure) => {
        if (!cancelled) setError(describeError(failure).message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [connection?.id, view.schema, view.relation, reloadKey]);

  const orderBy = useMemo(() => {
    if (!columns) return "";
    const keys = columns.filter((column) => column.isPrimaryKey).map((column) => quoteIdent(column.name));
    return keys.length > 0 ? ` order by ${keys.join(", ")}` : "";
  }, [columns]);
  const primaryKey = useMemo(
    () => (columns ?? []).filter((column) => column.isPrimaryKey).map((column) => column.name),
    [columns],
  );

  const openInEditor = (sql: string, title: string) => {
    if (!connection) return;
    addTab({ connectionId: connection.id, sql, title });
    setView({ kind: "editor" });
  };

  if (!connection) {
    return <div className="p-6 text-[12.5px] text-ink-muted">{t("table.closed")}</div>;
  }

  return (
    <div className="flex h-full min-h-0 flex-col bg-surface">
      <header className="flex h-[38px] shrink-0 items-center gap-2 border-b border-line px-3">
        <div className="flex min-w-0 items-center gap-1.5 text-[12.5px]">
          <button
            onClick={() => setView({ kind: "editor" })}
            className="text-ink-muted hover:text-ink"
          >
            {connection.name}
          </button>
          <span className="text-ink-faint">/</span>
          <span className="text-ink-muted">{view.schema}</span>
          <span className="text-ink-faint">/</span>
          <span className="truncate font-medium text-ink">{view.relation}</span>
        </div>
        {relation && (
          <span className="shrink-0 text-[11px] text-ink-faint">
            {relation.kind === "table" ? "" : relation.kind}
            {relation.kind === "table" ? "" : " · "}
            {t("common.columnsShort", { count: formatCount(relation.columns) })} · {relation.size}
          </span>
        )}
        <div className="ml-auto flex items-center gap-1">
          <IconButton
            label={t("table.refresh")}
            size="sm"
            onClick={() => setReloadKey((key) => key + 1)}
            disabled={loading}
          >
            <RefreshCw className={cn(loading && "animate-spin")} />
          </IconButton>
          <Button
            size="sm"
            variant="ghost"
            icon={<Columns3 />}
            className={showStructure ? "bg-active text-ink" : undefined}
            onClick={() => setShowStructure((value) => !value)}
          >
            {t("table.columns")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            icon={<Download />}
            onClick={() =>
              openExport({
                connectionId: connection.id,
                schema: null,
                scope: [tableKey(view.schema, view.relation)],
              })
            }
          >
            {t("table.export")}
          </Button>
          <Button
            size="sm"
            variant="secondary"
            icon={<ArrowUpRight />}
            onClick={() =>
              openInEditor(`select * from ${qualified}${orderBy} limit 200;`, view.relation)
            }
          >
            {t("table.query")}
          </Button>
        </div>
      </header>

      {showStructure && (
        <StructurePanel columns={columns} loading={loading && !columns} />
      )}

      <div className="min-h-0 flex-1">
        {error ? (
          <div className="m-4 rounded-[var(--radius-md)] border border-[color-mix(in_oklab,var(--danger)_28%,var(--line))] bg-danger-wash px-3.5 py-3 text-[12.5px] text-ink">
            <p className="font-medium">{error}</p>
            <button
              onClick={() => setReloadKey((key) => key + 1)}
              className="mt-1.5 text-[11.5px] text-accent-ink hover:underline"
            >
              {t("table.tryAgain")}
            </button>
          </div>
        ) : loading ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 text-[12px] text-ink-muted">
            <LoaderCircle className="size-4 animate-spin" />
            {t("table.loadingRows")}
          </div>
        ) : data?.columns ? (
          <ResultGrid
            key={`${view.schema}.${view.relation}`}
            columns={data.columns}
            rows={data.rows ?? []}
            canLoadMore={(data.rows?.length ?? 0) >= PAGE}
            loadingMore={loadingMore}
            // 第一页不带排序读取；一旦开始分页，各页按主键排序，
            // 这样 offset 不会重复或跳过某一行。
            sortable={false}
            loadedCount={PAGE}
            onLoadMore={async () => {
              const rows = data.rows ?? [];
              const offset = rows.length;
              const startedAt = Date.now();
              setLoadingMore(true);
              try {
                const page = await api.execute(
                  connection.id,
                  tablePageSql(view.schema, view.relation, primaryKey, PAGE, offset),
                );
                const next = page.results[0];
                setData({
                  ...data,
                  rows: [...rows, ...(next?.rows ?? [])],
                  durationMs: data.durationMs + (Date.now() - startedAt),
                });
              } catch (failure) {
                pushToast({
                  tone: "error",
                  retitle: () => ({
                    title: t("results.couldNotLoadMore"),
                    detail: describeError(failure).message,
                  }),
                });
              } finally {
                setLoadingMore(false);
              }
            }}
          />
        ) : (
          <div className="flex h-full items-center justify-center text-[12.5px] text-ink-muted">
            {t("table.noColumns")}
          </div>
        )}
      </div>

      <footer className="flex h-[26px] shrink-0 items-center gap-3 border-t border-line bg-chrome px-3 text-[11px] text-ink-muted">
        <span className="tabular">
          {data ? t("common.rows", { count: formatCount(data.rows?.length ?? 0) }) : "—"}
        </span>
        <span className="text-ink-faint">·</span>
        <span className="tabular">{data ? formatDuration(data.durationMs) : ""}</span>
        <span className="ml-auto flex items-center gap-1 text-ink-faint">
          <Copy className="size-3" />
          {t("table.footerHint")}
        </span>
      </footer>
    </div>
  );
}

function StructurePanel({ columns, loading }: { columns: ColumnInfo[] | null; loading: boolean }) {
  const t = useT();
  if (loading) {
    return (
      <div className="flex h-[120px] items-center justify-center border-b border-line bg-chrome/50 text-[12px] text-ink-muted">
        <LoaderCircle className="mr-2 size-3.5 animate-spin" />
        {t("table.readingColumns")}
      </div>
    );
  }
  return (
    <div className="max-h-[180px] shrink-0 overflow-y-auto border-b border-line bg-chrome/50">
      <table className="w-full text-[11.5px]">
        <thead className="sticky top-0 bg-chrome text-ink-muted">
          <tr className="text-left">
            <th className="w-8 py-1.5 pl-3 font-medium">{t("table.headerIndex")}</th>
            <th className="py-1.5 font-medium">{t("table.headerColumn")}</th>
            <th className="py-1.5 font-medium">{t("table.headerType")}</th>
            <th className="py-1.5 font-medium">{t("table.headerNullable")}</th>
            <th className="py-1.5 font-medium">{t("table.headerDefault")}</th>
          </tr>
        </thead>
        <tbody className="font-mono">
          {(columns ?? []).map((column) => (
            <tr key={column.name} className="border-t border-line/70">
              <td className="py-1 pl-3 text-ink-faint tabular">{column.position}</td>
              <td className="py-1">
                <span className="flex items-center gap-1 text-ink">
                  {column.isPrimaryKey && <KeyRound className="size-3 text-warn" />}
                  {column.name}
                </span>
              </td>
              <td className="py-1 text-[var(--time)]">{column.dataType}</td>
              <td className="py-1 text-ink-muted">{column.isNullable ? t("common.yes") : t("common.no")}</td>
              <td className="max-w-[280px] truncate py-1 text-ink-muted">
                {column.defaultValue || "—"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
