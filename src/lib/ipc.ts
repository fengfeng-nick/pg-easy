import { invoke } from "@tauri-apps/api/core";
import type { Language } from "@/lib/i18n";
import type { Translated } from "@/lib/errors";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type SslMode = "disable" | "prefer" | "require" | "verify-full";

export type ConnectParams = {
  host: string;
  port: number;
  database: string;
  user: string;
  password?: string;
  ssl: SslMode;
  sslRootCert?: string;
  connectTimeoutMs?: number;
  applicationName?: string;
  /**
   * Send the password even if the server refuses TLS. libpq's
   * `PGSSLMODE=allow`; off by default because SCRAM over a clear-text channel
   * is what a downgrade attack looks like.
   */
  allowUnencryptedPassword?: boolean;
};

export type SavedConnection = ConnectParams & {
  id: string;
  name: string;
  savePassword: boolean;
};

export type SaveResult = { connection: SavedConnection; passwordSaved: boolean };

export type ValueKind =
  | "null"
  | "bool"
  | "number"
  | "text"
  | "json"
  | "binary"
  | "time"
  | "uuid"
  | "array"
  | "other";

export type Column = {
  name: string;
  typeName: string;
  typeOid: number;
  kind: ValueKind;
};

export type StatementResult = {
  /** 0-based position of the statement in the script. */
  index: number;
  sql: string;
  /** Byte offset of the statement in the script, for error mapping. */
  start: number;
  hasRows: boolean;
  columns?: Column[];
  rows?: (string | null)[][];
  /** Rows the statement modified (INSERT/UPDATE/DELETE/MERGE). */
  affected: number;
  /** Rows a SELECT returned. */
  returned: number;
  /** The server's command tag, e.g. "UPDATE 3". */
  tag?: string;
  durationMs: number;
  /** More rows arrived than the display limit. */
  truncated: boolean;
  /** The server has no further rows for this statement. */
  exhausted?: boolean;
  /** Tokens the UI formats (`notice`, `notice_at`). */
  notices?: Translated[];
};

/** Where the session's transaction stands when a script ends. */
export type TxStatus = "idle" | "intransaction" | "inerror";

export type ExecResult = {
  results: StatementResult[];
  totalDurationMs: number;
  txStatus: TxStatus;
};

export type Page = {
  offset: number;
  limit: number;
  rows: (string | null)[][];
  exhausted: boolean;
};

export type SessionInfo = {
  id: string;
  txStatus: TxStatus;
  serverVersion: string;
  fingerprint: string;
  backendPid: number;
};

export type ConnectionStatus = {
  id: string;
  txStatus: TxStatus;
  backendPid: number;
  serverVersion: string;
};

export type DatabaseInfo = {
  name: string;
  owner: string;
  encoding: string;
  size: string;
  allowConnections: boolean;
};

export type BrowseResult = {
  serverVersion: string;
  currentDatabase: string;
  currentUser: string;
  databases: DatabaseInfo[];
};

export type RelationInfo = {
  oid: number;
  schema: string;
  name: string;
  kind: "table" | "partitioned" | "view" | "matview" | "foreign" | "sequence";
  columns: number;
  estRows: number;
  size: string;
};

export type RoutineInfo = {
  schema: string;
  name: string;
  kind: "function" | "procedure" | "aggregate";
  arguments: string;
  returns: string;
};

export type Catalog = {
  schemas: { name: string }[];
  relations: Record<string, RelationInfo[]>;
  routines: Record<string, RoutineInfo[]>;
  fetchedAt: number;
};

export type ColumnInfo = {
  name: string;
  dataType: string;
  isNullable: boolean;
  isPrimaryKey: boolean;
  defaultValue: string | null;
  position: number;
};

export type Settings = {
  pageSize: number;
  maxRows: number;
  connectTimeoutMs: number;
  appName: string;
  /** `null` follows the system language until the user picks one. */
  language: Language | null;
};

