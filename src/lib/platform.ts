import { currentLanguage, translate } from "@/lib/i18n";

/** 两个平台使用不同的修饰键符号，窗口外观也略有差别。集中放在一处，
 *  快捷键提示就不会和处理逻辑对不上。 */
// UA 只用来选符号和默认的窗口内边距；行为从不按它分支，所以
// userAgent 嗅探被弃用对我们没有影响。
export const isMac =
  navigator.userAgent.includes("Macintosh") ||
  navigator.platform?.startsWith("Mac") === true;

export const mod = isMac ? "⌘" : "Ctrl";
export const alt = isMac ? "⌥" : "Alt";
export const shift = isMac ? "⇧" : "Shift";

/** 把快捷键格式化成一组键帽标签。 */
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

/** 平台的“主”修饰键被按下时为真。macOS 上 Alt/Ctrl 保留其原生含义，
 *  所以那里永远不要把 Ctrl 当作主修饰键。 */
export function isPrimary(event: KeyboardEvent | React.KeyboardEvent): boolean {
  return isMac ? event.metaKey : event.ctrlKey;
}

export function isModified(event: KeyboardEvent | React.KeyboardEvent): boolean {
  return event.metaKey || event.ctrlKey || event.altKey;
}

/** 把多行 SQL 预览压成一行，供日志和标签页使用。 */
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

/** “刚刚”、“3 分钟前”——schema 刷新标签用。 */
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
