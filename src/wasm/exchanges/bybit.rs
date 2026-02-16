//! Bybit WASM Exchange Implementation

use wasm_bindgen::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use crate::client::HttpClient;
use crate::wasm::config::WasmExchangeConfig;
use crate::wasm::types::{WasmTicker, WasmOrderBook};
use crate::wasm::utils::js_err;
use super::parsing::parse_f64;

const BASE_URL: &str = "https://api.bybit.com";

// Response types
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
    b: Vec<[String; 2]>,
    a: Vec<[String; 2]>,
    ts: Option<i64>,
}

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

#[derive(Deserialize)]
struct BybitKlineResult {
    list: Vec<Vec<serde_json::Value>>,
}

/// Convert unified symbol to Bybit format
fn to_market_id(symbol: &str) -> String {
    symbol.replace("/", "")
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

        let client = HttpClient::new(BASE_URL, &internal_config)
            .map_err(js_err)?;

        Ok(Self { client })
    }

    /// Fetch ticker for a symbol
    #[wasm_bindgen(js_name = fetchTicker)]
    pub async fn fetch_ticker(&self, symbol: &str) -> Result<WasmTicker, JsValue> {
        let mut params = HashMap::new();
        params.insert("category".to_string(), "spot".to_string());
        params.insert("symbol".to_string(), to_market_id(symbol));

        let response: BybitResponse<BybitTickerResult> = self.client
            .get("/v5/market/tickers", Some(params), None)
            .await
            .map_err(js_err)?;

        let ticker = response.result.list.first()
            .ok_or_else(|| js_err("No ticker data"))?;

        Ok(WasmTicker::new(
            symbol.to_string(),
            parse_f64(&ticker.last_price),
            parse_f64(&ticker.bid1_price),
            parse_f64(&ticker.ask1_price),
            parse_f64(&ticker.high_price24h),
            parse_f64(&ticker.low_price24h),
            parse_f64(&ticker.volume24h),
            Some(js_sys::Date::now() as i64),
        ))
    }

    /// Fetch order book for a symbol
    #[wasm_bindgen(js_name = fetchOrderBook)]
    pub async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> Result<WasmOrderBook, JsValue> {
        let mut params = HashMap::new();
        params.insert("category".to_string(), "spot".to_string());
        params.insert("symbol".to_string(), to_market_id(symbol));
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: BybitResponse<BybitOrderBookResult> = self.client
            .get("/v5/market/orderbook", Some(params), None)
            .await
            .map_err(js_err)?;

        let bids: Vec<(f64, f64)> = response.result.b
            .iter()
            .map(|b| (b[0].parse().unwrap_or_default(), b[1].parse().unwrap_or_default()))
            .collect();

        let asks: Vec<(f64, f64)> = response.result.a
            .iter()
            .map(|a| (a[0].parse().unwrap_or_default(), a[1].parse().unwrap_or_default()))
            .collect();

        Ok(WasmOrderBook::new(
            symbol.to_string(),
            bids,
            asks,
            response.result.ts,
        ))
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
                    "last": parse_f64(&t.last_price),
                    "bid": parse_f64(&t.bid1_price),
                    "ask": parse_f64(&t.ask1_price),
                    "high": parse_f64(&t.high_price24h),
                    "low": parse_f64(&t.low_price24h),
                    "volume": parse_f64(&t.volume24h)
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&tickers).unwrap_or_default()))
    }

    /// Fetch recent trades for a symbol
    #[wasm_bindgen(js_name = fetchTrades)]
    pub async fn fetch_trades(&self, symbol: &str, limit: Option<u32>) -> Result<JsValue, JsValue> {
        let mut params = HashMap::new();
        params.insert("category".to_string(), "spot".to_string());
        params.insert("symbol".to_string(), to_market_id(symbol));
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
        params.insert("symbol".to_string(), to_market_id(symbol));
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
