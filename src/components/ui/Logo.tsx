import { useId } from "react";
import { cn } from "@/lib/cn";

/**
 * 应用标记：一只带卷鼻子的卡通大象。
 *
 * 几何与 `src-tauri/icons/source.svg` 是同一份画稿——图标资源由
 * `scripts/make-icon.mjs` 从那份 SVG 渲染。改这里时请同步那边，否则窗口里
 * 的标记会和 Dock 里的图标长得不一样。
 *
 * `tile` 带深色底板（Dock、商店、欢迎页）；`mark` 去掉底板，保留大象
 * 的轮廓，方便在浅色界面里使用。
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
  const body = `body-${uid}`;
  const ear = `ear-${uid}`;
  const tusk = `tusk-${uid}`;
  const shadow = `shadow-${uid}`;

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
        <linearGradient id={body} x1="0.08" y1="0.03" x2="0.92" y2="0.97">
          <stop offset="0" stopColor="#6fe8d4" />
          <stop offset="0.55" stopColor="#37c5bb" />
          <stop offset="1" stopColor="#168e99" />
        </linearGradient>
        <linearGradient id={ear} x1="0.1" y1="0.05" x2="0.9" y2="0.95">
          <stop offset="0" stopColor="#48c9c1" />
          <stop offset="1" stopColor="#1c8c96" />
        </linearGradient>
        <linearGradient id={tusk} x1="0.15" y1="0" x2="0.85" y2="1">
          <stop offset="0" stopColor="#ffe7a1" />
          <stop offset="1" stopColor="#ffad4d" />
        </linearGradient>
        <filter id={shadow} x="-20%" y="-20%" width="140%" height="150%">
          <feDropShadow dx="0" dy="18" stdDeviation="14" floodColor="#071014" floodOpacity="0.35" />
        </filter>
      </defs>

      {variant === "tile" && <rect width="1024" height="1024" rx="236" fill="#101a1e" />}

      <g filter={`url(#${shadow})`}>
        <path
          d="M332 347C253 218 120 238 110 363c-8 102 41 222 143 246 66 15 125-24 144-86l-24-157c-10-8-25-14-41-19Z"
          fill={`url(#${ear})`}
          stroke="#0a171b"
          strokeWidth="24"
          strokeLinejoin="round"
        />
        <path
          d="M692 347c79-129 212-109 222 16 8 102-41 222-143 246-66 15-125-24-144-86l24-157c10-8 25-14 41-19Z"
          fill={`url(#${ear})`}
          stroke="#0a171b"
          strokeWidth="24"
          strokeLinejoin="round"
        />
        <path
          d="M281 370c-40-78-110-75-129-20-19 57 9 146 72 172 35 14 67-1 80-29l-17-123c-1 0-4 0-6 0Z"
          fill="#236f7b"
          opacity="0.72"
        />
        <path
          d="M743 370c40-78 110-75 129-20 19 57-9 146-72 172-35 14-67-1-80-29l17-123c1 0 4 0 6 0Z"
          fill="#236f7b"
          opacity="0.72"
        />
        <path
          d="M283 467c0-158 100-272 229-272s229 114 229 272v142c0 151-102 236-229 236S283 760 283 609V467Z"
          fill={`url(#${body})`}
          stroke="#0a171b"
          strokeWidth="26"
          strokeLinejoin="round"
        />
        <path
          d="M414 584c-61 18-83 67-56 117 14 26 44 27 61 2 20-29 34-62 36-98Z"
          fill={`url(#${tusk})`}
          stroke="#0a171b"
          strokeWidth="18"
          strokeLinejoin="round"
        />
        <path
          d="M610 584c61 18 83 67 56 117-14 26-44 27-61 2-20-29-34-62-36-98Z"
          fill={`url(#${tusk})`}
          stroke="#0a171b"
          strokeWidth="18"
          strokeLinejoin="round"
        />
        <circle cx="420" cy="438" r="35" fill="#0a171b" />
        <circle cx="604" cy="438" r="35" fill="#0a171b" />
        <circle cx="431" cy="426" r="10" fill="#f2fffb" />
        <circle cx="615" cy="426" r="10" fill="#f2fffb" />
        <path
          d="M382 382c26-26 66-31 97-12"
          fill="none"
          stroke="#0a171b"
          strokeWidth="18"
          strokeLinecap="round"
          opacity="0.82"
        />
        <path
          d="M642 382c-26-26-66-31-97-12"
          fill="none"
          stroke="#0a171b"
          strokeWidth="18"
          strokeLinecap="round"
          opacity="0.82"
        />
        <circle cx="365" cy="523" r="22" fill="#b2f7e4" opacity="0.35" />
        <circle cx="659" cy="523" r="22" fill="#b2f7e4" opacity="0.35" />
        <path
          d="M512 449c-54 0-88 37-88 90v113c0 49 33 80 78 80 51 0 87-30 87-72 0-28-17-46-43-46-22 0-37 15-37 37 0 8 2 15 7 20"
          fill="none"
          stroke="#0a171b"
          strokeWidth="96"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
        <path
          d="M512 449c-54 0-88 37-88 90v113c0 49 33 80 78 80 51 0 87-30 87-72 0-28-17-46-43-46-22 0-37 15-37 37 0 8 2 15 7 20"
          fill="none"
          stroke={`url(#${body})`}
          strokeWidth="68"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
        <path
          d="M475 723c17 11 42 13 61 6"
          fill="none"
          stroke="#a0f2df"
          strokeWidth="12"
          strokeLinecap="round"
          opacity="0.46"
        />
      </g>
    </svg>
  );
}
