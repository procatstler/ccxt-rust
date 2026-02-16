//! OKX WASM Exchange Implementation

use wasm_bindgen::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use crate::client::HttpClient;
use crate::wasm::config::WasmExchangeConfig;
use crate::wasm::types::{WasmTicker, WasmOrderBook};
use crate::wasm::utils::js_err;

const BASE_URL: &str = "https://www.okx.com";

// Response types
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

/// Convert unified symbol to OKX inst_id format
fn to_inst_id(symbol: &str) -> String {
    symbol.replace("/", "-")
}

/// Parse string to f64
fn parse_str(s: &Option<String>) -> Option<f64> {
    s.as_ref().and_then(|v| v.parse().ok())
}

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

        let client = HttpClient::new(BASE_URL, &internal_config)
            .map_err(js_err)?;

        Ok(Self { client })
    }

    /// Fetch ticker for a symbol
    #[wasm_bindgen(js_name = fetchTicker)]
    pub async fn fetch_ticker(&self, symbol: &str) -> Result<WasmTicker, JsValue> {
        let inst_id = to_inst_id(symbol);
        let mut params = HashMap::new();
        params.insert("instId".to_string(), inst_id);

        let response: OkxResponse<Vec<OkxTicker>> = self.client
            .get("/api/v5/market/ticker", Some(params), None)
            .await
            .map_err(js_err)?;

        let ticker = response.data.first()
            .ok_or_else(|| js_err("No ticker data"))?;

        Ok(WasmTicker::new(
            symbol.to_string(),
            parse_str(&ticker.last),
            parse_str(&ticker.bid_px),
            parse_str(&ticker.ask_px),
            parse_str(&ticker.high24h),
            parse_str(&ticker.low24h),
            parse_str(&ticker.vol24h),
            ticker.ts.as_ref().and_then(|s| s.parse().ok()),
        ))
    }

    /// Fetch order book for a symbol
    #[wasm_bindgen(js_name = fetchOrderBook)]
    pub async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> Result<WasmOrderBook, JsValue> {
        let inst_id = to_inst_id(symbol);
        let mut params = HashMap::new();
        params.insert("instId".to_string(), inst_id);
        if let Some(l) = limit {
            params.insert("sz".to_string(), l.min(400).to_string());
        }

        let response: OkxResponse<Vec<OkxOrderBook>> = self.client
            .get("/api/v5/market/books", Some(params), None)
            .await
            .map_err(js_err)?;

        let ob = response.data.first()
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
            ob.ts.as_ref().and_then(|s| s.parse().ok()),
        ))
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
                    "last": parse_str(&t.last),
                    "bid": parse_str(&t.bid_px),
                    "ask": parse_str(&t.ask_px),
                    "high": parse_str(&t.high24h),
                    "low": parse_str(&t.low24h),
                    "volume": parse_str(&t.vol24h),
                    "timestamp": t.ts.as_ref().and_then(|s| s.parse::<i64>().ok())
                })
            })
            .collect();

        Ok(JsValue::from_str(&serde_json::to_string(&tickers).unwrap_or_default()))
    }

    /// Fetch recent trades for a symbol
    #[wasm_bindgen(js_name = fetchTrades)]
    pub async fn fetch_trades(&self, symbol: &str, limit: Option<u32>) -> Result<JsValue, JsValue> {
        let inst_id = to_inst_id(symbol);
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
        let inst_id = to_inst_id(symbol);

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
