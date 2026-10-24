use std::sync::Arc;

use serde::Serialize;
use std::time::Instant;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::catalog;
use crate::connection::ConnectParams;
use crate::dump;
use crate::error::AppError;
use crate::import;
use crate::protocol::{
    BrowseResult, Catalog, ColumnInfo, ConnectionStatus, DumpOptions, ExecResult, ImportOptions,
    ImportOutcome, Page, PickedSqlFile, SavedConnection, SessionInfo,
};
use crate::query;
use crate::settings::Settings;
use crate::state::AppState;
use crate::store::{self, StoreFile};

type CmdResult<T> = Result<T, AppError>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub connection: SavedConnection,
    pub password_saved: bool,
}

#[tauri::command]
pub async fn list_connections(app: AppHandle) -> CmdResult<Vec<SavedConnection>> {
    Ok(StoreFile::load(&app)?.connections)
}

/// 新增或更新一条已保存连接。重复保存同一台服务器和同一个库时更新已有条目，
/// 而不是堆出列表分辨不出的重复项。提供了密码就写入钥匙串；没有提供时保留
/// 已存的那份（除非关闭了保存）。
#[tauri::command]
pub async fn save_connection(
    app: AppHandle,
    connection: SavedConnection,
    password: Option<String>,
) -> CmdResult<SaveResult> {
    let mut file = StoreFile::load(&app)?;
    let mut connection = connection;
    let is_new = connection.id.trim().is_empty();
    if is_new {
        connection.id = file
            .connections
            .iter()
            .find(|existing| {
                existing.params.user == connection.params.user
                    && existing.params.database == connection.params.database
                    && existing.params.host == connection.params.host
                    && existing.params.port == connection.params.port
            })
            .map(|existing| existing.id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
    }

    match file
        .connections
        .iter_mut()
        .find(|existing| existing.id == connection.id)
    {
        Some(existing) => *existing = connection.clone(),
        None => file.connections.push(connection.clone()),
    }
    file.save(&app)?;

    let mut password_saved = false;
    if connection.save_password {
        match password.as_deref().filter(|value| !value.is_empty()) {
            Some(password) => {
                store::save_password(&connection.id, password)?;
                password_saved = true;
            }
            // 编辑其他字段不能把之前保存的密码忘掉。
            None => password_saved = store::has_password(&connection.id),
        }
    } else if !is_new {
        store::delete_password(&connection.id);
    }

    Ok(SaveResult {
        connection,
        password_saved,
    })
}

#[tauri::command]
pub async fn delete_connection(app: AppHandle, id: String) -> CmdResult<Vec<SavedConnection>> {
    let mut file = StoreFile::load(&app)?;
    file.connections.retain(|connection| connection.id != id);
    file.save(&app)?;
    store::delete_password(&id);
    Ok(file.connections)
}

/// 为已保存连接预填编辑表单。按需读取而不是随列表一起下发，这样密钥在渲染
/// 进程内存中停留的时间不会超过表单打开的时间。
#[tauri::command]
pub async fn read_saved_password(id: String) -> CmdResult<Option<String>> {
    Ok(store::load_password(&id))
}

#[tauri::command]
pub async fn load_settings(state: State<'_, Arc<AppState>>) -> CmdResult<Settings> {
    Ok(state.settings().await)
}

#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    settings: Settings,
) -> CmdResult<()> {
    let mut file = StoreFile::load(&app)?;
    file.settings = settings.clone();
    file.save(&app)?;
    state.set_settings(settings).await;
    Ok(())
}

/// 一次连接尝试允许花多长时间，超时后报错，而不是让表单一直转圈。
const CONNECT_TIMEOUT_MS: u64 = 15_000;

/// 命令等待正在执行语句的会话最多多久。
const SESSION_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

#[tauri::command]
pub async fn connect(
    state: State<'_, Arc<AppState>>,
    params: ConnectParams,
) -> CmdResult<SessionInfo> {
    query::connect(&state, params, CONNECT_TIMEOUT_MS).await
}

