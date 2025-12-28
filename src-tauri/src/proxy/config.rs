use serde::{Deserialize, Serialize};
// use std::path::PathBuf;

/// Reverse Proxy Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Whether to enable reverse proxy service
    pub enabled: bool,

    /// Whether to allow LAN access
    /// - false: Localhost only 127.0.0.1 (default, privacy first)
    /// - true: Allow LAN access 0.0.0.0
    #[serde(default)]
    pub allow_lan_access: bool,

    /// Listening port
    pub port: u16,

    /// API Key
    pub api_key: String,

    /// Whether to auto-start
    pub auto_start: bool,

    /// Anthropic model mapping (key: Claude model name, value: Gemini model name)
    #[serde(default)]
    pub anthropic_mapping: std::collections::HashMap<String, String>,

    /// OpenAI model mapping (key: OpenAI model group, value: Gemini model name)
    #[serde(default)]
    pub openai_mapping: std::collections::HashMap<String, String>,

    /// Custom exact model mapping (key: original model name, value: target model name)
    #[serde(default)]
    pub custom_mapping: std::collections::HashMap<String, String>,

    /// API request timeout (seconds)
    #[serde(default = "default_request_timeout")]
    pub request_timeout: u64,

    /// Upstream proxy configuration
    #[serde(default)]
    pub upstream_proxy: UpstreamProxyConfig,
}

/// Upstream proxy configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpstreamProxyConfig {
    /// Whether to enable
    pub enabled: bool,
    /// Proxy address (http://, https://, socks5://)
    pub url: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_lan_access: false, // Default localhost only, privacy first
            port: 8045,
            api_key: format!("sk-{}", uuid::Uuid::new_v4().simple()),
            auto_start: false,
            anthropic_mapping: std::collections::HashMap::new(),
            openai_mapping: std::collections::HashMap::new(),
            custom_mapping: std::collections::HashMap::new(),
            request_timeout: default_request_timeout(),
            upstream_proxy: UpstreamProxyConfig::default(),
        }
    }
}

fn default_request_timeout() -> u64 {
    120 // Default 120 seconds, previous 60 seconds was too short
}

impl ProxyConfig {
    /// Get actual bind address
    /// - allow_lan_access = false: Returns "127.0.0.1" (default, privacy first)
    /// - allow_lan_access = true: Returns "0.0.0.0" (allow LAN access)
    pub fn get_bind_address(&self) -> &str {
        if self.allow_lan_access {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        }
    }
}
