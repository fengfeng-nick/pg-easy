import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, Copy, WrapText } from "lucide-react";
import type { Column } from "@/lib/ipc";
import { cn } from "@/lib/cn";
import { useT } from "@/lib/i18n";

type Props = {
  column: Column;
  value: string | null;
  anchor: DOMRect;
  onClose: () => void;
};

type View = "text" | "json";

function looksLikeJson(column: Column, value: string | null): boolean {
  if (!value) return false;
  if (column.kind === "json") return true;
  const trimmed = value.trimStart();
  return (
    (trimmed.startsWith("{") && trimmed.trimEnd().endsWith("}")) ||
    (trimmed.startsWith("[") && trimmed.trimEnd().endsWith("]"))
  );
}

function prettyJson(value: string): { text: string; ok: boolean } {
  try {
    return { text: JSON.stringify(JSON.parse(value), null, 2), ok: true };
  } catch {
    return { text: value, ok: false };
  }
}

export function CellDetail({ column, value, anchor, onClose }: Props) {
  const t = useT();
  const [copied, setCopied] = useState(false);
  const [wrap, setWrap] = useState(true);
  const [view, setView] = useState<View>(
    looksLikeJson(column, value) ? "json" : "text",
  );
  const asideRef = useRef<HTMLDivElement>(null);

  const pretty = useMemo(
    () => (value === null ? null : prettyJson(value)),
    [value],
  );

  const display = view === "json" && pretty?.ok ? pretty.text : (value ?? "");

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    const onPointerDown = (event: PointerEvent) => {
      if (!asideRef.current?.contains(event.target as Node)) onClose();
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("pointerdown", onPointerDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("pointerdown", onPointerDown, true);
    };
  }, [onClose]);

  const copy = async () => {
    await navigator.clipboard.writeText(view === "json" && pretty?.ok ? pretty.text : (value ?? ""));
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1200);
  };

  // 单元格在可视区域内时贴在它旁边，否则固定在右边缘，像检视器那样。
  const style: React.CSSProperties =
    anchor.width === 0
      ? { right: 16, bottom: 44, width: 460, maxHeight: "60vh" }
      : {
          left: Math.min(anchor.left, window.innerWidth - 520),
          top: Math.min(anchor.bottom + 4, window.innerHeight - 320),
          width: 460,
          maxHeight: 360,
        };

  return createPortal(
    <div
      ref={asideRef}
      role="dialog"
      aria-label={t("cell.valueOf", { column: column.name })}
      style={style}
      className={cn(
        "fixed z-[55] flex flex-col overflow-hidden rounded-[var(--radius-md)] border border-line-strong bg-raised shadow-[var(--shadow-lg)]",
        "row-enter",
      )}
    >
      <header className="flex items-center gap-2 border-b border-line px-3 py-2">
        <span className="truncate text-[12.5px] font-medium text-ink">{column.name}</span>
        <span className="font-mono text-[10.5px] text-ink-faint">{column.typeName}</span>
        <span className="ml-auto text-[11px] text-ink-faint">
          {value === null
            ? t("common.null")
            : t("cell.chars", { count: value.length.toLocaleString() })}
        </span>
      </header>
      <div className="min-h-0 flex-1 overflow-auto px-3 py-2.5">
        {value === null ? (
          <span className="font-mono text-[12px] italic text-[var(--null)]">{t("common.null")}</span>
        ) : view === "json" && !pretty?.ok ? (
          <div className="space-y-2">
            <p className="text-[11.5px] text-ink-muted">{t("cell.invalidJson")}</p>
            <pre className="whitespace-pre-wrap break-words font-mono text-[12px] leading-relaxed text-ink">
              {value}
            </pre>
          </div>
        ) : (
          <pre
            className={cn(
              "font-mono text-[12px] leading-[1.6] text-ink",
              wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre",
            )}
          >
            {display}
          </pre>
        )}
      </div>
      <footer className="flex items-center gap-1 border-t border-line bg-chrome px-2 py-1.5">
        {looksLikeJson(column, value) && (
          <div className="mr-1 flex rounded-[var(--radius-sm)] border border-line bg-sunken p-0.5">
            {(["text", "json"] as const).map((option) => (
              <button
                key={option}
                onClick={() => setView(option)}
                className={cn(
                  "h-5 rounded-[3px] px-2 text-[11px] capitalize",
                  view === option ? "bg-raised text-ink shadow-[var(--shadow-sm)]" : "text-ink-muted",
                )}
              >
                {option}
              </button>
            ))}
          </div>
        )}
        <button
          onClick={() => setWrap((current) => !current)}
          className={cn(
            "flex h-6 items-center gap-1 rounded px-1.5 text-[11.5px] hover:bg-hover",
            wrap ? "text-ink" : "text-ink-muted",
          )}
        >
          <WrapText className="size-3" />
          {t("cell.wrap")}
        </button>
        <button
          onClick={copy}
          className="ml-auto flex h-6 items-center gap-1 rounded px-2 text-[11.5px] text-ink-soft hover:bg-hover hover:text-ink"
        >
          {copied ? <Check className="size-3 text-ok" /> : <Copy className="size-3" />}
          {copied ? t("common.copied") : t("cell.copyValue")}
        </button>
      </footer>
    </div>,
    document.body,
  );
}
