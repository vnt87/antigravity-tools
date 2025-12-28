use crate::models::{Account, AppConfig, QuotaData, TokenData};
use crate::modules;
use tauri::Emitter;

// Export proxy commands
pub mod proxy;
// Export autostart commands
pub mod autostart;

/// List all accounts
#[tauri::command]
pub async fn list_accounts() -> Result<Vec<Account>, String> {
    modules::list_accounts()
}

/// Add account
#[tauri::command]
pub async fn add_account(
    app: tauri::AppHandle,
    _email: String,
    refresh_token: String,
) -> Result<Account, String> {
    // 1. Use refresh_token to get access_token
    // Note: We ignore the passed _email and get the real email directly from Google
    let token_res = modules::oauth::refresh_access_token(&refresh_token).await?;

    // 2. Get user info
    let user_info = modules::oauth::get_user_info(&token_res.access_token).await?;

    // 3. Construct TokenData
    let token = TokenData::new(
        token_res.access_token,
        refresh_token, // Continue to use the refresh_token passed by the user
        token_res.expires_in,
        Some(user_info.email.clone()),
        None, // project_id will be obtained when needed
        None, // session_id
    );

    // 4. Add or update account using real email
    let account =
        modules::upsert_account(user_info.email.clone(), user_info.get_display_name(), token)?;

    modules::logger::log_info(&format!("Account added successfully: {}", account.email));

    // 5. Automatically trigger quota refresh
    let mut account = account;
    let _ = internal_refresh_account_quota(&app, &mut account).await;

    Ok(account)
}

/// Delete account
#[tauri::command]
pub async fn delete_account(app: tauri::AppHandle, account_id: String) -> Result<(), String> {
    modules::logger::log_info(&format!("Received delete account request: {}", account_id));
    modules::delete_account(&account_id).map_err(|e| {
        modules::logger::log_error(&format!("Failed to delete account: {}", e));
        e
    })?;
    modules::logger::log_info(&format!("Account deleted successfully: {}", account_id));

    // Force sync tray
    crate::modules::tray::update_tray_menus(&app);
    Ok(())
}

/// Batch delete accounts
#[tauri::command]
pub async fn delete_accounts(
    app: tauri::AppHandle,
    account_ids: Vec<String>,
) -> Result<(), String> {
    modules::logger::log_info(&format!(
        "Received batch delete request, total {} accounts",
        account_ids.len()
    ));
    modules::account::delete_accounts(&account_ids).map_err(|e| {
        modules::logger::log_error(&format!("Batch delete failed: {}", e));
        e
    })?;

    // Force sync tray
    crate::modules::tray::update_tray_menus(&app);
    Ok(())
}

/// Switch account
#[tauri::command]
pub async fn switch_account(app: tauri::AppHandle, account_id: String) -> Result<(), String> {
    let res = modules::switch_account(&account_id).await;
    if res.is_ok() {
        crate::modules::tray::update_tray_menus(&app);
    }
    res
}

/// Get current account
#[tauri::command]
pub async fn get_current_account() -> Result<Option<Account>, String> {
    // println!("🚀 Backend Command: get_current_account called"); // Commented out to reduce noise for frequent calls, relies on frontend log for frequency
    // Actually user WANTS to see it.
    modules::logger::log_info("Backend Command: get_current_account called");

    let account_id = modules::get_current_account_id()?;

    if let Some(id) = account_id {
        // modules::logger::log_info(&format!("   Found current account ID: {}", id));
        modules::load_account(&id).map(Some)
    } else {
        modules::logger::log_info("   No current account set");
        Ok(None)
    }
}

/// Internal helper: Automatically refresh quota once after adding or importing an account
async fn internal_refresh_account_quota(
    app: &tauri::AppHandle,
    account: &mut Account,
) -> Result<QuotaData, String> {
    modules::logger::log_info(&format!(
        "Automatically triggering quota refresh: {}",
        account.email
    ));

    // Use query with retry (Shared logic)
    match modules::account::fetch_quota_with_retry(account).await {
        Ok(quota) => {
            // Update account quota
            let _ = modules::update_account_quota(&account.id, quota.clone());
            // Update tray menu
            crate::modules::tray::update_tray_menus(app);
            Ok(quota)
        }
        Err(e) => {
            modules::logger::log_warn(&format!(
                "Auto quota refresh failed ({}): {}",
                account.email, e
            ));
            Err(e.to_string())
        }
    }
}

