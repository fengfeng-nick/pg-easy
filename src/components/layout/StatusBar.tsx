import { Circle } from "lucide-react";
import { cn } from "@/lib/cn";
import { useT } from "@/lib/i18n";
import { formatCount, shortcut } from "@/lib/platform";
import { useApp } from "@/lib/store";

export function StatusBar({ onOpenPalette }: { onOpenPalette: () => void }) {
  const connection = useApp((state) => {
    const tab = state.tabs.find((item) => item.id === state.activeTabId);
    return state.connections.find((item) => item.id === tab?.connectionId) ?? null;
  });
  const tab = useApp((state) => state.tabs.find((item) => item.id === state.activeTabId));
  const result = tab?.result;
  const t = useT();

  const rows = (result?.results ?? []).reduce(
    (sum, statement) => sum + (statement.rows?.length ?? 0),
    0,
  );

  return (
    <footer className="flex h-[var(--statusbar-h)] shrink-0 items-center gap-3 border-t border-line bg-chrome px-3 text-[11px] text-ink-muted">
      {connection ? (
        <span className="flex items-center gap-1.5">
          <Circle className="size-[7px] fill-current text-ok" strokeWidth={0} />
          <span className="text-ink-soft">
            {connection.user}@{connection.host}:{connection.port}
          </span>
          <span className="text-ink-faint">/</span>
          <span>{connection.database}</span>
          {connection.backendPid > 0 && (
            <>
              <span className="text-ink-faint">·</span>
              <span className="tabular">{t("status.pid", { pid: connection.backendPid })}</span>
            </>
          )}
        </span>
      ) : (
        <span className="flex items-center gap-1.5">
          <Circle className="size-[7px] fill-current text-ink-faint" strokeWidth={0} />
          {t("status.notConnected")}
        </span>
      )}

      {rows > 0 && (
        <span className="tabular text-ink-faint">{t("common.rows", { count: formatCount(rows) })}</span>
      )}

      <div className="ml-auto flex items-center gap-3">
        {connection && (
          <span
            className={cn(
              "tabular",
              connection.txStatus === "inerror" ? "text-danger-ink" : "",
            )}
          >
            {connection.txStatus === "idle"
              ? t("status.idle")
              : connection.txStatus === "inerror"
                ? t("status.aborted")
                : t("status.inTransaction")}
          </span>
        )}
        {connection && <span className="text-ink-faint">v{connection.serverVersion.split(" ")[0]}</span>}
        <button
          onClick={onOpenPalette}
          className="flex items-center gap-1.5 rounded px-1.5 py-0.5 hover:bg-hover hover:text-ink-soft"
        >
          <span>{t("app.commands")}</span>
          <span className="kbd">{shortcut("mod", "K").join("")}</span>
        </button>
      </div>
    </footer>
  );
}
