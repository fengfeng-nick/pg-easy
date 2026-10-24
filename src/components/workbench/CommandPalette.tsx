import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ArrowRight, Database, Plus, Search, Settings2, Table2, Terminal } from "lucide-react";
import { cn } from "@/lib/cn";
import { useT } from "@/lib/i18n";
import { isMac, mod } from "@/lib/platform";
import { useApp } from "@/lib/store";

type Command = {
  id: string;
  label: string;
  hint?: string;
  group: "actions" | "tables" | "connections";
  icon: React.ReactNode;
  run: () => void;
};

export function CommandPalette({
  onNewConnection,
  onRequestConnect,
}: {
  onNewConnection: () => void;
  onRequestConnect: () => void;
}) {
  const open = useApp((state) => state.commandPaletteOpen);
  const setOpen = useApp((state) => state.openCommandPalette);
  const connections = useApp((state) => state.connections);
  const activeConnection = useApp((state) => {
    const tab = state.tabs.find((item) => item.id === state.activeTabId);
    return state.connections.find((item) => item.id === tab?.connectionId) ?? state.connections[0] ?? null;
  });

  const t = useT();
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const commands = useMemo<Command[]>(() => {
    const state = useApp.getState();
    const list: Command[] = [
      {
        id: "new-query",
        label: t("app.newQuery"),
        hint: `${mod} T`,
        group: "actions",
        icon: <Plus />,
        run: () => state.addTab({ connectionId: activeConnection?.id ?? null }),
      },
      {
        id: "connect",
        label: t("palette.newConnection"),
        group: "actions",
        icon: <Database />,
        run: onNewConnection,
      },
      {
        id: "run",
        label: t("palette.run"),
        hint: isMac ? "⌘ ↵" : "Ctrl ↵",
        group: "actions",
        icon: <Terminal />,
        run: () => {
          const tab = useApp.getState().tabs.find((item) => item.id === useApp.getState().activeTabId);
          if (tab) void import("@/lib/actions").then(({ runSql }) => runSql(tab.id, tab.sql));
        },
      },
      {
        id: "settings",
        label: t("app.settings"),
        group: "actions",
        icon: <Settings2 />,
        run: () => state.setView({ kind: "settings" }),
      },
    ];

    for (const connection of state.connections) {
      if (!connection.catalog) continue;
      for (const [schema, relations] of Object.entries(connection.catalog.relations)) {
        for (const relation of relations) {
          if (relation.kind === "sequence") continue;
          list.push({
            id: `table-${connection.id}-${schema}-${relation.name}`,
            label: `${schema}.${relation.name}`,
            hint: connection.name,
            group: "tables",
            icon: <Table2 />,
            run: () =>
              state.setView({
                kind: "table",
                connectionId: connection.id,
                schema,
                relation: relation.name,
              }),
          });
        }
      }
    }

    for (const connection of state.connections) {
      list.push({
        id: `conn-${connection.id}`,
        label: connection.name,
        hint: `${connection.user}@${connection.host}/${connection.database}`,
        group: "connections",
        icon: <ArrowRight />,
        run: () =>
          state.addTab({
            connectionId: connection.id,
          }),
      });
    }
    return list;
  }, [connections, activeConnection?.id, onNewConnection, t]);

  const filtered = useMemo(() => {
    const term = query.trim().toLowerCase();
    if (!term) return commands.slice(0, 60);
    // Prefix matches on the label rank above substring matches in the hint.
    const scored = commands
      .map((command) => {
        const label = command.label.toLowerCase();
        const hint = (command.hint ?? "").toLowerCase();
        let score = -1;
        if (label.startsWith(term)) score = 0;
        else if (label.includes(term)) score = 1;
        else if (hint.includes(term)) score = 2;
        return { command, score };
      })
      .filter((item) => item.score >= 0)
      .sort((a, b) => a.score - b.score);
    return scored.slice(0, 60).map((item) => item.command);
  }, [commands, query]);

  useEffect(() => {
    if (open) {
      setQuery("");
      setIndex(0);
      window.setTimeout(() => inputRef.current?.focus(), 10);
    }
  }, [open]);

  useEffect(() => setIndex(0), [query]);

  useEffect(() => {
    const element = listRef.current?.querySelector<HTMLElement>(`[data-index="${index}"]`);
    element?.scrollIntoView({ block: "nearest" });
  }, [index]);

  if (!open) return null;

  const groups = filtered.reduce<Record<string, { command: Command; position: number }[]>>(
    (acc, command, position) => {
      (acc[command.group] ??= []).push({ command, position });
      return acc;
    },
    {},
  );

  const close = () => setOpen(false);

  return createPortal(
    <div className="fixed inset-0 z-[65] flex items-start justify-center pt-[12vh]">
      <div className="absolute inset-0 bg-[var(--overlay)] backdrop-blur-[1px]" onClick={close} />
      <div
        role="dialog"
        aria-label={t("palette.label")}
        className="row-enter relative w-[560px] overflow-hidden rounded-[var(--radius-lg)] border border-line-strong bg-raised shadow-[var(--shadow-lg)]"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            close();
          } else if (event.key === "ArrowDown") {
            event.preventDefault();
            setIndex((value) => Math.min(filtered.length - 1, value + 1));
          } else if (event.key === "ArrowUp") {
            event.preventDefault();
            setIndex((value) => Math.max(0, value - 1));
          } else if (event.key === "Enter") {
            event.preventDefault();
            const command = filtered[index];
            if (command) {
              close();
              command.run();
            }
          }
        }}
      >
        <div className="flex h-11 items-center gap-2.5 border-b border-line px-3.5">
          <Search className="size-4 text-ink-muted" />
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t("palette.search")}
            spellCheck={false}
            className="h-full flex-1 bg-transparent text-[13.5px] text-ink placeholder:text-ink-faint"
          />
          <span className="kbd">esc</span>
        </div>
        <div ref={listRef} className="max-h-[340px] overflow-y-auto p-1.5">
          {filtered.length === 0 && (
            <div className="px-3 py-6 text-center text-[12px] text-ink-muted">
              {t("palette.noResults", { query })}
            </div>
          )}
          {Object.entries(groups).map(([group, items]) => (
            <div key={group}>
              <div className="px-2 pb-1 pt-2 text-[10.5px] font-semibold uppercase tracking-[0.06em] text-ink-faint">
                {t(`palette.groups.${group as Command["group"]}`)}
              </div>
              {items.map(({ command, position }) => (
                <button
                  key={command.id}
                  data-index={position}
                  onMouseMove={() => setIndex(position)}
                  onClick={() => {
                    close();
                    command.run();
                  }}
                  className={cn(
                    "flex h-8 w-full items-center gap-2.5 rounded-[var(--radius-sm)] px-2 text-left text-[12.5px]",
                    position === index ? "bg-accent-wash text-accent-ink" : "text-ink-soft",
                  )}
                >
                  <span className="flex size-4 items-center justify-center [&_svg]:size-[14px]">
                    {command.icon}
                  </span>
                  <span className="min-w-0 flex-1 truncate">{command.label}</span>
                  {command.hint && (
                    <span className="shrink-0 text-[10.5px] text-ink-faint">{command.hint}</span>
                  )}
                </button>
              ))}
            </div>
          ))}
        </div>
        {!activeConnection && (
          <div className="flex items-center justify-between border-t border-line bg-chrome px-3 py-2 text-[11px] text-ink-muted">
            {t("status.notConnected")}
            <button onClick={onRequestConnect} className="text-accent-ink hover:underline">
              {t("palette.connectToDatabase")}
            </button>
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
}
