mod catalog;
mod commands;
mod connection;
mod dump;
mod error;
mod pg;
mod protocol;
mod query;
mod settings;
mod sql;
mod state;
mod store;

/// Helpers for the integration tests in `tests/`.
#[doc(hidden)]
pub mod testing;

use std::sync::Arc;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let file = store::StoreFile::load(app.handle())?;
            app.manage(Arc::new(state::AppState::new(file.settings)));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                // Closing the window ends the process; dropping the managed
                // state closes every session socket with it.
                let _ = window.app_handle();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_connections,
            commands::save_connection,
            commands::delete_connection,
            commands::read_saved_password,
            commands::load_settings,
            commands::save_settings,
            commands::connect,
            commands::disconnect,
            commands::connection_status,
            commands::execute,
            commands::cancel_query,
            commands::fetch_more,
            commands::browse,
            commands::load_catalog,
            commands::load_columns,
            commands::export_sql,
            commands::save_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running pg easy");
}