#[tauri::command]
pub async fn disconnect(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<()> {
    state.remove(&id).await;
    Ok(())
}

/// 在同一条连接上换到 `database`。会话 id 不变，界面不用重建标签页。
#[tauri::command]
pub async fn switch_database(
    state: State<'_, Arc<AppState>>,
    id: String,
    database: String,
) -> CmdResult<SessionInfo> {
    query::switch_database(&state, &id, &database).await
}

#[tauri::command]
pub async fn connection_status(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> CmdResult<Option<ConnectionStatus>> {
    Ok(state.peek_status(&id).await)
}

/// 执行 `sql`。失败时错误里带着在此之前已完成的语句结果，部分成果不会被
/// 悄悄丢掉。
#[tauri::command]
pub async fn execute(
    state: State<'_, Arc<AppState>>,
    id: String,
    sql: String,
) -> CmdResult<ExecResult> {
    let handle = state.handle(&id).await?;
    let max_rows = state.settings().await.max_rows;
    query::execute_script(&handle, &sql, max_rows).await
}

/// 请服务器放弃这条会话上正在执行的语句。`execute` 调用本身会以取消错误
/// 返回。
#[tauri::command]
pub async fn cancel_query(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<()> {
    let handle = state.handle(&id).await?;
    query::request_cancel(&handle)
}

#[tauri::command]
pub async fn fetch_more(
    state: State<'_, Arc<AppState>>,
    id: String,
    sql: String,
    offset: usize,
    limit: Option<usize>,
) -> CmdResult<Page> {
    let handle = state.handle(&id).await?;
    let limit = match limit {
        Some(value) => value.clamp(1, 10_000),
        None => state.settings().await.page_size_clamped(),
    };
    query::fetch_page(&handle, &sql, offset, limit).await
}

#[tauri::command]
pub async fn browse(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<BrowseResult> {
    let handle = state.handle(&id).await?;
    let mut slot = handle.slot.lock().await;
    let session = &mut slot.session;
    let databases = catalog::load_overview(session).await?;
    Ok(BrowseResult {
        server_version: session.server_version.clone(),
        current_database: session.database.clone(),
        current_user: session.user.clone(),
        databases,
    })
}

#[tauri::command]
pub async fn load_catalog(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<Catalog> {
    let handle = state.handle(&id).await?;
    let mut slot = handle.slot.lock().await;
    catalog::load_catalog(&mut slot.session).await
}

/// 打开系统保存面板，把 `content`（base64）写到选定路径并返回它。面板和写入
/// 放在这里而不是渲染进程，渲染进程就不必持有一个不是自己选的路径。
#[tauri::command]
pub async fn save_file(
    title: String,
    default_name: String,
    content: String,
) -> CmdResult<Option<String>> {
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content)
        .map_err(AppError::unexpected)?;
    // 面板会阻塞；放到主线程之外运行，用户选文件夹时窗口仍能重绘。
    let Some(path) = tokio::task::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_title(&title)
            .set_file_name(&default_name)
            .save_file()
    })
    .await
    .map_err(AppError::unexpected)?
    else {
        return Ok(None);
    };
    std::fs::write(&path, bytes)
        .map_err(|err| AppError::new("io", "could_not_write_file").detail(err.to_string()))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// 构建整个数据库（`scope` 为空）或指定表的 SQL 转储，并以 base64 返回，
/// 因为 zip 不是 UTF-8。
///
/// 进度通过 `dump:progress` 事件（`{ exportId, done, total }`）上报，大导出
/// 期间对话框可以显示表数；调用在文件写完后才返回。
#[tauri::command]
pub async fn export_sql(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
    export_id: String,
    scope: Option<Vec<String>>,
    options: DumpOptions,
) -> CmdResult<ExportedDump> {
    let handle = state.handle(&id).await?;
    let slot = tokio::time::timeout(SESSION_WAIT, handle.slot.lock())
        .await
        .map_err(|_| {
            AppError::new("busy", "connection_busy_export")
        })?;
    let started = Instant::now();
    let mut slot = slot;
    slot.session.busy = true;

    let outcome = async {
        let relations = dump::plan(&mut slot.session, scope.as_deref(), options.include_data).await?;
        if relations.is_empty() {
            return Err(AppError::new("empty", "nothing_to_export"));
        }
        let total = relations
            .iter()
            .filter(|relation| relation.has_data)
            .count();
        let mut sink = ProgressSink {
            buffer: Vec::new(),
            total,
            app: &app,
            export_id: &export_id,
        };
        let tables = dump::run(
            &mut slot.session,
            &relations,
            &options,
            &mut sink,
            &|| false,
        )
        .await?;
        Ok((sink.buffer, tables))
    }
    .await;

    slot.session.busy = false;
    slot.session.idle_since = Instant::now();

    let (content, tables) = outcome?;
    Ok(ExportedDump {
        content: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &content),
        bytes: content.len() as u64,
        tables,
        duration_ms: elapsed_ms(started),
    })
}

/// 收集脚本，并在每张表之后上报一次，好让对话框显示长导出的进度。
struct ProgressSink<'a> {
    buffer: Vec<u8>,
    total: usize,
    app: &'a AppHandle,
    export_id: &'a str,
}

impl dump::DumpSink for ProgressSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), AppError> {
        self.buffer.extend_from_slice(bytes);
        if bytes.starts_with(b"copy ") {
            self.report();
        }
        Ok(())
    }
}

