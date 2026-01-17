//! WASM bindings for browser usage
//!
//! This module provides JavaScript-friendly bindings for the ccxt-rust library.
//! It exposes a simplified API that can be called from JavaScript/TypeScript.

#![cfg(feature = "wasm")]

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

/// Exchange configuration for WASM
#[wasm_bindgen]
pub struct WasmExchangeConfig {
    api_key: Option<String>,
    api_secret: Option<String>,
    password: Option<String>,
    uid: Option<String>,
    sandbox: bool,
    timeout_ms: u64,
}

#[wasm_bindgen]
impl WasmExchangeConfig {
    /// Create new configuration
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            api_key: None,
            api_secret: None,
            password: None,
            uid: None,
            sandbox: false,
            timeout_ms: 30000,
        }
    }

    /// Set API key
    #[wasm_bindgen(js_name = setApiKey)]
    pub fn set_api_key(&mut self, key: String) {
        self.api_key = Some(key);
    }

    /// Set API secret
    #[wasm_bindgen(js_name = setApiSecret)]
    pub fn set_api_secret(&mut self, secret: String) {
        self.api_secret = Some(secret);
    }

    /// Set password (for exchanges that require it)
    #[wasm_bindgen(js_name = setPassword)]
    pub fn set_password(&mut self, password: String) {
        self.password = Some(password);
    }

    /// Set UID (for exchanges that require it)
    #[wasm_bindgen(js_name = setUid)]
    pub fn set_uid(&mut self, uid: String) {
        self.uid = Some(uid);
    }

    /// Enable sandbox mode
    #[wasm_bindgen(js_name = setSandbox)]
    pub fn set_sandbox(&mut self, sandbox: bool) {
        self.sandbox = sandbox;
    }

    /// Set request timeout in milliseconds
    #[wasm_bindgen(js_name = setTimeout)]
    pub fn set_timeout(&mut self, timeout_ms: u64) {
        self.timeout_ms = timeout_ms;
    }

    /// Convert to internal ExchangeConfig
    pub(crate) fn to_internal(&self) -> crate::client::ExchangeConfig {
        let mut config = crate::client::ExchangeConfig::new()
            .with_timeout(self.timeout_ms);

        if let Some(ref key) = self.api_key {
            config = config.with_api_key(key);
        }
        if let Some(ref secret) = self.api_secret {
            config = config.with_api_secret(secret);
        }
        if let Some(ref password) = self.password {
            config = config.with_password(password);
        }
        if let Some(ref uid) = self.uid {
            config = config.with_uid(uid);
        }
        if self.sandbox {
            config = config.with_sandbox(true);
        }

        config
    }
}

impl Default for WasmExchangeConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Ticker data for JavaScript
#[wasm_bindgen]
pub struct WasmTicker {
    symbol: String,
    last: Option<f64>,
    bid: Option<f64>,
    ask: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
    volume: Option<f64>,
    timestamp: Option<i64>,
}

#[wasm_bindgen]
impl WasmTicker {
    #[wasm_bindgen(getter)]
    pub fn symbol(&self) -> String {
        self.symbol.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn last(&self) -> Option<f64> {
        self.last
    }

    #[wasm_bindgen(getter)]
    pub fn bid(&self) -> Option<f64> {
        self.bid
    }

    #[wasm_bindgen(getter)]
    pub fn ask(&self) -> Option<f64> {
        self.ask
    }

    #[wasm_bindgen(getter)]
    pub fn high(&self) -> Option<f64> {
        self.high
    }

    #[wasm_bindgen(getter)]
    pub fn low(&self) -> Option<f64> {
        self.low
    }

    #[wasm_bindgen(getter)]
    pub fn volume(&self) -> Option<f64> {
        self.volume
    }

    #[wasm_bindgen(getter)]
    pub fn timestamp(&self) -> Option<i64> {
        self.timestamp
    }

    /// Convert to JSON string
    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "symbol": self.symbol,
            "last": self.last,
            "bid": self.bid,
            "ask": self.ask,
            "high": self.high,
            "low": self.low,
            "volume": self.volume,
            "timestamp": self.timestamp
        }).to_string()
    }
}

impl From<crate::types::Ticker> for WasmTicker {
    fn from(ticker: crate::types::Ticker) -> Self {
        use rust_decimal::prelude::ToPrimitive;

        Self {
            symbol: ticker.symbol,
            last: ticker.last.and_then(|d| d.to_f64()),
            bid: ticker.bid.and_then(|d| d.to_f64()),
            ask: ticker.ask.and_then(|d| d.to_f64()),
            high: ticker.high.and_then(|d| d.to_f64()),
            low: ticker.low.and_then(|d| d.to_f64()),
            volume: ticker.base_volume.and_then(|d| d.to_f64()),
            timestamp: ticker.timestamp,
        }
    }
}

/// Order book entry for JavaScript
#[wasm_bindgen]
pub struct WasmOrderBookEntry {
    price: f64,
    amount: f64,
}

#[wasm_bindgen]
impl WasmOrderBookEntry {
    #[wasm_bindgen(getter)]
    pub fn price(&self) -> f64 {
        self.price
    }

    #[wasm_bindgen(getter)]
    pub fn amount(&self) -> f64 {
        self.amount
    }
}

/// Order book for JavaScript
#[wasm_bindgen]
pub struct WasmOrderBook {
    symbol: String,
    bids: Vec<WasmOrderBookEntry>,
    asks: Vec<WasmOrderBookEntry>,
    timestamp: Option<i64>,
}

