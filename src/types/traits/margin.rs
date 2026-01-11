//! Margin Trading API Trait
//!
//! Margin trading operations including borrowing and repaying.

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::errors::CcxtResult;
use crate::not_supported;
use crate::types::{
    BorrowInterest, CrossBorrowRate, IsolatedBorrowRate, MarginLoan,
    MarginMode, MarginModeInfo, MarginModification,
};

/// Margin Trading API
///
/// Operations for margin trading:
/// - Margin mode (isolated/cross)
/// - Borrowing and repaying
/// - Margin adjustment
/// - Borrow rates and interest
#[async_trait]
pub trait MarginApi: Send + Sync {
    // ========================================================================
    // Margin Mode
    // ========================================================================

    /// Set margin mode (isolated/cross)
    async fn set_margin_mode(
        &self,
        margin_mode: MarginMode,
        symbol: &str,
    ) -> CcxtResult<MarginModeInfo> {
        let _ = (margin_mode, symbol);
        not_supported!("setMarginMode")
    }

    /// Fetch current margin mode
    async fn fetch_margin_mode(&self, symbol: &str) -> CcxtResult<MarginModeInfo> {
        let _ = symbol;
        not_supported!("fetchMarginMode")
    }

    // ========================================================================
    // Borrowing
    // ========================================================================

    /// Borrow margin
    async fn borrow_margin(
        &self,
        code: &str,
        amount: Decimal,
        symbol: Option<&str>,
    ) -> CcxtResult<BorrowInterest> {
        let _ = (code, amount, symbol);
        not_supported!("borrowMargin")
    }

    /// Borrow cross margin
    async fn borrow_cross_margin(&self, code: &str, amount: Decimal) -> CcxtResult<MarginLoan> {
        let _ = (code, amount);
        not_supported!("borrowCrossMargin")
    }

    /// Borrow isolated margin
    async fn borrow_isolated_margin(
        &self,
        symbol: &str,
        code: &str,
        amount: Decimal,
    ) -> CcxtResult<MarginLoan> {
        let _ = (symbol, code, amount);
        not_supported!("borrowIsolatedMargin")
    }

    // ========================================================================
    // Repaying
    // ========================================================================

    /// Repay margin
    async fn repay_margin(
        &self,
        code: &str,
        amount: Decimal,
        symbol: Option<&str>,
    ) -> CcxtResult<BorrowInterest> {
        let _ = (code, amount, symbol);
        not_supported!("repayMargin")
    }

    /// Repay cross margin
    async fn repay_cross_margin(&self, code: &str, amount: Decimal) -> CcxtResult<MarginLoan> {
        let _ = (code, amount);
        not_supported!("repayCrossMargin")
    }

    /// Repay isolated margin
    async fn repay_isolated_margin(
        &self,
        symbol: &str,
        code: &str,
        amount: Decimal,
    ) -> CcxtResult<MarginLoan> {
        let _ = (symbol, code, amount);
        not_supported!("repayIsolatedMargin")
    }

    // ========================================================================
    // Margin Adjustment
    // ========================================================================

    /// Add margin to a position
    async fn add_margin(&self, symbol: &str, amount: Decimal) -> CcxtResult<MarginModification> {
        let _ = (symbol, amount);
        not_supported!("addMargin")
    }

    /// Reduce margin from a position
    async fn reduce_margin(&self, symbol: &str, amount: Decimal) -> CcxtResult<MarginModification> {
        let _ = (symbol, amount);
        not_supported!("reduceMargin")
    }

    // ========================================================================
    // Borrow Rates
    // ========================================================================

    /// Fetch cross margin borrow rate
    async fn fetch_cross_borrow_rate(&self, code: &str) -> CcxtResult<CrossBorrowRate> {
        let _ = code;
        not_supported!("fetchCrossBorrowRate")
    }

    /// Fetch isolated margin borrow rate
    async fn fetch_isolated_borrow_rate(&self, symbol: &str) -> CcxtResult<IsolatedBorrowRate> {
        let _ = symbol;
        not_supported!("fetchIsolatedBorrowRate")
    }

    /// Fetch borrow interest history
    async fn fetch_borrow_interest(
        &self,
        code: Option<&str>,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<BorrowInterest>> {
        let _ = (code, symbol, since, limit);
        not_supported!("fetchBorrowInterest")
    }
}
