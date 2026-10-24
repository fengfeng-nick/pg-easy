import { forwardRef, useEffect, useImperativeHandle, useRef } from "react";
import { Compartment, EditorState } from "@codemirror/state";
import {
  EditorView,
  crosshairCursor,
  drawSelection,
  dropCursor,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  placeholder,
  rectangularSelection,
  tooltips,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, foldGutter, foldKeymap, indentOnInput } from "@codemirror/language";
import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
  type CompletionSource,
} from "@codemirror/autocomplete";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { PostgreSQL, sql, type SQLConfig } from "@codemirror/lang-sql";
import type { Extension } from "@codemirror/state";
import type { CompletionData } from "@/lib/completion";
import { translate, type Language } from "@/lib/i18n";
import { useApp } from "@/lib/store";
import { isMac } from "@/lib/platform";
import { editorTheme } from "@/components/editor/theme";

export type SqlEditorHandle = {
  focus: () => void;
  getView: () => EditorView | null;
  replaceRange: (
    from: number,
    to: number,
    insert: string,
    select?: [number, number],
  ) => void;
  getSelectionOrLine: () => { text: string; from: number; to: number };
  revealPosition: (position: number) => void;
};

type Props = {
  value: string;
  /** 按连接共享，连接变化时由工作区替换。 */
  completion: CompletionData | null;
  readOnly?: boolean;
  onSelectionChange?: (selection: string | null) => void;
  onDocChange?: (doc: string) => void;
  /** 服务器报告的从 1 开始的位置，`stamp` 变化时聚焦到这里。 */
  errorAt?: { position: number; stamp: number } | null;
};

/** 语言加上本项目的补全源；用普通数组是为了让 CodeMirror 能把它当作
 *  一个 compartment 值。 */
function buildLanguage(data: CompletionData | null): Extension {
  const config: SQLConfig = {
    dialect: PostgreSQL,
    upperCaseKeywords: true,
  };
  if (data) {
    config.schema = data.namespace();
    // 连上库之后，未限定的表名按 public 补，而不是目录里碰巧排第一的 schema。
    config.defaultSchema = data.catalog?.schemas.some((item) => item.name === "public")
      ? "public"
      : data.catalog?.schemas[0]?.name;
    // 追加而不是 override：override 会关掉 lang-sql 自带的关键词、
    // 内置函数、表和列补全。挂到 SQL 语言数据上，字符串和注释里不触发。
    const support = sql(config);
    return [
      support,
      support.language.data.of({
        autocomplete: data.completionSource as CompletionSource,
      }),
      support.language.data.of({
        autocomplete: data.identifierSource as CompletionSource,
      }),
    ];
  }
  return sql(config);
}

function placeholderText(language: Language) {
  return placeholder(
    isMac ? translate(language, "workbench.placeholderMac") : translate(language, "workbench.placeholderOther"),
  );
}

