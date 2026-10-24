import { useEffect, useMemo, useState } from "react";
import {
  ChevronRight,
  Database,
  Download,
  FunctionSquare,
  KeyRound,
  Layers,
  MoreHorizontal,
  Pencil,
  Plus,
  RefreshCw,
  Table2,
  Trash2,
  Unplug,
  X,
} from "lucide-react";
import {
  api,
  type Catalog,
  type RelationInfo,
  type SavedConnection,
} from "@/lib/ipc";
import {
  closeConnection,
  disconnectSaved,
  openConnection,
  refreshCatalog,
  removeSavedConnection,
  tableKey,
} from "@/lib/actions";
import { describeError } from "@/lib/errors";
import { cn } from "@/lib/cn";
import { useT } from "@/lib/i18n";
import { isMac, timeAgo } from "@/lib/platform";
import { useApp, type Connection, type View } from "@/lib/store";
import { IconButton } from "@/components/ui/Button";
import { ConfirmDialog } from "@/components/ui/Dialog";
import { Menu, MenuItem, MenuSeparator } from "@/components/ui/Menu";

export function Sidebar({
  onNewConnection,
  onEditConnection,
}: {
  onNewConnection: () => void;
  onEditConnection: (saved: SavedConnection) => void;
}) {
  const connections = useApp((state) => state.connections);
  const saved = useApp((state) => state.savedConnections);
  const t = useT();

  return (
    <aside className="flex h-full w-full flex-col overflow-hidden border-r border-line bg-chrome">
      <div className="flex h-9 shrink-0 items-center justify-between pl-3 pr-1.5">
        <span className="text-[10.5px] font-semibold uppercase tracking-[0.07em] text-ink-faint">
          {t("sidebar.connections")}
        </span>
        <IconButton label={t("sidebar.newConnection")} size="sm" onClick={onNewConnection}>
          <Plus />
        </IconButton>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto pb-4">
        {connections.length === 0 && saved.length === 0 && (
          <div className="px-3.5 pt-1">
            <p className="text-[11.5px] leading-relaxed text-ink-muted">
              {t("sidebar.noConnections")}
            </p>
            <button
              onClick={onNewConnection}
              className="mt-2 text-[12px] font-medium text-accent-ink hover:underline"
            >
              {t("sidebar.addServer")}
            </button>
          </div>
        )}

        <ul className="px-1.5">
          {connections.map((connection) => (
            <OpenConnectionRow
              key={connection.id}
              connection={connection}
              savedEntry={saved.find((item) => item.id === connection.savedId) ?? null}
              onEditConnection={onEditConnection}
            />
          ))}
        </ul>

        {saved.length > 0 && (
          <>
            <div className="flex h-8 items-center pl-3 pt-3">
              <span className="text-[10.5px] font-semibold uppercase tracking-[0.07em] text-ink-faint">
                {t("sidebar.saved")}
              </span>
            </div>
            <ul className="px-1.5">
              {saved.map((entry) => (
                <SavedConnectionRow
                  key={entry.id}
                  entry={entry}
                  onEdit={() => onEditConnection(entry)}
                />
              ))}
            </ul>
          </>
        )}
      </div>
    </aside>
  );
}

