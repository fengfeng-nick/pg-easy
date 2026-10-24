import type { ValueKind } from "@/lib/ipc";
import { cn } from "@/lib/cn";

const kindClass: Record<Exclude<ValueKind, "null">, string> = {
  bool: "text-[var(--boolean)]",
  number: "text-[var(--number)]",
  text: "text-ink",
  json: "text-[var(--string)]",
  binary: "text-ink-muted font-normal",
  time: "text-[var(--time)]",
  uuid: "text-[var(--time)]",
  array: "text-ink-soft",
  other: "text-ink-soft",
};

/** Renders one cell value. NULL is deliberately quiet — it is the most common
 *  thing in a large table and should not compete with real data. */
export function CellValue({ value, kind }: { value: string | null; kind: ValueKind }) {
  if (value === null) {
    return <span className="select-none italic text-[var(--null)]">null</span>;
  }
  return (
    <span className={cn("whitespace-pre", kindClass[kind as Exclude<ValueKind, "null">] ?? "text-ink")}>
      {value}
    </span>
  );
}

/** A compact type badge for headers: "int8", "varchar", "jsonb". */
export function TypeBadge({ type }: { type: string }) {
  return (
    <span className="font-mono text-[10px] font-normal normal-case tracking-normal text-ink-faint">
      {type}
    </span>
  );
}
