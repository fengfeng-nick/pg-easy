import { useEffect, useMemo, useState, type ReactNode } from "react";
import { ArrowUpRight, Columns3, Copy, Download, KeyRound, LoaderCircle, Plus, RefreshCw, Trash2 } from "lucide-react";
import {
  api,
  type Column,
  type ColumnInfo,
  type RelationStructure,
  type StatementResult,
} from "@/lib/ipc";
import { describeError } from "@/lib/errors";
import { useT } from "@/lib/i18n";
import { tableKey } from "@/lib/actions";
import { cn } from "@/lib/cn";
import { formatCount, formatDuration } from "@/lib/platform";
import { useApp, type View } from "@/lib/store";
import { deleteRowSql, filterWhere, firstPageSql, insertRowSql, quoteIdent, tablePageSql, updateRowSql } from "@/lib/sql";
import { ResultGrid } from "@/components/results/ResultGrid";
import { Button, IconButton } from "@/components/ui/Button";

const PAGE = 500;

type DraftField = { text: string; isNull: boolean };
type RowEditorState = { mode: "insert" | "update"; draft: Record<string, DraftField> };

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
  const [structure, setStructure] = useState<RelationStructure | null>(null);
  const [data, setData] = useState<StatementResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [showStructure, setShowStructure] = useState(false);
  const [reloadKey, setReloadKey] = useState(0);
  const [filterText, setFilterText] = useState("");
  const [appliedFilter, setAppliedFilter] = useState("");
  const [editor, setEditor] = useState<RowEditorState | null>(null);
  const [editorError, setEditorError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [selectedRow, setSelectedRow] = useState<number | null>(null);
  const [pendingDelete, setPendingDelete] = useState(false);

  const relation = connection?.catalog?.relations[view.schema]?.find(
    (item) => item.name === view.relation,
  );
  const qualified = `${quoteIdent(view.schema)}.${quoteIdent(view.relation)}`;

  // 换表时在渲染里就把筛选和列清掉。放进 effect 的话，第一次取数
  // 还会拿着上一张表的列去拼条件。
  const scopeKey = `${connection?.id ?? ""}:${view.schema}:${view.relation}`;
  const [scopeSeen, setScopeSeen] = useState(scopeKey);
  if (scopeSeen !== scopeKey) {
    setScopeSeen(scopeKey);
    setFilterText("");
    setAppliedFilter("");
    setColumns(null);
    setStructure(null);
    setEditor(null);
    setEditorError(null);
    setSelectedRow(null);
    setPendingDelete(false);
  }

  const where = useMemo(
    () => filterWhere(appliedFilter, (columns ?? []).map((column) => column.name)),
    [appliedFilter, columns],
  );

  useEffect(() => {
    if (!connection) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    setData(null);
    const filter = filterWhere(appliedFilter, (columns ?? []).map((column) => column.name));
    Promise.all([
      api.loadStructure(connection.id, view.schema, view.relation),
      api.execute(
        connection.id,
        filter
          ? tablePageSql(view.schema, view.relation, [], PAGE, 0, filter)
          : firstPageSql(view.schema, view.relation, PAGE),
      ),
    ])
      .then(([loaded, result]) => {
        if (cancelled) return;
        setStructure(loaded);
        setColumns(loaded.columns);
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
  }, [connection?.id, view.schema, view.relation, reloadKey, appliedFilter]);

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

  const showDdl = async () => {
    if (!connection) return;
    try {
      const sql = await api.loadDdl(connection.id, view.schema, view.relation);
      openInEditor(sql, `${view.relation} DDL`);
    } catch (failure) {
      pushToast({
        tone: "error",
        retitle: (translate) => ({
          title: translate("table.couldNotLoadDdl"),
          detail: describeError(failure).message,
        }),
      });
    }
  };

  const newTable = () => {
    openInEditor(
      `create table ${quoteIdent(view.schema)}.name (\n  id bigint generated always as identity primary key\n);\n`,
      t("table.newTable"),
    );
  };

  const chooseRow = (row: number | null) => {
    setSelectedRow(row);
    setPendingDelete(false);
  };

  const rowValues = (name: string) => {
    const row = selectedRow === null ? null : data?.rows?.[selectedRow];
    if (!row || !data?.columns) return null;
    const index = data.columns.findIndex((column) => column.name === name);
    return index < 0 ? null : row[index] ?? null;
  };

  const beginInsert = () => {
    if (!columns) return;
    setEditorError(null);
    setEditor({ mode: "insert", draft: emptyDraft(columns) });
  };

  const beginEdit = () => {
    if (!columns || !data?.columns || selectedRow === null) return;
    const row = data.rows?.[selectedRow];
    if (!row) return;
    setEditorError(null);
    setPendingDelete(false);
    setEditor({ mode: "update", draft: draftFromRow(columns, data.columns, row) });
  };

  const saveEditor = async () => {
    if (!connection || !editor || !columns) return;
    setSaving(true);
    setEditorError(null);
    try {
      let sql: string;
      let done: "saved" | "inserted" = "saved";
      if (editor.mode === "insert") {
        const changes = columns
          .filter((column) => editor.draft[column.name] && !editor.draft[column.name].isNull)
          .map((column) => ({ column: column.name, value: editor.draft[column.name].text }));
        sql = changes.length
          ? insertRowSql(view.schema, view.relation, changes)
          : `insert into ${qualified} default values`;
        done = "inserted";
      } else {
        if (primaryKey.length === 0 || selectedRow === null) return;
        const changes = columns.flatMap((column) => {
          if (primaryKey.includes(column.name)) return [];
          const field = editor.draft[column.name];
          if (!field) return [];
          const next = field.isNull ? null : field.text;
          return next === rowValues(column.name) ? [] : [{ column: column.name, value: next }];
        });
        if (changes.length === 0) {
          setEditor(null);
          return;
        }
        sql = updateRowSql(
          view.schema,
          view.relation,
          primaryKey,
          primaryKey.map((name) => rowValues(name)),
          changes,
        );
      }
      await api.execute(connection.id, sql);
      setEditor(null);
      setSelectedRow(null);
      setReloadKey((key) => key + 1);
      pushToast({
        tone: "info",
        retitle: (translate) => ({
          title: translate(done === "inserted" ? "grid.inserted" : "grid.saved"),
        }),
      });
    } catch (failure) {
      setEditorError(describeError(failure).message);
    } finally {
      setSaving(false);
    }
  };

  const removeRow = async () => {
    if (!connection || primaryKey.length === 0 || selectedRow === null) return;
    if (!pendingDelete) {
      setPendingDelete(true);
      return;
    }
    setPendingDelete(false);
    try {
      await api.execute(
        connection.id,
        deleteRowSql(view.schema, view.relation, primaryKey, primaryKey.map((name) => rowValues(name))),
      );
      setEditor(null);
      setSelectedRow(null);
      setReloadKey((key) => key + 1);
      pushToast({ tone: "info", retitle: (translate) => ({ title: translate("grid.deleted") }) });
    } catch (failure) {
      pushToast({
        tone: "error",
        retitle: (translate) => ({
          title: translate("grid.couldNotSave"),
          detail: describeError(failure).message,
        }),
      });
    }
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
          <Button size="sm" variant="ghost" onClick={() => void showDdl()}>
            {t("table.ddl")}
          </Button>
          <Button size="sm" variant="ghost" onClick={newTable}>
            {t("table.newTable")}
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
        <StructurePanel structure={structure} loading={loading && !structure} />
      )}

      <div className="flex shrink-0 items-center gap-1.5 border-b border-line px-3 py-1.5">
        <span className="shrink-0 text-[11.5px] text-ink-muted">{t("grid.filter")}</span>
        <input
          value={filterText}
          disabled={!columns}
          onChange={(event) => setFilterText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Enter") return;
            event.preventDefault();
            setAppliedFilter(filterText);
            setSelectedRow(null);
            setEditor(null);
          }}
          placeholder={t("grid.filterPlaceholder")}
          className="h-7 min-w-0 flex-1 rounded border border-line bg-surface px-2 text-[12px] text-ink outline-none placeholder:text-ink-faint focus:border-accent"
        />
        <Button
          size="sm"
          variant="secondary"
          disabled={!columns}
          onClick={() => {
            setAppliedFilter(filterText);
            setSelectedRow(null);
            setEditor(null);
          }}
        >
          {t("grid.filterApply")}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={!filterText && !appliedFilter}
          onClick={() => {
            setFilterText("");
            setAppliedFilter("");
            setSelectedRow(null);
            setEditor(null);
          }}
        >
          {t("grid.filterClear")}
        </Button>
        <span className="mx-1 h-4 w-px bg-line" />
        <Button size="sm" variant="ghost" icon={<Plus />} disabled={!columns || saving} onClick={beginInsert}>
          {t("grid.addRow")}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={!columns || primaryKey.length === 0 || selectedRow === null}
          onClick={beginEdit}
        >
          {t("grid.editRow")}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          icon={<Trash2 />}
          disabled={!columns || primaryKey.length === 0 || selectedRow === null}
          onClick={() => void removeRow()}
        >
          {pendingDelete ? t("grid.confirmDelete") : t("grid.deleteRow")}
        </Button>
        {columns && primaryKey.length === 0 && (
          <span className="min-w-0 truncate text-[11px] text-ink-faint">{t("grid.noPrimaryKey")}</span>
        )}
      </div>

      {editor && columns && (
        <RowEditor
          columns={columns}
          primaryKey={primaryKey}
          mode={editor.mode}
          draft={editor.draft}
          saving={saving}
          error={editorError}
          onChange={(name, field) =>
            setEditor((current) =>
              current && { ...current, draft: { ...current.draft, [name]: field } },
            )
          }
          onSave={() => void saveEditor()}
          onCancel={() => {
            setEditor(null);
            setEditorError(null);
          }}
        />
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
            key={`${view.schema}.${view.relation}:${appliedFilter}`}
            source={{ schema: view.schema, relation: view.relation }}
            exportName={view.relation}
            onSelectRow={chooseRow}
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
                  tablePageSql(view.schema, view.relation, primaryKey, PAGE, offset, where),
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

function StructurePanel({
  structure,
  loading,
}: {
  structure: RelationStructure | null;
  loading: boolean;
}) {
  const t = useT();
  if (loading || !structure) {
    return (
      <div className="flex h-[120px] items-center justify-center border-b border-line bg-chrome/50 text-[12px] text-ink-muted">
        <LoaderCircle className="mr-2 size-3.5 animate-spin" />
        {t("table.readingColumns")}
      </div>
    );
  }
  const columns = structure.columns;
  return (
    <div className="max-h-[280px] shrink-0 overflow-y-auto border-b border-line bg-chrome/50">
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
      <StructureSection title={t("table.indexes")} empty={structure.indexes.length === 0}>
        {structure.indexes.map((index) => (
          <div key={index.name} className="border-t border-line/70 px-3 py-1.5">
            <div className="flex items-baseline gap-2">
              <span className="font-medium text-ink">{index.name}</span>
              <span className="text-ink-faint">
                {[
                  index.isPrimary ? t("table.primary") : "",
                  !index.isPrimary && index.isUnique ? t("table.unique") : "",
                  index.method,
                  index.columns.join(", "),
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
            </div>
            <div className="truncate font-mono text-[11px] text-ink-muted" title={index.definition}>
              {index.definition}
            </div>
          </div>
        ))}
      </StructureSection>
      <StructureSection title={t("table.foreignKeys")} empty={structure.foreignKeys.length === 0}>
        {structure.foreignKeys.map((key) => (
          <div key={key.name} className="border-t border-line/70 px-3 py-1.5">
            <div className="flex items-baseline gap-2">
              <span className="font-medium text-ink">{key.name}</span>
              <span className="text-ink-faint">
                {t("table.onUpdate")} {key.onUpdate} · {t("table.onDelete")} {key.onDelete}
              </span>
            </div>
            <div className="font-mono text-[11px] text-ink-muted">
              ({key.columns.join(", ")}) {t("table.references")} {key.foreignSchema}.{key.foreignTable} (
              {key.foreignColumns.join(", ")})
            </div>
          </div>
        ))}
      </StructureSection>
      <StructureSection title={t("table.triggers")} empty={structure.triggers.length === 0}>
        {structure.triggers.map((trigger) => (
          <div key={trigger.name} className="border-t border-line/70 px-3 py-1.5">
            <div className="flex items-baseline gap-2">
              <span className="font-medium text-ink">{trigger.name}</span>
              <span className="text-ink-faint">
                {trigger.timing} {trigger.events}
                {trigger.enabled ? "" : ` · ${t("table.disabled")}`}
              </span>
            </div>
            <div className="truncate font-mono text-[11px] text-ink-muted" title={trigger.definition}>
              {trigger.definition}
            </div>
          </div>
        ))}
      </StructureSection>
    </div>
  );
}

function StructureSection({
  title,
  empty,
  children,
}: {
  title: string;
  empty: boolean;
  children: ReactNode;
}) {
  const t = useT();
  return (
    <section className="border-t border-line text-[11.5px]">
      <h3 className="bg-chrome px-3 py-1 text-[11px] font-medium text-ink-muted">{title}</h3>
      {empty ? <p className="px-3 py-1.5 text-ink-faint">{t("table.none")}</p> : children}
    </section>
  );
}

function emptyDraft(columns: ColumnInfo[]): Record<string, DraftField> {
  return Object.fromEntries(columns.map((column) => [column.name, { text: "", isNull: true }]));
}

/** 结果列和目录列按名字对齐。对不上的列当作 NULL。 */
function draftFromRow(
  columns: ColumnInfo[],
  resultColumns: Column[],
  row: (string | null)[],
): Record<string, DraftField> {
  const index = new Map(resultColumns.map((column, position) => [column.name, position]));
  return Object.fromEntries(
    columns.map((column) => {
      const value = row[index.get(column.name) ?? -1] ?? null;
      return [column.name, { text: value ?? "", isNull: value === null }];
    }),
  );
}

function RowEditor({
  columns,
  primaryKey,
  mode,
  draft,
  saving,
  error,
  onChange,
  onSave,
  onCancel,
}: {
  columns: ColumnInfo[];
  primaryKey: string[];
  mode: RowEditorState["mode"];
  draft: Record<string, DraftField>;
  saving: boolean;
  error: string | null;
  onChange: (name: string, field: DraftField) => void;
  onSave: () => void;
  onCancel: () => void;
}) {
  const t = useT();
  return (
    <div className="max-h-[240px] shrink-0 overflow-y-auto border-b border-line bg-chrome/60 px-3 py-2">
      <div className="grid gap-1.5">
        {columns.map((column) => {
          const field = draft[column.name] ?? { text: "", isNull: true };
          const locked = mode === "update" && primaryKey.includes(column.name);
          return (
            <label key={column.name} className="grid grid-cols-[minmax(0,180px)_minmax(0,1fr)_auto] items-center gap-2">
              <span className="truncate text-[12px] text-ink">
                {column.name}
                <span className="ml-1.5 font-mono text-[10px] text-ink-faint">{column.dataType}</span>
              </span>
              <input
                value={field.isNull ? "" : field.text}
                disabled={locked || field.isNull || saving}
                onChange={(event) => onChange(column.name, { text: event.target.value, isNull: false })}
                placeholder={field.isNull ? t("grid.nullValue") : ""}
                className="h-7 rounded border border-line bg-surface px-2 font-mono text-[12px] text-ink outline-none disabled:text-ink-faint"
              />
              <span className="flex items-center gap-1 text-[11px] text-ink-muted">
                <input
                  type="checkbox"
                  checked={field.isNull}
                  disabled={locked || saving}
                  onChange={(event) =>
                    onChange(column.name, { text: field.text, isNull: event.target.checked })
                  }
                />
                {t("grid.setNull")}
              </span>
            </label>
          );
        })}
      </div>
      {error && <p className="mt-2 text-[12px] text-danger-ink">{error}</p>}
      <div className="mt-2 flex items-center gap-1.5">
        <Button size="sm" variant="secondary" disabled={saving} onClick={onSave}>
          {saving ? t("grid.saving") : t("grid.saveRow")}
        </Button>
        <Button size="sm" variant="ghost" disabled={saving} onClick={onCancel}>
          {t("common.cancel")}
        </Button>
      </div>
    </div>
  );
}
