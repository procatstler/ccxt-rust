//! CCXT-Rust: Cryptocurrency Exchange Trading Library
//!
//! CCXT를 Rust로 포팅한 거래소 통합 라이브러리

pub mod client;
pub mod exchanges;
pub mod types;
pub mod utils;

// Re-exports
pub use client::{ExchangeConfig, HttpClient, RateLimiter};
pub use types::Exchange;
