import { PanelLeft, Plus, Settings2 } from "lucide-react";
import { cn } from "@/lib/cn";
import { useT } from "@/lib/i18n";
import { isMac } from "@/lib/platform";
import { useApp } from "@/lib/store";
import { Button, IconButton } from "@/components/ui/Button";

/** App chrome. On macOS the traffic lights sit inside the overlay title bar,
 *  so the strip reserves room for them and centres the title like a document;
 *  on Windows the same strip is the native title bar area with our controls
 *  right-aligned and the OS close/minimise buttons drawn by the system. */
export function TitleBar({
  onNewConnection,
  onNewTab,
  onToggleSidebar,
}: {
  onNewConnection: () => void;
  onNewTab: () => void;
  onToggleSidebar: () => void;
}) {
  const view = useApp((state) => state.view);
  const setView = useApp((state) => state.setView);
  const t = useT();
  const title = view.kind === "table" ? view.relation : "";

  return (
    <header
      className={cn(
        "drag-region relative z-20 flex h-[var(--titlebar-h)] shrink-0 items-center border-b border-line bg-chrome",
        isMac ? "pl-[80px] pr-3" : "pl-2 pr-[140px]",
      )}
    >
      <div className="no-drag flex items-center">
        <IconButton label={t("app.toggleSidebar")} size="sm" onClick={onToggleSidebar}>
          <PanelLeft />
        </IconButton>
      </div>

      <span
        aria-hidden
        className="pointer-events-none absolute left-1/2 -translate-x-1/2 truncate text-[11.5px] font-medium text-ink-muted"
      >
        {isMac ? title : `pg easy${title ? ` — ${title}` : ""}`}
      </span>

      <div className="no-drag ml-auto flex items-center gap-1.5">
        <Button size="sm" variant="ghost" icon={<Plus />} onClick={onNewTab}>
          {t("app.newQuery")}
        </Button>
        <Button size="sm" variant="secondary" onClick={onNewConnection}>
          <span className="flex items-center gap-1.5">
            <PlugIcon />
            {t("app.connect")}
          </span>
        </Button>
        <IconButton
          label={t("app.settings")}
          size="sm"
          active={view.kind === "settings"}
          onClick={() => setView({ kind: "settings" })}
        >
          <Settings2 />
        </IconButton>
      </div>
    </header>
  );
}

/** Lucide's Plug is drawn at an angle that reads as decoration in a toolbar;
 *  this is the same idea on the app's 15px optical grid. */
function PlugIcon() {
  return (
    <svg viewBox="0 0 16 16" className="size-[14px]" fill="none" aria-hidden>
      <path
        d="M6 2v3M10 2v3M4.5 5h7v2.2a3.5 3.5 0 01-7 0V5zM8 10.7V14"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