function OpenConnectionRow({
  connection,
  savedEntry,
  onEditConnection,
}: {
  connection: Connection;
  savedEntry: SavedConnection | null;
  onEditConnection: (saved: SavedConnection) => void;
}) {
  const [expanded, setExpanded] = useState(true);
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const setView = useApp((state) => state.setView);
  const view = useApp((state) => state.view);
  const openExport = useApp((state) => state.openExport);
  const pushToast = useApp((state) => state.pushToast);
  const t = useT();

  const txBadge =
    connection.txStatus === "intransaction"
      ? { label: t("status.inTransaction"), className: "text-warn bg-warn-wash" }
      : connection.txStatus === "inerror"
        ? { label: t("status.aborted"), className: "text-danger-ink bg-danger-wash" }
        : null;

  return (
    <li>
      <div className="group flex h-[30px] items-center gap-1 rounded-[var(--radius-sm)] pl-1 pr-1 hover:bg-hover">
        <button
          onClick={() => setExpanded((value) => !value)}
          aria-label={expanded ? t("sidebar.collapse") : t("sidebar.expand")}
          className="flex size-5 items-center justify-center rounded text-ink-muted hover:text-ink"
        >
          <ChevronRight
            className={cn("size-3.5 transition-transform duration-150", expanded && "rotate-90")}
          />
        </button>
        <Database className="size-[14px] shrink-0 text-accent" strokeWidth={1.75} />
        <button
          onClick={() => setExpanded(true)}
          className="min-w-0 flex-1 truncate text-left text-[12.5px] font-medium text-ink"
          title={`${connection.user}@${connection.host}:${connection.port}/${connection.database}`}
        >
          {connection.name}
        </button>
        {txBadge && (
          <span className={cn("shrink-0 rounded px-1 py-px text-[9.5px] font-medium", txBadge.className)}>
            {txBadge.label}
          </span>
        )}
        <span className="flex shrink-0 items-center opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
          <Menu
            width={210}
            align="end"
            trigger={({ toggle }) => (
              <button
                onClick={toggle}
                aria-label={t("sidebar.connectionActions")}
                className="flex size-5 items-center justify-center rounded text-ink-muted hover:bg-active hover:text-ink"
              >
                <MoreHorizontal className="size-[15px]" />
              </button>
            )}
          >
            <MenuItem icon={<RefreshCw />} onSelect={() => void refreshCatalog(connection.id)}>
              {t("sidebar.refreshSchema")}
            </MenuItem>
            <MenuItem
              icon={<Download />}
              onSelect={() =>
                openExport({ connectionId: connection.id, schema: null, scope: null })
              }
            >
              {t("sidebar.exportDatabase")}
            </MenuItem>
            <MenuItem
              icon={<Pencil />}
              disabled={!savedEntry}
              onSelect={() => savedEntry && onEditConnection(savedEntry)}
            >
              {t("sidebar.editSavedConnection")}
            </MenuItem>
            <MenuSeparator />
            <MenuItem icon={<Unplug />} danger onSelect={() => setConfirmDisconnect(true)}>
              {t("sidebar.disconnect")}
            </MenuItem>
            <MenuItem
              icon={<Trash2 />}
              danger
              disabled={!savedEntry}
              onSelect={() => setConfirmRemove(true)}
            >
              {t("sidebar.remove")}
            </MenuItem>
          </Menu>
        </span>
      </div>

      {expanded && <ConnectionTree connection={connection} view={view} onOpen={setView} />}

      <ConfirmDialog
        open={confirmDisconnect}
        title={t("sidebar.disconnectTitle", { name: connection.name })}
        body={t("sidebar.disconnectBody")}
        confirmLabel={t("sidebar.disconnect")}
        tone="danger"
        onCancel={() => setConfirmDisconnect(false)}
        onConfirm={() => {
          setConfirmDisconnect(false);
          void closeConnection(connection);
        }}
      />

      {savedEntry && (
        <ConfirmDialog
          open={confirmRemove}
          title={t("sidebar.removeTitle", { name: savedEntry.name })}
          body={t("sidebar.removeBodyOpen")}
          confirmLabel={t("sidebar.remove")}
          tone="danger"
          onCancel={() => setConfirmRemove(false)}
          onConfirm={() => {
            setConfirmRemove(false);
            removeSavedConnection(savedEntry.id).catch((failure) => {
              const { message, detail } = describeError(failure);
              pushToast({
                tone: "error",
                retitle: (t) => ({ title: t("sidebar.couldNotRemove"), detail: message }),
                detail,
              });
            });
          }}
        />
      )}
    </li>
  );
}

/** What the tree under a connection shows: each schema with the relations
 *  the filter keeps. Without a filter, sequences stay out of the way; typing
 *  finds them along with everything else. */
export function treeGroups(catalog: Catalog | null, filter: string) {
  if (!catalog) return [];
  const query = filter.trim().toLowerCase();
  return Object.entries(catalog.relations).map(([schema, relations]) => ({
    schema,
    tables: relations.filter(
      (relation) =>
        query
          ? relation.name.toLowerCase().includes(query) ||
            `${schema}.${relation.name}`.toLowerCase().includes(query)
          : relation.kind !== "sequence",
    ),
    routineCount: catalog.routines[schema]?.length ?? 0,
  }));
}

