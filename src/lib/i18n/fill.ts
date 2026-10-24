/**
 * Fills `{name}` placeholders in a message. An unknown placeholder is left
 * as-is so a typo shows up in the UI instead of rendering "undefined".
 */
export function fill(template: string, vars?: Record<string, string | number>): string {
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in vars ? String(vars[name]) : match,
  );
}
