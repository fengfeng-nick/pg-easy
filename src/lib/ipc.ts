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
   * 即使服务器拒绝 TLS 也发送密码。对应 libpq 的 `PGSSLMODE=allow`；
   * 默认关闭，因为明文信道上的 SCRAM 正是降级攻击的样子。
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
  /** 语句在脚本中从 0 开始的位置。 */
  index: number;
  sql: string;
  /** 语句在脚本中的字节偏移，用于错误定位。 */
  start: number;
  hasRows: boolean;
  columns?: Column[];
  rows?: (string | null)[][];
  /** 语句影响的行数（INSERT/UPDATE/DELETE/MERGE）。 */
  affected: number;
  /** SELECT 返回的行数。 */
  returned: number;
  /** 服务器的命令标签，例如 "UPDATE 3"。 */
  tag?: string;
  durationMs: number;
  /** 到达的行数超过显示上限。 */
  truncated: boolean;
  /** 服务器在这条语句上已经没有更多行了。 */
  exhausted?: boolean;
  /** 由界面格式化的 token（`notice`、`notice_at`）。 */
  notices?: Translated[];
};

/** 脚本结束时，会话所处的事务状态。 */
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
  /** 为 `null` 时跟随系统语言，直到用户自己选一个。 */
  language: Language | null;
  /** 界面字号，相对默认的百分比。缺省或旧配置按 100 处理。 */
  fontScale?: number;
};

/** 每个命令都会返回的结构化错误。 */
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
  /** 本应用组合的消息 token（见 `lib/errors.ts`），或者 PostgreSQL 自己
   *  的文本，原样显示。 */
  message: string;
  /** 自由文本（服务器或 I/O），或者一个 token 加它的参数。 */
  detail?: string | Translated | null;
  /** `message` 的占位值，按消息期望的顺序排列。 */
  args?: (string | number)[];
  code?: string;
  /** 错误在失败语句中从 1 开始的字符位置。 */
  position?: number;
  /** 出错的语句从 0 开始的索引。 */
  statementIndex?: number;
  txStatus?: TxStatus;
  /** 失败之前已经执行完成的语句。 */
  results?: StatementResult[];
  resultsDurationMs?: number;
  /** 失败的那条语句此前已经收到的行。 */
  rows?: StatementResult;
};

/** 导出对话框询问的内容。 */
export type DumpOptions = {
  /** 决定生成的文件名，并出现在脚本头部。 */
  database: string;
  format: "sql" | "zip";
  /** `false` 时只导出结构。 */
  includeData: boolean;
};

export type ExportedDump = {
  /** Base64：zip 不是 UTF-8。 */
  content: string;
  bytes: number;
  /** 贡献了数据的表数（只导出结构时为 0）。 */
  tables: number;
  durationMs: number;
};

export type DumpProgress = {
  exportId: string;
  done: number;
  total: number;
};

/** 用户选中的 SQL 文件，在读取任何内容之前先描述出来。 */
export type PickedSqlFile = {
  path: string;
  name: string;
  bytes: number;
};

/** 导入对话框询问的内容。 */
export type ImportOptions = {
  /** 整个脚本放在一个事务里：失败时不留任何残留。 */
  inTransaction: boolean;
};

export type ImportOutcome = {
  statements: number;
  /** COPY 块以及 INSERT/UPDATE/DELETE 写入的行数。 */
  rows: number;
  /** 其中经 COPY 写入的行数。 */
  copiedRows: number;
  bytes: number;
  durationMs: number;
};

export type ImportProgress = {
  importId: string;
  /** 已执行的语句数。 */
  statements: number;
  total: number;
  rows: number;
  copiedRows: number;
};

export function isAppError(value: unknown): value is AppError {
  return (
    typeof value === "object" &&
    value !== null &&
    "kind" in value &&
    "message" in value
  );
}

/** 跨 IPC 边界时命令从不以裸字符串拒绝，除非参数本身格式有误；两种形态
 *  都要归一化。 */
export function asAppError(value: unknown): AppError {
  if (isAppError(value)) return value;
  return {
    kind: "internal",
    // `lib/errors.ts` 会把它映射成界面语言。
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
  /** 同一条会话换库。会话 id 保持不变，所以已打开的标签页不用重建。 */
  switchDatabase: (id: string, database: string) =>
    invoke<SessionInfo>("switch_database", { id, database }),
  loadCatalog: (id: string) => invoke<Catalog>("load_catalog", { id }),
  loadColumns: (id: string, schema: string, relation: string) =>
    invoke<ColumnInfo[]>("load_columns", { id, schema, relation }),

  /** 省略 `scope` 时是整个数据库，否则是所列的 `schema.table`。 */
  exportSql: (
    id: string,
    exportId: string,
    scope: string[] | null,
    options: DumpOptions,
  ) => invoke<ExportedDump>("export_sql", { id, exportId, scope, options }),
  /** 打开选择面板；返回选中的文件，取消时为 null。 */
  pickSqlFile: (title: string) => invoke<PickedSqlFile | null>("pick_sql_file", { title }),
  /** 把 .sql 文件回放到连接中。 */
  importSql: (id: string, importId: string, path: string, options: ImportOptions) =>
    invoke<ImportOutcome>("import_sql", { id, importId, path, options }),
  /** 打开保存面板；返回写入的路径，取消时为 null。 */
  saveFile: (title: string, defaultName: string, content: string) =>
    invoke<string | null>("save_file", { title, defaultName, content }),
  onDumpProgress: (handler: (progress: DumpProgress) => void): Promise<UnlistenFn> =>
    listen<DumpProgress>("dump:progress", (event) => handler(event.payload)),
  onImportProgress: (handler: (progress: ImportProgress) => void): Promise<UnlistenFn> =>
    listen<ImportProgress>("import:progress", (event) => handler(event.payload)),
};
