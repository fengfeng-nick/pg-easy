import { useCallback, useEffect, useState } from "react";
import { api, type SavedConnection, type Settings } from "@/lib/ipc";
import { cancelQuery } from "@/lib/actions";
import { systemLanguage, type Language } from "@/lib/i18n";
import { isPrimary } from "@/lib/platform";
import { useApp } from "@/lib/store";
import { ConnectionDialog } from "@/components/connection/ConnectionDialog";
import { TitleBar } from "@/components/layout/TitleBar";
import { StatusBar } from "@/components/layout/StatusBar";
import { SettingsView } from "@/components/settings/SettingsView";
import { Sidebar } from "@/components/sidebar/Sidebar";
import { Toasts } from "@/components/ui/Toasts";
import { Welcome } from "@/components/welcome/Welcome";
import { CommandPalette } from "@/components/workbench/CommandPalette";
import { ExportDialog } from "@/components/workbench/ExportDialog";
import { ImportDialog } from "@/components/workbench/ImportDialog";
import { QueryWorkbench } from "@/components/workbench/QueryWorkbench";
import { TableView } from "@/components/workbench/TableView";
import { cn } from "@/lib/cn";
import { useResizablePanel } from "@/hooks/useResizablePanel";

export default function App() {
  const booted = useApp((state) => state.booted);
  const view = useApp((state) => state.view);
  const sidebarWidth = useApp((state) => state.sidebarWidth);
  const setSidebarWidth = useApp((state) => state.setSidebarWidth);
  const [dialog, setDialog] = useState<{ open: boolean; initial: SavedConnection | null }>({
    open: false,
    initial: null,
  });

  useBootstrap();
  useTheme();
  useDocumentLanguage();
  useGlobalShortcuts({
    onNewConnection: () => setDialog({ open: true, initial: null }),
  });

  const { startDrag } = useResizablePanel(setSidebarWidth);
  const collapsed = sidebarWidth <= 0;

  const openNewConnection = useCallback(
    () => setDialog({ open: true, initial: null }),
    [],
  );

  if (!booted) {
    return <div className="h-full bg-surface" />;
  }

  return (
    <div className="flex h-full flex-col bg-surface text-ink">
      <TitleBar
        onNewConnection={openNewConnection}
        onNewTab={() => {
          const state = useApp.getState();
          const tab = state.tabs.find((item) => item.id === state.activeTabId);
          state.addTab({ connectionId: tab?.connectionId ?? state.connections[0]?.id ?? null });
        }}
        onToggleSidebar={() => setSidebarWidth(collapsed ? 252 : -sidebarWidth)}
      />

      <div className="flex min-h-0 flex-1">
        <div
          className={cn(
            "relative shrink-0 transition-[width] duration-150 ease-out",
            collapsed && "overflow-hidden",
          )}
          style={{ width: collapsed ? 0 : sidebarWidth }}
        >
          <Sidebar
            onNewConnection={openNewConnection}
            onEditConnection={(saved) => setDialog({ open: true, initial: saved })}
          />
        </div>

        {!collapsed && (
          <div
            onPointerDown={startDrag}
            className="relative z-10 -ml-px w-[3px] shrink-0 cursor-col-resize hover:bg-accent/40"
            role="separator"
            aria-orientation="vertical"
          />
        )}

        <main className="min-w-0 flex-1">
          {view.kind === "welcome" || (view.kind === "editor" && useApp.getState().tabs.length === 0) ? (
            <Welcome onNewConnection={openNewConnection} />
          ) : view.kind === "settings" ? (
            <SettingsView />
          ) : view.kind === "table" ? (
            <TableView view={view} />
          ) : (
            <QueryWorkbench onRequestConnect={openNewConnection} />
          )}
        </main>
      </div>

      <StatusBar onOpenPalette={() => useApp.getState().openCommandPalette(true)} />

      <ConnectionDialog
        open={dialog.open}
        initial={dialog.initial}
        onClose={() => setDialog({ open: false, initial: null })}
      />
      <CommandPalette
        onNewConnection={openNewConnection}
        onRequestConnect={openNewConnection}
      />
      <ExportDialog />
      <ImportDialog />
      <Toasts />
    </div>
  );
}

