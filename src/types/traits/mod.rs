//! Exchange Sub-traits Module
//!
//! This module splits the monolithic Exchange trait into logical sub-traits
//! for better organization and maintainability.
//!
//! # Sub-traits
//!
//! - [`ExchangeBase`] - Core metadata, market loading, and utilities
//! - [`PublicMarketApi`] - Public market data (ticker, orderbook, trades, OHLCV)
//! - [`SpotTradingApi`] - Spot trading operations (orders, balance)
//! - [`AccountApi`] - Account and wallet operations (deposits, withdrawals, transfers)
//! - [`DerivativesApi`] - Futures/perpetual operations (positions, leverage, funding)
//! - [`MarginApi`] - Margin trading operations (borrow, repay)
//! - [`OptionsApi`] - Options trading operations
//! - [`ConvertApi`] - Currency conversion operations

mod base;
mod public_market;
mod spot_trading;
mod account;
mod derivatives;
mod margin;
mod options;
mod convert;

pub use base::ExchangeBase;
pub use public_market::PublicMarketApi;
pub use spot_trading::SpotTradingApi;
pub use account::AccountApi;
pub use derivatives::DerivativesApi;
pub use margin::MarginApi;
pub use options::OptionsApi;
pub use convert::ConvertApi;

/// Macro to generate NotSupported error for unimplemented methods
#[macro_export]
macro_rules! not_supported {
    ($feature:expr) => {
        Err($crate::errors::CcxtError::NotSupported {
            feature: $feature.into(),
        })
    };
}

/// Re-export the macro for internal use
pub use not_supported;
