import { api, type Column } from "@/lib/ipc";
import { toCsv, toInserts, toJson } from "@/lib/sql";

export type RowTable = {
  columns: Column[];
  rows: (string | null)[][];
};

/** `save_file` 收的是 base64。`btoa` 只接受 Latin-1，中文会直接抛掉。 */
export function encodeBase64(text: string) {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

export function rowsAsCsv(table: RowTable) {
  return toCsv(table);
}

export function rowsAsJson(table: RowTable) {
  return toJson(table);
}

export function rowsAsInserts(schema: string, relation: string, table: RowTable) {
  return toInserts(schema, relation, table);
}

/** 选中的行；没选中时用全部已加载的行。 */
export function rowsForCopy(table: RowTable, selectedRow: number | null) {
  if (selectedRow === null) return table;
  const row = table.rows[selectedRow];
  return row ? { columns: table.columns, rows: [row] } : table;
}

export async function saveRows(title: string, fileName: string, text: string) {
  return api.saveFile(title, fileName, encodeBase64(text));
}
