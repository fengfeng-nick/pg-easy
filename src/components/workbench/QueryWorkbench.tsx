import { useCallback, useEffect, useMemo, useRef } from "react";
import {
  AlignLeft,
  ChevronDown,
  Database,
  LoaderCircle,
  OctagonX,
  Play,
  Sparkles,
} from "lucide-react";
import { format as formatSql } from "sql-formatter";
import { CompletionData } from "@/lib/completion";
import { cancelQuery } from "@/lib/actions";
import { runSql } from "@/lib/actions";
import { cn } from "@/lib/cn";
import { useT } from "@/lib/i18n";
import { isMac, shortcut } from "@/lib/platform";
import { useApp, type Connection, type Tab } from "@/lib/store";
import { SqlEditor, type SqlEditorHandle } from "@/components/editor/SqlEditor";
import { ResultsPane } from "@/components/results/ResultsPane";
import { Button, IconButton } from "@/components/ui/Button";
import { Menu, MenuItem, MenuSeparator, MenuLabel } from "@/components/ui/Menu";
import { Tabs } from "@/components/workbench/Tabs";

export function QueryWorkbench({ onRequestConnect }: { onRequestConnect: () => void }) {
  const tabs = useApp((state) => state.tabs);
  const activeTabId = useApp((state) => state.activeTabId);
  const view = useApp((state) => state.view);
  const editorHeightRatio = useApp((state) => state.editorHeightRatio);
  const setEditorHeightRatio = useApp((state) => state.setEditorHeightRatio);

  const tab = tabs.find((item) => item.id === activeTabId) ?? null;
  const splitRef = useRef<HTMLDivElement>(null);

  const startDrag = useCallback(
    (event: React.PointerEvent) => {
      event.preventDefault();
      const container = splitRef.current;
      if (!container) return;
      const rect = container.getBoundingClientRect();
      const onMove = (move: PointerEvent) => {
        const ratio = (move.clientY - rect.top) / rect.height;
        setEditorHeightRatio(ratio);
      };
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        document.body.style.cursor = "";
      };
      document.body.style.cursor = "row-resize";
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [setEditorHeightRatio],
  );

  return (
    <div className="flex h-full min-h-0 flex-col">
      <Tabs onNewTab={() => useApp.getState().addTab({ connectionId: tab?.connectionId ?? null })} />
      {view.kind === "editor" && tab ? (
        <div ref={splitRef} key={tab.id} className="flex min-h-0 flex-1 flex-col">
          <div
            className="flex min-h-[120px] flex-col"
            style={{ height: `${editorHeightRatio * 100}%` }}
          >
            <QueryToolbar tab={tab} onRequestConnect={onRequestConnect} />
            <div className="min-h-0 flex-1">
              <TabEditor tab={tab} />
            </div>
          </div>
          <div
            onPointerDown={startDrag}
            className="group relative z-10 h-[5px] shrink-0 cursor-row-resize border-y border-line bg-chrome hover:bg-accent-wash"
            role="separator"
            aria-orientation="horizontal"
          >
            <span className="pointer-events-none absolute left-1/2 top-1/2 h-[2px] w-6 -translate-x-1/2 -translate-y-1/2 rounded-full bg-line-strong group-hover:bg-accent" />
          </div>
          <div className="min-h-0 flex-1">
            <ResultsPane tab={tab} />
          </div>
        </div>
      ) : (
        <div className="min-h-0 flex-1">
          {view.kind === "editor" && !tab ? (
            <NoTabs onRequestConnect={onRequestConnect} />
          ) : null}
        </div>
      )}
    </div>
  );
}

function TabEditor({ tab }: { tab: Tab }) {
  const editorRef = useRef<SqlEditorHandle>(null);
  const connection = useApp((state) =>
    state.connections.find((item) => item.id === tab.connectionId),
  );
  const completion = useCompletion(connection);
  const errorAt = useMemo(() => {
    const position = tab.result?.error?.position;
    return position ? { position, stamp: tab.result?.finishedAt ?? 0 } : null;
  }, [tab.result?.error?.position, tab.result?.finishedAt]);

  // 焦点跟着用户激活的标签页走，和编辑器一样。
  useEffect(() => {
    editorRef.current?.focus();
  }, [tab.id]);

  return (
    <SqlEditor
      ref={editorRef}
      value={tab.sql}
      completion={completion}
      errorAt={errorAt}
      onDocChange={(doc) => {
        useApp.getState().updateTab(tab.id, { sql: doc, dirty: true });
      }}
      onSelectionChange={(selection) => {
        useApp.getState().updateTab(tab.id, { selection });
      }}
    />
  );
}

/** 补全数据在同一条连接的各标签页之间共享，只在目录（或连接）变化时
 *  重建。 */
const completionByConnection = new Map<string, { catalog: unknown; data: CompletionData }>();

function useCompletion(connection: Connection | undefined) {
  return useMemo(() => {
    if (!connection) return null;
    const cached = completionByConnection.get(connection.id);
    if (cached && cached.catalog === connection.catalog) return cached.data;
    const data = new CompletionData(connection.catalog, connection.id);
    completionByConnection.set(connection.id, { catalog: connection.catalog, data });
    return data;
  }, [connection]);
}

