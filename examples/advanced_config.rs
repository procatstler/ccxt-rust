//! Advanced Configuration Example
//!
//! Demonstrates advanced configuration options in ccxt-rust:
//! - Timeouts and rate limiting
//! - Retry configuration with exponential backoff
//! - Proxy configuration
//! - API credentials management
//! - Sandbox/testnet mode
//! - Custom hostname configuration

#![cfg(feature = "cex")]

use ccxt_rust::client::{ExchangeConfig, RetryConfig};
use ccxt_rust::exchanges::cex::Binance;
use ccxt_rust::types::Exchange;
use ccxt_rust::CcxtResult;

#[tokio::main]
async fn main() -> CcxtResult<()> {
    println!("=== CCXT-Rust Advanced Configuration Example ===\n");

    // Example 1: Basic configuration with timeout
    println!("--- Example 1: Basic Configuration ---");
    basic_config_example().await?;

    // Example 2: Retry configuration
    println!("\n--- Example 2: Retry Configuration ---");
    retry_config_example();

    // Example 3: Full configuration with all options
    println!("\n--- Example 3: Full Configuration ---");
    full_config_example();

    // Example 4: Environment-based credentials
    println!("\n--- Example 4: Environment Credentials ---");
    env_credentials_example();

    println!("\n=== Advanced Configuration Example Complete ===");
    Ok(())
}

/// Basic configuration with timeout and rate limiting
async fn basic_config_example() -> CcxtResult<()> {
    // Create config with custom timeout (30 seconds)
    let config = ExchangeConfig::new()
        .with_timeout(30000) // 30 second timeout
        .with_rate_limit_ms(100); // 100ms between requests

    let exchange = Binance::new(config)?;

    println!("Exchange: {}", exchange.name());
    println!("Timeout: {}ms", 30000);
    println!("Rate limit: {}ms between requests", 100);

    // Test with a simple request
    let markets = exchange.fetch_markets().await?;
    println!("Fetched {} markets", markets.len());

    Ok(())
}

/// Demonstrates retry configuration options
fn retry_config_example() {
    // Default retry configuration
    let default_retry = RetryConfig::default();
    println!("Default RetryConfig:");
    println!("  max_retries: {}", default_retry.max_retries);
    println!("  initial_delay_ms: {}", default_retry.initial_delay_ms);
    println!("  max_delay_ms: {}", default_retry.max_delay_ms);
    println!("  backoff_multiplier: {}", default_retry.backoff_multiplier);
    println!(
        "  retry_on_network_error: {}",
        default_retry.retry_on_network_error
    );

    // Custom retry configuration using builder pattern
    let custom_retry = RetryConfig {
        max_retries: 5,
        initial_delay_ms: 500,
        max_delay_ms: 30000,
        backoff_multiplier: 2.0,
        retry_status_codes: vec![429, 500, 502, 503, 504],
        retry_on_network_error: true,
    };

    println!("\nCustom RetryConfig:");
    println!("  max_retries: {}", custom_retry.max_retries);
    println!("  initial_delay_ms: {}", custom_retry.initial_delay_ms);
    println!("  max_delay_ms: {}", custom_retry.max_delay_ms);

    // Apply to exchange config
    let _config = ExchangeConfig::new().with_retry(custom_retry);

    println!("\nRetry config applied to ExchangeConfig");
}

/// Full configuration with all available options
fn full_config_example() {
    // Comprehensive configuration example
    let config = ExchangeConfig::new()
        // Timing
        .with_timeout(30000) // Request timeout in ms
        .with_rate_limit_ms(100) // Minimum time between requests
        // Retry policy
        .with_max_retries(3) // Number of retry attempts
        // Sandbox mode (for testing)
        .with_sandbox(false); // Use production API

    println!("Full configuration created:");
    println!("  Timeout: {}ms", config.timeout_ms());
    println!("  Sandbox: {}", config.is_sandbox());
    println!("  Has API Key: {}", config.api_key().is_some());

    // Configuration with credentials (for private endpoints)
    let _authenticated_config =
        ExchangeConfig::new().with_credentials("your_api_key", "your_secret");

    println!("\nAuthenticated config created (credentials hidden)");
}

/// Environment-based credentials management
fn env_credentials_example() {
    println!("Reading credentials from environment variables...\n");

    // Best practice: Load credentials from environment
    let api_key = std::env::var("BINANCE_API_KEY").ok();
    let api_secret = std::env::var("BINANCE_SECRET").ok();

    match (&api_key, &api_secret) {
        (Some(_), Some(_)) => {
            let config = ExchangeConfig::new()
                .with_api_key(api_key.unwrap())
                .with_api_secret(api_secret.unwrap());

            println!("Credentials loaded from environment");
            println!("  Has API Key: {}", config.api_key().is_some());
            println!("  Has Secret: {}", config.api_secret().is_some());
        },
        _ => {
            println!("Environment variables not set.");
            println!("To use authenticated endpoints, set:");
            println!("  export BINANCE_API_KEY=\"your_key\"");
            println!("  export BINANCE_SECRET=\"your_secret\"");
        },
    }

    // Example of loading from different exchanges
    println!("\nSupported environment variable patterns:");
    println!("  BINANCE_API_KEY, BINANCE_SECRET");
    println!("  OKX_API_KEY, OKX_SECRET, OKX_PASSWORD");
    println!("  BYBIT_API_KEY, BYBIT_SECRET");
    println!("  HYPERLIQUID_PRIVATE_KEY (for DEX)");
}

/// Helper to create config from environment for any exchange
#[allow(dead_code)]
fn config_from_env(exchange_prefix: &str) -> ExchangeConfig {
    let api_key = std::env::var(format!("{}_API_KEY", exchange_prefix)).ok();
    let api_secret = std::env::var(format!("{}_SECRET", exchange_prefix)).ok();
    let password = std::env::var(format!("{}_PASSWORD", exchange_prefix)).ok();

    let mut config = ExchangeConfig::new();

    if let Some(key) = api_key {
        config = config.with_api_key(key);
    }
    if let Some(secret) = api_secret {
        config = config.with_api_secret(secret);
    }
    if let Some(pass) = password {
        config = config.with_password(pass);
    }

    config
}
