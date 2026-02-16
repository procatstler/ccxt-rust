//! Binance WASM Exchange Implementation

use wasm_bindgen::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use crate::client::HttpClient;
use crate::wasm::config::WasmExchangeConfig;
use crate::wasm::types::{WasmTicker, WasmOrderBook};
use crate::wasm::utils::js_err;
use super::parsing::parse_f64;

const BASE_URL: &str = "https://api.binance.com";

// Response types
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceTrade {
    id: u64,
    price: String,
    qty: String,
    time: i64,
    is_buyer_maker: bool,
}

/// Convert unified symbol to Binance format
fn to_market_id(symbol: &str) -> String {
    symbol.replace("/", "")
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

        let client = HttpClient::new(BASE_URL, &internal_config)
            .map_err(js_err)?;

        Ok(Self { client })
    }

    /// Fetch ticker for a symbol
    #[wasm_bindgen(js_name = fetchTicker)]
    pub async fn fetch_ticker(&self, symbol: &str) -> Result<WasmTicker, JsValue> {
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), to_market_id(symbol));

        let response: BinanceTicker = self.client
            .get("/api/v3/ticker/24hr", Some(params), None)
            .await
            .map_err(js_err)?;

        Ok(WasmTicker::new(
            symbol.to_string(),
            parse_f64(&response.last_price),
            parse_f64(&response.bid_price),
            parse_f64(&response.ask_price),
            parse_f64(&response.high_price),
            parse_f64(&response.low_price),
            parse_f64(&response.volume),
            response.close_time,
        ))
    }

    /// Fetch order book for a symbol
    #[wasm_bindgen(js_name = fetchOrderBook)]
    pub async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> Result<WasmOrderBook, JsValue> {
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), to_market_id(symbol));
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: BinanceOrderBook = self.client
            .get("/api/v3/depth", Some(params), None)
            .await
            .map_err(js_err)?;

        let bids: Vec<(f64, f64)> = response.bids
            .iter()
            .map(|b| (b[0].parse().unwrap_or_default(), b[1].parse().unwrap_or_default()))
            .collect();

        let asks: Vec<(f64, f64)> = response.asks
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
                    syms.iter().any(|s| s.replace("/", "") == t.symbol)
                } else {
                    true
                }
            })
            .map(|t| {
                serde_json::json!({
                    "symbol": t.symbol,
                    "last": parse_f64(&t.last_price),
                    "bid": parse_f64(&t.bid_price),
                    "ask": parse_f64(&t.ask_price),
                    "high": parse_f64(&t.high_price),
                    "low": parse_f64(&t.low_price),
                    "volume": parse_f64(&t.volume),
                    "timestamp": t.close_time
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&tickers).unwrap_or_default()))
    }

    /// Fetch recent trades for a symbol
    #[wasm_bindgen(js_name = fetchTrades)]
    pub async fn fetch_trades(&self, symbol: &str, limit: Option<u32>) -> Result<JsValue, JsValue> {
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), to_market_id(symbol));
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
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), to_market_id(symbol));
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
