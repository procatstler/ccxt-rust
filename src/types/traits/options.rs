//! Options Trading API Trait
//!
//! Options contract trading operations.

use async_trait::async_trait;

use crate::errors::CcxtResult;
use crate::not_supported;
use crate::types::{Greeks, OptionChain, OptionContract};

/// Options Trading API
///
/// Operations for options trading:
/// - Option contract information
/// - Option chains
/// - Greeks (delta, gamma, theta, vega, rho)
/// - Underlying assets
/// - Settlement and volatility history
#[async_trait]
pub trait OptionsApi: Send + Sync {
    // ========================================================================
    // Option Information
    // ========================================================================

    /// Fetch option contract information
    async fn fetch_option(&self, symbol: &str) -> CcxtResult<OptionContract> {
        let _ = symbol;
        not_supported!("fetchOption")
    }

    /// Fetch option chain for an underlying asset
    async fn fetch_option_chain(&self, underlying: &str) -> CcxtResult<OptionChain> {
        let _ = underlying;
        not_supported!("fetchOptionChain")
    }

    // ========================================================================
    // Greeks
    // ========================================================================

    /// Fetch Greeks for an option contract
    async fn fetch_greeks(&self, symbol: &str) -> CcxtResult<Greeks> {
        let _ = symbol;
        not_supported!("fetchGreeks")
    }

    // ========================================================================
    // Underlying Assets
    // ========================================================================

    /// Fetch available underlying assets for options
    async fn fetch_underlying_assets(&self) -> CcxtResult<Vec<String>> {
        not_supported!("fetchUnderlyingAssets")
    }

    // ========================================================================
    // Settlement & Volatility
    // ========================================================================

    /// Fetch settlement history for options
    async fn fetch_settlement_history(
        &self,
        underlying: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Settlement>> {
        let _ = (underlying, since, limit);
        not_supported!("fetchSettlementHistory")
    }

    /// Fetch volatility history for an underlying asset
    async fn fetch_volatility_history(
        &self,
        underlying: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::VolatilityHistory>> {
        let _ = (underlying, since, limit);
        not_supported!("fetchVolatilityHistory")
    }
}
