import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { cn } from "@/lib/cn";

type Variant = "primary" | "secondary" | "ghost" | "danger";
type Size = "sm" | "md";

type Props = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: Variant;
  size?: Size;
  icon?: ReactNode;
  shortcut?: ReactNode;
  loading?: boolean;
};

const variants: Record<Variant, string> = {
  primary:
    "bg-accent text-white hover:bg-accent-hover shadow-[inset_0_1px_0_rgba(255,255,255,0.12)] border border-transparent dark:text-[#0c1818] dark:font-semibold",
  secondary:
    "bg-raised text-ink border border-line-strong hover:bg-hover shadow-[var(--shadow-sm)]",
  ghost: "bg-transparent text-ink-soft hover:bg-hover hover:text-ink border border-transparent",
  danger: "bg-danger text-white hover:brightness-105 border border-transparent",
};

const sizes: Record<Size, string> = {
  sm: "h-7 px-2.5 gap-1.5 text-[12px] rounded-[var(--radius-sm)]",
  md: "h-8 px-3.5 gap-2 text-[13px] rounded-[var(--radius-md)]",
};

export const Button = forwardRef<HTMLButtonElement, Props>(function Button(
  { variant = "secondary", size = "md", icon, shortcut, loading, className, children, disabled, ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      disabled={disabled || loading}
      className={cn(
        "inline-flex select-none items-center justify-center whitespace-nowrap font-medium transition-[background-color,color,box-shadow] duration-100",
        "disabled:pointer-events-none disabled:opacity-45",
        variants[variant],
        sizes[size],
        className,
      )}
      {...rest}
    >
      {loading ? (
        <span className="size-3.5 animate-spin rounded-full border-[1.5px] border-current border-r-transparent opacity-70" />
      ) : (
        icon && <span className="flex shrink-0 items-center [&_svg]:size-[14px]">{icon}</span>
      )}
      {children}
      {shortcut && <span className="ml-1 flex items-center gap-0.5 opacity-60">{shortcut}</span>}
    </button>
  );
});

type IconButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  /** Used as the tooltip and the accessible name. */
  label: string;
  active?: boolean;
  size?: "sm" | "md";
  tone?: "default" | "danger";
};

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(
  function IconButton({ label, active, size = "md", tone = "default", className, children, ...rest }, ref) {
    return (
      <button
        ref={ref}
        title={label}
        aria-label={label}
        className={cn(
          "inline-flex shrink-0 items-center justify-center rounded-[var(--radius-sm)] transition-colors duration-100",
          "[&_svg]:size-[15px] [&_svg]:stroke-[1.75]",
          size === "sm" ? "size-6" : "size-7",
          tone === "danger"
            ? "text-ink-muted hover:bg-danger-wash hover:text-danger-ink"
            : active
              ? "bg-active text-ink"
              : "text-ink-muted hover:bg-hover hover:text-ink",
          "disabled:pointer-events-none disabled:opacity-40",
          className,
        )}
        {...rest}
      >
        {children}
      </button>
    );
  },
);