/// Query account quota
#[tauri::command]
pub async fn fetch_account_quota(
    app: tauri::AppHandle,
    account_id: String,
) -> crate::error::AppResult<QuotaData> {
    modules::logger::log_info(&format!("Manual quota refresh request: {}", account_id));
    let mut account =
        modules::load_account(&account_id).map_err(crate::error::AppError::Account)?;

    // Use query with retry (Shared logic)
    let quota = modules::account::fetch_quota_with_retry(&mut account).await?;

    // 4. Update account quota
    modules::update_account_quota(&account_id, quota.clone())
        .map_err(crate::error::AppError::Account)?;

    crate::modules::tray::update_tray_menus(&app);

    Ok(quota)
}

#[derive(serde::Serialize)]
pub struct RefreshStats {
    total: usize,
    success: usize,
    failed: usize,
    details: Vec<String>,
}

/// Refresh all account quotas
#[tauri::command]
pub async fn refresh_all_quotas() -> Result<RefreshStats, String> {
    modules::logger::log_info("Starting batch refresh of all account quotas");
    let accounts = modules::list_accounts()?;

    let mut success = 0;
    let mut failed = 0;
    let mut details = Vec::new();

    // Serial processing to ensure persistence safety (SQLite)
    for mut account in accounts {
        if let Some(ref q) = account.quota {
            if q.is_forbidden {
                modules::logger::log_info(&format!("  - Skipping {} (Forbidden)", account.email));
                continue;
            }
        }

        modules::logger::log_info(&format!("  - Processing {}", account.email));

        match modules::account::fetch_quota_with_retry(&mut account).await {
            Ok(quota) => {
                // Save quota
                if let Err(e) = modules::update_account_quota(&account.id, quota) {
                    failed += 1;
                    let msg = format!("Account {}: Save quota failed - {}", account.email, e);
                    details.push(msg.clone());
                    modules::logger::log_error(&msg);
                } else {
                    success += 1;
                    modules::logger::log_info("    ✅ Success");
                }
            }
            Err(e) => {
                failed += 1;
                // e might be AppError, assume it implements Display
                let msg = format!("Account {}: Fetch quota failed - {}", account.email, e);
                details.push(msg.clone());
                modules::logger::log_error(&msg);
            }
        }
    }

    modules::logger::log_info(&format!(
        "Batch refresh completed: {} success, {} failed",
        success, failed
    ));
    Ok(RefreshStats {
        total: success + failed,
        success,
        failed,
        details,
    })
}

/// Load config
#[tauri::command]
pub async fn load_config() -> Result<AppConfig, String> {
    modules::load_app_config()
}

/// Save config
#[tauri::command]
pub async fn save_config(
    app: tauri::AppHandle,
    proxy_state: tauri::State<'_, crate::commands::proxy::ProxyServiceState>,
    config: AppConfig,
) -> Result<(), String> {
    modules::save_app_config(&config)?;

    // Notify tray that config has been updated
    let _ = app.emit("config://updated", ());

    // Hot update running service
    let instance_lock = proxy_state.instance.read().await;
    if let Some(instance) = instance_lock.as_ref() {
        // Update model mapping
        instance.axum_server.update_mapping(&config.proxy).await;
        // Update upstream proxy
        instance
            .axum_server
            .update_proxy(config.proxy.upstream_proxy.clone())
            .await;
        tracing::info!("Synced hot update of proxy service configuration");
    }

    Ok(())
}

// --- OAuth Commands ---

#[tauri::command]
pub async fn start_oauth_login(app_handle: tauri::AppHandle) -> Result<Account, String> {
    modules::logger::log_info("Starting OAuth authorization flow...");

    // 1. Start OAuth flow to get Token
    let token_res = modules::oauth_server::start_oauth_flow(app_handle.clone()).await?;

    // 2. Check refresh_token
    let refresh_token = token_res.refresh_token.ok_or_else(|| {
        "Refresh Token not obtained.\n\n\
         Possible reasons:\n\
         1. You have previously authorized this app, Google will not return refresh_token again\n\n\
         Solutions:\n\
         1. Visit https://myaccount.google.com/permissions\n\
         2. Revoke access for 'Antigravity Tools'\n\
         3. Re-authorize OAuth\n\n\
         Or manually add account using 'Refresh Token' tab"
            .to_string()
    })?;

    // 3. Get user info
    let user_info = modules::oauth::get_user_info(&token_res.access_token).await?;
    modules::logger::log_info(&format!(
        "Successfully retrieved user info: {}",
        user_info.email
    ));

    // 4. Try to get project ID
    let project_id = crate::proxy::project_resolver::fetch_project_id(&token_res.access_token)
        .await
        .ok();

    if let Some(ref pid) = project_id {
        modules::logger::log_info(&format!("Successfully retrieved Project ID: {}", pid));
    } else {
        modules::logger::log_warn("Failed to retrieve Project ID, will lazy load later");
    }

    // 5. Construct TokenData
    let token_data = TokenData::new(
        token_res.access_token,
        refresh_token,
        token_res.expires_in,
        Some(user_info.email.clone()),
        project_id,
        None,
    );

    // 6. Add or update to account list
    modules::logger::log_info("Saving account info...");
    let mut account = modules::upsert_account(
        user_info.email.clone(),
        user_info.get_display_name(),
        token_data,
    )?;

    // 7. Automatically trigger quota refresh
    let _ = internal_refresh_account_quota(&app_handle, &mut account).await;

    Ok(account)
}

