//! WhiteBit WebSocket Implementation
//!
//! WhiteBit real-time data streaming

#![allow(clippy::manual_strip)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent,
};

const WS_PUBLIC_URL: &str = "wss://api.whitebit.com/ws";

/// WhiteBit WebSocket client
pub struct WhitebitWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    next_id: Arc<RwLock<i64>>,
}

impl WhitebitWs {
    /// Create new WhiteBit WebSocket client
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            next_id: Arc::new(RwLock::new(1)),
        }
    }

    /// Get next request ID
    async fn get_next_id(&self) -> i64 {
        let mut id = self.next_id.write().await;
        let current = *id;
        *id += 1;
        current
    }

    /// Convert symbol to WhiteBit format (BTC/USDT -> BTC_USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Convert timeframe to WhiteBit interval (in seconds)
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "60",
            Timeframe::Minute5 => "300",
            Timeframe::Minute15 => "900",
            Timeframe::Minute30 => "1800",
            Timeframe::Hour1 => "3600",
            Timeframe::Hour4 => "14400",
            Timeframe::Hour8 => "28800",
            Timeframe::Day1 => "86400",
            Timeframe::Week1 => "604800",
            _ => "60",
        }
    }

    /// Parse ticker message
    fn parse_ticker(data: &WhitebitTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.market);
        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|v| v.parse().ok()),
            low: data.low.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: None,
            ask: data.ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.deal.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// Parse order book message
    fn parse_order_book(data: &WhitebitOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: b[0].parse().ok()?,
                    amount: b[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: a[0].parse().ok()?,
                    amount: a[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let timestamp = data.timestamp
            .map(|t| (t * 1000.0) as i64)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        }
    }

    /// Parse trade message
    #[allow(dead_code)]
    fn parse_trade(data: &WhitebitTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.market);
        let timestamp = data.time
            .map(|t| (t * 1000.0) as i64)
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.amount.parse().unwrap_or_default();

        let trades = vec![Trade {
            id: data.id.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(data.trade_type.clone()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent { symbol, trades }
    }

    /// Parse candle message
    fn parse_candle(data: &[serde_json::Value], symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        if data.len() < 6 {
            return None;
        }

        let timestamp = data[0].as_i64()?;
        let ohlcv = OHLCV::new(
            timestamp * 1000, // Convert to milliseconds
            data[1].as_str()?.parse().ok()?,
            data[4].as_str()?.parse().ok()?, // close
            data[2].as_str()?.parse().ok()?, // high
            data[3].as_str()?.parse().ok()?, // low
            data[5].as_str()?.parse().ok()?, // volume
        );

        Some(WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        })
    }

    /// Convert WhiteBit symbol to unified symbol (BTC_USDT -> BTC/USDT)
    fn to_unified_symbol(whitebit_symbol: &str) -> String {
        whitebit_symbol.replace("_", "/")
    }

    /// Process incoming WebSocket message
    fn process_message(msg: &str) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<WhitebitWsResponse>(msg) {
            // Check for errors
            if let Some(error) = &response.error {
                return Some(WsMessage::Error(format!("{}: {}", error.code, error.message)));
            }

            // Handle result (subscription confirmation)
            if response.result.is_some() && response.id.is_some() {
                return Some(WsMessage::Subscribed {
                    channel: response.method.clone().unwrap_or_default(),
                    symbol: None,
                });
            }

            // Process updates based on method
            if let Some(method) = &response.method {
                match method.as_str() {
                    // Ticker update
                    "ticker_update" => {
                        if let Some(params) = &response.params {
                            if let Ok(ticker_data) = serde_json::from_value::<WhitebitTickerData>(params[0].clone()) {
                                return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                            }
                        }
                    }
                    // Depth update
                    "depth_update" => {
                        if let Some(params) = &response.params {
                            if params.len() >= 3 {
                                let is_snapshot = params[0].as_bool().unwrap_or(false);
                                if let Ok(ob_data) = serde_json::from_value::<WhitebitOrderBookData>(params[1].clone()) {
                                    let market = params[2].as_str().unwrap_or("");
                                    let symbol = Self::to_unified_symbol(market);
                                    return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol, is_snapshot)));
                                }
                            }
                        }
                    }
                    // Trades update
                    "trades_update" => {
                        if let Some(params) = &response.params {
                            if params.len() >= 2 {
                                let market = params[0].as_str().unwrap_or("");
                                if let Ok(trades_arr) = serde_json::from_value::<Vec<WhitebitTradeData>>(params[1].clone()) {
                                    let symbol = Self::to_unified_symbol(market);
                                    let trades: Vec<Trade> = trades_arr.iter().map(|t| {
                                        let timestamp = t.time.map(|time| (time * 1000.0) as i64)
                                            .unwrap_or_else(|| Utc::now().timestamp_millis());
                                        let price: Decimal = t.price.parse().unwrap_or_default();
                                        let amount: Decimal = t.amount.parse().unwrap_or_default();

                                        Trade {
                                            id: t.id.to_string(),
                                            order: None,
                                            timestamp: Some(timestamp),
                                            datetime: Some(
                                                chrono::DateTime::from_timestamp_millis(timestamp)
                                                    .map(|dt| dt.to_rfc3339())
                                                    .unwrap_or_default(),
                                            ),
                                            symbol: symbol.clone(),
                                            trade_type: None,
                                            side: Some(t.trade_type.clone()),
                                            taker_or_maker: None,
                                            price,
                                            amount,
                                            cost: Some(price * amount),
                                            fee: None,
                                            fees: Vec::new(),
                                            info: serde_json::to_value(t).unwrap_or_default(),
                                        }
                                    }).collect();

                                    return Some(WsMessage::Trade(WsTradeEvent { symbol, trades }));
                                }
                            }
                        }
                    }
                    // Candles update
                    "candles_update" => {
                        if let Some(params) = &response.params {
                            if let Some(candle_arr) = params.first() {
                                if let Ok(candle_data) = serde_json::from_value::<Vec<serde_json::Value>>(candle_arr.clone()) {
                                    if candle_data.len() >= 8 {
                                        let market = candle_data[7].as_str().unwrap_or("");
                                        let symbol = Self::to_unified_symbol(market);
                                        // Infer timeframe from interval - for now default to 1m
                                        // In production, you'd track subscriptions to know the timeframe
                                        let timeframe = Timeframe::Minute1;
                                        if let Some(event) = Self::parse_candle(&candle_data[..7], &symbol, timeframe) {
                                            return Some(WsMessage::Ohlcv(event));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        None
    }

    /// Subscribe to a stream and return event receiver
    async fn subscribe_stream(
        &mut self,
        method: &str,
        params: Vec<serde_json::Value>,
        channel: &str,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Send subscription message
        let id = self.get_next_id().await;
        let subscribe_msg = serde_json::json!({
            "id": id,
            "method": method,
            "params": params
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // Store subscription
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, method.to_string());
        }

        // Spawn event processing task
        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    }
                    WsEvent::Disconnected => {
                        let _ = tx.send(WsMessage::Disconnected);
                    }
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_message(&msg) {
                            let _ = tx.send(ws_msg);
                        }
                    }
                    WsEvent::Error(err) => {
                        let _ = tx.send(WsMessage::Error(err));
                    }
                    _ => {}
                }
            }
        });

        Ok(event_rx)
    }
}

