use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// 单次结果批次经 IPC 传多少行。
    pub page_size: usize,
    /// 通知界面停止分页之前的硬上限。
    pub max_rows: usize,
    pub connect_timeout_ms: u64,
    /// PostgreSQL 在 `pg_stat_activity` 里报告的会话名。
    pub app_name: String,
    /// 界面语言（`"en"` 或 `"zh"`）。`None` 跟随系统语言，直到用户自己选；
    /// 界面也按同样方式对待未知值。
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