export const SqlEditor = forwardRef<SqlEditorHandle, Props>(function SqlEditor(
  { value, completion, readOnly, onSelectionChange, onDocChange, errorAt },
  ref,
) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  // 这里指 CodeMirror 的“语言”（SQL 方言 + 补全），不是界面语言。
  const language = useRef(new Compartment());
  const placeholderSlot = useRef(new Compartment());
  const uiLanguage = useApp((state) => state.language);
  const fontScale = useApp((state) => state.settings?.fontScale ?? 100);
  const themeSlot = useRef(new Compartment());
  const handlers = useRef({ completion, onSelectionChange, onDocChange });
  handlers.current = { completion, onSelectionChange, onDocChange };

  useEffect(() => {
    if (!hostRef.current) return;
    const initialLanguage = buildLanguage(handlers.current.completion);

    const state = EditorState.create({
      doc: value,
      extensions: [
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightSpecialChars(),
        history(),
        foldGutter({ openText: "▾", closedText: "▸" }),
        drawSelection(),
        dropCursor(),
        EditorState.allowMultipleSelections.of(true),
        indentOnInput(),
        bracketMatching(),
        closeBrackets(),
        rectangularSelection(),
        crosshairCursor(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        // 提示框默认挂到 document.body，主题（cm-dark）和样式都加不上，
        // 列表会变成黑底黑字。挂回编辑器节点，主题和 z-index 才生效。
        tooltips({ parent: hostRef.current, position: "absolute" }),
        autocompletion({ activateOnTyping: true, icons: false, maxRenderedOptions: 80 }),
        language.current.of(initialLanguage),
        keymap.of([
          ...closeBracketsKeymap,
          ...defaultKeymap,
          ...searchKeymap,
          ...historyKeymap,
          ...foldKeymap,
          ...completionKeymap,
          indentWithTab,
        ]),
        placeholderSlot.current.of(placeholderText(uiLanguage)),
        themeSlot.current.of(editorTheme()),
        EditorState.readOnly.of(Boolean(readOnly)),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) handlers.current.onDocChange?.(update.state.doc.toString());
          if (update.selectionSet) {
            const { main } = update.state.selection;
            handlers.current.onSelectionChange?.(
              main.empty ? null : update.state.sliceDoc(main.from, main.to),
            );
          }
        }),
      ],
    });

    const view = new EditorView({ state, parent: hostRef.current });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // 编辑器在每个挂载的标签页上只创建一次；补全的变更通过下面的
    // 重配置 effect 应用。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 占位文案在编辑器创建时就固定了；换语言时直接替换它，
  // 不动文档、选区或撤销历史。
  useEffect(() => {
    viewRef.current?.dispatch({
      effects: placeholderSlot.current.reconfigure(placeholderText(uiLanguage)),
    });
  }, [uiLanguage]);

  // 字号变了要重建主题：CodeMirror 创建时就把像素写死了。
  useEffect(() => {
    viewRef.current?.dispatch({
      effects: themeSlot.current.reconfigure(editorTheme()),
    });
  }, [fontScale]);

  // 重新配置语言只替换补全用的 schema，不影响文档、撤销历史和选区。
  useEffect(() => {
    viewRef.current?.dispatch({
      effects: language.current.reconfigure(buildLanguage(completion)),
    });
  }, [completion]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || view.state.doc.toString() === value) return;
    const anchor = Math.min(view.state.selection.main.head, value.length);
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: value },
      selection: { anchor },
    });
  }, [value]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || !errorAt) return;
    const position = Math.min(Math.max(errorAt.position - 1, 0), view.state.doc.length);
    view.dispatch({ selection: { anchor: position }, scrollIntoView: true });
  }, [errorAt]);

  useImperativeHandle(
    ref,
    () => ({
      focus: () => viewRef.current?.focus(),
      getView: () => viewRef.current,
      replaceRange: (from, to, insert, select) => {
        const view = viewRef.current;
        if (!view) return;
        const length = view.state.doc.length;
        const start = Math.min(from, length);
        const end = Math.min(Math.max(to, start), length);
        view.dispatch({
          changes: { from: start, to: end, insert },
          selection: select
            ? { anchor: start + select[0], head: start + select[1] }
            : { anchor: start + insert.length },
          scrollIntoView: true,
        });
      },
      getSelectionOrLine: () => {
        const view = viewRef.current;
        if (!view) return { text: "", from: 0, to: 0 };
        const { main } = view.state.selection;
        if (!main.empty) {
          return { text: view.state.sliceDoc(main.from, main.to), from: main.from, to: main.to };
        }
        const line = view.state.doc.lineAt(main.head);
        return { text: line.text, from: line.from, to: line.to };
      },
      revealPosition: (position) => {
        const view = viewRef.current;
        if (!view) return;
        const offset = Math.min(Math.max(position - 1, 0), view.state.doc.length);
        view.dispatch({ selection: { anchor: offset }, scrollIntoView: true });
      },
    }),
    [],
  );

  return <div ref={hostRef} className="relative h-full min-h-0 overflow-hidden" />;
});
