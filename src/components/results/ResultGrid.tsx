import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Check, ChevronDown, ChevronUp, Copy, Maximize2 } from "lucide-react";
import type { Column } from "@/lib/ipc";
import { cn } from "@/lib/cn";
import { useT } from "@/lib/i18n";
import { formatCount, formatDuration, mod } from "@/lib/platform";
import { CellValue } from "@/components/results/value";
import { CellDetail } from "@/components/results/CellDetail";

const ROW_HEIGHT = 27;
const HEADER_HEIGHT = 30;
const GUTTER_WIDTH = 52;
const MIN_COL = 72;
const MAX_COL = 520;

function measureWidths(
  columns: Column[],
  rows: (string | null)[][],
  sample: number,
): number[] {
  // 宽度取表头与若干样本行中最长者，并做钳制。批量测量文本很便宜，
  // 逐行测量则浪费；取样已经足够，因为表格本身是虚拟滚动的，
  // 用户也会手动调宽或滚动。
  const counts = columns.map((column) => Math.min(24, column.name.length + 2));
  const limit = Math.min(rows.length, sample);
  for (let r = 0; r < limit; r += 1) {
    const row = rows[r];
    for (let c = 0; c < row.length; c += 1) {
      const value = row[c];
      const length = value === null ? 4 : Math.min(value.length, 120);
      if (length > counts[c]) counts[c] = length;
    }
  }
  return counts.map((chars) => Math.min(MAX_COL, Math.max(MIN_COL, chars * 7.6 + 24)));
}

type Props = {
  columns: Column[];
  rows: (string | null)[][];
  canLoadMore: boolean;
  loadingMore: boolean;
  loadedCount: number;
  onLoadMore: () => void;
  showRowNumbers?: boolean;
  /** 服务器在配置的行数上限处停止了收集。 */
  truncated?: boolean;
  /** 就地排序只会重排已加载的行；分页表的所有页并不都在内存里，
   *  因此对它关闭排序。 */
  sortable?: boolean;
  /** 这次查询的耗时，显示在结果栏。 */
  durationMs?: number;
};