function useBootstrap() {
  useEffect(() => {
    let cancelled = false;
    Promise.all([api.listConnections(), api.loadSettings()])
      .then(([connections, settings]) => {
        if (cancelled) return;
        const state = useApp.getState();
        state.setSavedConnections(connections);
        state.setSettings(settings);
        state.setLanguage(pickLanguage(settings));
        state.setBooted(true);
      })
      .catch(() => {
        if (!cancelled) useApp.getState().setBooted(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);
}

function useTheme() {
  const theme = useApp((state) => state.theme);
  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const resolved = theme === "system" ? (media.matches ? "dark" : "light") : theme;
      document.documentElement.dataset.theme = resolved;
      useApp.getState().setResolvedTheme(resolved);
    };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [theme]);
}

function useGlobalShortcuts({ onNewConnection }: { onNewConnection: () => void }) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // 用捕获阶段，让快捷键优先于 CodeMirror 自身的绑定；但当命令面板或
      // 对话框持有焦点时不拦截。
      const state = useApp.getState();
      const target = event.target as HTMLElement | null;
      const inDialog = Boolean(target?.closest('[role="dialog"]'));
      const primary = isPrimary(event);

      if (primary && event.key.toLowerCase() === "k") {
        event.preventDefault();
        event.stopPropagation();
        state.openCommandPalette(!state.commandPaletteOpen);
        return;
      }

      if (event.metaKey && !event.ctrlKey && !event.altKey && event.key.toLowerCase() === "t" && !inDialog) {
        event.preventDefault();
        const tab = state.tabs.find((item) => item.id === state.activeTabId);
        state.addTab({ connectionId: tab?.connectionId ?? state.connections[0]?.id ?? null });
        return;
      }

      if (primary && event.key === "Enter" && !inDialog) {
        event.preventDefault();
        event.stopPropagation();
        const tab = state.tabs.find((item) => item.id === state.activeTabId);
        if (!tab) return;
        void import("@/lib/actions").then(({ runSql }) => {
          const source = event.shiftKey ? (tab.selection ?? "") : tab.sql;
          if (event.shiftKey && !source.trim()) {
            state.pushToast({
              tone: "info",
              retitle: (t) => ({ title: t("workbench.nothingSelected") }),
            });
            return;
          }
          void runSql(tab.id, source);
        });
        return;
      }

      if (event.key === "F5" && !inDialog) {
        event.preventDefault();
        const tab = state.tabs.find((item) => item.id === state.activeTabId);
        if (!tab) return;
        void import("@/lib/actions").then(({ runSql }) => runSql(tab.id, tab.sql));
        return;
      }

      if (event.key === "Escape" && !inDialog) {
        const running = state.tabs.find(
          (item) => item.id === state.activeTabId && item.result?.status === "running",
        );
        if (running?.connectionId) {
          event.preventDefault();
          void cancelQuery(running.connectionId);
          return;
        }
        if (state.view.kind === "table") {
          state.setView({ kind: "editor" });
          return;
        }
        if (state.commandPaletteOpen) {
          state.openCommandPalette(false);
        }
      }

      if (primary && (event.key === "=" || event.key === "-" || event.key === "0")) {
        // 编辑器字体缩放尚未实现，这里刻意吞掉浏览器的缩放行为，
        // 以免布局被破坏。
        event.preventDefault();
      }

      if (primary && event.key.toLowerCase() === "o" && !inDialog) {
        event.preventDefault();
        onNewConnection();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [onNewConnection]);
}

/** 让文档语言（拼写检查、字形排版）与界面语言保持同步。 */
function useDocumentLanguage() {
  const language = useApp((state) => state.language);
  useEffect(() => {
    document.documentElement.lang = language === "zh" ? "zh-CN" : "en";
  }, [language]);
}

/** 启动时使用的语言：取保存的选择，否则跟随系统。 */
function pickLanguage(settings: Settings): Language {
  return settings.language === "zh" || settings.language === "en"
    ? settings.language
    : systemLanguage();
}
