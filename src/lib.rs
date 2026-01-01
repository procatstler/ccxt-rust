//! CCXT-Rust: Cryptocurrency Exchange Trading Library
//!
//! CCXT를 Rust로 포팅한 거래소 통합 라이브러리

pub mod client;
pub mod crypto;
pub mod errors;
pub mod exchanges;
pub mod types;
pub mod utils;

// Re-exports
pub use client::{ExchangeConfig, HttpClient, RateLimiter};
pub use errors::{CcxtError, CcxtResult};
pub use types::{
    Balance, Balances, BidAsk, Currency, DepositWithdrawFee, Exchange, ExchangeFeatures,
    ExchangeId, ExchangeStatus, ExchangeUrls, Fee, FeeInfo, Market, MarketLimits, MarketPrecision,
    MarketType, NetworkFee, OHLCV, Order, OrderBook, OrderBookEntry, OrderRequest, OrderSide,
    OrderStatus, OrderType, SignedRequest, StopOrderParams, Ticker, TimeInForce, Timeframe,
    Trade, TradingFee, TradingFees, Transaction, TransactionStatus, TransactionType, TriggerType,
};
pub use utils::Precise;