export function ResultGrid({
  columns,
  rows,
  canLoadMore,
  loadingMore,
  loadedCount,
  onLoadMore,
  showRowNumbers = true,
  truncated = false,
  sortable = true,
  durationMs,
}: Props) {
  const t = useT();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [sort, setSort] = useState<{ index: number; dir: 1 | -1 } | null>(null);
  const [selected, setSelected] = useState<{ row: number; col: number } | null>(null);
  const [detail, setDetail] = useState<{ row: number; col: number; rect: DOMRect } | null>(null);
  const [copied, setCopied] = useState(false);
  const widths = useMemo(() => measureWidths(columns, rows, 80), [columns, rows]);

  const sortedRows = useMemo(() => {
    if (!sort) return rows;
    const { index, dir } = sort;
    // 两个方向都把 NULL 排在最后，与 PostgreSQL 的默认排序一致。
    return [...rows].sort((a, b) => {
      const left = a[index];
      const right = b[index];
      if (left === right) return 0;
      if (left === null) return 1;
      if (right === null) return -1;
      return left.localeCompare(right, undefined, { numeric: true }) * dir;
    });
  }, [rows, sort]);

  const totalWidth = widths.reduce((sum, width) => sum + width, 0) + (showRowNumbers ? GUTTER_WIDTH : 0);

  const virtualizer = useVirtualizer({
    count: sortedRows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  });

  const toggleSort = (index: number) => {
    setSort((current) => {
      if (!current || current.index !== index) return { index, dir: 1 };
      if (current.dir === 1) return { index, dir: -1 };
      return null;
    });
  };

  useEffect(() => {
    setSort(null);
    setSelected(null);
    scrollRef.current?.scrollTo({ top: 0 });
  }, [columns]);

  const copySelection = useCallback(async () => {
    if (!selected) return;
    const value = sortedRows[selected.row]?.[selected.col];
    await navigator.clipboard.writeText(value === null || value === undefined ? "" : value);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1200);
  }, [selected, sortedRows]);

  // 表格内的键盘导航。
  const onKeyDown = (event: React.KeyboardEvent) => {
    if (!selected) {
      if (["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight"].includes(event.key)) {
        event.preventDefault();
        setSelected({ row: 0, col: 0 });
      }
      return;
    }
    const { row, col } = selected;
    const lastRow = sortedRows.length - 1;
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        setSelected({ row: Math.min(lastRow, row + 1), col });
        break;
      case "ArrowUp":
        event.preventDefault();
        setSelected({ row: Math.max(0, row - 1), col });
        break;
      case "ArrowRight":
      case "Tab":
        if (event.key === "Tab" && event.shiftKey) {
          event.preventDefault();
          setSelected(row === 0 && col === 0 ? selected : { row: col === 0 ? row - 1 : row, col: col === 0 ? columns.length - 1 : col - 1 });
        } else {
          event.preventDefault();
          setSelected({
            row: col === columns.length - 1 ? Math.min(lastRow, row + 1) : row,
            col: col === columns.length - 1 ? 0 : col + 1,
          });
        }
        break;
      case "ArrowLeft":
        event.preventDefault();
        setSelected({ row, col: Math.max(0, col - 1) });
        break;
      case "Enter":
        event.preventDefault();
        setDetail({ row, col, rect: new DOMRect(0, 0, 0, 0) });
        break;
      case "Escape":
        setSelected(null);
        break;
      default:
        if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "c") {
          event.preventDefault();
          void copySelection();
        }
    }
  };

  useEffect(() => {
    if (!selected) return;
    virtualizer.scrollToIndex(selected.row, { align: "auto" });
  }, [selected, virtualizer]);

  return (
    <div className="relative flex h-full min-h-0 flex-col bg-surface">
      {(canLoadMore || truncated || loadingMore) && (
        <div className="flex shrink-0 items-center gap-3 border-b border-line bg-chrome/70 px-3 py-1.5 text-[11.5px] text-ink-muted">
          <span className="tabular">
            {t("grid.rowsLoaded", { count: formatCount(sortedRows.length) })}
          </span>
          {canLoadMore && (
            <button
              onClick={onLoadMore}
              disabled={loadingMore}
              className="flex items-center gap-1.5 rounded px-1.5 py-0.5 font-medium text-accent-ink hover:bg-accent-wash disabled:opacity-60"
            >
              {loadingMore && (
                <span className="size-2.5 animate-spin rounded-full border-[1.5px] border-current border-r-transparent" />
              )}
              {loadingMore ? t("grid.loading") : t("grid.loadMore", { count: formatCount(loadedCount) })}
            </button>
          )}
          {truncated && !canLoadMore && (
            <span>{t("grid.stoppedAtLimit")}</span>
          )}
        </div>
      )}

      <div
        ref={scrollRef}
        tabIndex={0}
        onKeyDown={onKeyDown}
        className="relative min-h-0 flex-1 overflow-auto outline-none"
        aria-label={t("grid.label")}
      >
        <div style={{ width: totalWidth, minWidth: "100%", position: "relative" }}>
          {/* 表头 */}
          <div
            className="sticky top-0 z-10 flex bg-chrome text-[11.5px] font-medium text-ink-soft"
            style={{ height: HEADER_HEIGHT }}
          >
            {showRowNumbers && (
              <div
                className="flex shrink-0 items-center justify-end border-b border-r border-line px-2 text-[10.5px] font-normal text-ink-faint"
                style={{ width: GUTTER_WIDTH }}
              >
                #
              </div>
            )}
            {columns.map((column, index) => (
              <button
                key={`${column.name}-${index}`}
                disabled={!sortable}
                onClick={() => sortable && toggleSort(index)}
                style={{ width: widths[index] }}
                className={cn(
                  "group flex shrink-0 items-center gap-1.5 border-b border-r border-line px-2.5 text-left",
                  "hover:bg-active",
                  selected?.col === index && "bg-active",
                )}
                title={
                  sortable
                    ? `${column.name} · ${column.typeName}\n${t("grid.clickToSort")}`
                    : `${column.name} · ${column.typeName}`
                }
              >
                <span className="truncate text-ink">{column.name}</span>
                <span className="ml-auto flex shrink-0 items-center gap-1">
                  <span className="font-mono text-[9.5px] font-normal text-ink-faint">
                    {column.typeName}
                  </span>
                  {sort?.index === index &&
                    (sort.dir === 1 ? (
                      <ChevronUp className="size-3 text-ink-soft" />
                    ) : (
                      <ChevronDown className="size-3 text-ink-soft" />
                    ))}
                </span>
              </button>
            ))}
          </div>

          {/* 数据行 */}
          <div
            style={{
              height: virtualizer.getTotalSize(),
              position: "relative",
            }}
          >
            {virtualizer.getVirtualItems().map((virtualRow) => {
              const row = sortedRows[virtualRow.index];
              const isSelectedRow = selected?.row === virtualRow.index;
              return (
                <div
                  key={virtualRow.key}
                  className={cn(
                    "absolute left-0 flex font-mono text-[12px] leading-[var(--row-h)]",
                    isSelectedRow ? "bg-active" : "hover:bg-[var(--grid-hover)]",
                  )}
                  style={{
                    top: virtualRow.start,
                    height: ROW_HEIGHT,
                    width: totalWidth,
                    lineHeight: `${ROW_HEIGHT - 1}px`,
                  }}
                >
                  {showRowNumbers && (
                    <div
                      className="flex shrink-0 items-center justify-end border-b border-r border-line px-2 tabular text-[10.5px] text-ink-faint"
                      style={{ width: GUTTER_WIDTH }}
                    >
                      {virtualRow.index + 1}
                    </div>
                  )}
                  {columns.map((column, colIndex) => {
                    const value = row[colIndex] ?? null;
                    const isSelected = isSelectedRow && selected?.col === colIndex;
                    return (
                      <div
                        key={colIndex}
                        onMouseDown={(event) => {
                          if (event.button === 0) {
                            setSelected({ row: virtualRow.index, col: colIndex });
                          }
                        }}
                        onDoubleClick={(event) =>
                          setDetail({
                            row: virtualRow.index,
                            col: colIndex,
                            rect: (event.target as HTMLElement).getBoundingClientRect(),
                          })
                        }
                        style={{ width: widths[colIndex] }}
                        className={cn(
                          "shrink-0 cursor-default truncate border-b border-r border-line px-2.5",
                          isSelected && "outline outline-1 -outline-offset-1 outline-accent",
                        )}
                      >
                        <CellValue value={value} kind={column.kind} />
                      </div>
                    );
                  })}
                </div>
              );
            })}
          </div>

          {sortedRows.length === 0 && (
            <div className="flex h-24 items-center justify-center text-[12.5px] text-ink-muted">
              {t("grid.noRows")}
            </div>
          )}
        </div>
      </div>

      {/* 选中信息栏：显示当前聚焦单元格的完整值。 */}
      <div className="flex h-[27px] shrink-0 items-center gap-3 border-t border-line bg-chrome px-3 text-[11px] text-ink-muted">
        {selected ? (
          <>
            <span className="tabular">
              {t("results.rowOf", {
                row: formatCount(selected.row + 1),
                column: columns[selected.col]?.name ?? "",
              })}
            </span>
            {durationMs != null && (
              <span className="tabular text-ink-faint">
                {t("results.elapsed", { duration: formatDuration(durationMs) })}
              </span>
            )}
            <span className="min-w-0 flex-1 truncate font-mono text-ink-soft">
              {sortedRows[selected.row]?.[selected.col] === null
                ? t("common.null")
                : (sortedRows[selected.row]?.[selected.col] ?? "")}
            </span>
            <button
              onClick={copySelection}
              className="flex items-center gap-1 rounded px-1.5 py-0.5 hover:bg-hover hover:text-ink"
            >
              {copied ? <Check className="size-3 text-ok" /> : <Copy className="size-3" />}
              {copied ? t("common.copied") : t("common.copy")}
            </button>
            <button
              onClick={() => setDetail({ row: selected.row, col: selected.col, rect: new DOMRect() })}
              className="flex items-center gap-1 rounded px-1.5 py-0.5 hover:bg-hover hover:text-ink"
            >
              <Maximize2 className="size-3" />
              {t("grid.view")}
            </button>
          </>
        ) : (
          <span className="flex items-center gap-3">
            <span className="tabular">
              {t("common.rows", { count: formatCount(sortedRows.length) })}
            </span>
            {durationMs != null && (
              <>
                <span className="text-ink-faint">·</span>
                <span className="tabular">
                  {t("results.elapsed", { duration: formatDuration(durationMs) })}
                </span>
              </>
            )}
            <span className="text-ink-faint">|</span>
            <span title={t("common.copyShortcutTip", { shortcut: mod })}>{t("grid.clickToInspect")}</span>
          </span>
        )}
      </div>

      {detail && (
        <CellDetail
          column={columns[detail.col]}
          value={sortedRows[detail.row]?.[detail.col] ?? null}
          anchor={detail.rect}
          onClose={() => setDetail(null)}
        />
      )}
    </div>
  );
}
