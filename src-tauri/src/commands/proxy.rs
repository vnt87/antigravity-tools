use crate::proxy::{ProxyConfig, TokenManager};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;
use tokio::sync::RwLock;

/// Proxy service status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyStatus {
    pub running: bool,
    pub port: u16,
    pub base_url: String,
    pub active_accounts: usize,
}

/// Proxy service statistics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxyStats {
    pub total_requests: u64,
    pub success_count: u64,
    pub error_count: u64,
}

/// Proxy service global state
pub struct ProxyServiceState {
    pub instance: Arc<RwLock<Option<ProxyServiceInstance>>>,
}

/// Proxy service instance
pub struct ProxyServiceInstance {
    pub config: ProxyConfig,
    pub token_manager: Arc<TokenManager>,
    pub axum_server: crate::proxy::AxumServer,
    pub server_handle: tokio::task::JoinHandle<()>,
}

impl ProxyServiceState {
    pub fn new() -> Self {
        Self {
            instance: Arc::new(RwLock::new(None)),
        }
    }
}

/// Start proxy service
#[tauri::command]
pub async fn start_proxy_service(
    config: ProxyConfig,
    state: State<'_, ProxyServiceState>,
    _app_handle: tauri::AppHandle,
) -> Result<ProxyStatus, String> {
    let mut instance_lock = state.instance.write().await;

    // Prevent duplicate start
    if instance_lock.is_some() {
        return Err("Service is already running".to_string());
    }

    // 2. Initialize Token Manager
    let app_data_dir = crate::modules::account::get_data_dir()?;
    let accounts_dir = app_data_dir.clone();

    let token_manager = Arc::new(TokenManager::new(accounts_dir));

    // 3. Load accounts
    let active_accounts = token_manager
        .load_accounts()
        .await
        .map_err(|e| format!("Failed to load accounts: {}", e))?;

    if active_accounts == 0 {
        return Err("No available accounts, please add an account first".to_string());
    }

    // Start Axum server
    let (axum_server, server_handle) = match crate::proxy::AxumServer::start(
        config.get_bind_address().to_string(),
        config.port,
        token_manager.clone(),
        config.anthropic_mapping.clone(),
        config.openai_mapping.clone(),
        config.custom_mapping.clone(),
        config.request_timeout,
        config.upstream_proxy.clone(),
    )
    .await
    {
        Ok((server, handle)) => (server, handle),
        Err(e) => return Err(format!("Failed to start Axum server: {}", e)),
    };

    // Create service instance
    let instance = ProxyServiceInstance {
        config: config.clone(),
        token_manager: token_manager.clone(), // Clone for ProxyServiceInstance
        axum_server,
        server_handle,
    };

    *instance_lock = Some(instance);

    // Save configuration to global AppConfig
    let mut app_config = crate::modules::config::load_app_config().map_err(|e| e)?;
    app_config.proxy = config.clone();
    crate::modules::config::save_app_config(&app_config).map_err(|e| e)?;

    Ok(ProxyStatus {
        running: true,
        port: config.port,
        base_url: format!("http://127.0.0.1:{}", config.port),
        active_accounts,
    })
}

/// Stop proxy service
#[tauri::command]
pub async fn stop_proxy_service(state: State<'_, ProxyServiceState>) -> Result<(), String> {
    let mut instance_lock = state.instance.write().await;

    if instance_lock.is_none() {
        return Err("Service is not running".to_string());
    }

    // Stop Axum server
    if let Some(instance) = instance_lock.take() {
        instance.axum_server.stop();
        // Wait for server task to complete
        instance.server_handle.await.ok();
    }

    Ok(())
}

/// Get proxy service status
#[tauri::command]
pub async fn get_proxy_status(state: State<'_, ProxyServiceState>) -> Result<ProxyStatus, String> {
    let instance_lock = state.instance.read().await;

    match instance_lock.as_ref() {
        Some(instance) => Ok(ProxyStatus {
            running: true,
            port: instance.config.port,
            base_url: format!("http://127.0.0.1:{}", instance.config.port),
            active_accounts: instance.token_manager.len(),
        }),
        None => Ok(ProxyStatus {
            running: false,
            port: 0,
            base_url: String::new(),
            active_accounts: 0,
        }),
    }
}

/// Get proxy service statistics
#[tauri::command]
pub async fn get_proxy_stats(_state: State<'_, ProxyServiceState>) -> Result<ProxyStats, String> {
    // TODO: Implement statistics collection
    Ok(ProxyStats::default())
}

/// Generate API Key
#[tauri::command]
pub fn generate_api_key() -> String {
    format!("sk-{}", uuid::Uuid::new_v4().simple())
}

/// Reload accounts (called when main app adds/removes accounts)
#[tauri::command]
pub async fn reload_proxy_accounts(state: State<'_, ProxyServiceState>) -> Result<usize, String> {
    let instance_lock = state.instance.read().await;

    if let Some(instance) = instance_lock.as_ref() {
        // Reload accounts
        let count = instance
            .token_manager
            .load_accounts()
            .await
            .map_err(|e| format!("Failed to reload accounts: {}", e))?;
        Ok(count)
    } else {
        Err("Service is not running".to_string())
    }
}

/// Update model mapping table (hot update)
#[tauri::command]
pub async fn update_model_mapping(
    config: ProxyConfig,
    state: State<'_, ProxyServiceState>,
) -> Result<(), String> {
    let instance_lock = state.instance.read().await;

    // 1. If the service is running, immediately update the mapping in memory (currently only updates RwLock for anthropic_mapping,
    // subsequently can be made to read the full config in resolve_model_route if needed)
    if let Some(instance) = instance_lock.as_ref() {
        instance.axum_server.update_mapping(&config).await;
        tracing::info!("Backend service has received full model mapping configuration");
    }

    // 2. Save to global configuration persistence regardless of whether it is running
    let mut app_config = crate::modules::config::load_app_config().map_err(|e| e)?;
    app_config.proxy.anthropic_mapping = config.anthropic_mapping;
    app_config.proxy.openai_mapping = config.openai_mapping;
    app_config.proxy.custom_mapping = config.custom_mapping;
    crate::modules::config::save_app_config(&app_config).map_err(|e| e)?;

    Ok(())
}
