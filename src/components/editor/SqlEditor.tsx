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
  startCompletion,
  type CompletionSource,
} from "@codemirror/autocomplete";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { PostgreSQL, sql } from "@codemirror/lang-sql";
import type { Extension } from "@codemirror/state";
import type { PgCompletion } from "@/lib/pgComplete";
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
  completion: PgCompletion | null;
  readOnly?: boolean;
  onSelectionChange?: (selection: string | null) => void;
  onDocChange?: (doc: string) => void;
  /** 服务器报告的从 1 开始的位置，`stamp` 变化时聚焦到这里。 */
  errorAt?: { position: number; stamp: number } | null;
};

/** 只拿 PostgreSQL 方言做高亮。补全由 autocompletion 的 override 单独提供，
 *  不走 lang-sql 那份不区分子句的混合建议。 */
function buildLanguage(): Extension {
  return sql({ dialect: PostgreSQL, upperCaseKeywords: true });
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
    const initialLanguage = buildLanguage();

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
        autocompletion({
          // 编辑器只建一次，补全数据会随连接和懒加载的列更换，所以每次都读当前这份。
          // override 同时关掉 lang-sql 自带的那份：它不区分子句，FROM 后面也堆关键词。
          override: [
            ((context) => handlers.current.completion?.completionSource(context) ?? null) as CompletionSource,
          ],
          activateOnTyping: true,
          icons: true,
          maxRenderedOptions: 80,
          addToOptions: [
            {
              render: (completion) => {
                if (!completion.detail) return null;
                const detail = document.createElement("span");
                detail.className = "cm-completionDetail";
                detail.textContent = completion.detail;
                return detail;
              },
              position: 80,
            },
          ],
        }),
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
        // CodeMirror 只在输入标识符字符时才自动打开补全，空格不算。
        // `FROM `、`JOIN ` 这类位置正是表名该出现的地方。更新还在进行时
        // 不能直接开补全，所以放到这次更新之后。
        EditorView.updateListener.of((update) => {
          if (!update.docChanged || !handlers.current.completion) return;
          const cursor = update.state.selection.main.head;
          if (!update.state.selection.main.empty) return;
          const before = update.state.sliceDoc(Math.max(0, cursor - 24), cursor);
          if (!/\b(from|join|into|update|table|on|where|set)\s$/i.test(before)) return;
          const view = update.view;
          queueMicrotask(() => startCompletion(view));
        }),
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