impl Default for WhitebitWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for WhitebitWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let params = vec![serde_json::json!(market_id)];
        client.subscribe_stream("ticker_subscribe", params, "ticker", Some(symbol)).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let depth = limit.unwrap_or(100);
        let price_interval = "0"; // No price interval grouping
        let params = vec![
            serde_json::json!(market_id),
            serde_json::json!(depth),
            serde_json::json!(price_interval),
            serde_json::json!(true), // Allow multiple subscriptions
        ];
        client.subscribe_stream("depth_subscribe", params, "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let params = vec![serde_json::json!(market_id)];
        client.subscribe_stream("trades_subscribe", params, "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe).parse::<i64>().unwrap_or(60);
        let params = vec![
            serde_json::json!(market_id),
            serde_json::json!(interval),
        ];
        client.subscribe_stream("candles_subscribe", params, "ohlcv", Some(symbol)).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // WhiteBit connects automatically on subscription
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = &self.ws_client {
            client.close()?;
        }
        self.ws_client = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        if let Some(client) = &self.ws_client {
            client.is_connected().await
        } else {
            false
        }
    }
}

// === WhiteBit WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct WhitebitWsResponse {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<WhitebitError>,
}

#[derive(Debug, Deserialize)]
struct WhitebitError {
    code: i32,
    message: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct WhitebitTickerData {
    #[serde(default)]
    market: String,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    bid: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    deal: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WhitebitOrderBookData {
    #[serde(default)]
    timestamp: Option<f64>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WhitebitTradeData {
    id: i64,
    #[serde(default)]
    price: String,
    #[serde(default)]
    amount: String,
    #[serde(rename = "type")]
    #[serde(default)]
    trade_type: String,
    #[serde(default)]
    time: Option<f64>,
    #[serde(default)]
    market: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(WhitebitWs::format_symbol("BTC/USDT"), "BTC_USDT");
        assert_eq!(WhitebitWs::format_symbol("ETH/USDT"), "ETH_USDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(WhitebitWs::to_unified_symbol("BTC_USDT"), "BTC/USDT");
        assert_eq!(WhitebitWs::to_unified_symbol("ETH_USDT"), "ETH/USDT");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(WhitebitWs::format_interval(Timeframe::Minute1), "60");
        assert_eq!(WhitebitWs::format_interval(Timeframe::Hour1), "3600");
        assert_eq!(WhitebitWs::format_interval(Timeframe::Day1), "86400");
    }
}