function QueryToolbar({ tab, onRequestConnect }: { tab: Tab; onRequestConnect: () => void }) {
  const connection = useApp((state) =>
    state.connections.find((item) => item.id === tab.connectionId),
  );
  const connections = useApp((state) => state.connections);
  const updateTab = useApp((state) => state.updateTab);
  const t = useT();
  const running = tab.result?.status === "running";

  const run = (selectionOnly: boolean) => {
    const state = useApp.getState();
    const current = state.tabs.find((item) => item.id === tab.id);
    if (!current) return;
    let sql = current.sql;
    if (selectionOnly) {
      sql = current.selection ?? "";
      if (!sql.trim()) {
        state.pushToast({
          tone: "info",
          retitle: (t) => ({
            title: t("workbench.nothingSelected"),
            detail: t("workbench.nothingSelectedDetail", {
              shortcut: isMac ? "⌘⇧↵" : "Ctrl+Shift+Enter",
            }),
          }),
        });
        return;
      }
    }
    void runSql(tab.id, sql);
  };

  const format = () => {
    const current = useApp.getState().tabs.find((item) => item.id === tab.id);
    if (!current) return;
    try {
      const formatted = formatSql(current.sql, {
        language: "postgresql",
        keywordCase: "upper",
        tabWidth: 2,
        linesBetweenQueries: 1,
      });
      updateTab(tab.id, { sql: formatted });
    } catch {
      useApp.getState().pushToast({
        tone: "warn",
        retitle: (t) => ({
          title: t("workbench.couldNotFormat"),
          detail: t("workbench.couldNotFormatDetail"),
        }),
      });
    }
  };

  const selectConnection = async (id: string | null) => {
    updateTab(tab.id, { connectionId: id });
  };

  return (
    <div className="flex h-[38px] shrink-0 items-center gap-1.5 border-b border-line px-2">
      {running ? (
        <Button
          size="sm"
          variant="secondary"
          icon={<OctagonX />}
          className="text-danger-ink"
          onClick={() => tab.connectionId && void cancelQuery(tab.connectionId)}
        >
          {t("workbench.stop")}
          <span className="ml-1 flex items-center gap-0.5 opacity-60">
            {isMac ? "⎋" : "Esc"}
          </span>
        </Button>
      ) : (
        <Button
          size="sm"
          variant="primary"
          icon={<Play className="fill-current" strokeWidth={0} />}
          onClick={() => run(false)}
          disabled={!tab.connectionId}
          title={t("workbench.runTitle", { shortcut: isMac ? "⌘↵" : "Ctrl+Enter" })}
        >
          {t("workbench.run")}
        </Button>
      )}
      <IconButton
        label={t("workbench.runSelectionTitle", {
          shortcut: shortcut("mod", "shift", "enter").join(""),
        })}
        size="sm"
        onClick={() => run(true)}
        disabled={running || !tab.connectionId}
      >
        <Play className="size-[13px]" strokeWidth={1.8} />
      </IconButton>
      <IconButton label={t("workbench.format")} size="sm" onClick={format} disabled={running}>
        <AlignLeft />
      </IconButton>

      <span className="mx-1 h-4 w-px bg-line" />

      <Menu
        width={240}
        trigger={({ toggle }) => (
          <button
            onClick={toggle}
            className="flex h-6 max-w-[240px] items-center gap-1.5 rounded-[var(--radius-sm)] px-1.5 text-[12px] text-ink-soft hover:bg-hover hover:text-ink"
          >
            <Database className="size-3.5 text-ink-muted" strokeWidth={1.7} />
            <span className="truncate">
              {connection ? connection.name : t("workbench.noConnection")}
            </span>
            {connection && (
              <span className="truncate text-[10.5px] text-ink-faint">
                {connection.database}
              </span>
            )}
            <ChevronDown className="size-3 shrink-0 text-ink-muted" />
          </button>
        )}
      >
        <MenuLabel>{t("workbench.runOn")}</MenuLabel>
        {connections.length === 0 && (
          <MenuItem icon={<Database />} onSelect={onRequestConnect}>
            {t("workbench.connectToDatabase")}
          </MenuItem>
        )}
        {connections.map((item) => (
          <MenuItem
            key={item.id}
            icon={<Database />}
            onSelect={() => void selectConnection(item.id)}
          >
            {item.name}
            <span className="ml-1.5 text-[10.5px] text-ink-faint">{item.database}</span>
          </MenuItem>
        ))}
        <MenuSeparator />
        <MenuItem icon={<Database />} onSelect={() => void selectConnection(null)} disabled={!tab.connectionId}>
          {t("workbench.detach")}
        </MenuItem>
      </Menu>

      <div className="ml-auto flex items-center gap-2">
        {running && (
          <span className="flex items-center gap-1.5 text-[11px] text-ink-muted">
            <LoaderCircle className="size-3 animate-spin" />
            {t("workbench.running")}
          </span>
        )}
        {connection && connection.txStatus !== "idle" && (
          <span
            className={cn(
              "rounded px-1.5 py-[2px] text-[10.5px] font-medium",
              connection.txStatus === "inerror"
                ? "bg-danger-wash text-danger-ink"
                : "bg-warn-wash text-warn",
            )}
          >
            {t(connection.txStatus === "inerror" ? "workbench.transactionAborted" : "workbench.inTransaction")}
          </span>
        )}
        <span className="hidden items-center gap-1 text-[10.5px] text-ink-faint xl:flex">
          <Sparkles className="size-3" />
          {isMac ? "⌘↵" : "Ctrl+↵"}
        </span>
      </div>
    </div>
  );
}

function NoTabs({ onRequestConnect }: { onRequestConnect: () => void }) {
  const t = useT();
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3">
      <p className="text-[13px] text-ink-soft">{t("workbench.noQueryOpen")}</p>
      <Button variant="secondary" size="sm" onClick={onRequestConnect}>
        {t("palette.connectToDatabase")}
      </Button>
    </div>
  );
}