function ConnectionTree({
  connection,
  view,
  onOpen,
}: {
  connection: Connection;
  view: View;
  onOpen: (view: View) => void;
}) {
  const [filter, setFilter] = useState("");
  const t = useT();

  const groups = useMemo(
    () => treeGroups(connection.catalog, filter),
    [connection.catalog, filter],
  );

  if (connection.browsing && !connection.catalog) {
    return (
      <ul className="py-1.5 pl-[34px] pr-3">
        {[68, 84, 52].map((width, index) => (
          <li
            key={index}
            className="my-[7px] h-[9px] animate-pulse rounded-full bg-active"
            style={{ width, animationDelay: `${index * 140}ms` }}
          />
        ))}
      </ul>
    );
  }

  if (connection.catalogError) {
    return (
      <div className="px-3 py-1.5 pl-[34px] text-[11px] leading-snug text-ink-muted">
        <p className="break-words">{connection.catalogError}</p>
        <button
          onClick={() => void refreshCatalog(connection.id)}
          className="mt-1 text-accent-ink hover:underline"
        >
          {t("sidebar.reload")}
        </button>
      </div>
    );
  }

  if (!connection.catalog) return null;
  const matchCount = groups.reduce((sum, group) => sum + group.tables.length, 0);

  return (
    <div className="pb-1">
      <div className="py-1 pl-[28px] pr-2">
        <input
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
          placeholder={t("sidebar.filterTables")}
          spellCheck={false}
          className="h-6 w-full rounded-[4px] border border-line bg-raised/70 px-1.5 text-[11.5px] text-ink placeholder:text-ink-faint focus:border-line-strong"
        />
      </div>
      {filter && matchCount === 0 && (
        <div className="py-1 pl-[34px] text-[11px] text-ink-muted">{t("sidebar.noTablesMatch")}</div>
      )}
      {groups.map((group) =>
        group.tables.length === 0 ? null : (
          <SchemaGroup
            key={group.schema}
            schema={group.schema}
            tables={group.tables}
            routineCount={group.routineCount}
            connection={connection}
            forceOpen={Boolean(filter)}
            view={view}
            onOpen={onOpen}
          />
        ),
      )}
      <div className="px-3 pl-[34px] pt-1 text-[10.5px] text-ink-faint">
        {t("sidebar.updatedAgo", { when: timeAgo(connection.catalog.fetchedAt) })}
      </div>
    </div>
  );
}

