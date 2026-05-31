use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Initialize logging with structured output
pub fn init_logging() {
    // Create a filter that respects RUST_LOG environment variable
    // Default to info level if not set
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,hyper=warn,reqwest=warn"));

    // Create a formatting layer with JSON output for production
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        .json(); // Use JSON format for structured logging

    // Initialize the subscriber
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .init();

    tracing::info!("Logging initialized");
}

/// Initialize logging with pretty output for development
pub fn init_logging_pretty() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("debug,sqlx=warn,hyper=warn,reqwest=warn"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_file(true)
        .with_line_number(true)
        .pretty(); // Use pretty format for development

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .init();

    tracing::info!("Logging initialized (pretty mode)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logging_init() {
        // This test just ensures the logging initialization doesn't panic
        // We can't actually test the output without more complex setup
        init_logging_pretty();
    }
}