#[wasm_bindgen]
impl WasmOrderBook {
    #[wasm_bindgen(getter)]
    pub fn symbol(&self) -> String {
        self.symbol.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn timestamp(&self) -> Option<i64> {
        self.timestamp
    }

    /// Get bids as JSON array
    #[wasm_bindgen(js_name = getBids)]
    pub fn get_bids(&self) -> String {
        let bids: Vec<[f64; 2]> = self.bids.iter().map(|e| [e.price, e.amount]).collect();
        serde_json::to_string(&bids).unwrap_or_default()
    }

    /// Get asks as JSON array
    #[wasm_bindgen(js_name = getAsks)]
    pub fn get_asks(&self) -> String {
        let asks: Vec<[f64; 2]> = self.asks.iter().map(|e| [e.price, e.amount]).collect();
        serde_json::to_string(&asks).unwrap_or_default()
    }

    /// Convert to JSON string
    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> String {
        let bids: Vec<[f64; 2]> = self.bids.iter().map(|e| [e.price, e.amount]).collect();
        let asks: Vec<[f64; 2]> = self.asks.iter().map(|e| [e.price, e.amount]).collect();

        serde_json::json!({
            "symbol": self.symbol,
            "bids": bids,
            "asks": asks,
            "timestamp": self.timestamp
        }).to_string()
    }
}

impl From<crate::types::OrderBook> for WasmOrderBook {
    fn from(ob: crate::types::OrderBook) -> Self {
        use rust_decimal::prelude::ToPrimitive;

        Self {
            symbol: ob.symbol,
            bids: ob
                .bids
                .into_iter()
                .map(|e| WasmOrderBookEntry {
                    price: e.price.to_f64().unwrap_or_default(),
                    amount: e.amount.to_f64().unwrap_or_default(),
                })
                .collect(),
            asks: ob
                .asks
                .into_iter()
                .map(|e| WasmOrderBookEntry {
                    price: e.price.to_f64().unwrap_or_default(),
                    amount: e.amount.to_f64().unwrap_or_default(),
                })
                .collect(),
            timestamp: ob.timestamp,
        }
    }
}

/// Market info for JavaScript
#[wasm_bindgen]
pub struct WasmMarket {
    id: String,
    symbol: String,
    base: String,
    quote: String,
    active: bool,
}

#[wasm_bindgen]
impl WasmMarket {
    #[wasm_bindgen(getter)]
    pub fn id(&self) -> String {
        self.id.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn symbol(&self) -> String {
        self.symbol.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn base(&self) -> String {
        self.base.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn quote(&self) -> String {
        self.quote.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn active(&self) -> bool {
        self.active
    }
}

impl From<crate::types::Market> for WasmMarket {
    fn from(m: crate::types::Market) -> Self {
        Self {
            id: m.id,
            symbol: m.symbol,
            base: m.base,
            quote: m.quote,
            active: m.active,
        }
    }
}

/// Get list of supported exchanges
#[wasm_bindgen(js_name = getSupportedExchanges)]
pub fn get_supported_exchanges() -> String {
    let exchanges = vec![
        "binance", "binanceus", "binanceusdm", "binancecoinm",
        "okx", "bybit", "coinbase", "kraken", "kucoin",
        "gate", "htx", "bitget", "mexc", "bitfinex",
        "bitstamp", "gemini", "crypto_com", "bithumb",
        "upbit", "korbit", "coinone",
        // DEX
        "hyperliquid", "dydx", "paradex", "apex"
    ];
    serde_json::to_string(&exchanges).unwrap_or_default()
}

/// HMAC-SHA256 signing utility (for API authentication)
#[wasm_bindgen(js_name = hmacSha256)]
pub fn hmac_sha256(secret: &str, message: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// HMAC-SHA512 signing utility (for API authentication)
#[wasm_bindgen(js_name = hmacSha512)]
pub fn hmac_sha512(secret: &str, message: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha512;

    type HmacSha512 = Hmac<Sha512>;

    let mut mac = HmacSha512::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// SHA256 hash utility
#[wasm_bindgen(js_name = sha256)]
pub fn sha256_hash(data: &str) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

/// SHA512 hash utility
#[wasm_bindgen(js_name = sha512)]
pub fn sha512_hash(data: &str) -> String {
    use sha2::{Sha512, Digest};
    let mut hasher = Sha512::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

/// Base64 encode
#[wasm_bindgen(js_name = base64Encode)]
pub fn base64_encode(data: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data.as_bytes())
}

/// Base64 decode
#[wasm_bindgen(js_name = base64Decode)]
pub fn base64_decode(data: &str) -> Result<String, JsValue> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| JsValue::from_str(&e.to_string()))
        .and_then(|bytes| {
            String::from_utf8(bytes)
                .map_err(|e| JsValue::from_str(&e.to_string()))
        })
}

/// URL encode
#[wasm_bindgen(js_name = urlEncode)]
pub fn url_encode(data: &str) -> String {
    urlencoding::encode(data).to_string()
}

/// Generate UUID v4
#[wasm_bindgen(js_name = generateUuid)]
pub fn generate_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Get current timestamp in milliseconds
#[wasm_bindgen(js_name = getCurrentTimestamp)]
pub fn get_current_timestamp() -> i64 {
    js_sys::Date::now() as i64
}

/// Parse JSON safely
#[wasm_bindgen(js_name = parseJson)]
pub fn parse_json(json_str: &str) -> Result<JsValue, JsValue> {
    serde_json::from_str::<serde_json::Value>(json_str)
        .map(|v| JsValue::from_str(&v.to_string()))
        .map_err(|e| JsValue::from_str(&e.to_string()))
}
