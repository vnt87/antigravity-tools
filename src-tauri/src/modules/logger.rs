use crate::modules::account::get_data_dir;
use std::fs;
use std::path::PathBuf;
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub fn get_log_dir() -> Result<PathBuf, String> {
    let data_dir = get_data_dir()?;
    let log_dir = data_dir.join("logs");

    if !log_dir.exists() {
        fs::create_dir_all(&log_dir)
            .map_err(|e| format!("Failed to create log directory: {}", e))?;
    }

    Ok(log_dir)
}

/// Initialize logger system
pub fn init_logger() {
    // Capture log macro logs
    let _ = tracing_log::LogTracer::init();

    let log_dir = match get_log_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("Failed to initialize log directory: {}", e);
            return;
        }
    };

    // 1. Set file Appender (using tracing-appender for rolling logs)
    // Use daily rolling strategy
    let file_appender = tracing_appender::rolling::daily(log_dir, "app.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // 2. Console output layer
    let console_layer = fmt::Layer::new()
        .with_target(false)
        .with_thread_ids(false)
        .with_level(true);

    // 3. File output layer (disable ANSI formatting)
    let file_layer = fmt::Layer::new()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_level(true);

    // 4. Set filter layer (default to INFO and above)
    let filter_layer = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // 5. Initialize global subscriber (use try_init to avoid crash on re-initialization)
    let _ = tracing_subscriber::registry()
        .with(filter_layer)
        .with(console_layer)
        .with(file_layer)
        .try_init();

    // Leak _guard to ensure its lifetime lasts until program exit
    // This is recommended when using tracing_appender::non_blocking (if manual flush is not needed)
    std::mem::forget(_guard);

    info!("Logger system initialized (Console + File Persistence)");
}

/// Clear log cache (use truncate mode to keep file handles valid)
pub fn clear_logs() -> Result<(), String> {
    let log_dir = get_log_dir()?;
    if log_dir.exists() {
        // Iterate through all files in directory and truncate, instead of deleting directory
        let entries =
            fs::read_dir(&log_dir).map_err(|e| format!("Failed to read log directory: {}", e))?;
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_file() {
                    // Open file in truncate mode, setting size to 0
                    let _ = fs::OpenOptions::new().write(true).truncate(true).open(path);
                }
            }
        }
    }
    Ok(())
}

/// Log info message (backward compatible interface)
pub fn log_info(message: &str) {
    info!("{}", message);
}

/// Log warning message (backward compatible interface)
pub fn log_warn(message: &str) {
    warn!("{}", message);
}

/// Log error message (backward compatible interface)
pub fn log_error(message: &str) {
    error!("{}", message);
}
