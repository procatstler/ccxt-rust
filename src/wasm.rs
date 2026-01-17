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

// ============================================================================
// Exchange Implementations
// ============================================================================

use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::HashMap;
use crate::client::HttpClient;

/// Helper to convert JsValue error
fn js_err(msg: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&msg.to_string())
}

// ----------------------------------------------------------------------------
// Binance
// ----------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceTicker {
    symbol: String,
    last_price: Option<String>,
    bid_price: Option<String>,
    ask_price: Option<String>,
    high_price: Option<String>,
    low_price: Option<String>,
    volume: Option<String>,
    close_time: Option<i64>,
}

#[derive(Deserialize)]
struct BinanceOrderBook {
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceMarket {
    symbol: String,
    base_asset: String,
    quote_asset: String,
    status: String,
}

#[derive(Deserialize)]
struct BinanceExchangeInfo {
    symbols: Vec<BinanceMarket>,
}

/// Binance exchange for WASM
#[wasm_bindgen]
pub struct WasmBinance {
    client: HttpClient,
}

#[wasm_bindgen]
impl WasmBinance {
    /// Create new Binance instance
    #[wasm_bindgen(constructor)]
    pub fn new(config: Option<WasmExchangeConfig>) -> Result<WasmBinance, JsValue> {
        let internal_config = config
            .map(|c| c.to_internal())
            .unwrap_or_else(crate::client::ExchangeConfig::new);

        let client = HttpClient::new("https://api.binance.com", &internal_config)
            .map_err(js_err)?;

        Ok(Self { client })
    }

    /// Fetch ticker for a symbol
    #[wasm_bindgen(js_name = fetchTicker)]
    pub async fn fetch_ticker(&self, symbol: &str) -> Result<WasmTicker, JsValue> {
        let market_id = symbol.replace("/", "");
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        let response: BinanceTicker = self.client
            .get("/api/v3/ticker/24hr", Some(params), None)
            .await
            .map_err(js_err)?;

        Ok(WasmTicker {
            symbol: symbol.to_string(),
            last: response.last_price.and_then(|s| s.parse().ok()),
            bid: response.bid_price.and_then(|s| s.parse().ok()),
            ask: response.ask_price.and_then(|s| s.parse().ok()),
            high: response.high_price.and_then(|s| s.parse().ok()),
            low: response.low_price.and_then(|s| s.parse().ok()),
            volume: response.volume.and_then(|s| s.parse().ok()),
            timestamp: response.close_time,
        })
    }