/// Complete OAuth authorization (do not automatically open browser)
#[tauri::command]
pub async fn complete_oauth_login(app_handle: tauri::AppHandle) -> Result<Account, String> {
    modules::logger::log_info("Completing OAuth authorization flow (manual)...");

    // 1. Wait for callback and exchange Token (do not open browser)
    let token_res = modules::oauth_server::complete_oauth_flow(app_handle.clone()).await?;

    // 2. Check refresh_token
    let refresh_token = token_res.refresh_token.ok_or_else(|| {
        "Refresh Token not obtained.\n\n\
         Possible reasons:\n\
         1. You have previously authorized this app, Google will not return refresh_token again\n\n\
         Solutions:\n\
         1. Visit https://myaccount.google.com/permissions\n\
         2. Revoke access for 'Antigravity Tools'\n\
         3. Re-authorize OAuth\n\n\
         Or manually add account using 'Refresh Token' tab"
            .to_string()
    })?;

    // 3. Get user info
    let user_info = modules::oauth::get_user_info(&token_res.access_token).await?;
    modules::logger::log_info(&format!(
        "Successfully retrieved user info: {}",
        user_info.email
    ));

    // 4. Try to get project ID
    let project_id = crate::proxy::project_resolver::fetch_project_id(&token_res.access_token)
        .await
        .ok();

    if let Some(ref pid) = project_id {
        modules::logger::log_info(&format!("Successfully retrieved Project ID: {}", pid));
    } else {
        modules::logger::log_warn("Failed to retrieve Project ID, will lazy load later");
    }

    // 5. Construct TokenData
    let token_data = TokenData::new(
        token_res.access_token,
        refresh_token,
        token_res.expires_in,
        Some(user_info.email.clone()),
        project_id,
        None,
    );

    // 6. Add or update to account list
    modules::logger::log_info("Saving account info...");
    let mut account = modules::upsert_account(
        user_info.email.clone(),
        user_info.get_display_name(),
        token_data,
    )?;

    // 7. Automatically trigger quota refresh
    let _ = internal_refresh_account_quota(&app_handle, &mut account).await;

    Ok(account)
}

/// Pre-generate OAuth authorization link (do not open browser)
#[tauri::command]
pub async fn prepare_oauth_url(app_handle: tauri::AppHandle) -> Result<String, String> {
    crate::modules::oauth_server::prepare_oauth_url(app_handle).await
}

#[tauri::command]
pub async fn cancel_oauth_login() -> Result<(), String> {
    modules::oauth_server::cancel_oauth_flow();
    Ok(())
}

// --- Import Commands ---

#[tauri::command]
pub async fn import_v1_accounts(app: tauri::AppHandle) -> Result<Vec<Account>, String> {
    let accounts = modules::migration::import_from_v1().await?;

    // Try to refresh the imported accounts
    for mut account in accounts.clone() {
        let _ = internal_refresh_account_quota(&app, &mut account).await;
    }

    Ok(accounts)
}

#[tauri::command]
pub async fn import_from_db(app: tauri::AppHandle) -> Result<Account, String> {
    // Wrap synchronous function as async
    let mut account = modules::migration::import_from_db().await?;

    // Since it is imported from the database (i.e., the current IDE account), automatically set it as the Manager's current account
    let account_id = account.id.clone();
    modules::account::set_current_account_id(&account_id)?;

    // Automatically trigger quota refresh
    let _ = internal_refresh_account_quota(&app, &mut account).await;

    // Refresh tray icon display
    crate::modules::tray::update_tray_menus(&app);

    Ok(account)
}

#[tauri::command]
#[allow(dead_code)]
pub async fn import_custom_db(app: tauri::AppHandle, path: String) -> Result<Account, String> {
    // Call refactored custom import function
    let mut account = modules::migration::import_from_custom_db_path(path).await?;

    // Automatically set as current account
    let account_id = account.id.clone();
    modules::account::set_current_account_id(&account_id)?;

    // Automatically trigger quota refresh
    let _ = internal_refresh_account_quota(&app, &mut account).await;

    // Refresh tray icon display
    crate::modules::tray::update_tray_menus(&app);

    Ok(account)
}

