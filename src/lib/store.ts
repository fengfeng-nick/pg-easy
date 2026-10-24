import { create } from "zustand";
import type {
  AppError,
  Catalog,
  ConnectionStatus,
  SavedConnection,
  Settings,
  StatementResult,
  TxStatus,
} from "@/lib/ipc";
import { translate, type Language, type Translate } from "@/lib/i18n";

export type ThemeChoice = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

export type Connection = {
  id: string;
  savedId: string | null;
  name: string;
  user: string;
  database: string;
  host: string;
  port: number;
  serverVersion: string;
  backendPid: number;
  txStatus: TxStatus;
  catalog: Catalog | null;
  catalogError: string | null;
  browsing: boolean;
  open: boolean;
  /** 从这条连接导出转储期间置位。 */
  exporting?: { done: number; total: number } | null;
};

export type ResultState = {
  status: "running" | "done" | "error" | "cancelled";
  results: StatementResult[];
  error: {
    message: string;
    detail?: string;
    code?: string;
    position?: number;
    /** 失败所属语句从 0 开始的索引。 */
    statementIndex?: number;
    kind: AppError["kind"];
    /** 用于翻译后端消息的占位值。 */
    args?: (string | number)[];
    /** 失败的那条语句此前已经收到的行。 */
    rows?: StatementResult;
  } | null;
  startedAt: number;
  finishedAt?: number;
  totalDurationMs?: number;
};

export type Tab = {
  id: string;
  connectionId: string | null;
  title: string;
  sql: string;
  dirty: boolean;
  cursor: number;
  /** 选中的文本，由编辑器同步维护，供“执行选中部分”使用。 */
  selection: string | null;
  result: ResultState | null;
  activeResult: number;
};

export type Toast = {
  id: string;
  tone: "info" | "success" | "warn" | "error";
  title: string;
  detail?: string;
  /** 已翻译的 toast 在语言变化时如何自我重渲染。 */
  retitle?: (t: Translate) => { title: string; detail?: string };
};

export type View =
  | { kind: "editor" }
  | { kind: "table"; connectionId: string; schema: string; relation: string }
  | { kind: "settings" }
  | { kind: "welcome" };

/** 打开导出对话框：整个连接，或者列出的 `schema.table`。 */
export type ExportRequest = {
  connectionId: string;
  /** `null` 表示导出整个数据库。 */
  schema: string | null;
  /** `schema.table` 键；只在 `schema` 为 null 且给了 `scope` 时读取。 */
  scope: string[] | null;
};

/** 打开导入对话框：把选中的 .sql 文件回放到这个连接里。 */
export type ImportRequest = {
  connectionId: string;
};

type AppStore = {
  booted: boolean;
  theme: ThemeChoice;
  resolvedTheme: ResolvedTheme;
  language: Language;

  connections: Connection[];
  connectionError: Record<string, string>;
  connectingTo: string | null;

  savedConnections: SavedConnection[];
  settings: Settings | null;

  tabs: Tab[];
  activeTabId: string | null;
  view: View;

  sidebarWidth: number;
  editorHeightRatio: number;

  toasts: Toast[];
  commandPaletteOpen: boolean;
  exportRequest: ExportRequest | null;
  importRequest: ImportRequest | null;


  setBooted: (booted: boolean) => void;
  setTheme: (theme: ThemeChoice) => void;
  setResolvedTheme: (theme: ResolvedTheme) => void;
  setLanguage: (language: Language) => void;

  setSavedConnections: (connections: SavedConnection[]) => void;
  setSettings: (settings: Settings) => void;
  setSidebarWidth: (width: number) => void;
  setEditorHeightRatio: (ratio: number) => void;

  addConnection: (connection: Connection) => void;
  updateConnection: (id: string, patch: Partial<Connection>) => void;
  removeConnection: (id: string) => void;
  setConnectionError: (id: string, error: string | null) => void;
  setConnectingTo: (id: string | null) => void;
  mergeStatus: (status: ConnectionStatus & { connectionId: string }) => void;

  addTab: (tab: Partial<Tab> & { connectionId: string | null }) => string;
  updateTab: (id: string, patch: Partial<Tab>) => void;
  closeTab: (id: string) => void;
  activateTab: (id: string) => void;

  setView: (view: View) => void;
  /** 需要跟随语言的文本优先用 `retitle` 而非 `title`。 */
  pushToast: (toast: Omit<Toast, "id" | "title"> & { title?: string }) => string;
  dismissToast: (id: string) => void;
  openCommandPalette: (open: boolean) => void;
  openExport: (request: ExportRequest | null) => void;
  openImport: (request: ImportRequest | null) => void;
};

let tabSeq = 0;
const nextTabId = () => `tab-${++tabSeq}`;

/** 绑定到某一语言的 `t`，供 React 之外运行的代码使用。 */
export function translateTo(language: Language): Translate {
  return (key, vars) => translate(language, key, vars);
}

/** 起始查询，使用用户的语言：其中的注释是自然语言，SQL 本身不翻译。 */
function newTabSql(language: Language) {
  return `-- ${translate(language, "workbench.query")} 1
select now() as started_at,
       current_database() as database,
       current_user as "user";
`;
}

