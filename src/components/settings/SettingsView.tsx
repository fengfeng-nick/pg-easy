import { useState } from "react";
import { Check } from "lucide-react";
import { api, type Settings } from "@/lib/ipc";
import { describeError } from "@/lib/errors";
import { LANGUAGES, useT, type Language } from "@/lib/i18n";
import { useApp, type ThemeChoice } from "@/lib/store";
import { Button } from "@/components/ui/Button";
import { Field, Input, Segmented } from "@/components/ui/Field";

export function SettingsView() {
  const stored = useApp((state) => state.settings);
  const setSettings = useApp((state) => state.setSettings);
  const theme = useApp((state) => state.theme);
  const setTheme = useApp((state) => state.setTheme);
  const language = useApp((state) => state.language);
  const setLanguage = useApp((state) => state.setLanguage);
  const pushToast = useApp((state) => state.pushToast);
  const t = useT();

  const [form, setForm] = useState<Settings | null>(stored);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  const current = form ?? stored;

  const update = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    setSaved(false);
    setForm((previous) => ({ ...(previous ?? stored ?? empty), [key]: value }));
  };

  const save = async () => {
    if (!current) return;
    setSaving(true);
    try {
      const normalized: Settings = {
        ...current,
        pageSize: clamp(current.pageSize, 50, 10_000),
        maxRows: clamp(current.maxRows, 100, 2_000_000),
        connectTimeoutMs: clamp(current.connectTimeoutMs, 500, 120_000),
        appName: current.appName.trim() || "pg easy",
      };
      await api.saveSettings(normalized);
      setSettings(normalized);
      setForm(normalized);
      setSaved(true);
      window.setTimeout(() => setSaved(false), 2000);
    } catch (failure) {
      const error = describeError(failure);
      pushToast({
        tone: "error",
        retitle: (t) => ({ title: t("settings.couldNotSave"), detail: [error.message, error.detail].filter(Boolean).join("\n") }),
      });
    } finally {
      setSaving(false);
    }
  };

  if (!current) {
    return <div className="p-8 text-[12.5px] text-ink-muted">{t("settings.loading")}</div>;
  }

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-[620px] px-8 py-8">
        <h1 className="text-[16px] font-semibold tracking-[-0.01em] text-ink">{t("settings.title")}</h1>
        <p className="mt-1 text-[12px] text-ink-muted">{t("settings.subtitle")}</p>

        <Section title={t("settings.appearance")}>
          <div className="flex items-center justify-between">
            <div>
              <p className="text-[12.5px] font-medium text-ink">{t("settings.language")}</p>
              <p className="mt-0.5 text-[11.5px] text-ink-muted">{t("settings.languageHint")}</p>
            </div>
            <Segmented<Language>
              value={language}
              onChange={(next) => {
                // Applies at once, and is remembered: a language you have to
                // save before you can read the Save button is a trap.
                setLanguage(next);
                update("language", next);
                void api.saveSettings({ ...current, language: next }).catch(() => undefined);
              }}
              options={LANGUAGES}
            />
          </div>
          <div className="mt-4 flex items-center justify-between">
            <div>
              <p className="text-[12.5px] font-medium text-ink">{t("settings.theme")}</p>
              <p className="mt-0.5 text-[11.5px] text-ink-muted">{t("settings.themeHint")}</p>
            </div>
            <Segmented<ThemeChoice>
              value={theme}
              onChange={(value) => setTheme(value)}
              options={[
                { value: "system", label: t("settings.system") },
                { value: "light", label: t("settings.light") },
                { value: "dark", label: t("settings.dark") },
              ]}
            />
          </div>
        </Section>

        <Section title={t("settings.results")}>
          <div className="grid grid-cols-2 gap-4">
            <Field
              label={t("settings.rowsPerPage")}
              hint={t("settings.rowsPerPageHint")}
            >
              {(id) => (
                <Input
                  id={id}
                  type="number"
                  min={50}
                  max={10000}
                  value={current.pageSize}
                  onChange={(event) => update("pageSize", Number(event.target.value))}
                />
              )}
            </Field>
            <Field
              label={t("settings.rowLimit")}
              hint={t("settings.rowLimitHint")}
            >
              {(id) => (
                <Input
                  id={id}
                  type="number"
                  min={100}
                  max={2_000_000}
                  value={current.maxRows}
                  onChange={(event) => update("maxRows", Number(event.target.value))}
                />
              )}
            </Field>
          </div>
        </Section>

        <Section title={t("settings.connections")}>
          <div className="grid grid-cols-2 gap-4">
            <Field label={t("settings.connectTimeout")} hint={t("settings.connectTimeoutHint")}>
              {(id) => (
                <Input
                  id={id}
                  type="number"
                  min={500}
                  max={120000}
                  step={500}
                  value={current.connectTimeoutMs}
                  onChange={(event) => update("connectTimeoutMs", Number(event.target.value))}
                />
              )}
            </Field>
            <Field label={t("settings.sessionName")} hint={t("settings.sessionNameHint")}>
              {(id) => (
                <Input
                  id={id}
                  value={current.appName}
                  onChange={(event) => update("appName", event.target.value)}
                />
              )}
            </Field>
          </div>
        </Section>

        <Section title={t("settings.security")}>
          <p className="text-[11.5px] leading-relaxed text-ink-muted">{t("settings.securityBody")}</p>
        </Section>

        <div className="mt-6 flex items-center justify-end gap-2">
          {saved && (
            <span className="flex items-center gap-1 text-[11.5px] text-ok">
              <Check className="size-3" />
              {t("settings.saved")}
            </span>
          )}
          <Button variant="primary" size="sm" onClick={save} loading={saving}>
            {t("settings.saveChanges")}
          </Button>
        </div>
      </div>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="mt-7 border-t border-line pt-5">
      <h2 className="mb-3 text-[10.5px] font-semibold uppercase tracking-[0.07em] text-ink-faint">
        {title}
      </h2>
      {children}
    </section>
  );
}

const empty: Settings = {
  pageSize: 1000,
  maxRows: 200_000,
  connectTimeoutMs: 10_000,
  appName: "pg easy",
  language: null,
};

function clamp(value: number, min: number, max: number) {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, Math.round(value)));
}