/** Structured error returned by every command. */
export type AppError = {
  kind:
    | "database"
    | "connection"
    | "session"
    | "config"
    | "keychain"
    | "cancelled"
    | "busy"
    | "empty"
    | "noop"
    | "tls"
    | "sql"
    | "protocol"
    | "unsupported"
    | "auth"
    | "timeout"
    | "internal";
  /** A token for messages this app composed (see `lib/errors.ts`), or
   *  PostgreSQL's own text, shown as-is. */
  message: string;
  /** Free text (server or I/O), or a token with its arguments. */
  detail?: string | Translated | null;
  /** Placeholder values for `message`, in the order the message expects. */
  args?: (string | number)[];
  code?: string;
  /** 1-based character position of the error inside the failing statement. */
  position?: number;
  /** 0-based index of the statement that failed. */
  statementIndex?: number;
  txStatus?: TxStatus;
  /** Statements that completed before the failure. */
  results?: StatementResult[];
  resultsDurationMs?: number;
  /** Rows the failing statement had already received. */
  rows?: StatementResult;
};

/** What the export dialog asks for. */
export type DumpOptions = {
  /** Names the generated file and appears in the script header. */
  database: string;
  format: "sql" | "zip";
  /** `false` exports the structure only. */
  includeData: boolean;
};

export type ExportedDump = {
  /** Base64: a zip is not UTF-8. */
  content: string;
  bytes: number;
  /** Tables that contributed rows (0 when the structure was all that was asked for). */
  tables: number;
  durationMs: number;
};

export type DumpProgress = {
  exportId: string;
  done: number;
  total: number;
};

export function isAppError(value: unknown): value is AppError {
  return (
    typeof value === "object" &&
    value !== null &&
    "kind" in value &&
    "message" in value
  );
}

/** Commands never reject with plain strings across the IPC boundary except
 *  when the argument itself is malformed; normalise both shapes. */
export function asAppError(value: unknown): AppError {
  if (isAppError(value)) return value;
  return {
    kind: "internal",
    // `lib/errors.ts` maps this to the UI's language.
    message: "something_went_wrong",
    detail: typeof value === "string" ? value : JSON.stringify(value),
  } satisfies AppError;
}

export const api = {
  listConnections: () => invoke<SavedConnection[]>("list_connections"),
  saveConnection: (connection: SavedConnection, password: string | null) =>
    invoke<SaveResult>("save_connection", { connection, password }),
  deleteConnection: (id: string) =>
    invoke<SavedConnection[]>("delete_connection", { id }),
  readSavedPassword: (id: string) =>
    invoke<string | null>("read_saved_password", { id }),
  loadSettings: () => invoke<Settings>("load_settings"),
  saveSettings: (settings: Settings) =>
    invoke<void>("save_settings", { settings }),

  connect: (params: ConnectParams) =>
    invoke<SessionInfo>("connect", { params }),
  disconnect: (id: string) => invoke<void>("disconnect", { id }),
  connectionStatus: (id: string) =>
    invoke<ConnectionStatus | null>("connection_status", { id }),

  execute: (id: string, sql: string) =>
    invoke<ExecResult>("execute", { id, sql }),
  cancelQuery: (id: string) => invoke<void>("cancel_query", { id }),
  fetchMore: (id: string, sql: string, offset: number, limit?: number) =>
    invoke<Page>("fetch_more", { id, sql, offset, limit }),

  browse: (id: string) => invoke<BrowseResult>("browse", { id }),
  loadCatalog: (id: string) => invoke<Catalog>("load_catalog", { id }),
  loadColumns: (id: string, schema: string, relation: string) =>
    invoke<ColumnInfo[]>("load_columns", { id, schema, relation }),

  /** Whole database when `scope` is omitted, otherwise the named `schema.table`s. */
  exportSql: (
    id: string,
    exportId: string,
    scope: string[] | null,
    options: DumpOptions,
  ) => invoke<ExportedDump>("export_sql", { id, exportId, scope, options }),
  /** Opens a save panel; resolves to the path written, or null if cancelled. */
  saveFile: (title: string, defaultName: string, content: string) =>
    invoke<string | null>("save_file", { title, defaultName, content }),
  onDumpProgress: (handler: (progress: DumpProgress) => void): Promise<UnlistenFn> =>
    listen<DumpProgress>("dump:progress", (event) => handler(event.payload)),
};
