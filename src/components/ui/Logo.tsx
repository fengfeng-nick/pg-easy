import { useId } from "react";
import { cn } from "@/lib/cn";

/**
 * 应用标记：一摞数据层，右下角切进一道执行光标。
 *
 * 几何与 `src-tauri/icons/source.svg` 是同一份画稿——图标资源由
 * `scripts/make-icon.mjs` 从那份 SVG 渲染。改这里时请同步那边，否则窗口里
 * 的标记会和 Dock 里的图标长得不一样。
 *
 * `tile` 带深色底板（Dock、商店、欢迎页）；`mark` 去掉底板，线稿跟随
 * `currentColor`，光标则跳过描边间隙——它本来就不压在数据层上。
 */
export function Logo({
  variant = "tile",
  className,
  label,
}: {
  variant?: "tile" | "mark";
  className?: string;
  /** 作为有含义的图形出现时传入（例如欢迎页）；纯装饰则省略。 */
  label?: string;
}) {
  const uid = useId().replace(/[^a-zA-Z0-9]/g, "");
  const data = `data-${uid}`;
  const cursor = `cursor-${uid}`;
  const arrow = "M687 487 907 695 793 715 757 827Z";

  return (
    <svg
      viewBox="0 0 1024 1024"
      className={cn("shrink-0", className)}
      role={label ? "img" : undefined}
      aria-hidden={label ? undefined : true}
      aria-label={label}
    >
      {label && <title>{label}</title>}
      <defs>
        <linearGradient id={data} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#5ce0d1" />
          <stop offset="1" stopColor="#169b9b" />
        </linearGradient>
        <linearGradient id={cursor} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#ffd166" />
          <stop offset="1" stopColor="#ff8a3d" />
        </linearGradient>
      </defs>

      {variant === "tile" && <rect width="1024" height="1024" rx="236" fill="#101a1e" />}

      <g
        fill="none"
        stroke={variant === "tile" ? `url(#${data})` : "currentColor"}
        strokeWidth="52"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <ellipse cx="441" cy="303" rx="300" ry="104" />
        <path d="M141 303v180c0 57 134 104 300 104s300-47 300-104V303" />
        <path d="M141 483v180c0 57 134 104 300 104s300-47 300-104V483" />
      </g>

      {variant === "tile" && (
        // 底板色的描边把光标同数据层分开，看起来像真的压在数据上面。
        <path d={arrow} fill="#101a1e" stroke="#101a1e" strokeWidth="44" strokeLinejoin="round" />
      )}
      <path d={arrow} fill={`url(#${cursor})`} />
    </svg>
  );
}
