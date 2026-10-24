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
  /** Set while a dump streams out of this connection. */
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
    /** 0-based index of the statement the failure belongs to. */
    statementIndex?: number;
    kind: AppError["kind"];
    /** Placeholder values for a translated backend message. */
    args?: (string | number)[];
    /** Rows the failing statement had already received. */
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
  /** Selected text, kept in sync by the editor for "run selection". */
  selection: string | null;
  result: ResultState | null;
  activeResult: number;
};

export type Toast = {
  id: string;
  tone: "info" | "success" | "warn" | "error";
  title: string;
  detail?: string;
  /** How a translated toast re-renders itself when the language changes. */
  retitle?: (t: Translate) => { title: string; detail?: string };
};

export type View =
  | { kind: "editor" }
  | { kind: "table"; connectionId: string; schema: string; relation: string }
  | { kind: "settings" }
  | { kind: "welcome" };

/** Open export dialog: the whole connection, or the listed `schema.table`s. */
export type ExportRequest = {
  connectionId: string;
  /** `null` exports the whole database. */
  schema: string | null;
  /** `schema.table` keys; only read when `schema` is null and `scope` is given. */
  scope: string[] | null;
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
  /** `retitle` is preferred over `title` for text that should follow the
   *  language. */
  pushToast: (toast: Omit<Toast, "id" | "title"> & { title?: string }) => string;
  dismissToast: (id: string) => void;
  openCommandPalette: (open: boolean) => void;
  openExport: (request: ExportRequest | null) => void;
};

let tabSeq = 0;
const nextTabId = () => `tab-${++tabSeq}`;

/** A `t` bound to one language, for code that runs outside React. */
export function translateTo(language: Language): Translate {
  return (key, vars) => translate(language, key, vars);
}

/** The starter query, in the user's language: the comment is prose, the SQL
 *  itself is not translated. */
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

  setBooted: (booted) => set({ booted }),
  setTheme: (theme) => set({ theme }),
  setResolvedTheme: (resolvedTheme) => set({ resolvedTheme }),
  setLanguage: (language) => {
    const before = get();
    const previousQuery = translate(before.language, "workbench.query");
    const nextQuery = translate(language, "workbench.query");
    set({
      language,
      // An untouched starter script follows the language; one the user has
      // edited keeps exactly what they wrote.
      tabs: before.tabs.map((tab) =>
        !tab.dirty && tab.sql === newTabSql(before.language)
          ? { ...tab, sql: newTabSql(language), title: tab.title.replace(previousQuery, nextQuery) }
          : tab,
      ),
    });
    // Toasts still on screen translate themselves.
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
        // A tabless window has nothing to show but the welcome screen.
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
        // Focus the tab that slid into the closed tab's place, like editors do.
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
    // "editor" is a request to show the current tab, not a tabless state.
    if (view.kind === "editor" && !get().activeTabId) {
      const last = get().tabs.at(-1);
      set(last ? { view, activeTabId: last.id } : { view: { kind: "welcome" } });
      return;
    }
    set({ view });
  },

  pushToast: (toast) => {
    const id = `toast-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
    // The toast is shown when it was raised; if the language changes while it
    // is still on screen it translates itself.
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
}));

export function useActiveTab(): Tab | null {
  return useApp((state) =>
    state.tabs.find((tab) => tab.id === state.activeTabId) ?? null,
  );
}
