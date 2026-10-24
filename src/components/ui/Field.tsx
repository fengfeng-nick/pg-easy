import { forwardRef, useId, type InputHTMLAttributes, type SelectHTMLAttributes, type ReactNode } from "react";
import { cn } from "@/lib/cn";

type FieldProps = {
  label: string;
  hint?: ReactNode;
  error?: string;
  children: (id: string) => ReactNode;
  className?: string;
};

export function Field({ label, hint, error, children, className }: FieldProps) {
  const id = useId();
  return (
    <div className={cn("flex flex-col gap-1.5", className)}>
      <label htmlFor={id} className="text-[11.5px] font-medium tracking-[0.01em] text-ink-soft">
        {label}
      </label>
      {children(id)}
      {error ? (
        <p className="text-[11.5px] text-danger-ink">{error}</p>
      ) : (
        hint && <p className="text-[11.5px] leading-snug text-ink-muted">{hint}</p>
      )}
    </div>
  );
}

const inputBase =
  "h-8 w-full rounded-[var(--radius-sm)] border border-line-strong bg-raised px-2.5 text-[13px] text-ink placeholder:text-ink-faint " +
  "transition-[border-color,box-shadow] duration-100 outline-none " +
  "focus:border-accent focus:ring-[3px] focus:ring-[color-mix(in_oklab,var(--accent)_16%,transparent)] " +
  "disabled:opacity-50";

export const Input = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement>>(
  function Input({ className, ...rest }, ref) {
    return <input ref={ref} className={cn(inputBase, "tabular", className)} {...rest} />;
  },
);

type SelectProps = SelectHTMLAttributes<HTMLSelectElement> & {
  options: { value: string; label: string }[];
};

export const Select = forwardRef<HTMLSelectElement, SelectProps>(function Select(
  { options, className, ...rest },
  ref,
) {
  return (
    <div className="relative">
      <select
        ref={ref}
        className={cn(inputBase, "appearance-none pr-7", className)}
        {...rest}
      >
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      <svg
        aria-hidden
        viewBox="0 0 10 6"
        className="pointer-events-none absolute right-2.5 top-1/2 h-[5px] w-[9px] -translate-y-1/2 text-ink-muted"
      >
        <path d="M1 1l4 4 4-4" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
      </svg>
    </div>
  );
});

type ToggleProps = {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  description?: string;
  id?: string;
};

export function Toggle({ checked, onChange, label, description, id }: ToggleProps) {
  const generated = useId();
  const controlId = id ?? generated;
  return (
    <div className="flex items-start justify-between gap-4">
      <div className="min-w-0">
        <label htmlFor={controlId} className="block text-[12.5px] font-medium text-ink">
          {label}
        </label>
        {description && (
          <p className="mt-0.5 text-[11.5px] leading-snug text-ink-muted">{description}</p>
        )}
      </div>
      <button
        id={controlId}
        role="switch"
        aria-checked={checked}
        onClick={() => onChange(!checked)}
        className={cn(
          "relative mt-0.5 h-[20px] w-[34px] shrink-0 rounded-full transition-colors duration-150",
          checked ? "bg-accent" : "bg-line-strong",
        )}
      >
        <span
          className={cn(
            "absolute top-[2px] size-4 rounded-full bg-white shadow-[0_1px_2px_rgba(0,0,0,0.25)] transition-[left] duration-150",
            checked ? "left-[16px]" : "left-[2px]",
          )}
        />
      </button>
    </div>
  );
}

/** 一组互斥选项，用于 SSL 模式和主题。 */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  className,
}: {
  value: T;
  options: { value: T; label: string; title?: string }[];
  onChange: (value: T) => void;
  className?: string;
}) {
  return (
    <div
      role="radiogroup"
      className={cn(
        "inline-flex items-center gap-0.5 rounded-[var(--radius-md)] border border-line bg-sunken p-0.5",
        className,
      )}
    >
      {options.map((option) => (
        <button
          key={option.value}
          role="radio"
          aria-checked={value === option.value}
          title={option.title}
          onClick={() => onChange(option.value)}
          className={cn(
            "h-6 rounded-[5px] px-2.5 text-[12px] font-medium transition-colors duration-100",
            value === option.value
              ? "bg-raised text-ink shadow-[var(--shadow-sm)]"
              : "text-ink-muted hover:text-ink",
          )}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}
