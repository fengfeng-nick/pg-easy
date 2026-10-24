import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import type { Extension } from "@codemirror/state";

/**
 * 编辑器通过 CSS 变量借用应用的 design token。用变量（而不是编译好的
 * Theme）构建 CSS 基主题，切换主题时就不必重建 compartment，
 * CodeMirror 的外观也能与窗口其余部分保持一致。
 */
const baseTheme = EditorView.theme({
  "&": {
    height: "100%",
    fontSize: "13px",
    backgroundColor: "var(--surface)",
    color: "var(--ink)",
  },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily: "var(--font-mono)",
    lineHeight: "1.65",
    padding: "10px 0 40px",
  },
  ".cm-content": { caretColor: "var(--accent)", paddingInline: "0" },
  ".cm-gutters": {
    backgroundColor: "var(--surface)",
    color: "var(--ink-faint)",
    border: "none",
    paddingRight: "8px",
    minWidth: "44px",
  },
  ".cm-lineNumbers .cm-gutterElement": {
    padding: "0 8px 0 12px",
    fontSize: "11.5px",
    fontVariantNumeric: "tabular-nums",
  },
  ".cm-activeLineGutter": {
    backgroundColor: "transparent",
    color: "var(--ink-soft)",
  },
  ".cm-activeLine": {
    backgroundColor: "color-mix(in oklab, var(--ink) 3%, transparent)",
  },
  ".cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection": {
    backgroundColor: "var(--selection) !important",
  },
  ".cm-cursor, .cm-dropCursor": {
    borderLeftColor: "var(--accent)",
    borderLeftWidth: "2px",
  },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground": {
    backgroundColor: "var(--selection)",
  },
  ".cm-panels": {
    backgroundColor: "var(--chrome)",
    color: "var(--ink)",
    borderColor: "var(--line)",
  },
  ".cm-search": {
    padding: "6px 10px",
    borderBottom: "1px solid var(--line)",
    fontSize: "12px",
  },
  ".cm-search input, .cm-search button": {
    height: "24px",
    borderRadius: "4px",
    border: "1px solid var(--line-strong)",
    backgroundColor: "var(--raised)",
    color: "var(--ink)",
    fontSize: "12px",
  },
  ".cm-search label": { fontSize: "11.5px", color: "var(--ink-soft)" },
  ".cm-searchMatch": {
    backgroundColor: "color-mix(in oklab, var(--warn) 28%, transparent)",
    borderRadius: "2px",
  },
  ".cm-searchMatch.cm-searchMatch-selected": {
    backgroundColor: "color-mix(in oklab, var(--accent) 35%, transparent)",
  },
  ".cm-matchingBracket, &.cm-focused .cm-matchingBracket": {
    backgroundColor: "color-mix(in oklab, var(--accent) 16%, transparent)",
    outline: "1px solid color-mix(in oklab, var(--accent) 35%, transparent)",
  },
  ".cm-tooltip": {
    backgroundColor: "var(--raised)",
    border: "1px solid var(--line-strong)",
    borderRadius: "8px",
    boxShadow: "var(--shadow-md)",
    overflow: "hidden",
  },
  ".cm-tooltip-autocomplete > ul": {
    fontFamily: "var(--font-mono)",
    fontSize: "12px",
    maxHeight: "260px",
  },
  ".cm-tooltip-autocomplete > ul > li": {
    padding: "3px 10px 3px 8px",
    color: "var(--ink-soft)",
  },
  ".cm-tooltip-autocomplete > ul > li[aria-selected]": {
    backgroundColor: "var(--accent-wash)",
    color: "var(--accent-ink)",
  },
  ".cm-completionIcon": { opacity: "0.45", paddingRight: "6px" },
});

const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.operatorKeyword, t.controlKeyword], color: "var(--accent-ink)", fontWeight: "600" },
  { tag: [t.string, t.special(t.string)], color: "var(--string)" },
  { tag: [t.number, t.bool, t.null], color: "var(--number)" },
  { tag: [t.lineComment, t.blockComment], color: "var(--ink-muted)", fontStyle: "italic" },
  { tag: [t.typeName, t.standard(t.name), t.className], color: "var(--boolean)" },
  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: "var(--time)" },
  { tag: [t.propertyName, t.attributeName], color: "var(--ink)" },
  { tag: [t.name, t.variableName], color: "var(--ink)" },
  { tag: t.operator, color: "var(--ink-soft)" },
  { tag: t.punctuation, color: "var(--ink-muted)" },
  { tag: t.invalid, color: "var(--danger-ink)" },
]);

export const editorTheme: Extension = [baseTheme, syntaxHighlighting(highlight)];
