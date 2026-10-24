import { cn } from "@/lib/cn";
import { useT } from "@/lib/i18n";
import { useApp, type Toast } from "@/lib/store";

const toneRule: Record<Toast["tone"], string> = {
  info: "bg-ink-faint",
  success: "bg-ok",
  warn: "bg-warn",
  error: "bg-danger",
};

function ToastItem({ toast }: { toast: Toast }) {
  const dismiss = useApp((state) => state.dismissToast);
  const t = useT();

  return (
    <div
      role="status"
      className="row-enter pointer-events-auto relative flex w-[348px] items-start gap-3 overflow-hidden rounded-[var(--radius-md)] border border-line-strong bg-raised py-2.5 pl-3.5 pr-2.5 shadow-[var(--shadow-md)]"
    >
      <span className={cn("absolute inset-y-0 left-0 w-[2.5px]", toneRule[toast.tone])} />
      <div className="min-w-0 flex-1">
        <p className="text-[12.5px] font-medium leading-snug text-ink">{toast.title}</p>
        {toast.detail && (
          <p className="mt-0.5 line-clamp-4 whitespace-pre-wrap break-words text-[11.5px] leading-relaxed text-ink-muted">
            {toast.detail}
          </p>
        )}
      </div>
      <button
        onClick={() => dismiss(toast.id)}
        aria-label={t("common.dismiss")}
        className="-mr-0.5 flex size-5 shrink-0 items-center justify-center rounded text-ink-faint hover:bg-hover hover:text-ink"
      >
        <svg viewBox="0 0 12 12" className="size-2.5" aria-hidden>
          <path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
        </svg>
      </button>
    </div>
  );
}

export function Toasts() {
  const toasts = useApp((state) => state.toasts);
  return (
    <div className="pointer-events-none fixed bottom-[34px] right-4 z-[70] flex flex-col items-end gap-2">
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} />
      ))}
    </div>
  );
}

export function toast(tone: Toast["tone"], title: string, detail?: string) {
  useApp.getState().pushToast({ tone, title, detail });
}
