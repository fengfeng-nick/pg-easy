/**
 * 填充消息里的 `{name}` 占位符。未知的占位符原样保留，这样拼写错误会
 * 直接显现在界面上，而不是渲染成 “undefined”。
 */
export function fill(template: string, vars?: Record<string, string | number>): string {
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in vars ? String(vars[name]) : match,
  );
}