    /// Fetch order book for a symbol
    #[wasm_bindgen(js_name = fetchOrderBook)]
    pub async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> Result<WasmOrderBook, JsValue> {
        let market_id = symbol.replace("/", "");
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: BinanceOrderBook = self.client
            .get("/api/v3/depth", Some(params), None)
            .await
            .map_err(js_err)?;

        let bids: Vec<WasmOrderBookEntry> = response.bids
            .iter()
            .map(|b| WasmOrderBookEntry {
                price: b[0].parse().unwrap_or_default(),
                amount: b[1].parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<WasmOrderBookEntry> = response.asks
            .iter()
            .map(|a| WasmOrderBookEntry {
                price: a[0].parse().unwrap_or_default(),
                amount: a[1].parse().unwrap_or_default(),
            })
            .collect();

        Ok(WasmOrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp: Some(js_sys::Date::now() as i64),
        })
    }

    /// Fetch all markets
    #[wasm_bindgen(js_name = fetchMarkets)]
    pub async fn fetch_markets(&self) -> Result<JsValue, JsValue> {
        let response: BinanceExchangeInfo = self.client
            .get("/api/v3/exchangeInfo", None, None)
            .await
            .map_err(js_err)?;

        let markets: Vec<serde_json::Value> = response.symbols
            .iter()
            .filter(|m| m.status == "TRADING")
            .map(|m| {
                let symbol = format!("{}/{}", m.base_asset, m.quote_asset);
                serde_json::json!({
                    "id": m.symbol,
                    "symbol": symbol,
                    "base": m.base_asset,
                    "quote": m.quote_asset,
                    "active": true
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&markets).unwrap_or_default()))
    }

    /// Fetch tickers for multiple symbols (or all if none specified)
    #[wasm_bindgen(js_name = fetchTickers)]
    pub async fn fetch_tickers(&self, symbols: Option<Vec<String>>) -> Result<JsValue, JsValue> {
        let response: Vec<BinanceTicker> = self.client
            .get("/api/v3/ticker/24hr", None, None)
            .await
            .map_err(js_err)?;

        let tickers: Vec<serde_json::Value> = response
            .iter()
            .filter(|t| {
                if let Some(ref syms) = symbols {
                    let unified = t.symbol.clone(); // Already in BTCUSDT format
                    syms.iter().any(|s| s.replace("/", "") == unified)
                } else {
                    true
                }
            })
            .map(|t| {
                serde_json::json!({
                    "symbol": t.symbol,
                    "last": t.last_price.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "bid": t.bid_price.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "ask": t.ask_price.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "high": t.high_price.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "low": t.low_price.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "volume": t.volume.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "timestamp": t.close_time
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&tickers).unwrap_or_default()))
    }

    /// Fetch recent trades for a symbol
    #[wasm_bindgen(js_name = fetchTrades)]
    pub async fn fetch_trades(&self, symbol: &str, limit: Option<u32>) -> Result<JsValue, JsValue> {
        let market_id = symbol.replace("/", "");
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(1000).to_string());
        }

        let response: Vec<BinanceTrade> = self.client
            .get("/api/v3/trades", Some(params), None)
            .await
            .map_err(js_err)?;

        let trades: Vec<serde_json::Value> = response
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id.to_string(),
                    "symbol": symbol,
                    "price": t.price.parse::<f64>().unwrap_or_default(),
                    "amount": t.qty.parse::<f64>().unwrap_or_default(),
                    "side": if t.is_buyer_maker { "sell" } else { "buy" },
                    "timestamp": t.time
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&trades).unwrap_or_default()))
    }

    /// Fetch OHLCV candles for a symbol
    #[wasm_bindgen(js_name = fetchOhlcv)]
    pub async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> Result<JsValue, JsValue> {
        let market_id = symbol.replace("/", "");
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("interval".to_string(), timeframe.to_string());
        if let Some(s) = since {
            params.insert("startTime".to_string(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(1000).to_string());
        }

        let response: Vec<Vec<serde_json::Value>> = self.client
            .get("/api/v3/klines", Some(params), None)
            .await
            .map_err(js_err)?;

        let candles: Vec<serde_json::Value> = response
            .iter()
            .map(|k| {
                serde_json::json!({
                    "timestamp": k.get(0).and_then(|v| v.as_i64()).unwrap_or_default(),
                    "open": k.get(1).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                    "high": k.get(2).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                    "low": k.get(3).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                    "close": k.get(4).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                    "volume": k.get(5).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default()
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&candles).unwrap_or_default()))
    }
}

/// Binance trade response
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceTrade {
    id: u64,
    price: String,
    qty: String,
    time: i64,
    is_buyer_maker: bool,
}

// ----------------------------------------------------------------------------
// Upbit (Korean Exchange)
// ----------------------------------------------------------------------------

#[derive(Deserialize)]
struct UpbitTicker {
    market: String,
    trade_price: Option<f64>,
    opening_price: Option<f64>,
    high_price: Option<f64>,
    low_price: Option<f64>,
    acc_trade_volume_24h: Option<f64>,
    timestamp: Option<i64>,
}

#[derive(Deserialize)]
struct UpbitOrderBookUnit {
    ask_price: f64,
    bid_price: f64,
    ask_size: f64,
    bid_size: f64,
}

#[derive(Deserialize)]
struct UpbitOrderBook {
    market: String,
    orderbook_units: Vec<UpbitOrderBookUnit>,
    timestamp: Option<i64>,
}

#[derive(Deserialize)]
struct UpbitMarket {
    market: String,
    korean_name: String,
    english_name: String,
}

/// Upbit exchange for WASM
#[wasm_bindgen]
pub struct WasmUpbit {
    client: HttpClient,
}

#[wasm_bindgen]
impl WasmUpbit {
    /// Create new Upbit instance
    #[wasm_bindgen(constructor)]
    pub fn new(config: Option<WasmExchangeConfig>) -> Result<WasmUpbit, JsValue> {
        let internal_config = config
            .map(|c| c.to_internal())
            .unwrap_or_else(crate::client::ExchangeConfig::new);

        let client = HttpClient::new("https://api.upbit.com", &internal_config)
            .map_err(js_err)?;

        Ok(Self { client })
    }

    /// Convert symbol to Upbit market ID (BTC/KRW -> KRW-BTC)
    fn to_market_id(symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            format!("{}-{}", parts[1], parts[0])
        } else {
            symbol.to_string()
        }
    }

    /// Fetch ticker for a symbol
    #[wasm_bindgen(js_name = fetchTicker)]
    pub async fn fetch_ticker(&self, symbol: &str) -> Result<WasmTicker, JsValue> {
        let market_id = Self::to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("markets".to_string(), market_id);

        let response: Vec<UpbitTicker> = self.client
            .get("/v1/ticker", Some(params), None)
            .await
            .map_err(js_err)?;

        let ticker = response.first().ok_or_else(|| js_err("No ticker data"))?;

        Ok(WasmTicker {
            symbol: symbol.to_string(),
            last: ticker.trade_price,
            bid: None, // Upbit ticker doesn't include bid/ask
            ask: None,
            high: ticker.high_price,
            low: ticker.low_price,
            volume: ticker.acc_trade_volume_24h,
            timestamp: ticker.timestamp,
        })
    }

    /// Fetch order book for a symbol
    #[wasm_bindgen(js_name = fetchOrderBook)]
    pub async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> Result<WasmOrderBook, JsValue> {
        let market_id = Self::to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("markets".to_string(), market_id);

        let response: Vec<UpbitOrderBook> = self.client
            .get("/v1/orderbook", Some(params), None)
            .await
            .map_err(js_err)?;

        let orderbook = response.first().ok_or_else(|| js_err("No orderbook data"))?;

        let bids: Vec<WasmOrderBookEntry> = orderbook.orderbook_units
            .iter()
            .map(|u| WasmOrderBookEntry {
                price: u.bid_price,
                amount: u.bid_size,
            })
            .collect();

        let asks: Vec<WasmOrderBookEntry> = orderbook.orderbook_units
            .iter()
            .map(|u| WasmOrderBookEntry {
                price: u.ask_price,
                amount: u.ask_size,
            })
            .collect();

        Ok(WasmOrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp: orderbook.timestamp,
        })
    }

    /// Fetch all markets
    #[wasm_bindgen(js_name = fetchMarkets)]
    pub async fn fetch_markets(&self) -> Result<JsValue, JsValue> {
        let response: Vec<UpbitMarket> = self.client
            .get("/v1/market/all", None, None)
            .await
            .map_err(js_err)?;

        let markets: Vec<serde_json::Value> = response
            .iter()
            .map(|m| {
                let parts: Vec<&str> = m.market.split('-').collect();
                let (quote, base) = if parts.len() == 2 {
                    (parts[0], parts[1])
                } else {
                    ("", &m.market[..])
                };
                let symbol = format!("{}/{}", base, quote);

                serde_json::json!({
                    "id": m.market,
                    "symbol": symbol,
                    "base": base,
                    "quote": quote,
                    "active": true,
                    "name": m.english_name
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&markets).unwrap_or_default()))
    }

    /// Fetch tickers for multiple symbols
    #[wasm_bindgen(js_name = fetchTickers)]
    pub async fn fetch_tickers(&self, symbols: Option<Vec<String>>) -> Result<JsValue, JsValue> {
        let markets_param = if let Some(ref syms) = symbols {
            syms.iter()
                .map(|s| Self::to_market_id(s))
                .collect::<Vec<_>>()
                .join(",")
        } else {
            // Upbit requires market codes, fetch all first
            let markets: Vec<UpbitMarket> = self.client
                .get("/v1/market/all", None, None)
                .await
                .map_err(js_err)?;
            markets.iter().map(|m| m.market.clone()).collect::<Vec<_>>().join(",")
        };

        let mut params = HashMap::new();
        params.insert("markets".to_string(), markets_param);

        let response: Vec<UpbitTicker> = self.client
            .get("/v1/ticker", Some(params), None)
            .await
            .map_err(js_err)?;

        let tickers: Vec<serde_json::Value> = response
            .iter()
            .map(|t| {
                let parts: Vec<&str> = t.market.split('-').collect();
                let symbol = if parts.len() == 2 {
                    format!("{}/{}", parts[1], parts[0])
                } else {
                    t.market.clone()
                };
                serde_json::json!({
                    "symbol": symbol,
                    "last": t.trade_price,
                    "high": t.high_price,
                    "low": t.low_price,
                    "volume": t.acc_trade_volume_24h,
                    "timestamp": t.timestamp
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&tickers).unwrap_or_default()))
    }

    /// Fetch recent trades for a symbol
    #[wasm_bindgen(js_name = fetchTrades)]
    pub async fn fetch_trades(&self, symbol: &str, limit: Option<u32>) -> Result<JsValue, JsValue> {
        let market_id = Self::to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("count".to_string(), l.min(200).to_string());
        }

        let response: Vec<UpbitTrade> = self.client
            .get("/v1/trades/ticks", Some(params), None)
            .await
            .map_err(js_err)?;

        let trades: Vec<serde_json::Value> = response
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.sequential_id.to_string(),
                    "symbol": symbol,
                    "price": t.trade_price,
                    "amount": t.trade_volume,
                    "side": &t.ask_bid,
                    "timestamp": t.timestamp
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&trades).unwrap_or_default()))
    }

    /// Fetch OHLCV candles for a symbol
    #[wasm_bindgen(js_name = fetchOhlcv)]
    pub async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> Result<JsValue, JsValue> {
        let market_id = Self::to_market_id(symbol);

        // Map timeframe to Upbit format
        let endpoint = match timeframe {
            "1m" => "/v1/candles/minutes/1",
            "3m" => "/v1/candles/minutes/3",
            "5m" => "/v1/candles/minutes/5",
            "15m" => "/v1/candles/minutes/15",
            "30m" => "/v1/candles/minutes/30",
            "1h" => "/v1/candles/minutes/60",
            "4h" => "/v1/candles/minutes/240",
            "1d" => "/v1/candles/days",
            "1w" => "/v1/candles/weeks",
            "1M" => "/v1/candles/months",
            _ => "/v1/candles/minutes/1",
        };

        let mut params = HashMap::new();
        params.insert("market".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("count".to_string(), l.min(200).to_string());
        }

        let response: Vec<UpbitCandle> = self.client
            .get(endpoint, Some(params), None)
            .await
            .map_err(js_err)?;

        let candles: Vec<serde_json::Value> = response
            .iter()
            .map(|c| {
                serde_json::json!({
                    "timestamp": c.timestamp,
                    "open": c.opening_price,
                    "high": c.high_price,
                    "low": c.low_price,
                    "close": c.trade_price,
                    "volume": c.candle_acc_trade_volume
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&candles).unwrap_or_default()))
    }
}

