use std::sync::Arc;

use serde::Serialize;
use std::time::Instant;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::catalog;
use crate::connection::ConnectParams;
use crate::dump;
use crate::error::AppError;
use crate::protocol::{
    BrowseResult, Catalog, ColumnInfo, ConnectionStatus, DumpOptions, ExecResult, Page,
    SavedConnection, SessionInfo,
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

/// Upserts a saved connection. Saving the same server and database twice
/// updates the existing entry instead of piling up duplicates the list cannot
/// tell apart. A supplied password goes to the keychain; when none is supplied
/// the stored one is kept (unless saving is turned off).
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
            // Editing other fields must not forget a password saved earlier.
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

/// Pre-fills the edit form for a saved connection. Read on demand rather than
/// shipped with the list so secrets never sit in the renderer's memory longer
/// than the form is open.
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

/// How long a connect attempt is allowed to take before it is reported as a
/// timeout instead of leaving the form spinning.
const CONNECT_TIMEOUT_MS: u64 = 15_000;

/// How long a command waits for a session that is running a statement.
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

#[tauri::command]
pub async fn connection_status(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> CmdResult<Option<ConnectionStatus>> {
    Ok(state.peek_status(&id).await)
}

/// Executes `sql`. On failure the error carries the results of statements that
/// completed first, so partial work is never silently lost.
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

/// Asks the server to abandon the statement running on this session. The
/// `execute` call itself resolves with a cancelled error.
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

/// Opens the system save panel and writes `content` (base64) to the chosen
/// path, returning it. The panel and the write live here rather than in the
/// renderer so the renderer never has to hold a path it did not pick itself.
#[tauri::command]
pub async fn save_file(
    title: String,
    default_name: String,
    content: String,
) -> CmdResult<Option<String>> {
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content)
        .map_err(AppError::unexpected)?;
    // The panel blocks; running it off the main thread keeps the window
    // repainting while the user chooses a folder.
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

/// Builds an SQL dump of the whole database (`scope` empty) or of the named
/// tables and returns it base64-encoded, because a zip is not UTF-8.
///
/// Progress is reported as `dump:progress` events (`{ exportId, done, total }`)
/// so the dialog can show a table count while a large export runs; the call
/// resolves once the file is complete.
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

/// Collects the script and reports after every table, so the dialog can
/// show how far a long export has come.
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
        // Count the tables written so far from the blocks emitted: cheap, and
        // it keeps the sink independent of the dump's ordering.
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
