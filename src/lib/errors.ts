import { asAppError, type AppError } from "@/lib/ipc";
import {
  currentLanguage,
  translate,
  type Language,
  type MessageKey,
  type Vars,
} from "@/lib/i18n";

/**
 * `detail` 要么是自由文本——PostgreSQL 自己的措辞，或底层 I/O 错误，这些
 * 原样透传不翻译——要么是本应用围绕某个 token 组合出来的 `Translated`。
 */
export type Translated = {
  token: string;
  args?: (string | number)[];
};

export type Detail = string | Translated | null;

/** 每个命令都可能以这种结构失败（见 `ipc.ts` 里的 `AppError`）。 */
export type LocalizedError = Omit<AppError, "detail"> & {
  detail?: Detail;
  /** 按顺序填充消息占位符的值。 */
  args?: (string | number)[];
};

/**
 * 按约定，后端消息是英文；界面上用用户的语言显示。`AppError.message`
 * 对本应用组合的消息携带稳定 token（见 `BACKEND_MESSAGES`）；其余情况
 * ——PostgreSQL 自己的文本、意外失败——原样透传。
 */
const BACKEND_MESSAGES: Record<string, { key: MessageKey; vars?: string[] }> = {
  something_went_wrong: { key: "error.somethingWentWrong" },
  could_not_write_file: { key: "error.messages.couldNotWriteFile" },
  nothing_to_export: { key: "error.messages.nothingToExport" },
  nothing_to_run: { key: "error.messages.nothingToRun" },
  connections_file_read: { key: "error.messages.connectionsFileRead" },
  connections_file_open: { key: "error.messages.connectionsFileOpen" },
  keychain_unavailable: { key: "error.messages.keychainUnavailable" },
  keychain_save: { key: "error.messages.keychainSave" },
  query_cancelled: { key: "error.messages.queryCancelled" },
  export_cancelled: { key: "error.messages.exportCancelled" },
  session_closed: { key: "error.messages.sessionClosed" },
  connect_timed_out: { key: "error.messages.connectTimedOut", vars: ["host", "port"] },
  connect_failed: { key: "error.messages.connectFailed", vars: ["host", "port"] },
  not_accepting_tls: { key: "error.messages.notAcceptingTls", vars: ["host", "port"] },
  tls_handshake_failed: { key: "error.messages.tlsHandshakeFailed", vars: ["host"] },
  invalid_server_name: { key: "error.messages.invalidServerName", vars: ["host"] },
  server_said_plaintext: { key: "error.messages.serverSaidPlaintext" },
  root_cert_read: { key: "error.messages.rootCertRead" },
  root_cert_invalid: { key: "error.messages.rootCertInvalid" },
  root_cert_no_cert: { key: "error.messages.rootCertNoCert" },
  read_server_error: { key: "error.messages.readServerError" },
  encode_statement: { key: "error.messages.encodeStatement" },
  read_copy_data: { key: "error.messages.readCopyData" },
  connection_lost: { key: "error.messages.connectionLost" },
  read_columns: { key: "error.messages.readColumns" },
  read_row: { key: "error.messages.readRow" },
  read_tag: { key: "error.messages.readTag" },
  decode_message: { key: "error.messages.decodeMessage" },
  start_connection: { key: "error.messages.startConnection" },
  send_password: { key: "error.messages.sendPassword" },
  read_challenge: { key: "error.messages.readChallenge" },
  answer_challenge: { key: "error.messages.answerChallenge" },
  could_not_reach_server: { key: "error.messages.couldNotReachServer" },
  decode_login: { key: "error.messages.decodeLogin" },
  read_setting: { key: "error.messages.readSetting" },
  unexpected_rows: { key: "error.messages.unexpectedRows", vars: ["tag"] },
  cancel_not_acknowledged: { key: "error.messages.cancelNotAcknowledged" },
  copy_from_stdin: { key: "error.messages.copyFromStdin" },
  server_closed: { key: "error.messages.serverClosed" },
  auth_unsupported: { key: "error.messages.authUnsupported" },
  login_unexpected_message: { key: "error.messages.loginUnexpectedMessage" },
  login_no_auth_step: { key: "error.messages.loginNoAuthStep" },
  scram_unsupported: { key: "error.messages.scramUnsupported" },
  password_not_sent: { key: "error.messages.passwordNotSent" },
  login_timeout: { key: "error.messages.loginTimeout" },
  no_answer_for: { key: "error.messages.noAnswerFor", vars: ["seconds"] },
  closed_during_login: { key: "error.messages.closedDuringLogin" },
  unexpected_after_login: { key: "error.messages.unexpectedAfterLogin" },
  password_required: { key: "error.messages.passwordRequired" },
  password_required_for: { key: "error.messages.passwordRequiredFor", vars: ["user"] },
  login_accepted_no_finish: { key: "error.messages.loginAcceptedNoFinish" },
  no_tables_selected: { key: "error.messages.noTablesSelected" },
  relation_not_found: { key: "error.messages.relationNotFound", vars: ["name"] },
  io_path: { key: "error.messages.ioPath", vars: ["path", "error"] },
  tables_not_found: { key: "error.messages.tablesNotFound", vars: ["count", "list"] },
  table_not_found_hint: { key: "error.messages.tableNotFoundHint" },
  archive_failed: { key: "error.messages.archiveFailed" },
  connection_busy_export: { key: "error.messages.connectionBusyExport" },
  connection_busy_switch: { key: "error.messages.connectionBusySwitch" },
  database_required: { key: "error.messages.databaseRequired" },
  connection_busy_import: { key: "error.messages.connectionBusyImport" },
  could_not_read_file: { key: "error.messages.couldNotReadFile" },
  file_not_utf8: { key: "error.messages.fileNotUtf8" },
  nothing_to_import: { key: "error.messages.nothingToImport" },
  copy_block_unterminated: { key: "error.messages.copyBlockUnterminated" },
  import_cancelled: { key: "error.messages.importCancelled" },
};

