import { currentLanguage, translate } from "@/lib/i18n";

/** The two platforms get different modifier glyphs and a little different
 *  chrome. Keep this in one place so shortcut hints never disagree with the
 *  handlers. */
// The UA is only used to pick glyphs and default window padding; behaviour
// never branches on it, so the deprecation of userAgent sniffing is harmless.
export const isMac =
  navigator.userAgent.includes("Macintosh") ||
  navigator.platform?.startsWith("Mac") === true;

export const mod = isMac ? "⌘" : "Ctrl";
export const alt = isMac ? "⌥" : "Alt";
export const shift = isMac ? "⇧" : "Shift";

/** Formats a shortcut as an array of keycap labels. */
export function shortcut(...keys: string[]): string[] {
  return keys.map((key) => {
    if (key === "mod") return mod;
    if (key === "alt") return alt;
    if (key === "shift") return shift;
    if (key === "enter") return isMac ? "↩" : "Enter";
    if (key === "backspace") return isMac ? "⌫" : "Backspace";
    return key.toUpperCase();
  });
}

/** True when the platform's primary modifier is held. Alt/Ctrl are reserved
 *  for their native meanings on macOS, so never treat Ctrl as primary there. */
export function isPrimary(event: KeyboardEvent | React.KeyboardEvent): boolean {
  return isMac ? event.metaKey : event.ctrlKey;
}

export function isModified(event: KeyboardEvent | React.KeyboardEvent): boolean {
  return event.metaKey || event.ctrlKey || event.altKey;
}

/** Collapses a multi-line SQL preview into one line for logs and tabs. */
export function oneLine(text: string, max = 80): string {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
}

export function formatDuration(ms: number): string {
  if (ms < 1) return "<1 ms";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return `${minutes}m ${seconds}s`;
}

export function formatCount(value: number): string {
  return new Intl.NumberFormat().format(value);
}

/** "just now", "3 min ago" — used by the schema refresh label. */
export function timeAgo(timestamp: number): string {
  const language = currentLanguage();
  const seconds = Math.max(0, Math.round((Date.now() - timestamp) / 1000));
  if (seconds < 10) return translate(language, "time.justNow");
  if (seconds < 60) return translate(language, "time.seconds", { count: seconds });
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return translate(language, "time.minutes", { count: minutes });
  const hours = Math.round(minutes / 60);
  return translate(language, "time.hours", { count: hours });
}
