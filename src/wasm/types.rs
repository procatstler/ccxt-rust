//! WASM Data Types
//!
//! Common data types for JavaScript interop.

use wasm_bindgen::prelude::*;

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

impl WasmTicker {
    /// Create a new WasmTicker
    pub fn new(
        symbol: String,
        last: Option<f64>,
        bid: Option<f64>,
        ask: Option<f64>,
        high: Option<f64>,
        low: Option<f64>,
        volume: Option<f64>,
        timestamp: Option<i64>,
    ) -> Self {
        Self { symbol, last, bid, ask, high, low, volume, timestamp }
    }
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

impl WasmOrderBookEntry {
    pub fn new(price: f64, amount: f64) -> Self {
        Self { price, amount }
    }
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

impl WasmOrderBook {
    /// Create a new WasmOrderBook
    pub fn new(
        symbol: String,
        bids: Vec<(f64, f64)>,
        asks: Vec<(f64, f64)>,
        timestamp: Option<i64>,
    ) -> Self {
        Self {
            symbol,
            bids: bids.into_iter().map(|(p, a)| WasmOrderBookEntry::new(p, a)).collect(),
            asks: asks.into_iter().map(|(p, a)| WasmOrderBookEntry::new(p, a)).collect(),
            timestamp,
        }
    }
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

impl WasmMarket {
    pub fn new(id: String, symbol: String, base: String, quote: String, active: bool) -> Self {
        Self { id, symbol, base, quote, active }
    }
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
