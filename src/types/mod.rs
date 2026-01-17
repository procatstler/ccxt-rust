//! CCXT Type System
//!
//! CCXT 원본의 모든 타입을 Rust로 포팅
//!
//! # Module Organization
//!
//! ## Core Types
//! Basic trading types like orders, trades, balances, etc.
//!
//! ## Sub-traits
//! The monolithic `Exchange` trait is logically split into sub-traits:
//! - [`traits::ExchangeBase`] - Core metadata and utilities
//! - [`traits::PublicMarketApi`] - Public market data
//! - [`traits::SpotTradingApi`] - Spot trading operations
//! - [`traits::AccountApi`] - Account/wallet operations
//! - [`traits::DerivativesApi`] - Futures/perpetual operations
//! - [`traits::MarginApi`] - Margin trading
//! - [`traits::OptionsApi`] - Options trading
//! - [`traits::ConvertApi`] - Currency conversion

// Core types
mod balance;
mod currency;
mod exchange;
mod fee;
mod market;
mod ohlcv;
mod order;
mod orderbook;
mod ticker;
mod trade;
mod transaction;

// Derivatives & Futures types
mod convert;
mod derivatives;
mod funding;
mod leverage;
mod liquidation;
mod margin;
mod open_interest;
mod position;

// Account types
mod account;

// WebSocket types (native only - uses tokio)
#[cfg(feature = "native")]
mod ws_exchange;

// Sub-traits module
pub mod traits;

// Core type exports
pub use balance::*;
pub use currency::*;
pub use exchange::*;
pub use fee::*;
pub use market::*;
pub use ohlcv::*;
pub use order::*;
pub use orderbook::*;
pub use ticker::*;
pub use trade::*;
pub use transaction::*;

// Derivatives & Futures exports
pub use convert::*;
pub use derivatives::*;
pub use funding::*;
pub use leverage::*;
pub use liquidation::*;
pub use margin::*;
pub use open_interest::*;
pub use position::*;

// Account exports
pub use account::*;

// WebSocket exports (native only)
#[cfg(feature = "native")]
pub use ws_exchange::*;

// Sub-trait re-exports for convenience
pub use traits::{
    AccountApi, ConvertApi, DerivativesApi, ExchangeBase, MarginApi,
    OptionsApi, PublicMarketApi, SpotTradingApi,
};
