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

/** Falls back to English for any key a translation is missing. */
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

/** Resolves one key. Outside React (actions, backend error mapping) call this
 *  with the language the store holds; components should use `useT()`. */
export function translate(language: Language, key: MessageKey, vars?: Vars): string {
  return fill(lookup(language, key) ?? lookup("en", key) ?? key, vars);
}

export type Translate = (key: MessageKey, vars?: Vars) => string;

export function useT(): Translate {
  const language = useApp((state) => state.language);
  return (key, vars) => translate(language, key, vars);
}

/** The current language, for code that runs outside a React render. */
export function currentLanguage(): Language {
  return useApp.getState().language;
}

/** Interpolates a message, grouping the listed numeric values so 1234 reads
 *  1,234 (or 1 234) as the locale expects. */
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
 * Language to open the first launch with: Chinese for Chinese-language
 * systems, English otherwise. An explicit choice in Settings is persisted and
 * wins from then on.
 */
export function systemLanguage(): Language {
  const tag =
    typeof navigator !== "undefined"
      ? (navigator.language ?? (navigator.languages?.[0] ?? ""))
      : "";
  return /^zh\b/i.test(tag) ? "zh" : "en";
}

export type { MessageKey, Vars };