function SchemaGroup({
  schema,
  tables,
  routineCount,
  connection,
  forceOpen,
  view,
  onOpen,
}: {
  schema: string;
  tables: RelationInfo[];
  routineCount: number;
  connection: Connection;
  forceOpen: boolean;
  view: View;
  onOpen: (view: View) => void;
}) {
  const [open, setOpen] = useState(schema === "public");
  const expanded = forceOpen || open;
  const openExport = useApp((state) => state.openExport);
  // Tables picked with ⌘/Ctrl-click or ⇧-click. Clicking a picked table opens
  // it and keeps the rest; clicking any other one drops the selection.
  const [picked, setPicked] = useState<string[]>([]);
  const t = useT();
  const [anchor, setAnchor] = useState<string | null>(null);
  // Only what is still shown: a filter that hides a picked table would leave
  // its name in the export without the user seeing it.
  const pickedTables = tables.filter((relation) => picked.includes(relation.name));

  // Escape clears a selection the way it would in a file list, except inside
  // a dialog, which closes itself instead.
  useEffect(() => {
    if (picked.length === 0) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      if (event.target instanceof Element && event.target.closest('[role="dialog"]')) return;
      setPicked([]);
      setAnchor(null);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [picked.length]);

  const select = (relation: RelationInfo, event: React.MouseEvent) => {
    if (event.shiftKey && anchor !== null) {
      const from = tables.findIndex((item) => item.name === anchor);
      const to = tables.findIndex((item) => item.name === relation.name);
      if (from !== -1 && to !== -1) {
        const range = tables.slice(Math.min(from, to), Math.max(from, to) + 1);
        setPicked(range.map((item) => item.name));
        return;
      }
    }
    setAnchor(relation.name);
    if (event.metaKey || event.ctrlKey) {
      setPicked((current) =>
        current.includes(relation.name)
          ? current.filter((name) => name !== relation.name)
          : [...current, relation.name],
      );
      return;
    }
    if (picked.length > 0 && picked.includes(relation.name)) {
      // Clicking one of several picked tables opens it and keeps the rest,
      // so the choice survives opening a table to look at it.
      onOpen({ kind: "table", connectionId: connection.id, schema, relation: relation.name });
      return;
    }
    setPicked([]);
    onOpen({ kind: "table", connectionId: connection.id, schema, relation: relation.name });
  };

  return (
    <div>
      <button
        onClick={() => setOpen((value) => !value)}
        className="group flex h-[26px] w-full items-center gap-1 pl-[28px] pr-2 text-left hover:bg-hover"
      >
        <ChevronRight
          className={cn(
            "size-3 shrink-0 text-ink-faint transition-transform duration-150",
            expanded && "rotate-90",
          )}
        />
        <Layers className="size-3.5 shrink-0 text-ink-muted" strokeWidth={1.6} />
        <span className="min-w-0 flex-1 truncate text-[12px] text-ink-soft">{schema}</span>
        <span className="tabular text-[10px] text-ink-faint opacity-0 group-hover:opacity-100">
          {tables.length}
        </span>
      </button>

      {expanded && (
        <ul>
          {tables.map((relation) => {
            const active =
              view.kind === "table" &&
              view.connectionId === connection.id &&
              view.schema === schema &&
              view.relation === relation.name;
            const isView = relation.kind === "view" || relation.kind === "matview";
            const isPicked = picked.includes(relation.name);
            return (
              <li key={relation.oid} className="group/row relative">
                <button
                  onClick={(event) => select(relation, event)}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    if (!isPicked) setPicked([relation.name]);
                  }}
                  className={cn(
                    "flex h-[26px] w-full items-center gap-1.5 pl-[44px] pr-2 text-left",
                    isPicked
                      ? "bg-accent-wash text-accent-ink"
                      : active
                        ? "bg-accent-wash text-accent-ink"
                        : "text-ink-soft hover:bg-hover",
                    isPicked && "pr-11",
                  )}
                  title={t("sidebar.relationTitle", {
                    schema,
                    relation: relation.name,
                    columns: relation.columns,
                    rows: relation.estRows.toLocaleString(),
                    size: relation.size,
                  }).replace("⌘-click", isMac ? "⌘-click" : "Ctrl-click")}
                >
                  {relation.kind === "sequence" ? (
                    <KeyRound className="size-3.5 shrink-0 opacity-70" strokeWidth={1.6} />
                  ) : (
                    <Table2
                      className={cn("size-3.5 shrink-0", isView && "opacity-70")}
                      strokeWidth={1.6}
                    />
                  )}
                  <span className="min-w-0 flex-1 truncate text-[12px]">{relation.name}</span>
                  {isView && (
                    <span className="shrink-0 text-[9.5px] uppercase tracking-[0.04em] text-ink-faint">
                      {t("sidebar.view")}
                    </span>
                  )}
                </button>
                <span
                  className={cn(
                    "absolute right-1 top-1/2 -translate-y-1/2",
                    isPicked
                      ? "opacity-100"
                      : "opacity-0 group-hover/row:opacity-100 focus-within:opacity-100",
                  )}
                >
                  <Menu
                    width={190}
                    align="end"
                    trigger={({ toggle }) => (
                      <button
                        onClick={toggle}
                        aria-label={t("sidebar.actionsFor", { name: relation.name })}
                        className="flex size-5 items-center justify-center rounded text-ink-muted hover:bg-active hover:text-ink"
                      >
                        <MoreHorizontal className="size-[14px]" />
                      </button>
                    )}
                  >
                    <MenuItem
                      icon={<Download />}
                      onSelect={() =>
                        openExport({
                          connectionId: connection.id,
                          schema: null,
                          // Whatever was picked plus the row this menu was
                          // opened on: right-clicking an unpicked row does
                          // not open a menu, so one or the other is present.
                          scope: [
                            ...new Set([
                              ...pickedTables.map((item) => tableKey(schema, item.name)),
                              tableKey(schema, relation.name),
                            ]),
                          ],
                        })
                      }
                    >
                      {isPicked && picked.length > 1
                        ? t("sidebar.exportTables", { count: picked.length })
                        : relation.kind === "table" || relation.kind === "partitioned"
                          ? t("sidebar.exportThisTable")
                          : t("sidebar.exportThisRelation")}
                    </MenuItem>
                    <MenuItem
                      icon={<Download />}
                      onSelect={() =>
                        openExport({
                          connectionId: connection.id,
                          schema,
                          scope: null,
                        })
                      }
                    >
                      {t("sidebar.exportAllInSchema", { schema })}
                    </MenuItem>
                    {isPicked && (
                      <>
                        <MenuSeparator />
                        <MenuItem
                          icon={<X />}
                          onSelect={() => {
                            setPicked([]);
                            setAnchor(null);
                          }}
                        >
                          {t("sidebar.clearSelection")}
                        </MenuItem>
                      </>
                    )}
                  </Menu>
                </span>
              </li>
            );
          })}
          {routineCount > 0 && (
            <li className="flex h-[24px] items-center gap-1.5 pl-[44px] text-[11px] text-ink-faint">
              <FunctionSquare className="size-3" />
              {t("sidebar.routines", { count: routineCount })}
            </li>
          )}
        </ul>
      )}
    </div>
  );
}

