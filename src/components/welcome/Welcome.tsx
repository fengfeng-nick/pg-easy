import { ArrowRight, Database, Keyboard } from "lucide-react";
import { api } from "@/lib/ipc";
import { openConnection } from "@/lib/actions";
import { useT } from "@/lib/i18n";
import { isMac, shortcut } from "@/lib/platform";
import { useApp } from "@/lib/store";
import { Button } from "@/components/ui/Button";

export function Welcome({ onNewConnection }: { onNewConnection: () => void }) {
  const saved = useApp((state) => state.savedConnections);
  const recent = useApp((state) => state.connections);
  const t = useT();
  const keys = [
    [shortcut("mod", "enter").join(""), t("welcome.runStatement")],
    [shortcut("mod", "shift", "enter").join(""), t("welcome.runSelection")],
    [shortcut("mod", "t").join(""), t("welcome.newQueryTab")],
    [shortcut("mod", "k").join(""), t("welcome.jump")],
    [shortcut("mod", "s").join(""), t("welcome.save")],
  ];

  return (
    <div className="flex h-full items-center justify-center overflow-y-auto px-8 py-10">
      <div className="w-full max-w-[640px]">
        <div className="mb-9 flex items-center gap-3">
          <div className="flex size-9 items-center justify-center rounded-[10px] bg-accent text-white shadow-[0_1px_2px_rgba(0,0,0,0.15)] dark:text-[#0c1818]">
            <Database className="size-[18px]" strokeWidth={2} />
          </div>
          <div>
            <h1 className="text-[16px] font-semibold tracking-[-0.01em] text-ink">{t("app.name")}</h1>
            <p className="text-[12px] text-ink-muted">{t("app.tagline")}</p>
          </div>
        </div>

        <div className="grid gap-6 sm:grid-cols-[1.1fr_1fr]">
          <section>
            <h2 className="mb-2 text-[10.5px] font-semibold uppercase tracking-[0.07em] text-ink-faint">
              {recent.length > 0
                ? t("welcome.openSessions")
                : saved.length > 0
                  ? t("welcome.savedConnections")
                  : t("welcome.getStarted")}
            </h2>
            {recent.length === 0 && saved.length === 0 && (
              <Button variant="primary" onClick={onNewConnection} icon={<ArrowRight />}>
                {t("welcome.addConnection")}
              </Button>
            )}
            <ul className="space-y-0.5">
              {recent.map((connection) => (
                <li key={connection.id}>
                  <button
                    onClick={() => {
                      const tab = useApp.getState().addTab({ connectionId: connection.id });
                      useApp.getState().activateTab(tab);
                    }}
                    className="group flex h-9 w-full items-center gap-2.5 rounded-[var(--radius-sm)] px-2 text-left hover:bg-hover"
                  >
                    <Database className="size-3.5 text-ink-muted" />
                    <span className="min-w-0 flex-1 truncate text-[12.5px] text-ink">
                      {connection.name}
                    </span>
                    <span className="text-[11px] text-ink-faint opacity-0 group-hover:opacity-100">
                      {t("welcome.newQuery")}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
            {saved.length > 0 && (
              <ul className="mt-1 space-y-0.5">
                {saved
                  .filter(
                    (entry) =>
                      !recent.some(
                        (connection) =>
                          connection.host === entry.host &&
                          connection.port === entry.port &&
                          connection.database === entry.database &&
                          connection.user === entry.user,
                      ),
                  )
                  .map((entry) => (
                    <li key={entry.id}>
                      <button
                        onClick={async () => {
                          let password = entry.password;
                          if (entry.savePassword && !password) {
                            password = (await api.readSavedPassword(entry.id)) ?? undefined;
                          }
                          await openConnection({ ...entry, password }, entry);
                        }}
                        className="group flex h-9 w-full items-center gap-2.5 rounded-[var(--radius-sm)] px-2 text-left hover:bg-hover"
                      >
                        <span className="size-3.5 rounded-full border-[1.5px] border-line-strong" />
                        <span className="min-w-0 flex-1 truncate text-[12.5px] text-ink-soft">
                          {entry.name}
                        </span>
                        <span className="text-[11px] text-accent-ink opacity-0 group-hover:opacity-100">
                          {t("welcome.connect")}
                        </span>
                      </button>
                    </li>
                  ))}
              </ul>
            )}
            <button
              onClick={onNewConnection}
              className="mt-2 px-2 text-[12px] font-medium text-accent-ink hover:underline"
            >
              {t("welcome.newConnectionHint")}
            </button>
          </section>

          <section>
            <h2 className="mb-2 flex items-center gap-1.5 text-[10.5px] font-semibold uppercase tracking-[0.07em] text-ink-faint">
              <Keyboard className="size-3" />
              {t("welcome.keyboard")}
            </h2>
            <dl className="space-y-1.5">
              {keys.map(([key, label]) => (
                <div key={key} className="flex items-center justify-between gap-3">
                  <dt className="text-[11.5px] text-ink-muted">{label}</dt>
                  <dd>
                    <span className="kbd">{key}</span>
                  </dd>
                </div>
              ))}
            </dl>
            <p className="mt-3 text-[11px] leading-relaxed text-ink-faint">
              {isMac ? t("welcome.macHint") : t("welcome.otherHint")}
            </p>
          </section>
        </div>
      </div>
    </div>
  );
}
