import { useApp } from "@/lib/store";
import { en, type Messages } from "./en";
import { fill } from "./fill";
import type { MessageKey, Vars } from "./messages";
import { zh } from "./zh";

export type Language = "en" | "zh";
export const LANGUAGES: { value: Language; label: string }[] = [
  { value: "en", label: "English" },
  { value: "zh", label: "中文" },
];

const catalogues: Record<Language, Messages> = { en, zh };

/** 任何缺失的译文都回退到英文。 */
export function catalogueFor(language: Language): Messages {
  return catalogues[language] ?? en;
}

function lookup(language: Language, key: string): string | undefined {
  let node: unknown = catalogues[language];
  for (const part of key.split(".")) {
    if (node === undefined || node === null) return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return typeof node === "string" ? node : undefined;
}

/** 解析单个 key。React 之外（actions、后端错误映射）调用时传入 store 里
 *  的语言；组件里应该用 `useT()`。 */
export function translate(language: Language, key: MessageKey, vars?: Vars): string {
  return fill(lookup(language, key) ?? lookup("en", key) ?? key, vars);
}

export type Translate = (key: MessageKey, vars?: Vars) => string;

export function useT(): Translate {
  const language = useApp((state) => state.language);
  return (key, vars) => translate(language, key, vars);
}

/** 当前语言，供 React 渲染之外运行的代码使用。 */
export function currentLanguage(): Language {
  return useApp.getState().language;
}

/** 插值一条消息，并按本地习惯给列出的数值加千分位，让 1234 显示成
 *  1,234（或 1 234）。 */
export function tn(
  t: Translate,
  key: MessageKey,
  vars: Record<string, string | number>,
  numeric: string[] = [],
): string {
  const grouped: Vars = { ...vars };
  for (const name of numeric) {
    if (typeof grouped[name] === "number") {
      grouped[name] = new Intl.NumberFormat().format(grouped[name] as number);
    }
  }
  return t(key, grouped);
}

export function t(key: MessageKey, vars?: Vars): string {
  return translate(currentLanguage(), key, vars);
}

/**
 * 首次启动时使用的语言：中文系统用中文，其他用英文。在设置里显式选择过
 * 之后会被持久化，并从此优先。
 */
export function systemLanguage(): Language {
  const tag =
    typeof navigator !== "undefined"
      ? (navigator.language ?? (navigator.languages?.[0] ?? ""))
      : "";
  return /^zh\b/i.test(tag) ? "zh" : "en";
}

export type { MessageKey, Vars };
