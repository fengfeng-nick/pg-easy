//! Saved connections and preferences.
//!
//! Non-secret fields live in a small JSON file; passwords go to the OS
//! keychain (macOS Keychain / Windows Credential Manager) keyed by connection
//! id, never into the JSON. If the keychain is unavailable the password is
//! simply not saved rather than being written to disk in clear text.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error::AppError;
use crate::protocol::SavedConnection;
use crate::settings::Settings;

const KEYCHAIN_SERVICE: &str = "dev.pgeasy.app";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreFile {
    #[serde(default)]
    pub connections: Vec<SavedConnection>,
    #[serde(default)]
    pub settings: Settings,
}

impl StoreFile {
    pub fn path(app: &AppHandle) -> Result<PathBuf, AppError> {
        let dir = app
            .path()
            .app_config_dir()
            .map_err(AppError::unexpected)?;
        Ok(dir.join("connections.json"))
    }

    pub fn load(app: &AppHandle) -> Result<Self, AppError> {
        let path = Self::path(app)?;
        read_json(&path)
    }

    pub fn save(&self, app: &AppHandle) -> Result<(), AppError> {
        let path = Self::path(app)?;
        write_json(&path, self)
    }
}

pub fn read_json(path: &Path) -> Result<StoreFile, AppError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|err| {
            AppError::new("config", "connections_file_read")
                .with_io_path(path, &err)
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(StoreFile::default()),
        Err(err) => Err(AppError::new("config", "connections_file_open")
            .with_io_path(path, &err)),
    }
}

/// Writes atomically so a crash mid-save cannot truncate the file.
pub fn write_json(path: &Path, value: &StoreFile) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::unexpected)?;
    }
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(value).map_err(AppError::unexpected)?;
    std::fs::write(&tmp, data).map_err(AppError::unexpected)?;
    std::fs::rename(&tmp, path).map_err(AppError::unexpected)?;
    restrict_permissions(path);
    Ok(())
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

pub fn keychain_account(connection_id: &str) -> String {
    format!("connection:{connection_id}")
}

fn entry(account: &str) -> Result<keyring::Entry, AppError> {
    keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(|err| {
        AppError::new("keychain", "keychain_unavailable").detail(err.to_string())
    })
}

pub fn save_password(connection_id: &str, password: &str) -> Result<(), AppError> {
    entry(&keychain_account(connection_id))?
        .set_password(password)
        .map_err(|err| {
            AppError::new("keychain", "keychain_save")
                .detail(err.to_string())
        })
}

pub fn load_password(connection_id: &str) -> Option<String> {
    entry(&keychain_account(connection_id))
        .ok()?
        .get_password()
        .ok()
}

pub fn has_password(connection_id: &str) -> bool {
    load_password(connection_id).is_some()
}

pub fn delete_password(connection_id: &str) {
    if let Ok(entry) = entry(&keychain_account(connection_id)) {
        let _ = entry.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("pgeasy-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("connections.json");
        let file = StoreFile::default();
        write_json(&path, &file).unwrap();
        read_json(&path).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file_is_the_default() {
        let path = std::env::temp_dir().join("pgeasy-missing/nope.json");
        let loaded = read_json(&path).unwrap();
        assert!(loaded.connections.is_empty());
    }
}