export const useApp = create<AppStore>((set, get) => ({
  booted: false,
  theme: "system",
  resolvedTheme: "light",
  language: "en",

  connections: [],
  connectionError: {},
  connectingTo: null,

  savedConnections: [],
  settings: null,

  tabs: [],
  activeTabId: null,
  view: { kind: "welcome" },

  sidebarWidth: 252,
  editorHeightRatio: 0.42,

  toasts: [],
  commandPaletteOpen: false,
  exportRequest: null,
  importRequest: null,

  setBooted: (booted) => set({ booted }),
  setTheme: (theme) => set({ theme }),
  setResolvedTheme: (resolvedTheme) => set({ resolvedTheme }),
  setLanguage: (language) => {
    const before = get();
    const previousQuery = translate(before.language, "workbench.query");
    const nextQuery = translate(language, "workbench.query");
    set({
      language,
      // 没被改过的起始脚本跟随语言；用户改过之后则完全保留他们写的内容。
      tabs: before.tabs.map((tab) =>
        !tab.dirty && tab.sql === newTabSql(before.language)
          ? { ...tab, sql: newTabSql(language), title: tab.title.replace(previousQuery, nextQuery) }
          : tab,
      ),
    });
    // 还留在屏幕上的 toast 会自己翻译。
    const t = translateTo(language);
    set((state) => ({
      toasts: state.toasts.map((toast) =>
        toast.retitle ? { ...toast, ...toast.retitle(t) } : toast,
      ),
    }));
  },

  setSavedConnections: (savedConnections) => set({ savedConnections }),
  setSettings: (settings) => set({ settings }),
  setSidebarWidth: (sidebarWidth) =>
    set({ sidebarWidth: Math.min(420, Math.max(200, sidebarWidth)) }),
  setEditorHeightRatio: (ratio) =>
    set({ editorHeightRatio: Math.min(0.8, Math.max(0.18, ratio)) }),

  addConnection: (connection) =>
    set((state) => ({ connections: [...state.connections, connection] })),

  updateConnection: (id, patch) =>
    set((state) => ({
      connections: state.connections.map((connection) =>
        connection.id === id ? { ...connection, ...patch } : connection,
      ),
    })),

  removeConnection: (id) =>
    set((state) => {
      const tabs = state.tabs.filter((tab) => tab.connectionId !== id);
      const activeTabId =
        state.activeTabId && tabs.some((tab) => tab.id === state.activeTabId)
          ? state.activeTabId
          : (tabs.at(-1)?.id ?? null);
      return {
        connections: state.connections.filter(
          (connection) => connection.id !== id,
        ),
        tabs,
        activeTabId,
        // 没有标签页的窗口只能显示欢迎页。
        view:
          state.view.kind === "table" && state.view.connectionId === id
            ? { kind: activeTabId ? "editor" : "welcome" }
            : !activeTabId && state.view.kind === "editor"
              ? { kind: "welcome" }
              : state.view,
      };
    }),

  setConnectionError: (id, error) =>
    set((state) => ({ connectionError: { ...state.connectionError, [id]: error ?? "" } })),

  setConnectingTo: (connectingTo) => set({ connectingTo }),

  mergeStatus: (status) =>
    set((state) => ({
      connections: state.connections.map((connection) =>
        connection.id === status.connectionId
          ? {
              ...connection,
              txStatus: status.txStatus,
              backendPid: status.backendPid,
            }
          : connection,
      ),
    })),

  addTab: (partial) => {
    const id = nextTabId();
    const state = get();
    const sameConnection = state.tabs.filter(
      (tab) => tab.connectionId === partial.connectionId,
    ).length;
    const language = state.language;
    const tab: Tab = {
      sql: newTabSql(language),
      title: `${translate(language, "workbench.query")} ${sameConnection + 1}`,
      dirty: false,
      cursor: 0,
      selection: null,
      result: null,
      activeResult: 0,
      ...partial,
      id,
      connectionId: partial.connectionId,
    };
    set({ tabs: [...state.tabs, tab], activeTabId: id, view: { kind: "editor" } });
    return id;
  },

  updateTab: (id, patch) =>
    set((state) => ({
      tabs: state.tabs.map((tab) => (tab.id === id ? { ...tab, ...patch } : tab)),
    })),

  closeTab: (id) =>
    set((state) => {
      const index = state.tabs.findIndex((tab) => tab.id === id);
      if (index === -1) return state;
      const tabs = state.tabs.filter((tab) => tab.id !== id);
      let activeTabId = state.activeTabId;
      if (state.activeTabId === id) {
        // 焦点落到滑进被关闭位置的标签页，和编辑器一致。
        activeTabId = (tabs[index] ?? tabs[index - 1] ?? null)?.id ?? null;
      }
      return {
        tabs,
        activeTabId,
        view:
          state.view.kind === "editor" && !activeTabId
            ? { kind: "welcome" }
            : state.view,
      };
    }),

  activateTab: (id) => set({ activeTabId: id, view: { kind: "editor" } }),

  setView: (view) => {
    // "editor" 是要求显示当前标签页，不是无标签页状态。
    if (view.kind === "editor" && !get().activeTabId) {
      const last = get().tabs.at(-1);
      set(last ? { view, activeTabId: last.id } : { view: { kind: "welcome" } });
      return;
    }
    set({ view });
  },

  pushToast: (toast) => {
    const id = `toast-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
    // toast 在产生时就显示了；如果它还在屏幕上时语言变了，它会自己翻译。
    const t = translateTo(get().language);
    const label = toast.retitle?.(t) ?? { title: toast.title ?? "" };
    set((state) => ({ toasts: [...state.toasts.slice(-3), { ...toast, ...label, id }] }));
    if (toast.tone !== "error") {
      setTimeout(() => get().dismissToast(id), 4200);
    }
    return id;
  },

  dismissToast: (id) =>
    set((state) => ({ toasts: state.toasts.filter((toast) => toast.id !== id) })),

  openCommandPalette: (commandPaletteOpen) => set({ commandPaletteOpen }),

  openExport: (exportRequest) => set({ exportRequest }),

  openImport: (importRequest) => set({ importRequest }),
}));

export function useActiveTab(): Tab | null {
  return useApp((state) =>
    state.tabs.find((tab) => tab.id === state.activeTabId) ?? null,
  );
}
