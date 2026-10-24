import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Database, FileArchive, FileCode2, LoaderCircle, Table2 } from "lucide-react";
import { exportDump, tableKey } from "@/lib/actions";
import { cn } from "@/lib/cn";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import type { Catalog } from "@/lib/ipc";
import { Toggle } from "@/components/ui/Field";

/** 导出整个数据库，或其中选定的若干表。 */
export function ExportDialog() {
  const request = useApp((state) => state.exportRequest);
  const openExport = useApp((state) => state.openExport);
  const connection = useApp((state) =>
    state.connections.find((item) => item.id === state.exportRequest?.connectionId),
  );

  const t = useT();
  const [includeData, setIncludeData] = useState(true);
  const [format, setFormat] = useState<"sql" | "zip">("sql");
  const [running, setRunning] = useState(false);
  /**
   * 要导出哪些表在对话框打开时冻结：多选调用传一个列表，单表菜单传一个
   * 名字，schema 菜单传它所有的表，连接菜单不传（整个数据库）。
   */
  const [scope, setScope] = useState<{ tables: string[] | null } | null>(null);
  /** “选择表”模式：执行时读 `picked`，而不是冻结的那份清单。 */
  const [onlySelection, setOnlySelection] = useState(false);
  const [picked, setPicked] = useState<Set<string>>(new Set());

  const catalog = connection?.catalog;
  const tree = useMemo(() => {
    if (!catalog) return [];
    return Object.entries(catalog.relations)
      .map(([schema, relations]) => ({
        schema,
        relations: relations.filter((relation) => relation.kind !== "sequence"),
      }))
      .filter((group) => group.relations.length > 0)
      .sort((a, b) => a.schema.localeCompare(b.schema));
  }, [catalog]);

  useEffect(() => {
    if (!request) return;
    // 只在新的请求到达时执行；目录稍后填充完成（或用户切换开关）
    // 不能撤销任何东西。
    const tables =
      request.scope ?? (request.schema !== null ? allTables(catalog, request.schema) : null);
    setScope(tables === null ? null : { tables });
    setPicked(new Set(tables ?? []));
    setOnlySelection(false);
    setIncludeData(true);
    setFormat("sql");
    // 保存下来的选择是列表而不是标签：标签要跟随语言，在渲染时推导。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request]);

  if (!request || !connection) return null;

  const everything = onlySelection ? [...picked].sort() : null;
  const tables = onlySelection ? everything : (scope?.tables ?? null);
  const nothing = tables !== null && tables.length === 0;

  const close = () => {
    if (!running) openExport(null);
  };

  const start = async () => {
    setRunning(true);
    const path = await exportDump({
      connectionId: connection.id,
      scope: tables,
      includeData,
      format,
    });
    setRunning(false);
    if (path !== null) openExport(null);
  };

  // 退出“选择”模式后回到菜单当初要求的范围：那张表、整个数据库，
  // 或者多选交过来的列表。
  const toggle = (key: string) =>
    setPicked((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const exporting = connection.exporting;

  return (
    <Dialog
      open
      onClose={close}
      dismissible={!running}
      title={t("export.title")}
      description={
        <span className="block truncate">
          {connection.name}
          <span className="text-ink-faint"> · {connection.user}@{connection.host}/{connection.database}</span>
        </span>
      }
      width={520}
      footer={
        <>
          <span className="mr-auto text-[11.5px] text-ink-muted">
            {running && exporting && exporting.total > 0
              ? t("export.readingTables", {
                  done: Math.min(exporting.done, exporting.total),
                  total: exporting.total,
                })
              : tables === null
                ? t("export.wholeDatabase")
                : t(tables.length === 1 ? "export.tableCountOne" : "export.tableCount", {
                    count: tables.length,
                  })}
          </span>
          <Button variant="ghost" onClick={close} disabled={running}>
            {t("export.cancel")}
          </Button>
          <Button
            data-autofocus
            variant="primary"
            onClick={() => void start()}
            disabled={running || nothing || (onlySelection && picked.size === 0)}
            loading={running}
            icon={running ? <LoaderCircle className="animate-spin" /> : undefined}
          >
            {t("export.export")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="rounded-[var(--radius-md)] border border-line bg-raised px-3.5 py-2.5">
          <div className="flex items-center gap-2 text-[12.5px] font-medium text-ink">
            {scope === null ? (
              <Database className="size-3.5 text-accent" />
            ) : (
              <Table2 className="size-3.5 text-accent" />
            )}
            {onlySelection
              ? t("export.chosenTables")
              : scope === null
                ? t("export.entireDatabase", { database: connection.database })
                : describe(t, scope.tables ?? [], request.schema)}
          </div>
          <p className="mt-1 text-[11.5px] leading-snug text-ink-muted">
            {tables === null
              ? t("export.everySchema")
              : onlySelection
                ? t("export.onlyTicked")
                : t("export.withDeps")}
          </p>
          {tree.length > 0 && (
            <button
              onClick={() => setOnlySelection((value) => !value)}
              className="mt-1.5 text-[11.5px] font-medium text-accent-ink hover:underline"
            >
              {onlySelection ? t("export.keepSelection") : t("export.chooseTables")}
            </button>
          )}
        </div>

        {onlySelection && (
          <div className="max-h-[190px] overflow-y-auto rounded-[var(--radius-md)] border border-line-strong">
            {tree.map((group) => {
              const keys = group.relations.map((relation) => tableKey(group.schema, relation.name));
              const all = keys.every((key) => picked.has(key));
              return (
                <div key={group.schema}>
                  <button
                    onClick={() =>
                      setPicked((current) => {
                        const next = new Set(current);
                        for (const key of keys) {
                          if (all) next.delete(key);
                          else next.add(key);
                        }
                        return next;
                      })
                    }
                    className="flex h-7 w-full items-center gap-2 bg-chrome px-3 text-left text-[11.5px] font-medium text-ink-soft hover:text-ink"
                  >
                    <Check checked={all} partial={!all && keys.some((key) => picked.has(key))} />
                    {group.schema}
                  </button>
                  <ul>
                    {group.relations.map((relation) => {
                      const key = tableKey(group.schema, relation.name);
                      return (
                        <li key={key}>
                          <button
                            onClick={() => toggle(key)}
                            className={cn(
                              "flex h-7 w-full items-center gap-2 pl-7 pr-3 text-left text-[12px] hover:bg-hover",
                              picked.has(key) ? "text-ink" : "text-ink-muted",
                            )}
                          >
                            <Check checked={picked.has(key)} />
                            <span className="min-w-0 flex-1 truncate">{relation.name}</span>
                            <span className="shrink-0 text-[10.5px] text-ink-faint">
                              {relation.kind === "view" || relation.kind === "matview"
                                ? relation.kind
                                : relation.size}
                            </span>
                          </button>
                        </li>
                      );
                    })}
                  </ul>
                </div>
              );
            })}
          </div>
        )}

        <Toggle
          checked={includeData}
          onChange={setIncludeData}
          label={t("export.includeData")}
          description={t(includeData ? "export.includeDataHint" : "export.structureOnlyHint")}
        />

        <div>
          <p className="mb-1.5 text-[11.5px] font-medium text-ink-soft">{t("export.format")}</p>
          <div className="grid grid-cols-2 gap-2">
            <FormatCard
              selected={format === "sql"}
              onSelect={() => setFormat("sql")}
              icon={<FileCode2 />}
              title={t("export.sqlScript")}
              detail={t("export.sqlScriptDetail")}
            />
            <FormatCard
              selected={format === "zip"}
              onSelect={() => setFormat("zip")}
              icon={<FileArchive />}
              title={t("export.zipArchive")}
              detail={t("export.zipArchiveDetail")}
            />
          </div>
        </div>
      </div>
    </Dialog>
  );
}

function allTables(catalog: Catalog | null | undefined, schema: string) {
  return (catalog?.relations[schema] ?? [])
    .filter((relation) => relation.kind !== "sequence")
    .map((relation) => tableKey(schema, relation.name));
}

function describe(t: ReturnType<typeof useT>, tables: string[], schema: string | null) {
  if (tables.length === 1) return tables[0];
  if (schema !== null) return t("export.allTablesInSchema", { count: tables.length, schema });
  return t("export.selectedTables", { count: tables.length });
}

function Check({ checked, partial }: { checked: boolean; partial?: boolean }) {
  return (
    <span
      className={cn(
        "flex size-[13px] shrink-0 items-center justify-center rounded-[3px] border",
        checked || partial ? "border-accent bg-accent text-white" : "border-line-strong",
      )}
    >
      {checked ? (
        <svg viewBox="0 0 10 10" className="size-2" aria-hidden>
          <path d="M1.5 5.2l2.2 2.2L8.5 2.6" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      ) : partial ? (
        <svg viewBox="0 0 10 10" className="size-2" aria-hidden>
          <path d="M2 5h6" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
        </svg>
      ) : null}
    </span>
  );
}

function FormatCard({
  selected,
  onSelect,
  icon,
  title,
  detail,
}: {
  selected: boolean;
  onSelect: () => void;
  icon: ReactNode;
  title: string;
  detail: string;
}) {
  return (
    <button
      role="radio"
      aria-checked={selected}
      onClick={onSelect}
      className={cn(
        "flex flex-col items-start gap-1 rounded-[var(--radius-md)] border px-3 py-2.5 text-left transition-colors duration-100",
        selected
          ? "border-accent bg-accent-wash"
          : "border-line-strong bg-raised hover:bg-hover",
      )}
    >
      <span className="flex items-center gap-1.5 text-[12.5px] font-medium text-ink [&_svg]:size-3.5">
        {icon}
        {title}
      </span>
      <span className="text-[11px] leading-snug text-ink-muted">{detail}</span>
    </button>
  );
}
