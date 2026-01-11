//! Backpack WebSocket client implementation
//!
//! WebSocket URL: wss://ws.backpack.exchange
//! Subscription format: {"method": "SUBSCRIBE", "params": ["ticker.BTC_USDC"]}
//! Supports: ticker, orderbook (depth), trades

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage, WsOrderBookEvent,
    WsTickerEvent, WsTradeEvent,
};

const WS_URL: &str = "wss://ws.backpack.exchange";

/// Backpack WebSocket client
pub struct BackpackWs {
    request_id: AtomicI64,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    orderbooks: Arc<RwLock<HashMap<String, OrderBook>>>,
    _connected: Arc<RwLock<bool>>,
}

impl BackpackWs {
    pub fn new() -> Self {
        Self {
            request_id: AtomicI64::new(1),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbooks: Arc::new(RwLock::new(HashMap::new())),
            _connected: Arc::new(RwLock::new(false)),
        }
    }

    fn get_next_id(&self) -> i64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Convert unified symbol to Backpack format
    /// BTC/USDC -> BTC_USDC
    /// ETH/USDC:USDC -> ETH_USDC_PERP
    fn format_symbol(symbol: &str) -> String {
        if symbol.contains(':') {
            // Perpetual: ETH/USDC:USDC -> ETH_USDC_PERP
            let parts: Vec<&str> = symbol.split(':').collect();
            let base_quote = parts[0].replace('/', "_");
            format!("{base_quote}_PERP")
        } else {
            // Spot: BTC/USDC -> BTC_USDC
            symbol.replace('/', "_")
        }
    }

    /// Convert Backpack symbol to unified format
    fn to_unified_symbol(backpack_symbol: &str) -> String {
        if backpack_symbol.ends_with("_PERP") {
            let base = backpack_symbol.trim_end_matches("_PERP");
            let parts: Vec<&str> = base.split('_').collect();
            if parts.len() >= 2 {
                return format!("{}/{}:{}", parts[0], parts[1], parts[1]);
            }
        }
        backpack_symbol.replace('_', "/")
    }

    fn parse_ticker(&self, data: &Value) -> WsTickerEvent {
        let symbol_id = data["s"].as_str().unwrap_or("");
        let symbol = Self::to_unified_symbol(symbol_id);
        let timestamp = data["E"]
            .as_i64()
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data["h"].as_str().and_then(|s| Decimal::from_str(s).ok()),
            low: data["l"].as_str().and_then(|s| Decimal::from_str(s).ok()),
            bid: data["b"].as_str().and_then(|s| Decimal::from_str(s).ok()),
            bid_volume: None,
            ask: data["a"].as_str().and_then(|s| Decimal::from_str(s).ok()),
            ask_volume: None,
            vwap: None,
            open: data["o"].as_str().and_then(|s| Decimal::from_str(s).ok()),
            close: data["c"].as_str().and_then(|s| Decimal::from_str(s).ok()),
            last: data["c"].as_str().and_then(|s| Decimal::from_str(s).ok()),
            previous_close: None,
            change: None,
            percentage: data["P"].as_str().and_then(|s| Decimal::from_str(s).ok()),
            average: None,
            base_volume: data["v"].as_str().and_then(|s| Decimal::from_str(s).ok()),
            quote_volume: data["V"].as_str().and_then(|s| Decimal::from_str(s).ok()),
            index_price: None,
            mark_price: None,
            info: data.clone(),
        };

