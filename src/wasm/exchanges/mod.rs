//! WASM Exchange Implementations
//!
//! This module contains exchange-specific implementations for WASM.

mod binance;
mod upbit;
mod bybit;
mod okx;
mod kraken;

pub use binance::WasmBinance;
pub use upbit::WasmUpbit;
pub use bybit::WasmBybit;
pub use okx::WasmOkx;
pub use kraken::WasmKraken;

/// Common parsing utilities for exchanges
pub(crate) mod parsing {
    /// Parse a string to f64, returning None on failure
    pub fn parse_f64(s: &Option<String>) -> Option<f64> {
        s.as_ref().and_then(|v| v.parse().ok())
    }
}
