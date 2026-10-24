import { useEffect, useId, useMemo, useState } from "react";
import { ArrowRight, Eye, EyeOff, KeyRound, LoaderCircle, ShieldCheck } from "lucide-react";
import { api, type SavedConnection, type SslMode } from "@/lib/ipc";
import { describeError } from "@/lib/errors";
import { useT, type MessageKey } from "@/lib/i18n";
import { openConnection, removeSavedConnection } from "@/lib/actions";
import { cn } from "@/lib/cn";
import { useApp } from "@/lib/store";
import { Button } from "@/components/ui/Button";
import { ConfirmDialog, Dialog } from "@/components/ui/Dialog";
import { Field, Input, Segmented, Toggle } from "@/components/ui/Field";

type Form = {
  id: string;
  name: string;
  host: string;
  port: string;
  database: string;
  user: string;
  password: string;
  ssl: SslMode;
  sslRootCert: string;
  allowUnencryptedPassword: boolean;
  savePassword: boolean;
};

type Props = {
  open: boolean;
  initial: SavedConnection | null;
  onClose: () => void;
};

const blank: Form = {
  id: "",
  name: "",
  host: "localhost",
  port: "5432",
  database: "postgres",
  user: "",
  password: "",
  ssl: "prefer",
  sslRootCert: "",
  allowUnencryptedPassword: false,
  savePassword: true,
};

function toForm(entry: SavedConnection): Form {
  return {
    id: entry.id,
    name: entry.name,
    host: entry.host || "localhost",
    port: String(entry.port || 5432),
    database: entry.database,
    user: entry.user,
    password: entry.password ?? "",
    ssl: entry.ssl,
    sslRootCert: entry.sslRootCert ?? "",
    allowUnencryptedPassword: entry.allowUnencryptedPassword ?? false,
    savePassword: entry.savePassword,
  };
}

const SSL_OPTIONS: { value: SslMode; label: MessageKey; title: MessageKey }[] = [
  { value: "disable", label: "connection.sslOff", title: "connection.sslOffTitle" },
  { value: "prefer", label: "connection.sslPrefer", title: "connection.sslPreferTitle" },
  { value: "require", label: "connection.sslRequire", title: "connection.sslRequireTitle" },
  { value: "verify-full", label: "connection.sslVerify", title: "connection.sslVerifyTitle" },
];

const isLocalHost = (host: string) =>
  ["localhost", "127.0.0.1", "::1", ""].includes(host.trim());

