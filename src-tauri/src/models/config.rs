use crate::proxy::ProxyConfig;
use serde::{Deserialize, Serialize};

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub language: String,
    pub theme: String,
    pub auto_refresh: bool,
    pub refresh_interval: i32, // Minutes
    pub auto_sync: bool,
    pub sync_interval: i32, // Minutes
    pub default_export_path: Option<String>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    pub antigravity_executable: Option<String>, // [NEW] Manually specified Antigravity executable path
    #[serde(default)]
    pub auto_launch: bool, // Auto launch on startup
}

impl AppConfig {
    pub fn new() -> Self {
        Self {
            language: "vi".to_string(),
            theme: "system".to_string(),
            auto_refresh: false,
            refresh_interval: 15,
            auto_sync: false,
            sync_interval: 5,
            default_export_path: None,
            proxy: ProxyConfig::default(),
            antigravity_executable: None,
            auto_launch: false,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self::new()
    }
}