        WsTickerEvent { symbol, ticker }
    }

    fn parse_trade(&self, data: &Value) -> WsTradeEvent {
        let symbol_id = data["s"].as_str().unwrap_or("");
        let symbol = Self::to_unified_symbol(symbol_id);
        let timestamp = data["E"]
            .as_i64()
            .or_else(|| data["T"].as_i64())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let price = data["p"]
            .as_str()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();
        let amount = data["q"]
            .as_str()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();
        let side = if data["m"].as_bool().unwrap_or(false) {
            "sell".to_string()
        } else {
            "buy".to_string()
        };

        let id = data["t"]
            .as_i64()
            .map(|i| i.to_string())
            .or_else(|| data["t"].as_str().map(|s| s.to_string()))
            .unwrap_or_default();

        let trade = Trade {
            id,
            order: None,
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp).map(|dt| dt.to_rfc3339()),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(side),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: vec![],
            info: data.clone(),
        };

        WsTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    fn parse_orderbook(&self, data: &Value, symbol: &str) -> WsOrderBookEvent {
        let symbol_id = data["s"].as_str().unwrap_or("");
        let unified_symbol = if symbol.is_empty() {
            Self::to_unified_symbol(symbol_id)
        } else {
            symbol.to_string()
        };

        let timestamp = data["E"]
            .as_i64()
            .or_else(|| data["u"].as_i64())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        if let Some(bid_arr) = data["b"].as_array() {
            for bid in bid_arr {
                if let Some(arr) = bid.as_array() {
                    if arr.len() >= 2 {
                        let price = arr[0]
                            .as_str()
                            .and_then(|s| Decimal::from_str(s).ok())
                            .unwrap_or_default();
                        let amount = arr[1]
                            .as_str()
                            .and_then(|s| Decimal::from_str(s).ok())
                            .unwrap_or_default();
                        if amount > Decimal::ZERO {
                            bids.push(OrderBookEntry { price, amount });
                        }
                    }
                }
            }
        }

        if let Some(ask_arr) = data["a"].as_array() {
            for ask in ask_arr {
                if let Some(arr) = ask.as_array() {
                    if arr.len() >= 2 {
                        let price = arr[0]
                            .as_str()
                            .and_then(|s| Decimal::from_str(s).ok())
                            .unwrap_or_default();
                        let amount = arr[1]
                            .as_str()
                            .and_then(|s| Decimal::from_str(s).ok())
                            .unwrap_or_default();
                        if amount > Decimal::ZERO {
                            asks.push(OrderBookEntry { price, amount });
                        }
                    }
                }
            }
        }

        // Sort: bids descending, asks ascending
        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data["u"].as_i64(),
            bids,
            asks,
            checksum: None,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot: true,
        }
    }

    async fn handle_message(&self, msg: &str, symbol: &str) -> Option<WsMessage> {
        let parsed: Value = serde_json::from_str(msg).ok()?;

        // Handle subscription responses
        if parsed.get("result").is_some() || parsed.get("id").is_some() {
            return None;
        }

        // Handle stream data with data field
        if let Some(data) = parsed.get("data") {
            let event_type = data["e"].as_str().unwrap_or("");

            match event_type {
                "ticker" | "24hrTicker" => {
                    return Some(WsMessage::Ticker(self.parse_ticker(data)));
                },
                "trade" => {
                    return Some(WsMessage::Trade(self.parse_trade(data)));
                },
                "depth" | "depthUpdate" => {
                    return Some(WsMessage::OrderBook(self.parse_orderbook(data, symbol)));
                },
                _ => {},
            }
        }

        // Handle stream wrapper format
        if let Some(stream) = parsed.get("stream") {
            let stream_name = stream.as_str().unwrap_or("");
            if let Some(data) = parsed.get("data") {
                if stream_name.starts_with("ticker.") {
                    return Some(WsMessage::Ticker(self.parse_ticker(data)));
                } else if stream_name.starts_with("trade.") {
                    return Some(WsMessage::Trade(self.parse_trade(data)));
                } else if stream_name.starts_with("depth.") {
                    return Some(WsMessage::OrderBook(self.parse_orderbook(data, symbol)));
                }
            }
        }

        None
    }

    async fn subscribe(
        &self,
        symbols: &[String],
        channel: &str,
        tx: mpsc::UnboundedSender<WsMessage>,
    ) -> CcxtResult<()> {
        let (ws_stream, _) = connect_async(WS_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: e.to_string(),
            })?;

        let (mut write, mut read) = ws_stream.split();

        // Build subscription params
        let mut params = Vec::new();
        for symbol in symbols {
            let market_id = Self::format_symbol(symbol);
            let topic = match channel {
                "ticker" => format!("ticker.{market_id}"),
                "trade" => format!("trade.{market_id}"),
                "depth" => format!("depth.{market_id}"),
                _ => continue,
            };
            params.push(topic);
        }

        // Send subscription message
        let subscribe_msg = json!({
            "method": "SUBSCRIBE",
            "params": params,
            "id": self.get_next_id()
        });

        write
            .send(Message::Text(subscribe_msg.to_string()))
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: e.to_string(),
            })?;

        let symbol = symbols.first().cloned().unwrap_or_default();
        let request_id = AtomicI64::new(self.request_id.load(Ordering::SeqCst));
        let subscriptions = Arc::new(RwLock::new(HashMap::new()));
        let orderbooks = Arc::new(RwLock::new(HashMap::new()));
        let connected = Arc::new(RwLock::new(true));

        let handler = BackpackWs {
            request_id,
            subscriptions,
            orderbooks,
            _connected: connected,
        };

        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Some(ws_msg) = handler.handle_message(&text, &symbol).await {
                            if tx.send(ws_msg).is_err() {
                                break;
                            }
                        }
                    },
                    Ok(Message::Ping(data)) => {
                        let _ = write.send(Message::Pong(data)).await;
                    },
                    Ok(Message::Close(_)) => break,
                    Err(_) => break,
                    _ => {},
                }
            }
        });

        Ok(())
    }
}

impl Default for BackpackWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for BackpackWs {
    fn clone(&self) -> Self {
        Self {
            request_id: AtomicI64::new(self.request_id.load(Ordering::SeqCst)),
            subscriptions: Arc::clone(&self.subscriptions),
            orderbooks: Arc::clone(&self.orderbooks),
            _connected: Arc::clone(&self._connected),
        }
    }
}

#[async_trait]
impl WsExchange for BackpackWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribe(&[symbol.to_string()], "ticker", tx).await?;
        Ok(rx)
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let symbols: Vec<String> = symbols.iter().map(|s| s.to_string()).collect();
        self.subscribe(&symbols, "ticker", tx).await?;
        Ok(rx)
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribe(&[symbol.to_string()], "depth", tx).await?;
        Ok(rx)
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribe(&[symbol.to_string()], "trade", tx).await?;
        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watch_ohlcv".to_string(),
        })
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        *self._connected.write().await = true;
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        *self._connected.write().await = false;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        *self._connected.read().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BackpackWs::format_symbol("BTC/USDC"), "BTC_USDC");
        assert_eq!(BackpackWs::format_symbol("ETH/USDC"), "ETH_USDC");
        assert_eq!(BackpackWs::format_symbol("SOL/USDC:USDC"), "SOL_USDC_PERP");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BackpackWs::to_unified_symbol("BTC_USDC"), "BTC/USDC");
        assert_eq!(
            BackpackWs::to_unified_symbol("ETH_USDC_PERP"),
            "ETH/USDC:USDC"
        );
    }

    #[test]
    fn test_new() {
        let ws = BackpackWs::new();
        assert_eq!(ws.request_id.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_clone() {
        let ws = BackpackWs::new();
        let _ = ws.get_next_id();
        let cloned = ws.clone();
        assert_eq!(cloned.request_id.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_default() {
        let ws = BackpackWs::default();
        assert_eq!(ws.request_id.load(Ordering::SeqCst), 1);
    }
}
