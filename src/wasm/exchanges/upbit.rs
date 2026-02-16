//! Upbit WASM Exchange Implementation (Korean Exchange)

use wasm_bindgen::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use crate::client::HttpClient;
use crate::wasm::config::WasmExchangeConfig;
use crate::wasm::types::{WasmTicker, WasmOrderBook};
use crate::wasm::utils::js_err;

const BASE_URL: &str = "https://api.upbit.com";

// Response types
#[derive(Deserialize)]
struct UpbitTicker {
    market: String,
    trade_price: Option<f64>,
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    market: String,
    orderbook_units: Vec<UpbitOrderBookUnit>,
    timestamp: Option<i64>,
}

#[derive(Deserialize)]
struct UpbitMarket {
    market: String,
    #[allow(dead_code)]
    korean_name: String,
    english_name: String,
}

#[derive(Deserialize)]
struct UpbitTrade {
    sequential_id: u64,
    trade_price: f64,
    trade_volume: f64,
    ask_bid: String,
    timestamp: i64,
}

#[derive(Deserialize)]
struct UpbitCandle {
    timestamp: i64,
    opening_price: f64,
    high_price: f64,
    low_price: f64,
    trade_price: f64,
    candle_acc_trade_volume: f64,
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

/// Convert Upbit market ID to unified symbol (KRW-BTC -> BTC/KRW)
fn to_unified_symbol(market: &str) -> String {
    let parts: Vec<&str> = market.split('-').collect();
    if parts.len() == 2 {
        format!("{}/{}", parts[1], parts[0])
    } else {
        market.to_string()
    }
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

        let client = HttpClient::new(BASE_URL, &internal_config)
            .map_err(js_err)?;

        Ok(Self { client })
    }

    /// Fetch ticker for a symbol
    #[wasm_bindgen(js_name = fetchTicker)]
    pub async fn fetch_ticker(&self, symbol: &str) -> Result<WasmTicker, JsValue> {
        let mut params = HashMap::new();
        params.insert("markets".to_string(), to_market_id(symbol));

        let response: Vec<UpbitTicker> = self.client
            .get("/v1/ticker", Some(params), None)
            .await
            .map_err(js_err)?;

        let ticker = response.first().ok_or_else(|| js_err("No ticker data"))?;

        Ok(WasmTicker::new(
            symbol.to_string(),
            ticker.trade_price,
            None, // Upbit ticker doesn't include bid/ask
            None,
            ticker.high_price,
            ticker.low_price,
            ticker.acc_trade_volume_24h,
            ticker.timestamp,
        ))
    }

    /// Fetch order book for a symbol
    #[wasm_bindgen(js_name = fetchOrderBook)]
    pub async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> Result<WasmOrderBook, JsValue> {
        let mut params = HashMap::new();
        params.insert("markets".to_string(), to_market_id(symbol));

        let response: Vec<UpbitOrderBook> = self.client
            .get("/v1/orderbook", Some(params), None)
            .await
            .map_err(js_err)?;

        let orderbook = response.first().ok_or_else(|| js_err("No orderbook data"))?;

        let bids: Vec<(f64, f64)> = orderbook.orderbook_units
            .iter()
            .map(|u| (u.bid_price, u.bid_size))
            .collect();

        let asks: Vec<(f64, f64)> = orderbook.orderbook_units
            .iter()
            .map(|u| (u.ask_price, u.ask_size))
            .collect();

        Ok(WasmOrderBook::new(
            symbol.to_string(),
            bids,
            asks,
            orderbook.timestamp,
        ))
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
                .map(|s| to_market_id(s))
                .collect::<Vec<_>>()
                .join(",")
        } else {
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
                serde_json::json!({
                    "symbol": to_unified_symbol(&t.market),
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
        let mut params = HashMap::new();
        params.insert("market".to_string(), to_market_id(symbol));
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
        params.insert("market".to_string(), to_market_id(symbol));
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
