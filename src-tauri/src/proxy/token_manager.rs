// Remove redundant top-level imports as they are handled by full path or local imports in the code
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ProxyToken {
    pub account_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub timestamp: i64,
    pub email: String,
    pub account_path: PathBuf, // Account file path, used for updates
    pub project_id: Option<String>,
}

pub struct TokenManager {
    tokens: Arc<DashMap<String, ProxyToken>>, // account_id -> ProxyToken
    current_index: Arc<AtomicUsize>,
    last_used_account: Arc<tokio::sync::Mutex<Option<(String, std::time::Instant)>>>,
    data_dir: PathBuf,
}

impl TokenManager {
    /// Create new TokenManager
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            tokens: Arc::new(DashMap::new()),
            current_index: Arc::new(AtomicUsize::new(0)),
            last_used_account: Arc::new(tokio::sync::Mutex::new(None)),
            data_dir,
        }
    }

    /// Load all accounts from the main app account directory
    pub async fn load_accounts(&self) -> Result<usize, String> {
        let accounts_dir = self.data_dir.join("accounts");

        if !accounts_dir.exists() {
            return Err(format!(
                "Account directory does not exist: {:?}",
                accounts_dir
            ));
        }

        let entries = std::fs::read_dir(&accounts_dir)
            .map_err(|e| format!("Failed to read account directory: {}", e))?;

        let mut count = 0;

        for entry in entries {
            let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            // Try to load account
            match self.load_single_account(&path).await {
                Ok(Some(token)) => {
                    let account_id = token.account_id.clone();
                    self.tokens.insert(account_id, token);
                    count += 1;
                }
                Ok(None) => {
                    // Skip invalid account
                }
                Err(e) => {
                    tracing::warn!("Failed to load account {:?}: {}", path, e);
                }
            }
        }

        Ok(count)
    }

    /// Load single account
    async fn load_single_account(&self, path: &PathBuf) -> Result<Option<ProxyToken>, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {}", e))?;

        let account: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| format!("Failed to parse JSON: {}", e))?;

        let account_id = account["id"]
            .as_str()
            .ok_or("Missing id field")?
            .to_string();

        let email = account["email"]
            .as_str()
            .ok_or("Missing email field")?
            .to_string();

        let token_obj = account["token"].as_object().ok_or("Missing token field")?;

        let access_token = token_obj["access_token"]
            .as_str()
            .ok_or("Missing access_token")?
            .to_string();

        let refresh_token = token_obj["refresh_token"]
            .as_str()
            .ok_or("Missing refresh_token")?
            .to_string();

        let expires_in = token_obj["expires_in"]
            .as_i64()
            .ok_or("Missing expires_in")?;

        let timestamp = token_obj["expiry_timestamp"]
            .as_i64()
            .ok_or("Missing expiry_timestamp")?;

        // project_id is optional
        let project_id = token_obj
            .get("project_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(Some(ProxyToken {
            account_id,
            access_token,
            refresh_token,
            expires_in,
            timestamp,
            email,
            account_path: path.clone(),
            project_id,
        }))
    }

    /// Get currently available Token (with 60s time window lock mechanism)
    /// Parameter _quota_group is used to distinguish "claude" vs "gemini" groups
    /// When parameter force_rotate is true, the lock will be ignored and the account will be forcibly rotated
    pub async fn get_token(
        &self,
        quota_group: &str,
        force_rotate: bool,
    ) -> Result<(String, String, String), String> {
        let total = self.tokens.len();
        if total == 0 {
            return Err("Token pool is empty".to_string());
        }

        // 1. Check time window lock (force reuse of the previous account within 60 seconds)
        // Optimization strategy: Image generation requests (image_gen) are not locked by default to maximize concurrency
        let mut target_token = None;
        if !force_rotate && quota_group != "image_gen" {
            let last_used = self.last_used_account.lock().await;
            if let Some((account_id, last_time)) = &*last_used {
                if last_time.elapsed().as_secs() < 60 {
                    if let Some(entry) = self.tokens.get(account_id) {
                        tracing::info!(
                            "Within 60s time window, forcing reuse of previous account: {}",
                            entry.email
                        );
                        target_token = Some(entry.value().clone());
                    }
                }
            }
        }

        // 2. If there is no lock, the lock expires, or forced rotation, poll records and update lock information
        let mut token = if let Some(t) = target_token {
            t
        } else {
            // Simple rotation strategy (Round Robin)
            let idx = self.current_index.fetch_add(1, Ordering::SeqCst) % total;
            let selected_token = self
                .tokens
                .iter()
                .nth(idx)
                .map(|entry| entry.value().clone())
                .ok_or("Failed to retrieve token from pool")?;

            // Update the last used account and time (if it is a normal conversation request)
            if quota_group != "image_gen" {
                let mut last_used = self.last_used_account.lock().await;
                *last_used = Some((selected_token.account_id.clone(), std::time::Instant::now()));
            }

            let action_msg = if force_rotate {
                "Force switch"
            } else {
                "Switch"
            };
            tracing::info!("{} to account: {}", action_msg, selected_token.email);
            selected_token
        };

        // 3. Check if token is expired (refresh 5 minutes in advance)
        let now = chrono::Utc::now().timestamp();
        if now >= token.timestamp - 300 {
            tracing::info!(
                "Token for account {} is about to expire, refreshing...",
                token.email
            );

            // Call OAuth to refresh token
            match crate::modules::oauth::refresh_access_token(&token.refresh_token).await {
                Ok(token_response) => {
                    tracing::info!("Token refreshed successfully!");

                    // Update local memory object for subsequent use
                    token.access_token = token_response.access_token.clone();
                    token.expires_in = token_response.expires_in;
                    token.timestamp = now + token_response.expires_in;

                    // Synchronously update cross-thread shared DashMap
                    if let Some(mut entry) = self.tokens.get_mut(&token.account_id) {
                        entry.access_token = token.access_token.clone();
                        entry.expires_in = token.expires_in;
                        entry.timestamp = token.timestamp;
                    }
                }
                Err(e) => {
                    tracing::error!("Token refresh failed: {}, trying next account", e);
                    return Err(format!("Token refresh failed: {}", e));
                }
            }
        }

        // 4. Ensure project_id exists
        let project_id = if let Some(pid) = &token.project_id {
            pid.clone()
        } else {
            tracing::info!(
                "Account {} missing project_id, attempting to fetch...",
                token.email
            );
            match crate::proxy::project_resolver::fetch_project_id(&token.access_token).await {
                Ok(pid) => {
                    if let Some(mut entry) = self.tokens.get_mut(&token.account_id) {
                        entry.project_id = Some(pid.clone());
                    }
                    let _ = self.save_project_id(&token.account_id, &pid).await;
                    pid
                }
                Err(e) => {
                    tracing::error!("Failed to fetch project_id for {}: {}", token.email, e);
                    return Err(format!("Failed to fetch project_id: {}", e));
                }
            }
        };

        Ok((token.access_token, project_id, token.email))
    }

    /// Save project_id to account file
    async fn save_project_id(&self, account_id: &str, project_id: &str) -> Result<(), String> {
        let entry = self
            .tokens
            .get(account_id)
            .ok_or("Account does not exist")?;

        let path = &entry.account_path;

        let mut content: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {}", e))?,
        )
        .map_err(|e| format!("Failed to parse JSON: {}", e))?;

        content["token"]["project_id"] = serde_json::Value::String(project_id.to_string());

        std::fs::write(path, serde_json::to_string_pretty(&content).unwrap())
            .map_err(|e| format!("Failed to write file: {}", e))?;

        tracing::info!("Saved project_id to account {}", account_id);
        Ok(())
    }

    /// Save refreshed token to account file
    #[allow(dead_code)]
    async fn save_refreshed_token(
        &self,
        account_id: &str,
        token_response: &crate::modules::oauth::TokenResponse,
    ) -> Result<(), String> {
        let entry = self
            .tokens
            .get(account_id)
            .ok_or("Account does not exist")?;

        let path = &entry.account_path;

        let mut content: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {}", e))?,
        )
        .map_err(|e| format!("Failed to parse JSON: {}", e))?;

        let now = chrono::Utc::now().timestamp();

        content["token"]["access_token"] =
            serde_json::Value::String(token_response.access_token.clone());
        content["token"]["expires_in"] =
            serde_json::Value::Number(token_response.expires_in.into());
        content["token"]["expiry_timestamp"] =
            serde_json::Value::Number((now + token_response.expires_in).into());

        std::fs::write(path, serde_json::to_string_pretty(&content).unwrap())
            .map_err(|e| format!("Failed to write file: {}", e))?;

        tracing::info!("Saved refreshed token to account {}", account_id);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.tokens.len()
    }
}