#[tauri::command]
pub async fn sync_account_from_db(app: tauri::AppHandle) -> Result<Option<Account>, String> {
    // 1. Get Refresh Token from DB
    let db_refresh_token = match modules::migration::get_refresh_token_from_db() {
        Ok(token) => token,
        Err(e) => {
            modules::logger::log_info(&format!("Auto-sync skipped: {}", e));
            return Ok(None);
        }
    };

    // 2. Get Manager current account
    let curr_account = modules::account::get_current_account()?;

    // 3. Compare: If Refresh Token is the same, the account has not changed, no need to import
    if let Some(acc) = curr_account {
        if acc.token.refresh_token == db_refresh_token {
            // Account unchanged, since it is already a periodic task, we can optionally refresh the quota, or return directly
            // Return directly here to save API traffic
            return Ok(None);
        }
        modules::logger::log_info(&format!(
            "Detected account switch ({} -> New DB account), syncing...",
            acc.email
        ));
    } else {
        modules::logger::log_info("Detected new login account, auto-syncing...");
    }

    // 4. Execute full import
    let account = import_from_db(app).await?;
    Ok(Some(account))
}

/// Save text file (bypass frontend Scope limit)
#[tauri::command]
pub async fn save_text_file(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| format!("Failed to write file: {}", e))
}

/// Clear log cache
#[tauri::command]
pub async fn clear_log_cache() -> Result<(), String> {
    modules::logger::clear_logs()
}

/// Open data directory
#[tauri::command]
pub async fn open_data_folder() -> Result<(), String> {
    let path = modules::account::get_data_dir()?;

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    Ok(())
}

/// Get data directory absolute path
#[tauri::command]
pub async fn get_data_dir_path() -> Result<String, String> {
    let path = modules::account::get_data_dir()?;
    Ok(path.to_string_lossy().to_string())
}

/// Show main window
#[tauri::command]
pub async fn show_main_window(window: tauri::Window) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())
}

/// Get Antigravity executable path
#[tauri::command]
pub async fn get_antigravity_path(bypass_config: Option<bool>) -> Result<String, String> {
    // 1. Prioritize query from config (unless explicitly requested to bypass)
    if bypass_config != Some(true) {
        if let Ok(config) = crate::modules::config::load_app_config() {
            if let Some(path) = config.antigravity_executable {
                if std::path::Path::new(&path).exists() {
                    return Ok(path);
                }
            }
        }
    }

    // 2. Execute real-time detection
    match crate::modules::process::get_antigravity_executable_path() {
        Some(path) => Ok(path.to_string_lossy().to_string()),
        None => Err("Antigravity installation path not found".to_string()),
    }
}

/// Update check response structure
#[derive(serde::Serialize)]
pub struct UpdateInfo {
    has_update: bool,
    latest_version: String,
    current_version: String,
    download_url: String,
}

/// Check for GitHub releases updates
#[tauri::command]
pub async fn check_for_updates() -> Result<UpdateInfo, String> {
    const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
    const GITHUB_API_URL: &str =
        "https://api.github.com/repos/lbjlaq/Antigravity-Manager/releases/latest";

    modules::logger::log_info("Checking for updates...");

    // Initiate HTTP request
    let client = crate::utils::http::create_client(15);
    let response = client
        .get(GITHUB_API_URL)
        .header("User-Agent", "Antigravity-Tools")
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("GitHub API returned error: {}", response.status()));
    }

    // Parse JSON response
    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    let latest_version = json["tag_name"]
        .as_str()
        .ok_or("Failed to get version number")?
        .trim_start_matches('v');

    let download_url = json["html_url"]
        .as_str()
        .unwrap_or("https://github.com/lbjlaq/Antigravity-Manager/releases")
        .to_string();

    // Compare version numbers
    let has_update = compare_versions(latest_version, CURRENT_VERSION);

    modules::logger::log_info(&format!(
        "Update check completed: Current v{}, Latest v{}, Has update: {}",
        CURRENT_VERSION, latest_version, has_update
    ));

    Ok(UpdateInfo {
        has_update,
        latest_version: format!("v{}", latest_version),
        current_version: format!("v{}", CURRENT_VERSION),
        download_url,
    })
}

/// Simple version number comparison (assuming format is x.y.z)
fn compare_versions(latest: &str, current: &str) -> bool {
    let parse_version =
        |v: &str| -> Vec<u32> { v.split('.').filter_map(|s| s.parse::<u32>().ok()).collect() };

    let latest_parts = parse_version(latest);
    let current_parts = parse_version(current);

    for i in 0..3 {
        let l = latest_parts.get(i).unwrap_or(&0);
        let c = current_parts.get(i).unwrap_or(&0);
        if l > c {
            return true;
        } else if l < c {
            return false;
        }
    }

    false
}
