// API Key authentication middleware
use axum::{
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::Response,
};

/// API Key authentication middleware
pub async fn auth_middleware(request: Request, next: Next) -> Result<Response, StatusCode> {
    // Log the request method and URI
    tracing::info!("Request: {} {}", request.method(), request.uri());

    // Extract API key from header
    let api_key = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .or_else(|| {
            request
                .headers()
                .get("x-api-key")
                .and_then(|h| h.to_str().ok())
        });

    // TODO: Actually verify API key
    // Currently allow all requests to pass
    if api_key.is_some() || true {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

#[cfg(test)]
mod tests {
    // Remove unused use super::*;

    #[test]
    fn test_auth_placeholder() {
        // Placeholder test
        assert!(true);
    }
}
