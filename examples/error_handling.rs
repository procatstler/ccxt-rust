//! Error Handling Example
//!
//! Demonstrates how to handle various error types in ccxt-rust:
//! - Network errors and retries
//! - Rate limiting
//! - Invalid symbols
//! - Authentication errors
//! - Error classification and recovery

#![cfg(feature = "cex")]

use ccxt_rust::client::ExchangeConfig;
use ccxt_rust::exchanges::cex::Binance;
use ccxt_rust::types::Exchange;
use ccxt_rust::{CcxtError, CcxtResult};
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> CcxtResult<()> {
    println!("=== CCXT-Rust Error Handling Example ===\n");

    let config = ExchangeConfig::new();
    let exchange = Binance::new(config)?;

    // Example 1: Handling invalid symbols
    println!("--- Example 1: Invalid Symbol ---");
    handle_invalid_symbol(&exchange).await;

    // Example 2: Using error classification
    println!("\n--- Example 2: Error Classification ---");
    demonstrate_error_classification();

    // Example 3: Retry logic with exponential backoff
    println!("\n--- Example 3: Retry with Backoff ---");
    retry_with_backoff(&exchange).await;

    // Example 4: Pattern matching on error types
    println!("\n--- Example 4: Pattern Matching ---");
    pattern_match_errors(&exchange).await;

    println!("\n=== Error Handling Example Complete ===");
    Ok(())
}

/// Demonstrates handling of invalid symbol errors
async fn handle_invalid_symbol(exchange: &Binance) {
    let invalid_symbol = "INVALID/SYMBOL";

    match exchange.fetch_ticker(invalid_symbol).await {
        Ok(ticker) => {
            println!("Unexpected success: {:?}", ticker.last);
        },
        Err(CcxtError::BadSymbol { symbol }) => {
            println!("Caught BadSymbol error for: {}", symbol);
            println!("  -> Suggestion: Check available markets with fetch_markets()");
        },
        Err(CcxtError::BadRequest { message }) => {
            println!("Bad request: {}", message);
        },
        Err(e) => {
            println!("Other error: {} (code: {})", e, e.code());
        },
    }
}

/// Demonstrates error classification methods
fn demonstrate_error_classification() {
    // Network error example
    let network_err = CcxtError::NetworkError {
        url: "https://api.binance.com".into(),
        message: "Connection refused".into(),
    };

    println!("NetworkError:");
    println!("  is_retryable: {}", network_err.is_retryable());
    println!("  is_network_error: {}", network_err.is_network_error());
    println!(
        "  suggested_retry_after: {:?}ms",
        network_err.suggested_retry_after()
    );

    // Rate limit error example
    let rate_err = CcxtError::RateLimitExceeded {
        message: "Too many requests".into(),
        retry_after_ms: Some(5000),
    };

    println!("\nRateLimitExceeded:");
    println!("  is_retryable: {}", rate_err.is_retryable());
    println!(
        "  suggested_retry_after: {:?}ms",
        rate_err.suggested_retry_after()
    );

    // Authentication error example
    let auth_err = CcxtError::AuthenticationError {
        message: "Invalid API key".into(),
    };

    println!("\nAuthenticationError:");
    println!("  is_retryable: {}", auth_err.is_retryable());
    println!("  is_auth_error: {}", auth_err.is_auth_error());
    println!("  is_permanent: {}", auth_err.is_permanent());

    // Order error example
    let order_err = CcxtError::OrderNotFound {
        order_id: "12345".into(),
    };

    println!("\nOrderNotFound:");
    println!("  is_order_error: {}", order_err.is_order_error());
    println!("  code: {}", order_err.code());
}

/// Demonstrates retry logic with exponential backoff
async fn retry_with_backoff(exchange: &Binance) {
    let symbol = "BTC/USDT";
    let max_retries = 3;
    let mut attempt = 0;
    let mut delay_ms = 1000u64;

    loop {
        attempt += 1;
        println!("Attempt {}/{}", attempt, max_retries);

        match exchange.fetch_ticker(symbol).await {
            Ok(ticker) => {
                println!("Success! Last price: {:?}", ticker.last);
                break;
            },
            Err(e) if e.is_retryable() && attempt < max_retries => {
                let wait = e.suggested_retry_after().unwrap_or(delay_ms);
                println!("Retryable error: {}. Waiting {}ms...", e.code(), wait);
                sleep(Duration::from_millis(wait)).await;
                delay_ms = (delay_ms * 2).min(30000); // Exponential backoff, max 30s
            },
            Err(e) => {
                println!("Non-retryable error or max retries reached: {}", e);
                break;
            },
        }
    }
}

/// Demonstrates comprehensive pattern matching on error types
async fn pattern_match_errors(exchange: &Binance) {
    // Try a valid request first
    let result = exchange.fetch_ticker("ETH/USDT").await;

    match result {
        Ok(ticker) => {
            println!("ETH/USDT price: {:?}", ticker.last);
        },
        // Exchange-specific errors
        Err(CcxtError::ExchangeError { message }) => {
            println!("Exchange error: {}", message);
        },
        // Authentication errors
        Err(CcxtError::AuthenticationError { message }) => {
            println!("Auth failed: {} - Check your API credentials", message);
        },
        Err(CcxtError::PermissionDenied { message }) => {
            println!("Permission denied: {} - Check API key permissions", message);
        },
        // Symbol errors
        Err(CcxtError::BadSymbol { symbol }) => {
            println!("Invalid symbol: {}", symbol);
        },
        // Network errors (retryable)
        Err(CcxtError::NetworkError { url, message }) => {
            println!("Network error to {}: {}", url, message);
        },
        Err(CcxtError::RequestTimeout { url }) => {
            println!("Request timeout: {}", url);
        },
        Err(CcxtError::RateLimitExceeded {
            message,
            retry_after_ms,
        }) => {
            println!(
                "Rate limited: {}. Retry after: {:?}ms",
                message, retry_after_ms
            );
        },
        // Exchange status
        Err(CcxtError::ExchangeNotAvailable { message }) => {
            println!("Exchange unavailable: {}", message);
        },
        Err(CcxtError::OnMaintenance { message }) => {
            println!("Exchange on maintenance: {}", message);
        },
        // Order errors
        Err(CcxtError::InsufficientFunds {
            currency,
            required,
            available,
        }) => {
            println!(
                "Insufficient {}: need {}, have {}",
                currency, required, available
            );
        },
        Err(CcxtError::OrderNotFound { order_id }) => {
            println!("Order not found: {}", order_id);
        },
        // Catch-all for other errors
        Err(e) => {
            println!("Unhandled error type: {} ({})", e, e.code());

            // Use helper methods to decide action
            if e.is_retryable() {
                println!("  -> This error is retryable");
            }
            if e.is_network_error() {
                println!("  -> This is a network-related error");
            }
        },
    }
}
