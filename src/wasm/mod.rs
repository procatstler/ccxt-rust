//! WASM bindings for browser usage
//!
//! This module provides JavaScript-friendly bindings for the ccxt-rust library.
//! It exposes a simplified API that can be called from JavaScript/TypeScript.
//!
//! ## Module Structure
//! - `config`: Exchange configuration
//! - `types`: Common data types (Ticker, OrderBook, Market)
//! - `utils`: Utility functions (hmac, sha, base64, etc.)
//! - `exchanges`: Exchange implementations
//! - `websocket`: WebSocket implementations

#![cfg(feature = "wasm")]

mod config;
mod types;
mod utils;
pub mod exchanges;
pub mod websocket;

// Re-export main types
pub use config::WasmExchangeConfig;
pub use types::{WasmTicker, WasmOrderBook, WasmOrderBookEntry, WasmMarket};

// Re-export exchanges
pub use exchanges::{
    WasmBinance,
    WasmUpbit,
    WasmBybit,
    WasmOkx,
    WasmKraken,
};

// Re-export WebSocket exchanges
pub use websocket::{
    WasmBinanceWs,
    WasmUpbitWs,
    WasmBybitWs,
    WasmWsMessage,
    WasmWsHandle,
};

use wasm_bindgen::prelude::*;

/// Initialize panic hook for better error messages in browser console
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// CCXT-Rust WASM version
#[wasm_bindgen]
pub fn version() -> String {
    crate::VERSION.to_string()
}

/// Get list of supported exchanges
#[wasm_bindgen(js_name = getSupportedExchanges)]
pub fn get_supported_exchanges() -> String {
    let exchanges = vec![
        "binance", "upbit", "bybit", "okx", "kraken",
        // Future additions
        "coinbase", "kucoin", "gate", "htx", "bitget",
    ];
    serde_json::to_string(&exchanges).unwrap_or_default()
}

// Re-export utility functions
pub use utils::{
    hmac_sha256,
    hmac_sha512,
    sha256_hash,
    sha512_hash,
    base64_encode,
    base64_decode,
    url_encode,
    generate_uuid,
    get_current_timestamp,
    parse_json,
};
