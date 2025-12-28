// proxy module - API reverse proxy service

// Existing modules (reserved)
pub mod config;
pub mod project_resolver;
pub mod server;
pub mod token_manager;

// New architecture modules
pub mod common;
pub mod handlers; // API endpoint handlers
pub mod mappers; // Protocol mappers
pub mod middleware; // Axum middleware
pub mod upstream; // Upstream client // Common tools

pub use config::ProxyConfig;
pub use server::AxumServer;
pub use token_manager::TokenManager;