/** 可以拿来填充 token 的位置参数。 */
const ARG = (index: number) => `{arg${index}}`;

function fillToken(
  token: string,
  args: (string | number)[] = [],
): { key: MessageKey; vars: Vars } | null {
  const entry = BACKEND_MESSAGES[token];
  if (!entry) return null;
  const vars: Vars = {};
  (entry.vars ?? []).forEach((name, index) => {
    vars[name] = args[index] ?? ARG(index);
  });
  return { key: entry.key, vars };
}

/** 当这条消息是本应用组合的（而不是转发服务器原文）时为真。 */
export function isOwnMessage(message: string): boolean {
  return message in BACKEND_MESSAGES;
}

/** 用当前语言格式化一个 token，例如 NOTICE 或 I/O 细节。 */
export function translateToken(
  token: Translated,
  language: Language = currentLanguage(),
): string {
  const filled = fillToken(token.token, token.args);
  return filled ? translate(language, filled.key, filled.vars) : token.token;
}

/** 错误对外显示的消息，使用当前语言。 */
export function errorMessage(error: LocalizedError): string {
  if (!isOwnMessage(error.message)) return error.message;
  const filled = fillToken(error.message, error.args);
  return filled ? translate(currentLanguage(), filled.key, filled.vars) : error.message;
}

/** 错误下方那一行细节，使用当前语言。 */
export function errorDetail(error: LocalizedError): string | undefined {
  if (!error.detail) return undefined;
  return typeof error.detail === "string" ? error.detail : translateToken(error.detail);
}

/** 命令被拒绝时，toast 或面板显示的内容。 */
export function describeError(value: unknown): {
  message: string;
  detail?: string;
  error: LocalizedError;
} {
  const error = asAppError(value) as LocalizedError;
  return { message: errorMessage(error), detail: errorDetail(error), error };
}
