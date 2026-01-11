//! Convert API Trait
//!
//! Currency conversion operations.

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::errors::CcxtResult;
use crate::not_supported;
use crate::types::{ConvertCurrencyPair, ConvertQuote, ConvertTrade};

/// Currency Conversion API
///
/// Operations for converting between currencies:
/// - Supported conversion pairs
/// - Quote fetching
/// - Trade execution
/// - Trade history
#[async_trait]
pub trait ConvertApi: Send + Sync {
    // ========================================================================
    // Conversion Information
    // ========================================================================

    /// Fetch supported currency pairs for conversion
    async fn fetch_convert_currencies(&self) -> CcxtResult<Vec<ConvertCurrencyPair>> {
        not_supported!("fetchConvertCurrencies")
    }

    // ========================================================================
    // Quotes
    // ========================================================================

    /// Fetch a quote for currency conversion
    async fn fetch_convert_quote(
        &self,
        from_code: &str,
        to_code: &str,
        amount: Decimal,
    ) -> CcxtResult<ConvertQuote> {
        let _ = (from_code, to_code, amount);
        not_supported!("fetchConvertQuote")
    }

    // ========================================================================
    // Trade Execution
    // ========================================================================

    /// Execute a convert trade using a quote
    async fn create_convert_trade(&self, quote_id: &str) -> CcxtResult<ConvertTrade> {
        let _ = quote_id;
        not_supported!("createConvertTrade")
    }

    // ========================================================================
    // Trade History
    // ========================================================================

    /// Fetch a specific convert trade
    async fn fetch_convert_trade(&self, id: &str) -> CcxtResult<ConvertTrade> {
        let _ = id;
        not_supported!("fetchConvertTrade")
    }

    /// Fetch convert trade history
    async fn fetch_convert_trade_history(
        &self,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<ConvertTrade>> {
        let _ = (since, limit);
        not_supported!("fetchConvertTradeHistory")
    }
}
