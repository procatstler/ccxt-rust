//! # CCXT-Rust: Cryptocurrency Exchange Trading Library
//!
//! A comprehensive Rust port of the popular CCXT library for cryptocurrency trading.
//! This library provides a unified API to interact with 100+ cryptocurrency exchanges,
//! supporting both REST and WebSocket protocols.
//!
//! ## Features
//!
//! - **100+ Exchanges**: Binance, OKX, Bybit, Coinbase, Kraken, KuCoin, and many more
//! - **Unified API**: Consistent interface across all exchanges
//! - **REST & WebSocket**: Full support for both protocols
//! - **Type Safety**: Strongly typed Rust implementation
//! - **Async/Await**: Built on Tokio for high-performance async operations
//! - **DEX Support**: Hyperliquid, dYdX v4, Paradex, and more
//!
//! ## Quick Start
//!
//! (Requires `cex` feature, enabled by default)
//!
//! ```rust,ignore
//! use ccxt_rust::{Exchange, ExchangeConfig, CcxtResult};
//! use ccxt_rust::exchanges::cex::Binance;
//!
//! #[tokio::main]
//! async fn main() -> CcxtResult<()> {
//!     // Create exchange instance
//!     let config = ExchangeConfig::new()
//!         .with_api_key("your_api_key")
//!         .with_api_secret("your_secret");
//!
//!     let exchange = Binance::new(config)?;
//!
//!     // Fetch markets
//!     let markets = exchange.fetch_markets().await?;
//!     println!("Available markets: {}", markets.len());
//!
//!     // Fetch ticker
//!     let ticker = exchange.fetch_ticker("BTC/USDT").await?;
//!     println!("BTC/USDT last price: {:?}", ticker.last);
//!
//!     Ok(())
//! }
//! ```
//!
//! ## WebSocket Example
//!
//! (Requires `cex` feature, enabled by default)
//!
//! ```rust,ignore
//! use ccxt_rust::exchanges::cex::BinanceWs;
//! use ccxt_rust::types::WsExchange;
//!
//! #[tokio::main]
//! async fn main() -> ccxt_rust::CcxtResult<()> {
//!     let ws = BinanceWs::new();
//!
//!     // Subscribe to ticker updates
//!     let mut rx = ws.watch_ticker("BTC/USDT").await?;
//!
//!     while let Some(msg) = rx.recv().await {
//!         println!("Received: {:?}", msg);
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Trading Example
//!
//! (Requires `cex` feature, enabled by default)
//!
//! ```rust,ignore
//! use ccxt_rust::{Exchange, ExchangeConfig, OrderType, OrderSide};
//! use ccxt_rust::exchanges::cex::Binance;
//! use rust_decimal_macros::dec;
//!
//! #[tokio::main]
//! async fn main() -> ccxt_rust::CcxtResult<()> {
//!     let config = ExchangeConfig::new()
//!         .with_api_key("your_api_key")
//!         .with_api_secret("your_secret");
//!
//!     let exchange = Binance::new(config)?;
//!
//!     // Place a limit order
//!     let order = exchange.create_order(
//!         "BTC/USDT",
//!         OrderType::Limit,
//!         OrderSide::Buy,
//!         dec!(0.001),           // amount
//!         Some(dec!(50000.0)),   // price
//!     ).await?;
//!
//!     println!("Order placed: {}", order.id);
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Supported Exchanges
//!
//! ### Tier 1 (High Volume)
//! - Binance, Binance US, Binance Futures
//! - OKX, OKX US
//! - Bybit
//! - Coinbase, Coinbase Pro, Coinbase International
//! - Kraken, Kraken Futures
//! - KuCoin, KuCoin Futures
//!
//! ### Tier 2 (Major)
//! - Gate.io, HTX (Huobi), Bitfinex, Bitget
//! - MEXC, BitMEX, Deribit, Phemex
//! - Gemini, Bitstamp, Crypto.com
//!
//! ### DEX
//! - Hyperliquid, dYdX v4, Paradex, Apex
//!
//! ## Module Structure
//!
//! - [`client`]: HTTP client, WebSocket client, rate limiting, caching
//! - [`crypto`]: Cryptographic utilities for EVM and Cosmos chains (requires `dex` feature)
//! - [`errors`]: Error types and result handling
//! - [`exchanges`]: Exchange implementations (CEX and DEX)
//! - [`types`]: Core types (Order, Trade, Ticker, etc.)
//! - [`utils`]: Utility functions and precise decimal arithmetic
//!
//! ## Feature Flags
//!
//! - `cex` (default): Centralized exchange support (Binance, OKX, Bybit, etc.)
//! - `dex`: Decentralized exchange support with crypto primitives (Hyperliquid, dYdX, Paradex)
//! - `full`: All features enabled
//!
//! ```toml
//! # CEX only (default, smaller binary)
//! ccxt-rust = "0.1"
//!
//! # DEX support
//! ccxt-rust = { version = "0.1", features = ["dex"] }
//!
//! # All features
//! ccxt-rust = { version = "0.1", features = ["full"] }
//! ```

