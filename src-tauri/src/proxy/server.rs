use crate::proxy::TokenManager;
use axum::{
    extract::DefaultBodyLimit,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tokio::sync::oneshot;
use tower_http::trace::TraceLayer;
use tracing::{debug, error};

/// Axum application state
#[derive(Clone)]
pub struct AppState {
    pub token_manager: Arc<TokenManager>,
    pub anthropic_mapping: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    pub openai_mapping: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    pub custom_mapping: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    #[allow(dead_code)]
    pub request_timeout: u64, // API request timeout (seconds)
    #[allow(dead_code)]
    pub thought_signature_map: Arc<tokio::sync::Mutex<std::collections::HashMap<String, String>>>, // Chain of thought signature map (ID -> Signature)
    #[allow(dead_code)]
    pub upstream_proxy: Arc<tokio::sync::RwLock<crate::proxy::config::UpstreamProxyConfig>>,
    pub upstream: Arc<crate::proxy::upstream::client::UpstreamClient>,
}

/// Axum server instance
pub struct AxumServer {
    shutdown_tx: Option<oneshot::Sender<()>>,
    anthropic_mapping: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    openai_mapping: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    custom_mapping: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    proxy_state: Arc<tokio::sync::RwLock<crate::proxy::config::UpstreamProxyConfig>>,
}

impl AxumServer {
    pub async fn update_mapping(&self, config: &crate::proxy::config::ProxyConfig) {
        {
            let mut m = self.anthropic_mapping.write().await;
            *m = config.anthropic_mapping.clone();
        }
        {
            let mut m = self.openai_mapping.write().await;
            *m = config.openai_mapping.clone();
        }
        {
            let mut m = self.custom_mapping.write().await;
            *m = config.custom_mapping.clone();
        }
        tracing::info!("Model mapping (Anthropic/OpenAI/Custom) has been fully hot updated");
    }

    /// Update proxy configuration
    pub async fn update_proxy(&self, new_config: crate::proxy::config::UpstreamProxyConfig) {
        let mut proxy = self.proxy_state.write().await;
        *proxy = new_config;
        tracing::info!("Upstream proxy configuration has been hot updated");
    }
    /// Start Axum server
    pub async fn start(
        host: String,
        port: u16,
        token_manager: Arc<TokenManager>,
        anthropic_mapping: std::collections::HashMap<String, String>,
        openai_mapping: std::collections::HashMap<String, String>,
        custom_mapping: std::collections::HashMap<String, String>,
        _request_timeout: u64,
        upstream_proxy: crate::proxy::config::UpstreamProxyConfig,
    ) -> Result<(Self, tokio::task::JoinHandle<()>), String> {
        let mapping_state = Arc::new(tokio::sync::RwLock::new(anthropic_mapping));
        let openai_mapping_state = Arc::new(tokio::sync::RwLock::new(openai_mapping));
        let custom_mapping_state = Arc::new(tokio::sync::RwLock::new(custom_mapping));
        let proxy_state = Arc::new(tokio::sync::RwLock::new(upstream_proxy.clone()));

        let state = AppState {
            token_manager: token_manager.clone(),
            anthropic_mapping: mapping_state.clone(),
            openai_mapping: openai_mapping_state.clone(),
            custom_mapping: custom_mapping_state.clone(),
            request_timeout: 300, // 5 minutes timeout
            thought_signature_map: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            upstream_proxy: proxy_state.clone(),
            upstream: Arc::new(crate::proxy::upstream::client::UpstreamClient::new(Some(
                upstream_proxy.clone(),
            ))),
        };

        // Build routes - Use new architecture handlers!
        use crate::proxy::handlers;
        // Build routes
        let app = Router::new()
            // OpenAI Protocol
            .route("/v1/models", get(handlers::openai::handle_list_models))
            .route(
                "/v1/chat/completions",
                post(handlers::openai::handle_chat_completions),
            )
            .route(
                "/v1/completions",
                post(handlers::openai::handle_completions),
            )
            .route("/v1/responses", post(handlers::openai::handle_completions)) // Compatible with Codex CLI
            // Claude Protocol
            .route("/v1/messages", post(handlers::claude::handle_messages))
            .route(
                "/v1/messages/count_tokens",
                post(handlers::claude::handle_count_tokens),
            )
            .route(
                "/v1/models/claude",
                get(handlers::claude::handle_list_models),
            )
            // Gemini Protocol (Native)
            .route("/v1beta/models", get(handlers::gemini::handle_list_models))
            // Handle both GET (get info) and POST (generateContent with colon) at the same route
            .route(
                "/v1beta/models/:model",
                get(handlers::gemini::handle_get_model).post(handlers::gemini::handle_generate),
            )
            .route(
                "/v1beta/models/:model/countTokens",
                post(handlers::gemini::handle_count_tokens),
            ) // Specific route priority
            .route("/healthz", get(health_check_handler))
            .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
            .layer(TraceLayer::new_for_http())
            .layer(axum::middleware::from_fn(
                crate::proxy::middleware::auth_middleware,
            ))
            .layer(crate::proxy::middleware::cors_layer())
            .with_state(state);

        // Bind address
        let addr = format!("{}:{}", host, port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("Failed to bind address {}: {}", addr, e))?;

        tracing::info!("Reverse proxy server started at http://{}", addr);

        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        let server_instance = Self {
            shutdown_tx: Some(shutdown_tx),
            anthropic_mapping: mapping_state.clone(),
            openai_mapping: openai_mapping_state.clone(),
            custom_mapping: custom_mapping_state.clone(),
            proxy_state,
        };

        // Start server in new task
        let handle = tokio::spawn(async move {
            use hyper::server::conn::http1;
            use hyper_util::rt::TokioIo;
            use hyper_util::service::TowerToHyperService;

            loop {
                tokio::select! {
                    res = listener.accept() => {
                        match res {
                            Ok((stream, _)) => {
                                let io = TokioIo::new(stream);
                                let service = TowerToHyperService::new(app.clone());

                                tokio::task::spawn(async move {
                                    if let Err(err) = http1::Builder::new()
                                        .serve_connection(io, service)
                                        .with_upgrades() // Support WebSocket (if needed later)
                                        .await
                                    {
                                        debug!("Connection handling finished or errored: {:?}", err);
                                    }
                                });
                            }
                            Err(e) => {
                                error!("Failed to accept connection: {:?}", e);
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        tracing::info!("Reverse proxy server stopped listening");
                        break;
                    }
                }
            }
        });

        Ok((server_instance, handle))
    }

    /// Stop server
    pub fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

// ===== API Handlers (Old code removed, taken over by src/proxy/handlers/*) =====

/// Health check handler
async fn health_check_handler() -> Response {
    Json(serde_json::json!({
        "status": "ok"
    }))
    .into_response()
}
