import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/cn";

type MenuContextValue = { close: () => void };
const MenuContext = createContext<MenuContextValue>({ close: () => {} });

type MenuProps = {
  trigger: (props: { open: boolean; toggle: () => void }) => ReactNode;
  children: ReactNode;
  align?: "start" | "end";
  width?: number;
  className?: string;
};

/** A popover menu anchored to its trigger. Renders in a portal so it is never
 *  clipped by scrolling containers (the result grid has several). */
export function Menu({ trigger, children, align = "start", width = 220, className }: MenuProps) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState({ top: 0, left: 0 });
  const anchorRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  const close = useCallback(() => setOpen(false), []);

  const place = useCallback(() => {
    const anchor = anchorRef.current;
    const panel = panelRef.current;
    if (!anchor) return;
    // `contents` has no layout box, so measure the rendered trigger instead.
    const trigger = anchor.firstElementChild;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const height = panel?.offsetHeight ?? 0;
    const top =
      rect.bottom + 4 + height > window.innerHeight - 8
        ? Math.max(8, rect.top - height - 4)
        : rect.bottom + 4;
    const left =
      align === "end"
        ? Math.min(window.innerWidth - width - 8, rect.right - width)
        : Math.min(window.innerWidth - width - 8, rect.left);
    setPos({ top, left });
  }, [align, width]);

  useLayoutEffect(() => {
    if (open) place();
  }, [open, place]);

  // The menu closes on `click`, not `pointerdown`: several items open a
  // confirm dialog, whose "scrim closes" listener fires on pointerdown. Closing
  // the menu first would let that same press reach the new dialog and dismiss
  // it before it is seen, making those items look dead.
  useEffect(() => {
    if (!open) return;
    const onClick = (event: MouseEvent) => {
      const target = event.target as Node;
      if (panelRef.current?.contains(target) || anchorRef.current?.contains(target)) return;
      setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        setOpen(false);
      }
    };
    const onScroll = () => setOpen(false);
    window.addEventListener("click", onClick);
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("resize", onScroll);
    window.addEventListener("blur", onScroll);
    return () => {
      window.removeEventListener("click", onClick);
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("resize", onScroll);
      window.removeEventListener("blur", onScroll);
    };
  }, [open]);

  return (
    <>
      <div ref={anchorRef} className="contents">
        {trigger({ open, toggle: () => setOpen((value) => !value) })}
      </div>
      {open &&
        createPortal(
          <MenuContext.Provider value={{ close }}>
            <div
              ref={panelRef}
              role="menu"
              style={{ top: pos.top, left: pos.left, width }}
              className={cn(
                "fixed z-[60] rounded-[var(--radius-md)] border border-line-strong bg-raised p-1 shadow-[var(--shadow-md)]",
                "[animation:menu-in_110ms_var(--ease)]",
                className,
              )}
            >
              {children}
            </div>
            <style>{`@keyframes menu-in { from { opacity: 0; transform: translateY(-3px) } to { opacity: 1; transform: none } }`}</style>
          </MenuContext.Provider>,
          document.body,
        )}
    </>
  );
}

type ItemProps = {
  children: ReactNode;
  onSelect?: () => void;
  icon?: ReactNode;
  shortcut?: string[];
  danger?: boolean;
  disabled?: boolean;
};

export function MenuItem({ children, onSelect, icon, shortcut, danger, disabled }: ItemProps) {
  const { close } = useContext(MenuContext);
  return (
    <button
      role="menuitem"
      disabled={disabled}
      onClick={() => {
        close();
        onSelect?.();
      }}
      className={cn(
        "flex h-7 w-full items-center gap-2 rounded-[var(--radius-sm)] px-2 text-left text-[12.5px]",
        "transition-colors duration-75 disabled:opacity-40",
        danger ? "text-danger-ink hover:bg-danger-wash" : "text-ink hover:bg-hover",
      )}
    >
      {icon && <span className="flex size-3.5 shrink-0 items-center justify-center text-ink-muted [&_svg]:size-[13px]">{icon}</span>}
      <span className="min-w-0 flex-1 truncate">{children}</span>
      {shortcut && (
        <span className="flex shrink-0 items-center gap-0.5 text-[11px] text-ink-faint">
          {shortcut.map((key, index) => (
            <span key={index}>{key}</span>
          ))}
        </span>
      )}
    </button>
  );
}

export function MenuSeparator() {
  return <div className="mx-1 my-1 h-px bg-line" />;
}

export function MenuLabel({ children }: { children: ReactNode }) {
  return (
    <div className="px-2 pb-1 pt-1.5 text-[10.5px] font-semibold uppercase tracking-[0.06em] text-ink-faint">
      {children}
    </div>
  );
}
