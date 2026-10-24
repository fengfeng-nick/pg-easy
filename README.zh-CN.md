<div align="center">

<img src="assets/icon-1024.png" width="112" alt="pg easy" />

# pg easy

一个快、以键盘为先的 PostgreSQL 桌面客户端，支持 macOS 与 Windows。

[English](README.md) · **简体中文**

[![License: MIT](https://img.shields.io/badge/license-MIT-3b82f6.svg)](LICENSE)
[![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)](https://tauri.app)
[![React 19](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black)](https://react.dev)
[![Rust 1.85+](https://img.shields.io/badge/Rust-1.85%2B-000000?logo=rust&logoColor=white)](https://www.rust-lang.org)
![Platforms](https://img.shields.io/badge/platform-macOS%2012%2B%20%7C%20Windows%2010%2B-lightgrey)

</div>

---

**pg easy** 是基于 [Tauri 2](https://tauri.app) 的桌面 PostgreSQL 客户端。界面用 React，数据库那一侧是直接用 Rust 说 PostgreSQL 线协议——没有 Electron，不打包 `psql`，也没有 JDBC 那一层。启动快、会话开在原生窗口里，日常操作基本不用离开键盘。

> **状态：v0.1.0（早期版本）。** 连接、浏览、查询、导出、导入这条主链路已经可用并且有测试覆盖，但界面和 IPC 接口在后续版本里仍可能变动。出现 bug 和不兼容改动是预期内的，欢迎提 issue。

## 目录

- [功能](#功能)
- [环境要求](#环境要求)
- [快速开始](#快速开始)
- [键盘快捷键](#键盘快捷键)
- [实现说明](#实现说明)
- [数据存在哪](#数据存在哪)
- [测试](#测试)
- [暂未实现](#暂未实现)
- [参与贡献](#参与贡献)
- [开源协议](#开源协议)

## 功能

### 连接

- 侧栏保存连接；同一台服务器、同一个库、同一个用户重复保存时更新原有条目，而不是堆出一串看不出区别的重复项。
- SSL 支持 **关闭 / 优先 / 强制 / 校验** 四档，默认 `校验`。可用 PEM 指定私有 CA 根证书，留空则走系统信任库。
- 密码存进 **macOS 钥匙串** 或 **Windows 凭据管理器**，绝不写进配置文件。钥匙串不可用时只是不保存密码，而不会以明文落盘。
- 「允许明文信道上传密码」默认关闭，需要本地无 SSL 的服务器时手动开启。
- 可配置连接超时和会话名（会话名会出现在 `pg_stat_activity` 里）。
- 状态栏显示实时会话状态：PID、事务状态（`idle` / `in transaction` / `aborted`）。

### 浏览

- 数据库 → schema → 表 / 视图 / 序列 / 函数 的树形结构，带列数、估算行数和占用空间。
- 同一条连接上切换数据库，标签页不用重建。
- 表视图：分页浏览数据、结构页（列、类型、可空、默认值），一键「导出」或「查询这张表」。
- 表名过滤、按需刷新 schema。

### SQL 编辑器

- CodeMirror 6，PostgreSQL 语法高亮，**按目录感知的自动补全**——表在连接时一次取好，列按关系懒加载并缓存，一千张表的库也不会因为补全拖慢连接。
- 多语句脚本：客户端用小词法器切分，认得字符串字面量、`E''` 转义、美元引用块、嵌套块注释和行注释，每条语句各有一块结果。
- 中途报错时，出错之前已经执行成功的语句结果会保留，不会被悄悄丢掉。
- 内置 `格式化 SQL`（[sql-formatter](https://github.com/sql-formatter-org/sql-formatter)）。
- `Esc` 取消正在执行的语句——是真的请求服务器中止，而不是让界面装作没在跑。

### 结果

- 虚拟滚动表格（TanStack Virtual），20 万行依然流畅。
- 点表头排序；分页加载更多，直到达到可配置的行数上限。
- 单元格详情：`json`/`jsonb`（以及看起来像 JSON 的值）自动美化、文本换行、复制。
- 服务端返回的 notice、warning 和每条语句的耗时都会展示出来，而不是吞掉。

### 导出与导入

- **导出**整个库、某个 schema 下的全部表，或手选若干张表；格式为单个 `.sql` 脚本，或拆成 `schema.sql` + `data.sql` 的 `.zip`。
- 可只导结构，也可结构 + 数据（用 `COPY … TO STDOUT` 流式写）。
- **导入**任意 `.sql` 脚本，包括 `pg_dump` 的产物和本应用自己导出的转储；`COPY … FROM STDIN` 数据块会流式写入。
- 导入可选单事务（一条失败则全部回滚）或逐条提交（`CREATE INDEX CONCURRENTLY` 这类不能在事务里跑的语句需要逐条）。
- 两个方向都有进度和取消；大导出会按表上报进度。

### 应用

- 命令面板（<kbd>⌘</kbd><kbd>K</kbd> / <kbd>Ctrl</kbd><kbd>K</kbd>）跳转到表或执行命令。
- 浅色、深色、跟随系统三档主题。
- 中英文界面（译文有类型检查，漏翻一条会编译报错，而不是界面上出现空白标签）。
- 原生窗口外观，侧栏可拖拽调宽/收起。

## 环境要求

| | |
|---|---|
| **系统** | macOS 12+ · Windows 10/11 |
| **Node.js** | 22 LTS 或更新 |
| **pnpm** | 9+（仓库使用 `pnpm-lock.yaml`，lockfile 版本 9） |
| **Rust** | 1.85+（edition 2021） |
| **macOS 工具链** | Xcode Command Line Tools（`xcode-select --install`） |
| **Windows 工具链** | MSVC 生成工具（Visual Studio 2022「使用 C++ 的桌面开发」）+ WebView2（安装包会自动引导） |

## 快速开始

```bash
git clone https://github.com/fengfeng-nick/pg-easy.git
cd pg-easy
pnpm install
pnpm app:dev          # 打开 Tauri 开发窗口，带热更新
```

打包发布版本：

```bash
pnpm app:build
# macOS:   src-tauri/target/release/bundle/dmg/*.dmg  （以及 .app）
# Windows: src-tauri/target/release/bundle/nsis/*.exe （以及 .msi）
```

只调界面时可以用 `pnpm dev`，渲染进程跑在 <http://localhost:1420>；但凡涉及数据库的功能都需要 Rust 那一侧，所以日常开发还是走 `pnpm app:dev`。

### 脚本

| 命令 | 作用 |
|---|---|
| `pnpm dev` | 仅启动渲染进程的 Vite 开发服务器（端口 1420） |
| `pnpm build` | 类型检查（`tsc -b`）并构建渲染进程到 `dist/` |
| `pnpm preview` | 预览构建产物 |
| `pnpm app:dev` | `tauri dev`，完整应用 + 热更新 |
| `pnpm app:build` | `tauri build`，为当前系统产出发布包 |
| `pnpm test` | Vitest（前端） |
| `pnpm test:watch` | Vitest 监听模式 |
| `pnpm tauri …` | 直接透传 Tauri CLI |

图标由一份源稿生成：

```bash
node scripts/make-icon.mjs                 # source.svg → source.png（macOS 走 sips）
npx tauri icon src-tauri/icons/source.png  # 生成各平台所需尺寸
```

## 键盘快捷键

下表的 `⌘` 在 Windows 上对应 <kbd>Ctrl</kbd>。

| 快捷键 | 动作 |
|---|---|
| <kbd>⌘</kbd><kbd>K</kbd> | 命令面板：搜索表和执行命令 |
| <kbd>⌘</kbd><kbd>↵</kbd> | 执行光标所在语句（或选中内容） |
| <kbd>⌘</kbd><kbd>⇧</kbd><kbd>↵</kbd> | 只执行选中的文本 |
| <kbd>F5</kbd> | 执行编辑器里的全部内容 |
| <kbd>Esc</kbd> | 取消正在执行的语句 · 关闭单元格详情 · 退出表视图 |
| <kbd>⌘</kbd><kbd>O</kbd> | 新建连接 |
| <kbd>⌘</kbd><kbd>T</kbd> | 新建查询标签页（仅 macOS，Windows 用工具栏按钮） |
| <kbd>↑</kbd> <kbd>↓</kbd> <kbd>←</kbd> <kbd>→</kbd> <kbd>Tab</kbd> | 在结果表格里移动单元格 |
| <kbd>↵</kbd> | 打开单元格详情 |
| <kbd>⌘</kbd><kbd>C</kbd> | 复制选中单元格 |

## 实现说明

```
src/                     React 19 渲染进程——界面、Zustand 状态、CodeMirror、国际化
src-tauri/src/           Rust 后端——PostgreSQL 线协议、目录、转储、导入
  connection.rs          连接参数、TLS（rustls）、socket 层
  pg/                    消息编解码、类型解码、COPY FROM STDIN 流
  query.rs               会话生命周期、语句执行、分页、取消
  sql.rs                 脚本切分（引号、美元引用、嵌套注释）
  catalog.rs             数据库、schema、关系、列
  dump.rs                SQL 转储的规划与写出
  import.rs              脚本回放、进度、取消
  store.rs               连接/配置 JSON + 系统钥匙串
  commands.rs            IPC 接口（`invoke` 处理函数）
src-tauri/tests/         面向真实 PostgreSQL 服务器的端到端测试
```

几处值得说明的取舍：

- **驱动是用在协议层的**，而不是套一个高层连接池客户端。这样一套 TLS connector 就能覆盖所有 SSL 模式，行类型完全按服务器的描述上报，状态栏的事务状态直接来自 `ReadyForQuery`，不必多一次往返。
- **语句逐条执行。** 脚本在客户端切分，每条语句单独执行、单独保留结果；脚本中途失败时，之前成功的结果依然在。
- **数据是流式的。** 导出通过 sink 写 `COPY … TO STDOUT`（deflate 用纯 Rust 实现，构建里不带 C 版 zlib），导入则把 `COPY … FROM STDIN` 的数据块喂回服务器。两个方向都不会把整张表读进内存。
- **渲染进程拿不到自己没选过的路径。** 文件选择面板和写文件都在 Rust 侧完成，webview 只收到用户亲自选定的路径。
- **CSP 收紧到 `self` + IPC。** 不加载远程脚本，不用外部字体。
- **密钥不以文本落盘。** 查询文本只留在内存，持久化的只有连接元数据和偏好设置。

## 数据存在哪

连接信息和偏好设置是应用配置目录下的一个 JSON 文件：

| 系统 | 路径 |
|---|---|
| macOS | `~/Library/Application Support/dev.pgeasy.app/connections.json` |
| Windows | `%APPDATA%\dev.pgeasy.app\connections.json` |

密码**不在**这个文件里——它存在系统钥匙串中，服务名为 `dev.pgeasy.app`，以连接 id 为键。写文件是原子的，Unix 下权限为 `0600`。删除一条已保存连接会同时删掉它的钥匙串条目，数据库本身不会被改动。

## 测试

```bash
pnpm test        # 前端单元测试（Vitest）
cargo test       # Rust 单元测试 + 集成测试
```

`src-tauri/tests/` 里的集成测试会连**真实**的 PostgreSQL 服务器，未设置 `PG_TEST_URL` 时自动跳过：

```bash
PG_TEST_URL=postgresql://user:password@localhost:5432/postgres \
  cargo test --manifest-path src-tauri/Cargo.toml -- --nocapture
```

覆盖内容包括线协议（`wire.rs`）、目录浏览（`catalog_browse.rs`），以及「导出 → 还原 → 逐对象比对」的完整往返（`dump.rs`，用到了一个带分区、标识列与生成列、外键、视图、触发器和一个独立序列的库）。请用一次性的库来跑：转储测试会创建和删除 schema。

## 暂未实现

一些你可能想找、但目前确实没有的东西：

- SSH 隧道（代码里没有，今天得自己在本地做端口转发）。
- Linux 安装包。代码能在 Linux 上编译（`rfd` 那边用 XDG portal），但发布只覆盖 macOS 与 Windows，且 Linux 上没有做过测试。
- 每个标签页同时连多个库，或分屏编辑器。
- 扩展管理（`CREATE EXTENSION` 就是 SQL，但没有做已安装扩展的界面）。
- 自动更新、签名安装包、预编译产物——目前没有 GitHub Releases，请从源码构建。
- ER 图、查询历史、代码片段收藏。

## 参与贡献

欢迎提 issue、带复现步骤的 bug 报告和 PR。

1. Fork 仓库，从 `main` 切分支。
2. 用 `pnpm app:dev` 跑起来，确认改动在真实应用里生效。
3. 补测试：`pnpm test`，后端改动跑 `cargo test`。
4. 注释保持现有风格——解释「为什么这么写」这类非显然的决策，而不是描述代码在做什么。
5. 提 PR 时简单说明行为变化和你如何验证的。

风格靠工具约束而不是靠文档：`tsc` 严格模式 + `noUnusedLocals` / `noUnusedParameters`，Rust 用 `cargo fmt` / `cargo clippy`；i18n 目录有类型检查，加一条文案就得同时加进 `en` 和 `zh`。

## 开源协议

[MIT](LICENSE) © pg easy contributors

PostgreSQL 是 PostgreSQL Global Development Group 的商标。本项目与其无隶属关系，也未获其背书。
