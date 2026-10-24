import { Database, Plus, SquareTerminal, X } from "lucide-react";
import { cn } from "@/lib/cn";
import { useT } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import { shortcut } from "@/lib/platform";

export function Tabs({ onNewTab }: { onNewTab: () => void }) {
  const tabs = useApp((state) => state.tabs);
  const activeTabId = useApp((state) => state.activeTabId);
  const view = useApp((state) => state.view);
  const activateTab = useApp((state) => state.activateTab);
  const closeTab = useApp((state) => state.closeTab);
  const connections = useApp((state) => state.connections);
  const t = useT();

  if (tabs.length === 0) return null;
  const editorVisible = view.kind === "editor";

  return (
    <div className="flex h-[var(--tabbar-h)] shrink-0 items-stretch border-b border-line bg-chrome pl-1.5">
      <div className="flex min-w-0 items-stretch overflow-x-auto">
        {tabs.map((tab) => {
          const connection = connections.find((item) => item.id === tab.connectionId);
          const active = tab.id === activeTabId && editorVisible;
          const running = tab.result?.status === "running";
          return (
            <div
              key={tab.id}
              role="tab"
              aria-selected={active}
              onMouseDown={(event) => {
                if (event.button === 1) {
                  event.preventDefault();
                  closeTab(tab.id);
                } else if (event.button === 0) {
                  activateTab(tab.id);
                }
              }}
              className={cn(
                "group relative flex shrink-0 items-center gap-1.5 border-r border-line pl-3 pr-1.5 text-[12px]",
                active
                  ? "bg-surface text-ink"
                  : "text-ink-muted hover:bg-hover/60 hover:text-ink-soft",
              )}
              style={{ minWidth: 118, maxWidth: 220 }}
            >
              {running ? (
                <span className="size-2.5 animate-spin rounded-full border-[1.5px] border-accent border-r-transparent" />
              ) : (
                <SquareTerminal className="size-3.5 shrink-0 opacity-60" strokeWidth={1.6} />
              )}
              <span className="min-w-0 flex-1 truncate">{tab.title}</span>
              {tab.dirty && !running && (
                <span className="size-[5px] shrink-0 rounded-full bg-ink-muted group-hover:hidden" />
              )}
              <button
                onClick={(event) => {
                  event.stopPropagation();
                  closeTab(tab.id);
                }}
                aria-label={t("workbench.closeTab", { name: tab.title })}
                className={cn(
                  "flex size-[18px] shrink-0 items-center justify-center rounded-[3px] text-ink-muted hover:bg-active hover:text-ink",
                  tab.dirty ? "hidden group-hover:flex" : "opacity-0 group-hover:opacity-100",
                )}
                title={connection ? `${connection.name} · ${connection.database}` : undefined}
              >
                <X className="size-3" />
              </button>
              {active && (
                <span className="absolute inset-x-0 top-0 h-[2px] bg-accent" />
              )}
            </div>
          );
        })}
      </div>
      <button
        onClick={onNewTab}
        aria-label={t("app.newQuery")}
        title={`${t("app.newQuery")} · ${shortcut("mod", "T").join("")}`}
        className="ml-0.5 flex w-8 shrink-0 items-center justify-center text-ink-muted hover:bg-hover hover:text-ink"
      >
        <Plus className="size-4" />
      </button>
      <div className="ml-auto flex shrink-0 items-center pr-2 text-[10.5px] text-ink-faint">
        {(() => {
          const tab = tabs.find((item) => item.id === activeTabId);
          const connection = connections.find((item) => item.id === tab?.connectionId);
          return connection ? (
            <>
              <Database className="mr-1 size-3" />
              {connection.database}
            </>
          ) : null;
        })()}
      </div>
    </div>
  );
}
