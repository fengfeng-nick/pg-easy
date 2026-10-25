mod catalog;
mod commands;
mod connection;
mod dump;
mod error;
mod import;
mod pg;
mod protocol;
mod query;
mod settings;
mod sql;
mod state;
mod store;

/// `tests/` 里集成测试用的辅助代码。
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
                // 关闭窗口就结束了进程；受管状态被丢弃时，所有会话 socket
                // 也一并关闭。
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
            commands::switch_database,
            commands::connection_status,
            commands::execute,
            commands::cancel_query,
            commands::fetch_more,
            commands::browse,
            commands::load_catalog,
            commands::load_columns,
            commands::load_structure,
            commands::load_ddl,
            commands::export_sql,
            commands::pick_sql_file,
            commands::import_sql,
            commands::save_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running pg easy");
}
