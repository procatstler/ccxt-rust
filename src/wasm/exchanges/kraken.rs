//! Kraken WASM Exchange Implementation

use wasm_bindgen::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use crate::client::HttpClient;
use crate::wasm::config::WasmExchangeConfig;
use crate::wasm::types::{WasmTicker, WasmOrderBook};
use crate::wasm::utils::js_err;

const BASE_URL: &str = "https://api.kraken.com";

// Response types
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

/// Convert unified symbol to Kraken pair format
fn to_pair(symbol: &str) -> String {
    symbol.replace("/", "")
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

        let client = HttpClient::new(BASE_URL, &internal_config)
            .map_err(js_err)?;

        Ok(Self { client })
    }

    /// Fetch ticker for a symbol
    #[wasm_bindgen(js_name = fetchTicker)]
    pub async fn fetch_ticker(&self, symbol: &str) -> Result<WasmTicker, JsValue> {
        let pair = to_pair(symbol);
        let mut params = HashMap::new();
        params.insert("pair".to_string(), pair);

        let response: KrakenResponse<HashMap<String, KrakenTicker>> = self.client
            .get("/0/public/Ticker", Some(params), None)
            .await
            .map_err(js_err)?;

        let ticker = response.result.values().next()
            .ok_or_else(|| js_err("No ticker data"))?;

        Ok(WasmTicker::new(
            symbol.to_string(),
            ticker.c[0].parse().ok(),
            ticker.b[0].parse().ok(),
            ticker.a[0].parse().ok(),
            ticker.h[1].parse().ok(),
            ticker.l[1].parse().ok(),
            ticker.v[1].parse().ok(),
            Some(js_sys::Date::now() as i64),
        ))
    }

    /// Fetch order book for a symbol
    #[wasm_bindgen(js_name = fetchOrderBook)]
    pub async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> Result<WasmOrderBook, JsValue> {
        let pair = to_pair(symbol);
        let mut params = HashMap::new();
        params.insert("pair".to_string(), pair);
        if let Some(l) = limit {
            params.insert("count".to_string(), l.min(500).to_string());
        }

        let response: KrakenResponse<HashMap<String, KrakenOrderBook>> = self.client
            .get("/0/public/Depth", Some(params), None)
            .await
            .map_err(js_err)?;

        let ob = response.result.values().next()
            .ok_or_else(|| js_err("No orderbook data"))?;

        let bids: Vec<(f64, f64)> = ob.bids
            .iter()
            .map(|b| (b[0].parse().unwrap_or_default(), b[1].parse().unwrap_or_default()))
            .collect();

        let asks: Vec<(f64, f64)> = ob.asks
            .iter()
            .map(|a| (a[0].parse().unwrap_or_default(), a[1].parse().unwrap_or_default()))
            .collect();

        Ok(WasmOrderBook::new(
            symbol.to_string(),
            bids,
            asks,
            Some(js_sys::Date::now() as i64),
        ))
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
            let pairs = syms.iter().map(|s| to_pair(s)).collect::<Vec<_>>().join(",");
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
        let pair = to_pair(symbol);
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
        let pair = to_pair(symbol);

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
