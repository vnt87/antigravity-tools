use crate::models::QuotaData;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::json;

const QUOTA_API_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels";
const LOAD_PROJECT_API_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const USER_AGENT: &str = "antigravity/1.11.3 Darwin/arm64";

#[derive(Debug, Serialize, Deserialize)]
struct QuotaResponse {
    models: std::collections::HashMap<String, ModelInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ModelInfo {
    #[serde(rename = "quotaInfo")]
    quota_info: Option<QuotaInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct QuotaInfo {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoadProjectResponse {
    #[serde(rename = "cloudaicompanionProject")]
    project_id: Option<String>,
}

/// Create a configured HTTP Client
fn create_client() -> reqwest::Client {
    crate::utils::http::create_client(15)
}

/// Get Project ID
async fn fetch_project_id(access_token: &str) -> Option<String> {
    let client = create_client();
    let body = json!({
        "metadata": {
            "ideType": "ANTIGRAVITY"
        }
    });

    // Simple retry
    for _ in 0..2 {
        match client
            .post(LOAD_PROJECT_API_URL)
            .bearer_auth(access_token)
            .header("User-Agent", USER_AGENT)
            .json(&body)
            .send()
            .await
        {
            Ok(res) => {
                if res.status().is_success() {
                    if let Ok(data) = res.json::<LoadProjectResponse>().await {
                        if let Some(pid) = data.project_id {
                            return Some(pid);
                        }
                    }
                }
            }
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }

    // If fetching fails, use built-in random generation logic as a fallback
    let mock_id = crate::proxy::project_resolver::generate_mock_project_id();
    crate::modules::logger::log_warn(&format!("Account is not eligible for official cloudaicompanionProject, quota query will use randomly generated Project ID as fallback: {}", mock_id));
    Some(mock_id)
}

/// Query account quota
pub async fn fetch_quota(
    access_token: &str,
) -> crate::error::AppResult<(QuotaData, Option<String>)> {
    use crate::error::AppError;
    crate::modules::logger::log_info("Starting external quota query...");
    let client = create_client();

    // 1. Get Project ID
    let project_id = fetch_project_id(access_token).await;
    crate::modules::logger::log_info(&format!("Project ID fetch result: {:?}", project_id));

    // 2. Build request body
    let mut payload = serde_json::Map::new();
    if let Some(ref pid) = project_id {
        payload.insert("project".to_string(), json!(pid));
    }

    let url = QUOTA_API_URL;
    let max_retries = 3;
    let mut last_error: Option<AppError> = None;

    crate::modules::logger::log_info(&format!("Sending quota request to {}", url));

    for attempt in 1..=max_retries {
        match client
            .post(url)
            .bearer_auth(access_token)
            .header("User-Agent", USER_AGENT)
            .json(&json!(payload))
            .send()
            .await
        {
            Ok(response) => {
                // Convert HTTP error status to AppError
                if let Err(_) = response.error_for_status_ref() {
                    let status = response.status();

                    // ✅ Special handling for 403 Forbidden - return directly, do not retry
                    if status == reqwest::StatusCode::FORBIDDEN {
                        crate::modules::logger::log_warn(&format!(
                            "Account has no permission (403 Forbidden), marking as forbidden status"
                        ));
                        let mut q = QuotaData::new();
                        q.is_forbidden = true;
                        return Ok((q, project_id));
                    }

                    // Continue retry logic for other errors
                    if attempt < max_retries {
                        let text = response.text().await.unwrap_or_default();
                        crate::modules::logger::log_warn(&format!(
                            "API Error: {} - {} (Attempt {}/{})",
                            status, text, attempt, max_retries
                        ));
                        last_error = Some(AppError::Unknown(format!("HTTP {} - {}", status, text)));
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    } else {
                        let text = response.text().await.unwrap_or_default();
                        return Err(AppError::Unknown(format!(
                            "API Error: {} - {}",
                            status, text
                        )));
                    }
                }

                let quota_response: QuotaResponse =
                    response.json().await.map_err(|e| AppError::Network(e))?;

                let mut quota_data = QuotaData::new();

                crate::modules::logger::log_info(&format!(
                    "Quota API returned {} models:",
                    quota_response.models.len()
                ));

                for (name, info) in quota_response.models {
                    crate::modules::logger::log_info(&format!("   - {}", name));
                    if let Some(quota_info) = info.quota_info {
                        let percentage = quota_info
                            .remaining_fraction
                            .map(|f| (f * 100.0) as i32)
                            .unwrap_or(0);

                        let reset_time = quota_info.reset_time.unwrap_or_default();

                        // Only save models we care about
                        if name.contains("gemini") || name.contains("claude") {
                            quota_data.add_model(name, percentage, reset_time);
                        }
                    }
                }

                return Ok((quota_data, project_id));
            }
            Err(e) => {
                crate::modules::logger::log_warn(&format!(
                    "Request failed: {} (Attempt {}/{})",
                    e, attempt, max_retries
                ));
                last_error = Some(AppError::Network(e));
                if attempt < max_retries {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| AppError::Unknown("Quota query failed".to_string())))
}

/// Batch query all account quotas (fallback function)
#[allow(dead_code)]
pub async fn fetch_all_quotas(
    accounts: Vec<(String, String)>,
) -> Vec<(String, crate::error::AppResult<QuotaData>)> {
    let mut results = Vec::new();

    for (account_id, access_token) in accounts {
        let result = fetch_quota(&access_token).await.map(|(q, _)| q);
        results.push((account_id, result));
    }

    results
}