/// Upbit trade response
#[derive(Deserialize)]
struct UpbitTrade {
    sequential_id: u64,
    trade_price: f64,
    trade_volume: f64,
    ask_bid: String,
    timestamp: i64,
}

/// Upbit candle response
#[derive(Deserialize)]
struct UpbitCandle {
    timestamp: i64,
    opening_price: f64,
    high_price: f64,
    low_price: f64,
    trade_price: f64,
    candle_acc_trade_volume: f64,
}

// ----------------------------------------------------------------------------
// Bybit
// ----------------------------------------------------------------------------

#[derive(Deserialize)]
struct BybitResponse<T> {
    result: T,
}

#[derive(Deserialize)]
struct BybitTickerResult {
    list: Vec<BybitTicker>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BybitTicker {
    symbol: String,
    last_price: Option<String>,
    bid1_price: Option<String>,
    ask1_price: Option<String>,
    high_price24h: Option<String>,
    low_price24h: Option<String>,
    volume24h: Option<String>,
}

#[derive(Deserialize)]
struct BybitOrderBookResult {
    b: Vec<[String; 2]>,  // bids
    a: Vec<[String; 2]>,  // asks
    ts: Option<i64>,
}

/// Bybit exchange for WASM
#[wasm_bindgen]
pub struct WasmBybit {
    client: HttpClient,
}

#[wasm_bindgen]
impl WasmBybit {
    /// Create new Bybit instance
    #[wasm_bindgen(constructor)]
    pub fn new(config: Option<WasmExchangeConfig>) -> Result<WasmBybit, JsValue> {
        let internal_config = config
            .map(|c| c.to_internal())
            .unwrap_or_else(crate::client::ExchangeConfig::new);

        let client = HttpClient::new("https://api.bybit.com", &internal_config)
            .map_err(js_err)?;

        Ok(Self { client })
    }

    /// Fetch ticker for a symbol
    #[wasm_bindgen(js_name = fetchTicker)]
    pub async fn fetch_ticker(&self, symbol: &str) -> Result<WasmTicker, JsValue> {
        let market_id = symbol.replace("/", "");
        let mut params = HashMap::new();
        params.insert("category".to_string(), "spot".to_string());
        params.insert("symbol".to_string(), market_id);

        let response: BybitResponse<BybitTickerResult> = self.client
            .get("/v5/market/tickers", Some(params), None)
            .await
            .map_err(js_err)?;

        let ticker = response.result.list.first()
            .ok_or_else(|| js_err("No ticker data"))?;

        Ok(WasmTicker {
            symbol: symbol.to_string(),
            last: ticker.last_price.as_ref().and_then(|s| s.parse().ok()),
            bid: ticker.bid1_price.as_ref().and_then(|s| s.parse().ok()),
            ask: ticker.ask1_price.as_ref().and_then(|s| s.parse().ok()),
            high: ticker.high_price24h.as_ref().and_then(|s| s.parse().ok()),
            low: ticker.low_price24h.as_ref().and_then(|s| s.parse().ok()),
            volume: ticker.volume24h.as_ref().and_then(|s| s.parse().ok()),
            timestamp: Some(js_sys::Date::now() as i64),
        })
    }