impl ProgressSink<'_> {
    fn report(&self) {
        // 从已输出的块里数出目前写了几张表：便宜，而且让 sink 不依赖转储的
        // 输出顺序。
        let done = self
            .buffer
            .split(|byte| *byte == b'\n')
            .filter(|line| line.starts_with(b"copy ") && line.ends_with(b"from stdin;"))
            .count();
        let _ = self.app.emit(
            "dump:progress",
            &serde_json::json!({
                "exportId": self.export_id,
                "done": done,
                "total": self.total,
            }),
        );
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedDump {
    pub content: String,
    pub bytes: u64,
    pub tables: usize,
    pub duration_ms: f64,
}

fn elapsed_ms(started: Instant) -> f64 {
    (started.elapsed().as_micros() as f64 / 100.0).round() / 10.0
}

/// 打开系统选择文件面板，并描述用户选中的 SQL 文件。
///
/// 面板放在这里而不是渲染进程，渲染进程就永远不必持有一个不是自己选的路径，
/// 文件的体积也能在读取任何内容之前先显示出来。
#[tauri::command]
pub async fn pick_sql_file(title: String) -> CmdResult<Option<PickedSqlFile>> {
    // 面板会阻塞；放到主线程之外运行，用户浏览文件时窗口仍能重绘。
    let Some(path) = tokio::task::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_title(&title)
            .add_filter("SQL", &["sql"])
            .add_filter("All files", &["*"])
            .pick_file()
    })
    .await
    .map_err(AppError::unexpected)?
    else {
        return Ok(None);
    };

    let metadata = std::fs::metadata(&path)
        .map_err(|err| AppError::new("io", "could_not_read_file").with_io_path(&path, &err))?;
    Ok(Some(PickedSqlFile {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
        path: path.to_string_lossy().into_owned(),
        bytes: metadata.len(),
    }))
}

/// 把 `path` 上的 SQL 文件回放进会话：结构、数据，以及转储携带的
/// `COPY … FROM STDIN` 块。
///
/// 进度通过 `import:progress` 事件
/// （`{ importId, statements, total, rows, copiedRows }`）上报，长文件跑到
/// 哪里对话框都能显示；整个脚本执行完后调用才返回，或者停在哪条语句就
/// 在哪里失败。
#[tauri::command]
pub async fn import_sql(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
    import_id: String,
    path: String,
    options: ImportOptions,
) -> CmdResult<ImportOutcome> {
    let handle = state.handle(&id).await?;

    // 读文件和切分是 CPU 与磁盘的活儿；解析 100 MB 转储期间异步运行时应该
    // 保持空闲。
    let file = std::path::PathBuf::from(&path);
    let read = file.clone();
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&read))
        .await
        .map_err(AppError::unexpected)?
        .map_err(|err| AppError::new("io", "could_not_read_file").with_io_path(&file, &err))?;
    let size = bytes.len() as u64;
    // 转储是文本；不是文本的文件会报错，而不是被粗暴解码。
    let text = String::from_utf8(bytes).map_err(|_| AppError::new("sql", "file_not_utf8"))?;

    let plan = import::plan(text.trim_start_matches('\u{feff}'))?;

    // 与编辑器使用同一条停止路径：`cancel_query` 取消这条会话持有的任何
    // token。
    let cancel = tokio_util::sync::CancellationToken::new();
    *handle.cancel.lock().await = cancel.clone();

    // 整个导入期间独占这条会话：批次之间的空档里，别的命令不能插进来在
    // 协议上写一半。
    let mut slot = tokio::time::timeout(SESSION_WAIT, handle.slot.lock())
        .await
        .map_err(|_| AppError::new("busy", "connection_busy_import"))?;
    slot.session.busy = true;

    let progress_id = import_id.clone();
    let outcome = import::run(&mut slot.session, plan, &options, &cancel, size, move |progress| {
        let _ = app.emit(
            "import:progress",
            &serde_json::json!({
                "importId": progress_id,
                "statements": progress.statements,
                "total": progress.total,
                "rows": progress.rows,
                "copiedRows": progress.copied_rows,
            }),
        );
    })
    .await;

    slot.session.busy = false;
    slot.session.idle_since = Instant::now();
    outcome
}

#[tauri::command]
pub async fn load_columns(
    state: State<'_, Arc<AppState>>,
    id: String,
    schema: String,
    relation: String,
) -> CmdResult<Vec<ColumnInfo>> {
    let handle = state.handle(&id).await?;
    let mut slot = handle.slot.lock().await;
    catalog::load_columns(&mut slot.session, &schema, &relation).await
}
