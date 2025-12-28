// Claude Protocol Handler

use axum::{
    body::Body,
    extract::{Json, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};
use tracing::{debug, error};

use crate::proxy::mappers::claude::{
    create_claude_sse_stream, transform_claude_request_in, transform_response, ClaudeRequest,
};
use crate::proxy::server::AppState;

const MAX_RETRY_ATTEMPTS: usize = 3;

/// Handle Claude messages request
///
/// Handle Chat message request flow
pub async fn handle_messages(
    State(state): State<AppState>,
    Json(request): Json<ClaudeRequest>,
) -> Response {
    // Get the latest "meaningful" message content (for logging and background task detection)
    // Strategy: Traverse backwards, first filter all messages related to the user (role="user")
    // Then extract its text content, skipping "Warmup" or system preset reminder
    let meaningful_msg = request
        .messages
        .iter()
        .rev()
        .filter(|m| m.role == "user")
        .find_map(|m| {
            let content = match &m.content {
                crate::proxy::mappers::claude::models::MessageContent::String(s) => s.to_string(),
                crate::proxy::mappers::claude::models::MessageContent::Array(arr) => {
                    // For arrays, extract all Text blocks and join them, ignoring ToolResult
                    arr.iter()
                        .filter_map(|block| match block {
                            crate::proxy::mappers::claude::models::ContentBlock::Text { text } => {
                                Some(text.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                }
            };

            // Filtering rules:
            // 1. Ignore empty messages
            // 2. Ignore "Warmup" messages
            // 3. Ignore <system-reminder> tag messages
            if content.trim().is_empty()
                || content.starts_with("Warmup")
                || content.contains("<system-reminder>")
            {
                None
            } else {
                Some(content)
            }
        });

    // If still not found after filtering (e.g. pure tool call), fallback to original display of the last message
    let latest_msg = meaningful_msg.unwrap_or_else(|| {
        request
            .messages
            .last()
            .map(|m| match &m.content {
                crate::proxy::mappers::claude::models::MessageContent::String(s) => s.clone(),
                crate::proxy::mappers::claude::models::MessageContent::Array(_) => {
                    "[Complex/Tool Message]".to_string()
                }
            })
            .unwrap_or_else(|| "[No Messages]".to_string())
    });

    crate::modules::logger::log_info(&format!(
        "Received Claude request for model: {}, content_preview: {:.100}...",
        request.model, latest_msg
    ));

    // 1. Get Session ID (Content-based hash deprecated, using TokenManager internal time window lock)
    let session_id: Option<&str> = None;

    // 2. Get UpstreamClient
    let upstream = state.upstream.clone();

    // 3. Prepare closure
    let mut request_for_body = request.clone();
    let token_manager = state.token_manager;

    let pool_size = token_manager.len();
    let max_attempts = MAX_RETRY_ATTEMPTS.min(pool_size).max(1);

    let mut last_error = String::new();
    let mut retried_without_thinking = false;

    for attempt in 0..max_attempts {
        // 3. Model routing and configuration parsing (parse early to determine request type)
        let mut mapped_model = crate::proxy::common::model_mapping::resolve_model_route(
            &request_for_body.model,
            &*state.custom_mapping.read().await,
            &*state.openai_mapping.read().await,
            &*state.anthropic_mapping.read().await,
        );
        let config = crate::proxy::mappers::common_utils::resolve_request_config(
            &request_for_body.model,
            &mapped_model,
        );

        // 4. Get Token (use accurate request_type)
        let (access_token, project_id, email) =
            match token_manager.get_token(&config.request_type, false).await {
                Ok(t) => t,
                Err(e) => {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({
                            "type": "error",
                            "error": {
                                "type": "overloaded_error",
                                "message": format!("No available accounts: {}", e)
                            }
                        })),
                    )
                        .into_response();
                }
            };

        tracing::info!(
            "Using account: {} for request (type: {})",
            email,
            config.request_type
        );

        // --- Core Optimization: Intelligent identification and interception of background automatic requests ---
        // Keyword recognition: title generation, summary extraction, next step prompt suggestions, etc.
        // [Optimization] Use longer preview window (500 chars) to capture more specific intent
        let preview_msg = latest_msg.chars().take(500).collect::<String>();
        let is_background_task = preview_msg.contains("write a 5-10 word title")
            || preview_msg.contains("Respond with the title")
            || preview_msg.contains("Concise summary")
            || preview_msg.contains("prompt suggestion generator");

        // Pass mapped model name
        let mut request_with_mapped = request_for_body.clone();

        if is_background_task {
            mapped_model = "gemini-2.5-flash".to_string();
            tracing::info!("[AUTO] Background task detected ({}...), intelligently redirected to cheap node: {}", 
                preview_msg,
                mapped_model
             );
            // [Optimization] **Background task purification**:
            // Such tasks are purely text processing and never need to execute tools.
            // Force clear tools field to completely eliminate "Multiple tools" (400) conflict risk.
            request_with_mapped.tools = None;
        } else {
            // [USER] Mark real user request
            // [Optimization] Use WARN level to highlight user messages to prevent being drowned by background task logs
            tracing::warn!(
                "[USER] User interaction request detected ({}...), keeping original model: {}",
                preview_msg,
                mapped_model
            );
        }

        request_with_mapped.model = mapped_model;

        // Generate Trace ID (simply use timestamp suffix)
        // let _trace_id = format!("req_{}", chrono::Utc::now().timestamp_subsec_millis());

        let gemini_body = match transform_claude_request_in(&request_with_mapped, &project_id) {
            Ok(b) => b,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "type": "error",
                        "error": {
                            "type": "api_error",
                            "message": format!("Transform error: {}", e)
                        }
                    })),
                )
                    .into_response();
            }
        };

        // 4. Upstream call
        let is_stream = request.stream;
        let method = if is_stream {
            "streamGenerateContent"
        } else {
            "generateContent"
        };
        let query = if is_stream { Some("alt=sse") } else { None };

        let response = match upstream
            .call_v1_internal(method, &access_token, gemini_body, query)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_error = e.clone();
                tracing::warn!(
                    "Request failed on attempt {}/{}: {}",
                    attempt + 1,
                    max_attempts,
                    e
                );
                continue;
            }
        };

        let status = response.status();

        // Success
        if status.is_success() {
            // Handle streaming response
            if request.stream {
                let stream = response.bytes_stream();
                let gemini_stream = Box::pin(stream);
                let claude_stream = create_claude_sse_stream(gemini_stream);

                // Convert to Bytes stream
                let sse_stream = claude_stream.map(|result| -> Result<Bytes, std::io::Error> {
                    match result {
                        Ok(bytes) => Ok(bytes),
                        Err(e) => Ok(Bytes::from(format!("data: {{\"error\":\"{}\"}}\n\n", e))),
                    }
                });

                return Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .header(header::CACHE_CONTROL, "no-cache")
                    .header(header::CONNECTION, "keep-alive")
                    .body(Body::from_stream(sse_stream))
                    .unwrap();
            } else {
                // Handle non-streaming response
                let bytes = match response.bytes().await {
                    Ok(b) => b,
                    Err(e) => {
                        return (
                            StatusCode::BAD_GATEWAY,
                            format!("Failed to read body: {}", e),
                        )
                            .into_response()
                    }
                };

                // Debug print
                if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                    debug!("Upstream Response for Claude request: {}", text);
                }

                let gemini_resp: Value = match serde_json::from_slice(&bytes) {
                    Ok(v) => v,
                    Err(e) => {
                        return (StatusCode::BAD_GATEWAY, format!("Parse error: {}", e))
                            .into_response()
                    }
                };

                // Unwrap response field (v1internal format)
                let raw = gemini_resp.get("response").unwrap_or(&gemini_resp);

                // Convert to Gemini Response structure
                let gemini_response: crate::proxy::mappers::claude::models::GeminiResponse =
                    match serde_json::from_value(raw.clone()) {
                        Ok(r) => r,
                        Err(e) => {
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("Convert error: {}", e),
                            )
                                .into_response()
                        }
                    };

                // Transform
                let claude_response = match transform_response(&gemini_response) {
                    Ok(r) => r,
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Transform error: {}", e),
                        )
                            .into_response()
                    }
                };

                return Json(claude_response).into_response();
            }
        }

        // Handle error
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| format!("HTTP {}", status));
        last_error = format!("HTTP {}: {}", status, error_text);

        let status_code = status.as_u16();

        // Handle transient 429s using upstream-provided retry delay (avoid surfacing errors to clients).
        if status_code == 429 {
            if let Some(delay_ms) = crate::proxy::upstream::retry::parse_retry_delay(&error_text) {
                let actual_delay = delay_ms.saturating_add(200).min(10_000);
                tracing::warn!(
                    "Claude Upstream 429 on attempt {}/{}, waiting {}ms then retrying",
                    attempt + 1,
                    max_attempts,
                    actual_delay
                );
                sleep(Duration::from_millis(actual_delay)).await;
                continue;
            }
        }

        // Special-case 400 errors caused by invalid/foreign thinking signatures (common after /resume).
        // Retry once by stripping thinking blocks & thinking config from the request, and by disabling
        // the "-thinking" model variant if present.
        if status_code == 400
            && !retried_without_thinking
            && (error_text.contains("Invalid `signature`")
                || error_text.contains("thinking.signature: Field required")
                || error_text.contains("thinking.signature"))
        {
            retried_without_thinking = true;
            tracing::warn!(
                "Upstream rejected thinking signature; retrying once with thinking stripped"
            );

            // 1) Remove thinking config
            request_for_body.thinking = None;

            // 2) Remove thinking blocks from message history
            for msg in request_for_body.messages.iter_mut() {
                if let crate::proxy::mappers::claude::models::MessageContent::Array(blocks) =
                    &mut msg.content
                {
                    blocks.retain(|b| {
                        !matches!(
                            b,
                            crate::proxy::mappers::claude::models::ContentBlock::Thinking { .. }
                        )
                    });
                }
            }

            // 3) Prefer non-thinking Claude model variant on retry (best-effort)
            if request_for_body.model.contains("claude-") {
                let mut m = request_for_body.model.clone();
                m = m.replace("-thinking", "");
                // If it's a dated alias, fall back to a stable non-thinking id
                if m.contains("claude-sonnet-4-5-") {
                    m = "claude-sonnet-4-5".to_string();
                } else if m.contains("claude-opus-4-5-") || m.contains("claude-opus-4-") {
                    m = "claude-opus-4-5".to_string();
                }
                request_for_body.model = m;
            }

            continue;
        }

        // Only 429 (Rate Limit), 403 (Permission/Region Restriction) and 401 (Auth Failure) trigger account rotation
        if status_code == 429 || status_code == 403 || status_code == 401 {
            // If it is 429 and marked as quota exhausted (explicit), report error directly to avoid penetrating the entire account pool
            if status_code == 429 && error_text.contains("QUOTA_EXHAUSTED") {
                error!(
                    "Claude Quota exhausted (429) on attempt {}/{}, stopping to protect pool.",
                    attempt + 1,
                    max_attempts
                );
                return (status, error_text).into_response();
            }

            tracing::warn!(
                "Claude Upstream {} on attempt {}/{}, rotating account",
                status,
                attempt + 1,
                max_attempts
            );
            continue;
        }

        // HTTP exceptions like 404 due to model configuration or path errors, report error directly, do not perform invalid rotation
        error!(
            "Claude Upstream non-retryable error {}: {}",
            status_code, error_text
        );
        return (status, error_text).into_response();
    }

    (StatusCode::TOO_MANY_REQUESTS, Json(json!({
        "type": "error",
        "error": {
            "type": "overloaded_error",
            "message": format!("All {} attempts failed. Last error: {}", max_attempts, last_error)
        }
    }))).into_response()
}

/// List available models
pub async fn handle_list_models() -> impl IntoResponse {
    Json(json!({
        "object": "list",
        "data": [
            {
                "id": "claude-sonnet-4-5",
                "object": "model",
                "created": 1706745600,
                "owned_by": "anthropic"
            },
            {
                "id": "claude-opus-4-5-thinking",
                "object": "model",
                "created": 1706745600,
                "owned_by": "anthropic"
            },
            {
                "id": "claude-3-5-sonnet-20241022",
                "object": "model",
                "created": 1706745600,
                "owned_by": "anthropic"
            }
        ]
    }))
}

/// Count tokens (placeholder)
pub async fn handle_count_tokens(Json(_body): Json<Value>) -> impl IntoResponse {
    Json(json!({
        "input_tokens": 0,
        "output_tokens": 0
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_handle_list_models() {
        let response = handle_list_models().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