    /// Fetch order book for a symbol
    #[wasm_bindgen(js_name = fetchOrderBook)]
    pub async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> Result<WasmOrderBook, JsValue> {
        let market_id = symbol.replace("/", "");
        let mut params = HashMap::new();
        params.insert("category".to_string(), "spot".to_string());
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: BybitResponse<BybitOrderBookResult> = self.client
            .get("/v5/market/orderbook", Some(params), None)
            .await
            .map_err(js_err)?;

        let bids: Vec<WasmOrderBookEntry> = response.result.b
            .iter()
            .map(|b| WasmOrderBookEntry {
                price: b[0].parse().unwrap_or_default(),
                amount: b[1].parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<WasmOrderBookEntry> = response.result.a
            .iter()
            .map(|a| WasmOrderBookEntry {
                price: a[0].parse().unwrap_or_default(),
                amount: a[1].parse().unwrap_or_default(),
            })
            .collect();

        Ok(WasmOrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp: response.result.ts,
        })
    }

    /// Fetch all markets
    #[wasm_bindgen(js_name = fetchMarkets)]
    pub async fn fetch_markets(&self) -> Result<JsValue, JsValue> {
        let mut params = HashMap::new();
        params.insert("category".to_string(), "spot".to_string());

        let response: BybitResponse<BybitInstrumentsResult> = self.client
            .get("/v5/market/instruments-info", Some(params), None)
            .await
            .map_err(js_err)?;

        let markets: Vec<serde_json::Value> = response.result.list
            .iter()
            .filter(|m| m.status == "Trading")
            .map(|m| {
                let symbol = format!("{}/{}", m.base_coin, m.quote_coin);
                serde_json::json!({
                    "id": m.symbol,
                    "symbol": symbol,
                    "base": m.base_coin,
                    "quote": m.quote_coin,
                    "active": true
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&markets).unwrap_or_default()))
    }

    /// Fetch tickers for multiple symbols
    #[wasm_bindgen(js_name = fetchTickers)]
    pub async fn fetch_tickers(&self, _symbols: Option<Vec<String>>) -> Result<JsValue, JsValue> {
        let mut params = HashMap::new();
        params.insert("category".to_string(), "spot".to_string());

        let response: BybitResponse<BybitTickerResult> = self.client
            .get("/v5/market/tickers", Some(params), None)
            .await
            .map_err(js_err)?;

        let tickers: Vec<serde_json::Value> = response.result.list
            .iter()
            .map(|t| {
                serde_json::json!({
                    "symbol": t.symbol,
                    "last": t.last_price.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "bid": t.bid1_price.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "ask": t.ask1_price.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "high": t.high_price24h.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "low": t.low_price24h.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "volume": t.volume24h.as_ref().and_then(|s| s.parse::<f64>().ok())
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&tickers).unwrap_or_default()))
    }

    /// Fetch recent trades for a symbol
    #[wasm_bindgen(js_name = fetchTrades)]
    pub async fn fetch_trades(&self, symbol: &str, limit: Option<u32>) -> Result<JsValue, JsValue> {
        let market_id = symbol.replace("/", "");
        let mut params = HashMap::new();
        params.insert("category".to_string(), "spot".to_string());
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(1000).to_string());
        }

        let response: BybitResponse<BybitTradesResult> = self.client
            .get("/v5/market/recent-trade", Some(params), None)
            .await
            .map_err(js_err)?;

        let trades: Vec<serde_json::Value> = response.result.list
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.exec_id,
                    "symbol": symbol,
                    "price": t.price.parse::<f64>().unwrap_or_default(),
                    "amount": t.size.parse::<f64>().unwrap_or_default(),
                    "side": t.side.to_lowercase(),
                    "timestamp": t.time.parse::<i64>().unwrap_or_default()
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&trades).unwrap_or_default()))
    }

    /// Fetch OHLCV candles for a symbol
    #[wasm_bindgen(js_name = fetchOhlcv)]
    pub async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> Result<JsValue, JsValue> {
        let market_id = symbol.replace("/", "");

        // Map timeframe to Bybit interval
        let interval = match timeframe {
            "1m" => "1",
            "3m" => "3",
            "5m" => "5",
            "15m" => "15",
            "30m" => "30",
            "1h" => "60",
            "2h" => "120",
            "4h" => "240",
            "6h" => "360",
            "12h" => "720",
            "1d" => "D",
            "1w" => "W",
            "1M" => "M",
            _ => "1",
        };

        let mut params = HashMap::new();
        params.insert("category".to_string(), "spot".to_string());
        params.insert("symbol".to_string(), market_id);
        params.insert("interval".to_string(), interval.to_string());
        if let Some(s) = since {
            params.insert("start".to_string(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(1000).to_string());
        }

        let response: BybitResponse<BybitKlineResult> = self.client
            .get("/v5/market/kline", Some(params), None)
            .await
            .map_err(js_err)?;

        let candles: Vec<serde_json::Value> = response.result.list
            .iter()
            .map(|k| {
                serde_json::json!({
                    "timestamp": k.get(0).and_then(|v| v.as_str()).and_then(|s| s.parse::<i64>().ok()).unwrap_or_default(),
                    "open": k.get(1).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                    "high": k.get(2).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                    "low": k.get(3).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                    "close": k.get(4).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                    "volume": k.get(5).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default()
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&candles).unwrap_or_default()))
    }
}

/// Bybit instruments response
#[derive(Deserialize)]
struct BybitInstrumentsResult {
    list: Vec<BybitInstrument>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BybitInstrument {
    symbol: String,
    base_coin: String,
    quote_coin: String,
    status: String,
}

/// Bybit trades response
#[derive(Deserialize)]
struct BybitTradesResult {
    list: Vec<BybitTradeItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BybitTradeItem {
    exec_id: String,
    price: String,
    size: String,
    side: String,
    time: String,
}

/// Bybit kline response
#[derive(Deserialize)]
struct BybitKlineResult {
    list: Vec<Vec<serde_json::Value>>,
}

// ----------------------------------------------------------------------------
// OKX
// ----------------------------------------------------------------------------

#[derive(Deserialize)]
struct OkxResponse<T> {
    data: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxTicker {
    inst_id: String,
    last: Option<String>,
    bid_px: Option<String>,
    ask_px: Option<String>,
    high24h: Option<String>,
    low24h: Option<String>,
    vol24h: Option<String>,
    ts: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxOrderBook {
    asks: Vec<[String; 4]>,
    bids: Vec<[String; 4]>,
    ts: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxInstrument {
    inst_id: String,
    base_ccy: String,
    quote_ccy: String,
    state: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxTrade {
    trade_id: String,
    px: String,
    sz: String,
    side: String,
    ts: String,
}

#[derive(Deserialize)]
struct OkxCandle(String, String, String, String, String, String, String, String, String);

/// OKX exchange for WASM
#[wasm_bindgen]
pub struct WasmOkx {
    client: HttpClient,
}

#[wasm_bindgen]
impl WasmOkx {
    /// Create new OKX instance
    #[wasm_bindgen(constructor)]
    pub fn new(config: Option<WasmExchangeConfig>) -> Result<WasmOkx, JsValue> {
        let internal_config = config
            .map(|c| c.to_internal())
            .unwrap_or_else(crate::client::ExchangeConfig::new);

        let client = HttpClient::new("https://www.okx.com", &internal_config)
            .map_err(js_err)?;

        Ok(Self { client })
    }

    fn to_inst_id(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Fetch ticker for a symbol
    #[wasm_bindgen(js_name = fetchTicker)]
    pub async fn fetch_ticker(&self, symbol: &str) -> Result<WasmTicker, JsValue> {
        let inst_id = Self::to_inst_id(symbol);
        let mut params = HashMap::new();
        params.insert("instId".to_string(), inst_id);

        let response: OkxResponse<Vec<OkxTicker>> = self.client
            .get("/api/v5/market/ticker", Some(params), None)
            .await
            .map_err(js_err)?;

        let ticker = response.data.first().ok_or_else(|| js_err("No ticker data"))?;

        Ok(WasmTicker {
            symbol: symbol.to_string(),
            last: ticker.last.as_ref().and_then(|s| s.parse().ok()),
            bid: ticker.bid_px.as_ref().and_then(|s| s.parse().ok()),
            ask: ticker.ask_px.as_ref().and_then(|s| s.parse().ok()),
            high: ticker.high24h.as_ref().and_then(|s| s.parse().ok()),
            low: ticker.low24h.as_ref().and_then(|s| s.parse().ok()),
            volume: ticker.vol24h.as_ref().and_then(|s| s.parse().ok()),
            timestamp: ticker.ts.as_ref().and_then(|s| s.parse().ok()),
        })
    }

    /// Fetch order book for a symbol
    #[wasm_bindgen(js_name = fetchOrderBook)]
    pub async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> Result<WasmOrderBook, JsValue> {
        let inst_id = Self::to_inst_id(symbol);
        let mut params = HashMap::new();
        params.insert("instId".to_string(), inst_id);
        if let Some(l) = limit {
            params.insert("sz".to_string(), l.min(400).to_string());
        }

        let response: OkxResponse<Vec<OkxOrderBook>> = self.client
            .get("/api/v5/market/books", Some(params), None)
            .await
            .map_err(js_err)?;

        let ob = response.data.first().ok_or_else(|| js_err("No orderbook data"))?;

        let bids: Vec<WasmOrderBookEntry> = ob.bids
            .iter()
            .map(|b| WasmOrderBookEntry {
                price: b[0].parse().unwrap_or_default(),
                amount: b[1].parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<WasmOrderBookEntry> = ob.asks
            .iter()
            .map(|a| WasmOrderBookEntry {
                price: a[0].parse().unwrap_or_default(),
                amount: a[1].parse().unwrap_or_default(),
            })
            .collect();

        Ok(WasmOrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp: ob.ts.as_ref().and_then(|s| s.parse().ok()),
        })
    }

    /// Fetch all markets
    #[wasm_bindgen(js_name = fetchMarkets)]
    pub async fn fetch_markets(&self) -> Result<JsValue, JsValue> {
        let mut params = HashMap::new();
        params.insert("instType".to_string(), "SPOT".to_string());

        let response: OkxResponse<Vec<OkxInstrument>> = self.client
            .get("/api/v5/public/instruments", Some(params), None)
            .await
            .map_err(js_err)?;

        let markets: Vec<serde_json::Value> = response.data
            .iter()
            .filter(|m| m.state == "live")
            .map(|m| {
                let symbol = format!("{}/{}", m.base_ccy, m.quote_ccy);
                serde_json::json!({
                    "id": m.inst_id,
                    "symbol": symbol,
                    "base": m.base_ccy,
                    "quote": m.quote_ccy,
                    "active": true
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&markets).unwrap_or_default()))
    }

    /// Fetch tickers for multiple symbols
    #[wasm_bindgen(js_name = fetchTickers)]
    pub async fn fetch_tickers(&self, _symbols: Option<Vec<String>>) -> Result<JsValue, JsValue> {
        let mut params = HashMap::new();
        params.insert("instType".to_string(), "SPOT".to_string());

        let response: OkxResponse<Vec<OkxTicker>> = self.client
            .get("/api/v5/market/tickers", Some(params), None)
            .await
            .map_err(js_err)?;

        let tickers: Vec<serde_json::Value> = response.data
            .iter()
            .map(|t| {
                let symbol = t.inst_id.replace("-", "/");
                serde_json::json!({
                    "symbol": symbol,
                    "last": t.last.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "bid": t.bid_px.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "ask": t.ask_px.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "high": t.high24h.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "low": t.low24h.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "volume": t.vol24h.as_ref().and_then(|s| s.parse::<f64>().ok()),
                    "timestamp": t.ts.as_ref().and_then(|s| s.parse::<i64>().ok())
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&tickers).unwrap_or_default()))
    }

    /// Fetch recent trades for a symbol
    #[wasm_bindgen(js_name = fetchTrades)]
    pub async fn fetch_trades(&self, symbol: &str, limit: Option<u32>) -> Result<JsValue, JsValue> {
        let inst_id = Self::to_inst_id(symbol);
        let mut params = HashMap::new();
        params.insert("instId".to_string(), inst_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(100).to_string());
        }

        let response: OkxResponse<Vec<OkxTrade>> = self.client
            .get("/api/v5/market/trades", Some(params), None)
            .await
            .map_err(js_err)?;

        let trades: Vec<serde_json::Value> = response.data
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.trade_id,
                    "symbol": symbol,
                    "price": t.px.parse::<f64>().unwrap_or_default(),
                    "amount": t.sz.parse::<f64>().unwrap_or_default(),
                    "side": t.side.to_lowercase(),
                    "timestamp": t.ts.parse::<i64>().unwrap_or_default()
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&trades).unwrap_or_default()))
    }

    /// Fetch OHLCV candles for a symbol
    #[wasm_bindgen(js_name = fetchOhlcv)]
    pub async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> Result<JsValue, JsValue> {
        let inst_id = Self::to_inst_id(symbol);

        // Map timeframe to OKX bar format
        let bar = match timeframe {
            "1m" => "1m",
            "3m" => "3m",
            "5m" => "5m",
            "15m" => "15m",
            "30m" => "30m",
            "1h" => "1H",
            "2h" => "2H",
            "4h" => "4H",
            "6h" => "6H",
            "12h" => "12H",
            "1d" => "1D",
            "1w" => "1W",
            "1M" => "1M",
            _ => "1m",
        };

        let mut params = HashMap::new();
        params.insert("instId".to_string(), inst_id);
        params.insert("bar".to_string(), bar.to_string());
        if let Some(s) = since {
            params.insert("after".to_string(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(300).to_string());
        }

        let response: OkxResponse<Vec<Vec<String>>> = self.client
            .get("/api/v5/market/candles", Some(params), None)
            .await
            .map_err(js_err)?;

        let candles: Vec<serde_json::Value> = response.data
            .iter()
            .map(|c| {
                serde_json::json!({
                    "timestamp": c.get(0).and_then(|s| s.parse::<i64>().ok()).unwrap_or_default(),
                    "open": c.get(1).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                    "high": c.get(2).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                    "low": c.get(3).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                    "close": c.get(4).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                    "volume": c.get(5).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default()
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&candles).unwrap_or_default()))
    }
}

// ----------------------------------------------------------------------------
// Kraken
// ----------------------------------------------------------------------------

#[derive(Deserialize)]
struct KrakenResponse<T> {
    result: T,
}

#[derive(Deserialize)]
struct KrakenTicker {
    c: [String; 2],  // close [price, lot volume]
    b: [String; 3],  // bid [price, whole lot volume, lot volume]
    a: [String; 3],  // ask [price, whole lot volume, lot volume]
    h: [String; 2],  // high [today, last 24h]
    l: [String; 2],  // low [today, last 24h]
    v: [String; 2],  // volume [today, last 24h]
}

#[derive(Deserialize)]
struct KrakenOrderBook {
    asks: Vec<[String; 3]>,
    bids: Vec<[String; 3]>,
}

#[derive(Deserialize)]
struct KrakenAssetPair {
    altname: String,
    base: String,
    quote: String,
    status: Option<String>,
}

/// Kraken exchange for WASM
#[wasm_bindgen]
pub struct WasmKraken {
    client: HttpClient,
}

#[wasm_bindgen]
impl WasmKraken {
    /// Create new Kraken instance
    #[wasm_bindgen(constructor)]
    pub fn new(config: Option<WasmExchangeConfig>) -> Result<WasmKraken, JsValue> {
        let internal_config = config
            .map(|c| c.to_internal())
            .unwrap_or_else(crate::client::ExchangeConfig::new);

        let client = HttpClient::new("https://api.kraken.com", &internal_config)
            .map_err(js_err)?;

        Ok(Self { client })
    }

    fn to_pair(symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// Fetch ticker for a symbol
    #[wasm_bindgen(js_name = fetchTicker)]
    pub async fn fetch_ticker(&self, symbol: &str) -> Result<WasmTicker, JsValue> {
        let pair = Self::to_pair(symbol);
        let mut params = HashMap::new();
        params.insert("pair".to_string(), pair.clone());

        let response: KrakenResponse<HashMap<String, KrakenTicker>> = self.client
            .get("/0/public/Ticker", Some(params), None)
            .await
            .map_err(js_err)?;

        let ticker = response.result.values().next()
            .ok_or_else(|| js_err("No ticker data"))?;

        Ok(WasmTicker {
            symbol: symbol.to_string(),
            last: ticker.c[0].parse().ok(),
            bid: ticker.b[0].parse().ok(),
            ask: ticker.a[0].parse().ok(),
            high: ticker.h[1].parse().ok(),
            low: ticker.l[1].parse().ok(),
            volume: ticker.v[1].parse().ok(),
            timestamp: Some(js_sys::Date::now() as i64),
        })
    }

    /// Fetch order book for a symbol
    #[wasm_bindgen(js_name = fetchOrderBook)]
    pub async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> Result<WasmOrderBook, JsValue> {
        let pair = Self::to_pair(symbol);
        let mut params = HashMap::new();
        params.insert("pair".to_string(), pair.clone());
        if let Some(l) = limit {
            params.insert("count".to_string(), l.min(500).to_string());
        }

        let response: KrakenResponse<HashMap<String, KrakenOrderBook>> = self.client
            .get("/0/public/Depth", Some(params), None)
            .await
            .map_err(js_err)?;

        let ob = response.result.values().next()
            .ok_or_else(|| js_err("No orderbook data"))?;

        let bids: Vec<WasmOrderBookEntry> = ob.bids
            .iter()
            .map(|b| WasmOrderBookEntry {
                price: b[0].parse().unwrap_or_default(),
                amount: b[1].parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<WasmOrderBookEntry> = ob.asks
            .iter()
            .map(|a| WasmOrderBookEntry {
                price: a[0].parse().unwrap_or_default(),
                amount: a[1].parse().unwrap_or_default(),
            })
            .collect();

        Ok(WasmOrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp: Some(js_sys::Date::now() as i64),
        })
    }

    /// Fetch all markets
    #[wasm_bindgen(js_name = fetchMarkets)]
    pub async fn fetch_markets(&self) -> Result<JsValue, JsValue> {
        let response: KrakenResponse<HashMap<String, KrakenAssetPair>> = self.client
            .get("/0/public/AssetPairs", None, None)
            .await
            .map_err(js_err)?;

        let markets: Vec<serde_json::Value> = response.result
            .iter()
            .filter(|(_, p)| p.status.as_deref() != Some("offline"))
            .map(|(id, p)| {
                let symbol = format!("{}/{}", p.base, p.quote);
                serde_json::json!({
                    "id": id,
                    "symbol": symbol,
                    "base": p.base,
                    "quote": p.quote,
                    "active": true,
                    "altname": p.altname
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&markets).unwrap_or_default()))
    }

    /// Fetch tickers for multiple symbols
    #[wasm_bindgen(js_name = fetchTickers)]
    pub async fn fetch_tickers(&self, symbols: Option<Vec<String>>) -> Result<JsValue, JsValue> {
        let params = if let Some(ref syms) = symbols {
            let pairs = syms.iter().map(|s| Self::to_pair(s)).collect::<Vec<_>>().join(",");
            let mut p = HashMap::new();
            p.insert("pair".to_string(), pairs);
            Some(p)
        } else {
            None
        };

        let response: KrakenResponse<HashMap<String, KrakenTicker>> = self.client
            .get("/0/public/Ticker", params, None)
            .await
            .map_err(js_err)?;

        let tickers: Vec<serde_json::Value> = response.result
            .iter()
            .map(|(id, t)| {
                serde_json::json!({
                    "symbol": id,
                    "last": t.c[0].parse::<f64>().ok(),
                    "bid": t.b[0].parse::<f64>().ok(),
                    "ask": t.a[0].parse::<f64>().ok(),
                    "high": t.h[1].parse::<f64>().ok(),
                    "low": t.l[1].parse::<f64>().ok(),
                    "volume": t.v[1].parse::<f64>().ok()
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&tickers).unwrap_or_default()))
    }

    /// Fetch recent trades for a symbol
    #[wasm_bindgen(js_name = fetchTrades)]
    pub async fn fetch_trades(&self, symbol: &str, _limit: Option<u32>) -> Result<JsValue, JsValue> {
        let pair = Self::to_pair(symbol);
        let mut params = HashMap::new();
        params.insert("pair".to_string(), pair);

        let response: KrakenResponse<HashMap<String, serde_json::Value>> = self.client
            .get("/0/public/Trades", Some(params), None)
            .await
            .map_err(js_err)?;

        let mut trades = Vec::new();
        for (key, value) in &response.result {
            if key == "last" { continue; }
            if let Some(arr) = value.as_array() {
                for t in arr.iter().take(100) {
                    if let Some(trade) = t.as_array() {
                        trades.push(serde_json::json!({
                            "id": "",
                            "symbol": symbol,
                            "price": trade.get(0).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                            "amount": trade.get(1).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                            "side": trade.get(3).and_then(|v| v.as_str()).map(|s| if s == "b" { "buy" } else { "sell" }).unwrap_or("buy"),
                            "timestamp": (trade.get(2).and_then(|v| v.as_f64()).unwrap_or_default() * 1000.0) as i64
                        }));
                    }
                }
            }
        }

        Ok(JsValue::from_str(&serde_json::to_string(&trades).unwrap_or_default()))
    }

    /// Fetch OHLCV candles for a symbol
    #[wasm_bindgen(js_name = fetchOhlcv)]
    pub async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: &str,
        since: Option<i64>,
        _limit: Option<u32>,
    ) -> Result<JsValue, JsValue> {
        let pair = Self::to_pair(symbol);

        // Map timeframe to Kraken interval (minutes)
        let interval = match timeframe {
            "1m" => "1",
            "5m" => "5",
            "15m" => "15",
            "30m" => "30",
            "1h" => "60",
            "4h" => "240",
            "1d" => "1440",
            "1w" => "10080",
            _ => "1",
        };

        let mut params = HashMap::new();
        params.insert("pair".to_string(), pair);
        params.insert("interval".to_string(), interval.to_string());
        if let Some(s) = since {
            params.insert("since".to_string(), (s / 1000).to_string());
        }

        let response: KrakenResponse<HashMap<String, serde_json::Value>> = self.client
            .get("/0/public/OHLC", Some(params), None)
            .await
            .map_err(js_err)?;

        let mut candles = Vec::new();
        for (key, value) in &response.result {
            if key == "last" { continue; }
            if let Some(arr) = value.as_array() {
                for c in arr {
                    if let Some(candle) = c.as_array() {
                        candles.push(serde_json::json!({
                            "timestamp": (candle.get(0).and_then(|v| v.as_i64()).unwrap_or_default()) * 1000,
                            "open": candle.get(1).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                            "high": candle.get(2).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                            "low": candle.get(3).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                            "close": candle.get(4).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default(),
                            "volume": candle.get(6).and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default()
                        }));
                    }
                }
            }
        }

        Ok(JsValue::from_str(&serde_json::to_string(&candles).unwrap_or_default()))
    }
}

// ============================================================================
// WebSocket Bindings
// ============================================================================

use crate::client::{WsClient, WsConfig, WsEvent, Subscription};
use futures_util::StreamExt;

/// WebSocket message types for JavaScript
#[wasm_bindgen]
#[derive(Clone)]
pub struct WasmWsMessage {
    #[wasm_bindgen(getter_with_clone)]
    pub message_type: String,
    #[wasm_bindgen(getter_with_clone)]
    pub symbol: String,
    #[wasm_bindgen(getter_with_clone)]
    pub data: String,
    pub timestamp: Option<i64>,
}

#[wasm_bindgen]
impl WasmWsMessage {
    /// Convert to JSON string
    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"type":"{}","symbol":"{}","data":{},"timestamp":{}}}"#,
            self.message_type,
            self.symbol,
            self.data,
            self.timestamp.map_or("null".to_string(), |t| t.to_string())
        )
    }
}

/// Binance WebSocket client for WASM
#[wasm_bindgen]
pub struct WasmBinanceWs {
    base_url: String,
}

#[wasm_bindgen]
impl WasmBinanceWs {
    /// Create new Binance WebSocket instance
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            base_url: "wss://stream.binance.com:9443/ws".to_string(),
        }
    }

    /// Watch ticker updates via callback
    ///
    /// The callback receives a WasmWsMessage with ticker data
    #[wasm_bindgen(js_name = watchTicker)]
    pub fn watch_ticker(&self, symbol: &str, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = symbol.replace("/", "").to_lowercase();
        let stream = format!("{}@ticker", market_id);
        let url = format!("{}/{}", self.base_url, stream);

        let config = WsConfig {
            url: url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                match event {
                    WsEvent::Message(msg) => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                            let ws_msg = WasmWsMessage {
                                message_type: "ticker".to_string(),
                                symbol: symbol_owned.clone(),
                                data: msg,
                                timestamp: data.get("E").and_then(|v| v.as_i64()),
                            };
                            let this = JsValue::null();
                            let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                        }
                    }
                    WsEvent::Error(err) => {
                        let ws_msg = WasmWsMessage {
                            message_type: "error".to_string(),
                            symbol: symbol_owned.clone(),
                            data: format!(r#"{{"error":"{}"}}"#, err),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    WsEvent::Connected => {
                        let ws_msg = WasmWsMessage {
                            message_type: "connected".to_string(),
                            symbol: symbol_owned.clone(),
                            data: r#"{"status":"connected"}"#.to_string(),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    WsEvent::Disconnected => {
                        let ws_msg = WasmWsMessage {
                            message_type: "disconnected".to_string(),
                            symbol: symbol_owned.clone(),
                            data: r#"{"status":"disconnected"}"#.to_string(),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    _ => {}
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }

    /// Watch order book updates via callback
    #[wasm_bindgen(js_name = watchOrderBook)]
    pub fn watch_order_book(&self, symbol: &str, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = symbol.replace("/", "").to_lowercase();
        let stream = format!("{}@depth@100ms", market_id);
        let url = format!("{}/{}", self.base_url, stream);

        let config = WsConfig {
            url: url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                match event {
                    WsEvent::Message(msg) => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                            let ws_msg = WasmWsMessage {
                                message_type: "orderbook".to_string(),
                                symbol: symbol_owned.clone(),
                                data: msg,
                                timestamp: data.get("E").and_then(|v| v.as_i64()),
                            };
                            let this = JsValue::null();
                            let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                        }
                    }
                    WsEvent::Error(err) => {
                        let ws_msg = WasmWsMessage {
                            message_type: "error".to_string(),
                            symbol: symbol_owned.clone(),
                            data: format!(r#"{{"error":"{}"}}"#, err),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    WsEvent::Connected => {
                        let ws_msg = WasmWsMessage {
                            message_type: "connected".to_string(),
                            symbol: symbol_owned.clone(),
                            data: r#"{"status":"connected"}"#.to_string(),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    _ => {}
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }

    /// Watch trades via callback
    #[wasm_bindgen(js_name = watchTrades)]
    pub fn watch_trades(&self, symbol: &str, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = symbol.replace("/", "").to_lowercase();
        let stream = format!("{}@trade", market_id);
        let url = format!("{}/{}", self.base_url, stream);

        let config = WsConfig {
            url: url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                if let WsEvent::Message(msg) = event {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                        let ws_msg = WasmWsMessage {
                            message_type: "trade".to_string(),
                            symbol: symbol_owned.clone(),
                            data: msg,
                            timestamp: data.get("E").and_then(|v| v.as_i64()),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }
}

/// Upbit WebSocket client for WASM
#[wasm_bindgen]
pub struct WasmUpbitWs {
    base_url: String,
}

#[wasm_bindgen]
impl WasmUpbitWs {
    /// Create new Upbit WebSocket instance
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            base_url: "wss://api.upbit.com/websocket/v1".to_string(),
        }
    }

    /// Watch ticker updates via callback
    #[wasm_bindgen(js_name = watchTicker)]
    pub fn watch_ticker(&self, symbol: &str, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = to_upbit_market_id(symbol);

        let config = WsConfig {
            url: self.base_url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        // Send subscription after connection
        let subscribe_msg = format!(
            r#"[{{"ticket":"wasm-ticker"}},{{"type":"ticker","codes":["{}"]}}]"#,
            market_id
        );

        let sub = Subscription {
            channel: "ticker".to_string(),
            symbol: Some(symbol.to_string()),
            subscribe_message: subscribe_msg,
            unsubscribe_message: None,
        };

        client.subscribe(sub).map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                match event {
                    WsEvent::Message(msg) => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                            let ws_msg = WasmWsMessage {
                                message_type: "ticker".to_string(),
                                symbol: symbol_owned.clone(),
                                data: msg,
                                timestamp: data.get("timestamp").and_then(|v| v.as_i64()),
                            };
                            let this = JsValue::null();
                            let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                        }
                    }
                    WsEvent::Connected => {
                        let ws_msg = WasmWsMessage {
                            message_type: "connected".to_string(),
                            symbol: symbol_owned.clone(),
                            data: r#"{"status":"connected"}"#.to_string(),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    WsEvent::Error(err) => {
                        let ws_msg = WasmWsMessage {
                            message_type: "error".to_string(),
                            symbol: symbol_owned.clone(),
                            data: format!(r#"{{"error":"{}"}}"#, err),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    _ => {}
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }

    /// Watch order book updates via callback
    #[wasm_bindgen(js_name = watchOrderBook)]
    pub fn watch_order_book(&self, symbol: &str, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = to_upbit_market_id(symbol);

        let config = WsConfig {
            url: self.base_url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        let subscribe_msg = format!(
            r#"[{{"ticket":"wasm-orderbook"}},{{"type":"orderbook","codes":["{}"]}}]"#,
            market_id
        );

        let sub = Subscription {
            channel: "orderbook".to_string(),
            symbol: Some(symbol.to_string()),
            subscribe_message: subscribe_msg,
            unsubscribe_message: None,
        };

        client.subscribe(sub).map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                if let WsEvent::Message(msg) = event {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                        let ws_msg = WasmWsMessage {
                            message_type: "orderbook".to_string(),
                            symbol: symbol_owned.clone(),
                            data: msg,
                            timestamp: data.get("timestamp").and_then(|v| v.as_i64()),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }
}

/// Bybit WebSocket client for WASM
#[wasm_bindgen]
pub struct WasmBybitWs {
    base_url: String,
}

#[wasm_bindgen]
impl WasmBybitWs {
    /// Create new Bybit WebSocket instance
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            base_url: "wss://stream.bybit.com/v5/public/spot".to_string(),
        }
    }

    /// Watch ticker updates via callback
    #[wasm_bindgen(js_name = watchTicker)]
    pub fn watch_ticker(&self, symbol: &str, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = symbol.replace("/", "");

        let config = WsConfig {
            url: self.base_url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        let subscribe_msg = format!(
            r#"{{"op":"subscribe","args":["tickers.{}"]}}"#,
            market_id
        );

        let sub = Subscription {
            channel: "tickers".to_string(),
            symbol: Some(symbol.to_string()),
            subscribe_message: subscribe_msg.clone(),
            unsubscribe_message: Some(format!(
                r#"{{"op":"unsubscribe","args":["tickers.{}"]}}"#,
                market_id
            )),
        };

        client.subscribe(sub).map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                match event {
                    WsEvent::Message(msg) => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                            // Skip subscription confirmation messages
                            if data.get("op").is_some() {
                                continue;
                            }
                            let ws_msg = WasmWsMessage {
                                message_type: "ticker".to_string(),
                                symbol: symbol_owned.clone(),
                                data: msg,
                                timestamp: data.get("ts").and_then(|v| v.as_i64()),
                            };
                            let this = JsValue::null();
                            let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                        }
                    }
                    WsEvent::Connected => {
                        let ws_msg = WasmWsMessage {
                            message_type: "connected".to_string(),
                            symbol: symbol_owned.clone(),
                            data: r#"{"status":"connected"}"#.to_string(),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    WsEvent::Error(err) => {
                        let ws_msg = WasmWsMessage {
                            message_type: "error".to_string(),
                            symbol: symbol_owned.clone(),
                            data: format!(r#"{{"error":"{}"}}"#, err),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    _ => {}
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }

    /// Watch order book updates via callback
    #[wasm_bindgen(js_name = watchOrderBook)]
    pub fn watch_order_book(&self, symbol: &str, depth: Option<u32>, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = symbol.replace("/", "");
        let depth_val = depth.unwrap_or(50);

        let config = WsConfig {
            url: self.base_url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        let subscribe_msg = format!(
            r#"{{"op":"subscribe","args":["orderbook.{}.{}"]}}"#,
            depth_val, market_id
        );

        let sub = Subscription {
            channel: "orderbook".to_string(),
            symbol: Some(symbol.to_string()),
            subscribe_message: subscribe_msg,
            unsubscribe_message: Some(format!(
                r#"{{"op":"unsubscribe","args":["orderbook.{}.{}"]}}"#,
                depth_val, market_id
            )),
        };

        client.subscribe(sub).map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                if let WsEvent::Message(msg) = event {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                        // Skip subscription confirmation messages
                        if data.get("op").is_some() {
                            continue;
                        }
                        let ws_msg = WasmWsMessage {
                            message_type: "orderbook".to_string(),
                            symbol: symbol_owned.clone(),
                            data: msg,
                            timestamp: data.get("ts").and_then(|v| v.as_i64()),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }
}

/// Handle for WebSocket connection
///
/// Note: In WASM, connection lifecycle is managed by the browser.
/// The handle is a placeholder for future close/unsubscribe functionality.
#[wasm_bindgen]
pub struct WasmWsHandle {
    _marker: u8,
}

#[wasm_bindgen]
impl WasmWsHandle {
    /// Check if connection is active
    #[wasm_bindgen(js_name = isActive)]
    pub fn is_active(&self) -> bool {
        true
    }
}

/// Helper to convert symbol to Upbit market ID
fn to_upbit_market_id(symbol: &str) -> String {
    let parts: Vec<&str> = symbol.split('/').collect();
    if parts.len() == 2 {
        format!("{}-{}", parts[1], parts[0])
    } else {
        symbol.to_string()
    }
}
