<div align="center">

<img src="brand/icon-1024.png" width="112" alt="pg easy" />

# pg easy

A fast, keyboard-first PostgreSQL client for macOS and Windows.

**English** · [简体中文](README.zh-CN.md)

[![License: MIT](https://img.shields.io/badge/license-MIT-3b82f6.svg)](LICENSE)
[![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)](https://tauri.app)
[![React 19](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black)](https://react.dev)
[![Rust 1.85+](https://img.shields.io/badge/Rust-1.85%2B-000000?logo=rust&logoColor=white)](https://www.rust-lang.org)
![Platforms](https://img.shields.io/badge/platform-macOS%2012%2B%20%7C%20Windows%2010%2B-lightgrey)

</div>

---

**pg easy** is a desktop PostgreSQL client built on [Tauri 2](https://tauri.app). The UI is React; the database side is Rust that speaks the PostgreSQL wire protocol directly — no Electron, no bundled `psql`, no JDBC layer. It opens instantly, keeps sessions in a native window, and stays out of the way with a keyboard-first workflow.

> **Status: v0.1.0 (early).** The core workflow — connect, browse, query, export, import — works and is covered by tests, but the UI and the IPC surface can still change between releases. Bugs and breaking changes are expected; issues are welcome.

## Contents

- [Features](#features)
- [Requirements](#requirements)
- [Getting started](#getting-started)
- [Keyboard shortcuts](#keyboard-shortcuts)
- [How it works](#how-it-works)
- [Where data lives](#where-data-lives)
- [Testing](#testing)
- [Not implemented yet](#not-implemented-yet)
- [Contributing](#contributing)
- [License](#license)

## Features

### Connections

- Saved connections in the sidebar; re-saving the same host/port/database/user updates the existing entry instead of piling up duplicates.
- SSL modes **Off / Prefer / Require / Verify**, with `Verify` as the default. Custom root CA in PEM for private CAs; empty means the OS trust store.
- Passwords go to the **macOS Keychain** or **Windows Credential Manager** — never into the config file. If the keychain is unavailable, the password simply isn't stored rather than being written in plaintext.
- Sending a password over an unencrypted channel is opt-in (needed for a local server with SSL off), off by default.
- Configurable connect timeout and session name (visible in `pg_stat_activity`).
- Live session status: PID, transaction state (`idle` / `in transaction` / `aborted`).

### Browsing

- Tree of databases → schemas → tables, views, sequences and routines, with column count, estimated rows and on-disk size.
- Switch database on the same connection without losing the tab.
- Table view: paginated rows, a structure tab (columns, types, nullability, defaults), and one-click export or "query this table".
- Filter tables by name; refresh schema on demand.

### Query editor

- CodeMirror 6 with PostgreSQL highlighting and **catalog-aware autocompletion** — tables are loaded once per connection, columns are fetched lazily and cached, so a 1000-table database doesn't slow down connecting.
- Multi-statement scripts: the script is split by a small lexer that understands string literals, `E''` escapes, dollar-quoted blocks, nested block comments and line comments. Every statement gets its own result card.
- A failure reports the statements that already succeeded before it, rather than discarding partial output.
- `Format SQL` via [sql-formatter](https://github.com/sql-formatter-org/sql-formatter).
- Cancel a running statement with `Esc` — the server is asked to abort it, not just ignored.

### Results

- Virtualized grid (TanStack Virtual) that stays smooth with 200k rows.
- Click a header to sort; load more rows a page at a time up to a configurable row limit.
- Cell inspector: pretty-printed JSON view for `json`/`jsonb` (and anything that looks like JSON), text wrap, copy value.
- Server notices, warnings and per-statement timing are surfaced instead of being swallowed.

### Export & import

- **Export** a whole database, every table in a schema, or a hand-picked set of tables — as a single `.sql` script, or a `.zip` split into `schema.sql` and `data.sql`.
- Structure only, or structure + data streamed with `COPY … TO STDOUT`.
- **Import** any `.sql` script, including `pg_dump` output and dumps made by this app. `COPY … FROM STDIN` blocks are streamed.
- Import either in one transaction (all-or-nothing) or statement by statement (needed for statements PostgreSQL refuses inside a transaction, e.g. `CREATE INDEX CONCURRENTLY`).
- Progress and cancel for both directions; long exports report table-by-table progress.

### App

- Command palette (<kbd>⌘</kbd><kbd>K</kbd> / <kbd>Ctrl</kbd><kbd>K</kbd>) to jump to a table or run a command.
- Light, dark and follow-system themes.
- English and 简体中文 UI (translation is type-checked, so a missing string fails the build instead of showing a blank label).
- Native window chrome, resizable/collapsible sidebar, drag-and-drop disabled on purpose.

## Requirements

| | |
|---|---|
| **OS** | macOS 12+ · Windows 10/11 |
| **Node.js** | 22 LTS or newer |
| **pnpm** | 9+ (the lockfile is `pnpm-lock.yaml`, lockfile version 9) |
| **Rust** | 1.85+ (edition 2021) |
| **macOS toolchain** | Xcode Command Line Tools (`xcode-select --install`) |
| **Windows toolchain** | MSVC build tools (Visual Studio 2022 "Desktop development with C++") + WebView2 (the installer bootstraps it) |

## Getting started

```bash
git clone https://github.com/fengfeng-nick/pg-easy.git
cd pg-easy
pnpm install
pnpm app:dev          # Tauri dev window with HMR
```

Build a release bundle:

```bash
pnpm app:build
# macOS:   src-tauri/target/release/bundle/dmg/*.dmg  (+ .app)
# Windows: src-tauri/target/release/bundle/nsis/*.exe (+ .msi)
```

If you only want to work on the UI, `pnpm dev` serves the renderer at <http://localhost:1420> — but the app needs the Rust side for anything database-related, so most work is done through `pnpm app:dev`.

### Scripts

| Command | What it does |
|---|---|
| `pnpm dev` | Vite dev server for the renderer only (port 1420) |
| `pnpm build` | Type-check (`tsc -b`) and build the renderer into `dist/` |
| `pnpm preview` | Serve the built renderer |
| `pnpm app:dev` | `tauri dev` — full app with HMR |
| `pnpm app:build` | `tauri build` — release bundles for the current OS |
| `pnpm test` | Vitest (frontend) |
| `pnpm test:watch` | Vitest in watch mode |
| `pnpm tauri …` | Raw Tauri CLI passthrough |

Icons are generated from one source drawing:

```bash
node scripts/make-icon.mjs                 # source.svg → source.png (macOS: uses sips)
npx tauri icon src-tauri/icons/source.png  # all platform sizes
```

## Keyboard shortcuts

`⌘` below means <kbd>⌘</kbd> on macOS and <kbd>Ctrl</kbd> on Windows.

| Shortcut | Action |
|---|---|
| <kbd>⌘</kbd><kbd>K</kbd> | Command palette: search tables and commands |
| <kbd>⌘</kbd><kbd>↵</kbd> | Run the statement under the cursor (or the selection) |
| <kbd>⌘</kbd><kbd>⇧</kbd><kbd>↵</kbd> | Run only the selected text |
| <kbd>F5</kbd> | Run the whole editor |
| <kbd>Esc</kbd> | Cancel the running statement · close the cell inspector · leave table view |
| <kbd>⌘</kbd><kbd>O</kbd> | New connection |
| <kbd>⌘</kbd><kbd>T</kbd> | New query tab (macOS only — use the toolbar button on Windows) |
| <kbd>↑</kbd> <kbd>↓</kbd> <kbd>←</kbd> <kbd>→</kbd> <kbd>Tab</kbd> | Move between cells in the result grid |
| <kbd>↵</kbd> | Open the cell inspector |
| <kbd>⌘</kbd><kbd>C</kbd> | Copy the selected cell |

## How it works

```
src/                     React 19 renderer — UI, Zustand store, CodeMirror, i18n
src-tauri/src/           Rust backend — PostgreSQL wire protocol, catalog, dump, import
  connection.rs          connect params, TLS (rustls), socket layer
  pg/                    message codec, type decoding, COPY FROM STDIN streams
  query.rs               session lifecycle, statement execution, paging, cancel
  sql.rs                 script splitter (quotes, dollar quoting, nested comments)
  catalog.rs             databases, schemas, relations, columns
  dump.rs                SQL dump planner + writer
  import.rs              script replay, progress, cancellation
  store.rs               connections/settings JSON + OS keychain
  commands.rs            the IPC surface (`invoke` handlers)
src-tauri/tests/         end-to-end tests against a real PostgreSQL server
```

A few decisions worth knowing about:

- **The driver is used at the protocol level**, not through a higher-level pooled client. That means one TLS connector serves every SSL mode, row types are reported exactly as the server describes them, and the transaction state in the status bar comes straight from `ReadyForQuery` instead of an extra round-trip.
- **Statements are executed one at a time.** A script is split client-side, then each statement is run and its result kept separately; a mid-script error still shows everything that succeeded before it.
- **Data streams.** Export writes `COPY … TO STDOUT` through a sink (deflate handled in pure Rust, so no C zlib in the build), and import feeds `COPY … FROM STDIN` chunks back. Neither direction holds the whole table in memory.
- **The renderer never sees a path it did not choose.** File dialogs and file writes happen in Rust; the webview only receives the path the user picked.
- **CSP is locked to `self` + IPC.** No remote scripts, no external fonts.
- **Secrets never touch disk as text.** Query text stays in memory; only connection metadata and preferences are persisted.

## Where data lives

Connection metadata and preferences are a single JSON file in the app config directory:

| OS | Path |
|---|---|
| macOS | `~/Library/Application Support/dev.pgeasy.app/connections.json` |
| Windows | `%APPDATA%\dev.pgeasy.app\connections.json` |

Passwords are **not** in that file — they live in the OS keychain under the service `dev.pgeasy.app`, keyed by connection id. Writes are atomic, and the file is `0600` on Unix. Deleting a saved connection removes its keychain entry; the database itself is never touched.

## Testing

```bash
pnpm test        # frontend unit tests (Vitest)
cargo test       # Rust unit + integration tests
```

The Rust integration tests in `src-tauri/tests/` talk to a **real** PostgreSQL server and are skipped unless `PG_TEST_URL` is set:

```bash
PG_TEST_URL=postgresql://user:password@localhost:5432/postgres \
  cargo test --manifest-path src-tauri/Cargo.toml -- --nocapture
```

They cover the wire protocol (`wire.rs`), catalog browsing (`catalog_browse.rs`) and a full dump → restore → compare round-trip on a database with partitions, identity and generated columns, foreign keys, views, triggers and a standalone sequence (`dump.rs`). Use a throwaway database: the dump test creates and drops schemas.

## Not implemented yet

Honest list of what you might miss:

- SSH tunneling (nothing in the codebase does it — you'd need a local port forward today).
- Linux bundles. The code builds for Linux (and `rfd` uses the XDG portal there), but releases target macOS and Windows only and nothing is tested on Linux.
- Multiple simultaneous connections per tab, or a split-view editor.
- Extension management (`CREATE EXTENSION` is just SQL, but there's no UI for installed extensions).
- Auto-update, signed installers, and prebuilt binaries — there are no GitHub Releases yet, so build from source.
- ER diagrams, query history, and saved snippets.

## Contributing

Issues, bug reports with a `PG_TEST_URL`-free repro, and pull requests are welcome.

1. Fork and branch from `main`.
2. Run `pnpm app:dev` and make sure the change works in the real app.
3. Add or update tests: `pnpm test`, and `cargo test` for backend changes.
4. Keep comments in the codebase's style — explain *why* a non-obvious decision was made, not what the line does.
5. Open the PR with a short description of the behaviour change and how you verified it.

Code style is enforced by tooling rather than a document: `tsc` in strict mode with `noUnusedLocals` and `noUnusedParameters`, `cargo fmt` / `cargo clippy` for Rust, and the i18n catalog is type-checked so adding a string means adding it to both `en` and `zh`.

## License

[MIT](LICENSE) © pg easy contributors

PostgreSQL is a trademark of the PostgreSQL Global Development Group. This project is not affiliated with or endorsed by them.