export function ConnectionDialog({ open, initial, onClose }: Props) {
  const titleId = useId();
  const t = useT();
  const [form, setForm] = useState<Form>(initial ? toForm(initial) : blank);
  const [showPassword, setShowPassword] = useState(false);
  const [busy, setBusy] = useState<"test" | "connect" | null>(null);
  const [passwordLoaded, setPasswordLoaded] = useState(false);
  const [tested, setTested] = useState<{ serverVersion: string; ms: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const pushToast = useApp((state) => state.pushToast);
  const savedConnections = useApp((state) => state.savedConnections);
  const setSavedConnections = useApp((state) => state.setSavedConnections);

  useEffect(() => {
    if (!open) return;
    setForm(initial ? toForm(initial) : blank);
    setError(null);
    setTested(null);
    setShowPassword(false);
    setConfirmRemove(false);
    setPasswordLoaded(!initial?.savePassword);
    if (initial?.savePassword) {
      api
        .readSavedPassword(initial.id)
        .then((password) => {
          if (password) setForm((current) => ({ ...current, password }));
        })
        .catch(() => undefined)
        .finally(() => setPasswordLoaded(true));
    }
  }, [open, initial]);

  const update = <K extends keyof Form>(key: K, value: Form[K]) => {
    setForm((current) => ({ ...current, [key]: value }));
    setTested(null);
  };

  const params = useMemo(
    () => ({
      host: form.host.trim() || "localhost",
      port: Number.parseInt(form.port, 10) || 5432,
      database: form.database.trim() || form.user.trim() || "postgres",
      user: form.user.trim(),
      ssl: form.ssl,
      allowUnencryptedPassword: form.allowUnencryptedPassword,
      savePassword: form.savePassword,
      name: form.name.trim(),
      id: form.id,
    }),
    [form],
  );

  const urlPreview = `postgresql://${params.user || "user"}@${params.host}:${params.port}/${params.database}`;
  const portValid = /^\d{1,5}$/.test(form.port) && Number(form.port) <= 65535;
  const canSubmit = params.user.length > 0 && portValid && !busy;

  const buildParams = () => ({
    host: params.host,
    port: params.port,
    database: params.database,
    user: params.user,
    password: form.password || undefined,
    ssl: form.ssl,
    ...(form.ssl === "verify-full" && form.sslRootCert.trim()
      ? { sslRootCert: form.sslRootCert.trim() }
      : {}),
    allowUnencryptedPassword: form.allowUnencryptedPassword,
  });

  const test = async () => {
    setBusy("test");
    setError(null);
    setTested(null);
    const started = performance.now();
    try {
      const info = await api.connect(buildParams());
      setTested({
        serverVersion: info.serverVersion,
        ms: Math.round(performance.now() - started),
      });
      await api.disconnect(info.id);
    } catch (failure) {
      const { message, detail } = describeError(failure);
      setError([message, detail].filter(Boolean).join("\n"));
    } finally {
      setBusy(null);
    }
  };

  const submit = async (save: boolean) => {
    if (!canSubmit) return;
    setBusy("connect");
    setError(null);
    try {
      let entry: SavedConnection | null = null;
      if (save) {
        const saved = await api.saveConnection(
          {
            ...params,
            name: params.name || `${params.database}@${params.host}`,
          },
          form.password || null,
        );
        entry = saved.connection;
        setSavedConnections([
          ...savedConnections.filter((item) => item.id !== entry!.id),
          entry,
        ]);
      }
      const id = await openConnection(
        { ...buildParams(), ...(entry ? { id: entry.id } : {}) },
        entry,
      );
      if (id) {
        onClose();
        if (save) {
          pushToast({
            tone: "success",
            retitle: (t) => ({
              title: t("connection.savedToast"),
              detail:
                form.password && form.savePassword ? t("connection.passwordStored") : undefined,
            }),
          });
        }
      } else {
        setError(t("connection.failedBanner"));
      }
    } catch (failure) {
      const { message, detail } = describeError(failure);
      setError([message, detail].filter(Boolean).join("\n"));
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    if (!initial) return;
    setConfirmRemove(false);
    try {
      const gone = await removeSavedConnection(initial.id);
      // Two entries can share a server, and the dialog may be showing one the
      // list could no longer act on; it stays open to say so.
      if (gone) onClose();
      else setError(t("connection.alreadyRemoved"));
    } catch (failure) {
      // A closed dialog can take neither the error nor the toast that reports
      // it, so it stays open to show why the removal failed.
      const described = describeError(failure);
      setError([described.message, described.detail].filter(Boolean).join("\n"));
    }
  };

  return (
    <>
      <Dialog
        open={open}
        onClose={onClose}
        title={initial ? t("connection.editTitle") : t("connection.newTitle")}
        labelId={titleId}
        width={520}
        dismissible={!busy}
        footer={
          <>
            <Button
              variant="ghost"
              size="sm"
              onClick={test}
              loading={busy === "test"}
              disabled={!canSubmit && busy !== "test"}
              className={initial ? undefined : "mr-auto"}
            >
              {t("connection.testConnection")}
            </Button>
            {initial && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setConfirmRemove(true)}
                disabled={Boolean(busy)}
                className="mr-auto text-danger-ink hover:bg-danger-wash hover:text-danger-ink"
              >
                {t("connection.remove")}
              </Button>
            )}
            <Button variant="ghost" size="sm" onClick={onClose} disabled={Boolean(busy)}>
              {t("common.cancel")}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => void submit(false)}
              disabled={!canSubmit}
              loading={busy === "connect"}
            >
              {t("app.connect")}
            </Button>
            <Button
              variant="primary"
              size="sm"
              onClick={() => void submit(true)}
              disabled={!canSubmit}
            >
              {t("connection.saveAndConnect")}
            </Button>
          </>
        }
      >
        <div className="space-y-3.5">
          <div className="grid grid-cols-[1fr_110px] gap-3">
            <Field label={t("connection.host")}>
              {(id) => (
                <Input
                  id={id}
                  data-autofocus
                  value={form.host}
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  onChange={(event) => update("host", event.target.value)}
                  placeholder="localhost"
                />
              )}
            </Field>
            <Field label={t("connection.port")} error={portValid ? undefined : t("connection.portError")}>
              {(id) => (
                <Input
                  id={id}
                  value={form.port}
                  inputMode="numeric"
                  onChange={(event) => update("port", event.target.value.replace(/[^\d]/g, ""))}
                  className={cn(!portValid && "border-danger")}
                />
              )}
            </Field>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <Field label={t("connection.database")}>
              {(id) => (
                <Input
                  id={id}
                  value={form.database}
                  spellCheck={false}
                  onChange={(event) => update("database", event.target.value)}
                  placeholder="postgres"
                />
              )}
            </Field>
            <Field label={t("connection.user")}>
              {(id) => (
                <Input
                  id={id}
                  value={form.user}
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  onChange={(event) => update("user", event.target.value)}
                  placeholder="postgres"
                />
              )}
            </Field>
          </div>

          <Field
            label={t("connection.password")}
            hint={form.savePassword ? t("connection.passwordHint") : undefined}
          >
            {(id) => (
              <div className="relative">
                <Input
                  id={id}
                  type={showPassword ? "text" : "password"}
                  value={form.password}
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(event) => update("password", event.target.value)}
                  className="pr-16"
                  placeholder={t(
                    initial?.savePassword
                      ? "connection.savedPasswordPlaceholder"
                      : "connection.emptyPasswordPlaceholder",
                  )}
                />
                <div className="absolute inset-y-0 right-1 flex items-center gap-0.5">
                  {initial?.savePassword && passwordLoaded && !form.password && (
                    <span className="flex items-center gap-1 pr-1 text-[10.5px] text-ink-faint">
                      <KeyRound className="size-3" />
                      {t("connection.savedBadge")}
                    </span>
                  )}
                  <button
                    type="button"
                    onClick={() => setShowPassword((value) => !value)}
                    aria-label={t(showPassword ? "connection.hidePassword" : "connection.showPassword")}
                    className="flex size-6 items-center justify-center rounded text-ink-muted hover:bg-hover hover:text-ink"
                  >
                    {showPassword ? <EyeOff className="size-[14px]" /> : <Eye className="size-[14px]" />}
                  </button>
                </div>
              </div>
            )}
          </Field>

          <div className="flex items-end justify-between gap-4">
            <Field label={t("connection.ssl")} className="gap-2">
              {(fieldId) => (
                <div id={fieldId}>
                  <Segmented
                    value={form.ssl}
                    options={SSL_OPTIONS.map((option) => ({
                      value: option.value,
                      label: t(option.label),
                      title: t(option.title),
                    }))}
                    onChange={(value) => update("ssl", value)}
                  />
                </div>
              )}
            </Field>
            <div className="pb-[3px]">
              <Toggle
                checked={form.savePassword}
                onChange={(value) => update("savePassword", value)}
                label={t("connection.rememberPassword")}
              />
            </div>
          </div>

          {form.ssl === "verify-full" && (
            <Field
              label={t("connection.rootCert")}
              hint={t("connection.rootCertHint")}
            >
              {(id) => (
                <Input
                  id={id}
                  value={form.sslRootCert}
                  spellCheck={false}
                  onChange={(event) => update("sslRootCert", event.target.value)}
                  placeholder="/path/to/ca.pem"
                  className="font-mono text-[12px]"
                />
              )}
            </Field>
          )}

          {form.ssl !== "disable" && (
            <Toggle
              checked={form.allowUnencryptedPassword}
              onChange={(value) => update("allowUnencryptedPassword", value)}
              label={t("connection.allowUnencrypted")}
              description={t(
                isLocalHost(form.host)
                  ? "connection.allowUnencryptedLocal"
                  : "connection.allowUnencryptedRemote",
              )}
            />
          )}

          <Field label={t("connection.name")} hint={t("connection.nameHint")}>
            {(id) => (
              <Input
                id={id}
                value={form.name}
                onChange={(event) => update("name", event.target.value)}
                placeholder={t("connection.namePlaceholder", {
                  database: params.database,
                  host: params.host,
                })}
              />
            )}
          </Field>

          <div className="flex items-center gap-2 overflow-hidden rounded-[var(--radius-sm)] border border-line bg-sunken px-2.5 py-1.5">
            <ArrowRight className="size-3 shrink-0 text-ink-faint" />
            <code className="truncate font-mono text-[11px] text-ink-muted">{urlPreview}</code>
            <span className="ml-auto flex shrink-0 items-center gap-1 text-[10.5px] text-ink-faint">
              <ShieldCheck className="size-3" />
              {t(isLocalHost(form.host) ? "connection.local" : "connection.network")}
            </span>
            {form.ssl !== "disable" && (
              <span className="shrink-0 rounded bg-accent-wash px-1 py-px text-[10px] font-medium text-accent-ink">
                TLS
              </span>
            )}
          </div>

          {tested && (
            <div className="flex items-center gap-2 rounded-[var(--radius-sm)] border border-line bg-raised px-2.5 py-1.5 text-[11.5px]">
              <span className="size-1.5 rounded-full bg-ok" />
              <span className="text-ink">
                {t("connection.testedOk", { version: tested.serverVersion.split(" ")[0] })}
              </span>
              <span className="tabular text-ink-muted">{tested.ms} ms</span>
            </div>
          )}

          {error && (
            <div className="rounded-[var(--radius-sm)] border border-[color-mix(in_oklab,var(--danger)_30%,var(--line))] bg-danger-wash px-2.5 py-2">
              <p className="whitespace-pre-wrap break-words font-mono text-[11.5px] leading-relaxed text-danger-ink">
                {error}
              </p>
            </div>
          )}

          {busy === "test" && (
            <p className="flex items-center gap-1.5 text-[11.5px] text-ink-muted">
              <LoaderCircle className="size-3 animate-spin" />
              {t("connection.testing")}
            </p>
          )}
        </div>
      </Dialog>

      {initial && (
        <ConfirmDialog
          open={open && confirmRemove}
          title={t("connection.removeTitle", { name: initial.name })}
          body={t("connection.removeBody")}
          confirmLabel={t("connection.remove")}
          tone="danger"
          onCancel={() => setConfirmRemove(false)}
          onConfirm={() => void remove()}
        />
      )}
    </>
  );
}

