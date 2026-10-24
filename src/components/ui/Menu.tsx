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

/** 锚定在触发器上的浮层菜单。渲染到 portal 里，这样永远不会被可滚动
 *  容器裁掉（结果表格里就有好几个）。 */
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
    // `contents` 没有布局盒，所以测量实际渲染出来的触发器。
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

  // 菜单在 `click` 而不是 `pointerdown` 上关闭：有几个菜单项会打开确认
  // 对话框，而对话框的“点遮罩关闭”监听在 pointerdown 上触发。若先关菜单，
  // 同一次按下会漏到新对话框上把它关掉，那些菜单项看起来就像失灵了。
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