// Macros must be defined first so they are available to other modules
#[macro_use]
pub mod macros;

pub mod client;
#[cfg(feature = "dex")]
pub mod crypto;
pub mod errors;
#[cfg(feature = "grpc")]
pub mod grpc;
pub mod types;
pub mod exchanges;
pub mod utils;

// WASM bindings (only when wasm feature is enabled)
#[cfg(feature = "wasm")]
pub mod wasm;

// Re-exports for convenience (requires native or wasm feature)
#[cfg(any(feature = "native", feature = "wasm"))]
pub use client::{ExchangeConfig, HttpClient, WsClient, WsConfig, WsEvent};

// RateLimiter only available in native builds
#[cfg(feature = "native")]
pub use client::RateLimiter;
pub use errors::{CcxtError, CcxtResult};
pub use types::{
    // Core trading types
    Balance,
    Balances,
    BidAsk,
    ConvertCurrencyPair,
    // Convert types
    ConvertQuote,
    ConvertTrade,
    Currency,
    // Fee types
    DepositWithdrawFee,
    // Exchange types
    Exchange,
    ExchangeFeatures,
    ExchangeId,
    ExchangeStatus,
    ExchangeUrls,
    Fee,
    FeeInfo,
    FundingRate,
    FundingRateHistory,
    Leverage,
    Liquidation,
    MarginMode,
    MarginModeInfo,
    Market,
    MarketLimits,
    MarketPrecision,
    MarketType,
    NetworkFee,
    OpenInterest,
    Order,
    OrderBook,
    OrderBookEntry,
    OrderRequest,
    OrderSide,
    OrderStatus,
    OrderType,
    // Futures/Margin types
    Position,
    PositionSide,
    SignedRequest,
    // Order parameters
    StopOrderParams,
    Ticker,
    TimeInForce,
    Timeframe,
    Trade,
    TradingFee,
    TradingFees,
    // Transaction types
    Transaction,
    TransactionStatus,
    TransactionType,
    TriggerType,
    OHLCV,
};

// WebSocket types (native only)
#[cfg(feature = "native")]
pub use types::{
    WsExchange,
    WsMessage,
    WsOhlcvEvent,
    WsOrderBookEvent,
    WsTickerEvent,
    WsTradeEvent,
};
pub use utils::Precise;

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Prelude module for convenient imports
pub mod prelude {
    #[cfg(any(feature = "native", feature = "wasm"))]
    pub use crate::client::{ExchangeConfig, HttpClient};
    #[cfg(feature = "native")]
    pub use crate::client::RateLimiter;
    pub use crate::errors::{CcxtError, CcxtResult};
    pub use crate::types::{
        Balance, Exchange, ExchangeId, Market, Order, OrderBook, OrderSide, OrderType, Ticker,
        Timeframe, Trade, OHLCV,
    };
    #[cfg(feature = "native")]
    pub use crate::types::{WsExchange, WsMessage};
    pub use rust_decimal::Decimal;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn test_prelude_imports() {
        use crate::prelude::*;
        let _ = Decimal::from(100);
    }
}
