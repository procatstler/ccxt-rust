//! HTTP Client and related utilities

mod config;

// Native platform (tokio-based)
#[cfg(feature = "native")]
mod cache;
#[cfg(feature = "native")]
mod http;
#[cfg(feature = "native")]
mod websocket;
#[cfg(feature = "native")]
mod rate_limiter;

// WASM platform (web-sys based)
#[cfg(feature = "wasm")]
mod http_wasm;
#[cfg(feature = "wasm")]
mod websocket_wasm;

pub use config::{ExchangeConfig, ProxyConfig, RetryConfig};

#[cfg(feature = "native")]
pub use cache::{create_shared_cache, Cache, CacheConfig, CacheStats, ExchangeCache, SharedCache};

#[cfg(feature = "native")]
pub use rate_limiter::{RateLimiter, RateLimiterStats};

// Re-export based on platform
#[cfg(feature = "native")]
pub use http::HttpClient;
#[cfg(feature = "native")]
pub use websocket::{
    ConnectionState, HealthCheckConfig, ReconnectConfig, Subscription, WsClient, WsCommand,
    WsConfig, WsEvent, WsMetrics, WsMetricsSnapshot,
};

#[cfg(feature = "wasm")]
pub use http_wasm::HttpClient;
#[cfg(feature = "wasm")]
pub use websocket_wasm::{
    ConnectionState, HealthCheckConfig, ReconnectConfig, Subscription, WsClient, WsCommand,
    WsConfig, WsEvent, WsMetrics, WsMetricsSnapshot,
};