function SavedConnectionRow({
  entry,
  onEdit,
}: {
  entry: SavedConnection;
  onEdit: () => void;
}) {
  const [connecting, setConnecting] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const pushToast = useApp((state) => state.pushToast);
  const t = useT();
  const open = useApp(
    (state) =>
      state.connections.find(
        (connection) =>
          connection.user === entry.user &&
          connection.database === entry.database &&
          connection.host === entry.host &&
          connection.port === entry.port,
      ) ?? null,
  );

  const connect = async () => {
    setConnecting(true);
    try {
      let password = entry.password;
      if (entry.savePassword && !password) {
        password = (await api.readSavedPassword(entry.id)) ?? undefined;
      }
      await openConnection({ ...entry, password }, entry);
    } finally {
      setConnecting(false);
    }
  };

  return (
    <li>
      <div className="group flex h-[28px] items-center gap-1 rounded-[var(--radius-sm)] pl-[28px] pr-1 hover:bg-hover">
        <button
          onClick={onEdit}
          onDoubleClick={() => void connect()}
          className="min-w-0 flex-1 text-left"
          title={`${entry.user}@${entry.host}:${entry.port}/${entry.database}\n${t("sidebar.doubleClickToConnect")}`}
        >
          <span className="block truncate text-[12px] text-ink-soft">{entry.name}</span>
        </button>
        {open ? (
          <span className="shrink-0 pr-1 text-[10px] text-ink-faint">{t("sidebar.open")}</span>
        ) : (
          <button
            onClick={() => void connect()}
            disabled={connecting}
            className="h-[20px] shrink-0 rounded px-1.5 text-[10.5px] font-medium text-accent-ink opacity-0 hover:bg-accent-wash group-hover:opacity-100 disabled:opacity-70"
          >
            {connecting ? "…" : t("app.connect")}
          </button>
        )}
        <span className="shrink-0 opacity-0 group-hover:opacity-100">
          <Menu
            width={190}
            align="end"
            trigger={({ toggle }) => (
              <button
                onClick={toggle}
                aria-label={t("sidebar.savedActions")}
                className="flex size-5 items-center justify-center rounded text-ink-muted hover:bg-active hover:text-ink"
              >
                <MoreHorizontal className="size-[15px]" />
              </button>
            )}
          >
            <MenuItem
              icon={<Unplug />}
              onSelect={() => void disconnectSaved(open, entry.id)}
              disabled={!open}
            >
              {t("sidebar.disconnect")}
            </MenuItem>
            <MenuItem icon={<Pencil />} onSelect={onEdit}>
              {t("sidebar.edit")}
            </MenuItem>
            <MenuSeparator />
            <MenuItem icon={<Trash2 />} danger onSelect={() => setConfirmDelete(true)}>
              {t("sidebar.remove")}
            </MenuItem>
          </Menu>
        </span>
      </div>

      <ConfirmDialog
        open={confirmDelete}
        title={t("sidebar.removeTitle", { name: entry.name })}
        body={t("sidebar.removeBody")}
        confirmLabel={t("sidebar.remove")}
        tone="danger"
        onCancel={() => setConfirmDelete(false)}
        onConfirm={async () => {
          setConfirmDelete(false);
          try {
            const gone = await removeSavedConnection(entry.id);
            if (!gone) {
              pushToast({
                tone: "error",
                retitle: (t) => ({ title: t("sidebar.couldNotRemove") }),
              });
            }
          } catch (failure) {
            const { message, detail } = describeError(failure);
            pushToast({
              tone: "error",
              retitle: (t) => ({ title: t("sidebar.couldNotRemove"), detail: message }),
              detail,
            });
          }
        }}
      />
    </li>
  );
}
