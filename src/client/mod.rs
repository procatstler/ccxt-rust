//! HTTP Client and related utilities

mod cache;
mod config;
mod http;
mod rate_limiter;
mod websocket;

pub use cache::{create_shared_cache, Cache, CacheConfig, CacheStats, ExchangeCache, SharedCache};
pub use config::{ExchangeConfig, ProxyConfig, RetryConfig};
pub use http::HttpClient;
pub use rate_limiter::{RateLimiter, RateLimiterStats};
pub use websocket::{
    ConnectionState, HealthCheckConfig, ReconnectConfig, Subscription, WsClient, WsCommand,
    WsConfig, WsEvent, WsMetrics, WsMetricsSnapshot,
};
