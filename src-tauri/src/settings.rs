use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// How many rows a single result batch carries over IPC.
    pub page_size: usize,
    /// Hard ceiling before the UI is told to stop paging.
    pub max_rows: usize,
    pub connect_timeout_ms: u64,
    /// The session name PostgreSQL reports in `pg_stat_activity`.
    pub app_name: String,
    /// UI language (`"en"` or `"zh"`). `None` follows the system language
    /// until the user picks one; the UI treats an unknown value the same way.
    pub language: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            page_size: 1000,
            max_rows: 200_000,
            connect_timeout_ms: 10_000,
            app_name: "pg easy".into(),
            language: None,
        }
    }
}

impl Settings {
    pub fn page_size_clamped(&self) -> usize {
        self.page_size.clamp(50, 10_000)
    }
}
