import { useEffect, useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/cn";
import { useT } from "@/lib/i18n";

type DialogProps = {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  description?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  width?: number;
  /** 默认情况下 Escape 和点击遮罩都会关闭；正在提交的表单会关掉这个行为。 */
  dismissible?: boolean;
  labelId?: string;
};

export function Dialog({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  width = 460,
  dismissible = true,
  labelId = "dialog-title",
}: DialogProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  const restoreFocus = useRef<HTMLElement | null>(null);
  const t = useT();

  useEffect(() => {
    if (!open) return;
    restoreFocus.current = document.activeElement as HTMLElement | null;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";

    const timer = window.setTimeout(() => {
      const panel = panelRef.current;
      if (!panel) return;
      const target = panel.querySelector<HTMLElement>(
        "[data-autofocus], input:not([type=hidden]):not([disabled]), select, textarea, button:not([disabled])",
      );
      (target ?? panel).focus();
    }, 30);

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && dismissible) {
        event.preventDefault();
        event.stopPropagation();
        onClose();
        return;
      }
      if (event.key !== "Tab") return;
      const panel = panelRef.current;
      if (!panel) return;
      const focusable = Array.from(
        panel.querySelectorAll<HTMLElement>(
          'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])',
        ),
      ).filter((element) => element.offsetParent !== null);
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("keydown", onKeyDown, true);
      document.body.style.overflow = previousOverflow;
      restoreFocus.current?.focus?.();
    };
  }, [open, dismissible, onClose]);

  if (!open) return null;

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[12vh]">
      <div
        className="absolute inset-0 bg-[var(--overlay)] backdrop-blur-[1.5px] [animation:fade-in_140ms_var(--ease)]"
        onClick={dismissible ? onClose : undefined}
      />
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelId}
        tabIndex={-1}
        style={{ width }}
        className={cn(
          "relative flex max-h-[76vh] flex-col overflow-hidden rounded-[var(--radius-lg)] border border-line-strong bg-surface shadow-[var(--shadow-lg)]",
          "[animation:dialog-in_180ms_var(--ease)]",
        )}
      >
        <header className="flex items-start justify-between gap-6 px-5 pb-3 pt-4">
          <div className="min-w-0">
            <h2 id={labelId} className="text-[14px] font-semibold tracking-[-0.01em] text-ink">
              {title}
            </h2>
            {description && (
              <p className="mt-1 text-[12px] leading-relaxed text-ink-muted">{description}</p>
            )}
          </div>
          <button
            onClick={onClose}
            aria-label={t("common.close")}
            className="-mr-1.5 -mt-0.5 flex size-6 items-center justify-center rounded-[var(--radius-sm)] text-ink-muted transition-colors hover:bg-hover hover:text-ink"
          >
            <svg viewBox="0 0 12 12" className="size-3" aria-hidden>
              <path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
            </svg>
          </button>
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 pb-5">{children}</div>
        {footer && (
          <footer className="flex items-center justify-end gap-2 border-t border-line bg-chrome px-5 py-3">
            {footer}
          </footer>
        )}
      </div>
      <style>{`
        @keyframes fade-in { from { opacity: 0 } to { opacity: 1 } }
        @keyframes dialog-in {
          from { opacity: 0; transform: translateY(6px) scale(0.985) }
          to { opacity: 1; transform: none }
        }
      `}</style>
    </div>,
    document.body,
  );
}

type ConfirmProps = {
  open: boolean;
  title: string;
  body: ReactNode;
  confirmLabel: string;
  tone?: "default" | "danger";
  onConfirm: () => void;
  onCancel: () => void;
};

export function ConfirmDialog({
  open,
  title,
  body,
  confirmLabel,
  tone = "default",
  onConfirm,
  onCancel,
}: ConfirmProps) {
  const t = useT();
  return (
    <Dialog
      open={open}
      onClose={onCancel}
      title={title}
      width={400}
      footer={
        <>
          <button
            onClick={onCancel}
            className="h-8 rounded-[var(--radius-md)] border border-line-strong bg-raised px-3.5 text-[13px] font-medium text-ink hover:bg-hover"
          >
            {t("common.cancel")}
          </button>
          <button
            data-autofocus
            onClick={onConfirm}
            className={cn(
              "h-8 rounded-[var(--radius-md)] px-3.5 text-[13px] font-medium text-white",
              tone === "danger" ? "bg-danger" : "bg-accent",
            )}
          >
            {confirmLabel}
          </button>
        </>
      }
    >
      <p className="text-[13px] leading-relaxed text-ink-soft">{body}</p>
    </Dialog>
  );
}
