//! IndependentReserve WebSocket Implementation
//!
//! IndependentReserve WebSocket API for real-time market data
//! URL: wss://websockets.independentreserve.com

#![allow(dead_code)]

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Timeframe, Trade, WsExchange, WsMessage,
    WsTradeEvent, WsOrderBookEvent,
};

const WS_URL_BASE: &str = "wss://websockets.independentreserve.com";

/// IndependentReserve WebSocket client
pub struct IndependentReserveWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
}

impl IndependentReserveWs {
    /// Create a new IndependentReserve WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Parse symbol to get base and quote currencies
    /// BTC/AUD -> (btc, aud)
    fn parse_symbol_parts(&self, symbol: &str) -> (String, String) {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            (parts[0].to_lowercase(), parts[1].to_lowercase())
        } else {
            ("".to_string(), "".to_string())
        }
    }

    /// Convert market ID to unified symbol
    /// btc-aud -> BTC/AUD
    fn market_id_to_symbol(&self, market_id: &str) -> String {
        let parts: Vec<&str> = market_id.split('-').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            market_id.to_uppercase()
        }
    }

    /// Start message processing loop
    fn start_message_loop(&self) {
        let ws_stream = self.ws_stream.clone();
        let subscriptions = self.subscriptions.clone();
        let orderbook_cache = self.orderbook_cache.clone();

        tokio::spawn(async move {
            if let Some(ws) = ws_stream {
                loop {
                    let msg = {
                        let mut ws_guard = ws.write().await;
                        ws_guard.next().await
                    };

                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(data) = serde_json::from_str::<Value>(&text) {
                                Self::handle_message_static(&data, &subscriptions, &orderbook_cache).await;
                            }
                        }
                        Some(Ok(Message::Ping(data))) => {
                            let mut ws_guard = ws.write().await;
                            let _ = ws_guard.send(Message::Pong(data)).await;
                        }
                        Some(Ok(Message::Close(_))) => break,
                        Some(Err(_)) => break,
                        None => break,
                        _ => {}
                    }
                }
            }
        });
    }

    /// Handle incoming WebSocket messages
    async fn handle_message_static(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        let event = data.get("Event").and_then(|v| v.as_str()).unwrap_or("");

        match event {
            "Trade" => {
                Self::handle_trade(data, subscriptions).await;
            }
            "OrderBookSnapshot" | "OrderBookChange" => {
                Self::handle_orderbook(data, subscriptions, orderbook_cache, event == "OrderBookSnapshot").await;
            }
            "Heartbeat" => {
                // Heartbeat - no action needed
            }
            "Subscriptions" => {
                // Subscriptions confirmation - no action needed
            }
            _ => {}
        }
    }

    /// Handle trade message
    async fn handle_trade(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        if let Some(trade_data) = data.get("Data") {
            let market_id = trade_data.get("Pair").and_then(|v| v.as_str()).unwrap_or("");
            let symbol = Self::market_id_to_symbol_static(market_id);
            let key = format!("trades:{symbol}");

            // Parse trade
            let trade_id = trade_data.get("TradeGuid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let price = trade_data.get("Price")
                .and_then(|v| v.as_f64())
                .map(|f| Decimal::from_f64(f).unwrap_or_default())
                .unwrap_or_default();

            let amount = trade_data.get("Volume")
                .and_then(|v| v.as_f64())
                .map(|f| Decimal::from_f64(f).unwrap_or_default())
                .unwrap_or_default();

            let mut trade = Trade::new(trade_id, symbol.clone(), price, amount);

            // Parse timestamp from TradeDate (ISO 8601)
            if let Some(date_str) = trade_data.get("TradeDate").and_then(|v| v.as_str()) {
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(date_str) {
                    trade = trade.with_timestamp(dt.timestamp_millis());
                }
            }

            // Parse side (Buy/Sell)
            if let Some(side) = trade_data.get("Side").and_then(|v| v.as_str()) {
                trade = trade.with_side(&side.to_lowercase());
            }

            let subs = subscriptions.read().await;
            if let Some(sender) = subs.get(&key) {
                let _ = sender.send(WsMessage::Trade(WsTradeEvent {
                    symbol: symbol.clone(),
                    trades: vec![trade],
                }));
            }
        }
    }

    /// Handle orderbook message
    async fn handle_orderbook(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
        is_snapshot: bool,
    ) {
        // Parse channel to get symbol and limit
        // Format: "orderbook/100/btc/aud"
        let channel = data.get("Channel").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = channel.split('/').collect();

        if parts.len() >= 4 {
            let limit = parts[1];
            let base = parts[2];
            let quote = parts[3];
            let symbol = format!("{}/{}", base.to_uppercase(), quote.to_uppercase());
            let key = format!("orderbook:{symbol}:{limit}");

            if let Some(ob_data) = data.get("Data") {
                let timestamp = data.get("Time").and_then(|v| v.as_i64());

                let mut bids: Vec<OrderBookEntry> = Vec::new();
                let mut asks: Vec<OrderBookEntry> = Vec::new();

                // Parse bids
                if let Some(bids_array) = ob_data.get("Bids").and_then(|v| v.as_array()) {
                    for bid in bids_array {
                        let price = bid.get("Price")
                            .and_then(|v| v.as_f64())
                            .map(|f| Decimal::from_f64(f).unwrap_or_default())
                            .unwrap_or_default();
                        let amount = bid.get("Volume")
                            .and_then(|v| v.as_f64())
                            .map(|f| Decimal::from_f64(f).unwrap_or_default())
                            .unwrap_or_default();

                        if !price.is_zero() {
                            bids.push(OrderBookEntry { price, amount });
                        }
                    }
                }

                // Parse asks (Offers)
                if let Some(asks_array) = ob_data.get("Offers").and_then(|v| v.as_array()) {
                    for ask in asks_array {
                        let price = ask.get("Price")
                            .and_then(|v| v.as_f64())
                            .map(|f| Decimal::from_f64(f).unwrap_or_default())
                            .unwrap_or_default();
                        let amount = ask.get("Volume")
                            .and_then(|v| v.as_f64())
                            .map(|f| Decimal::from_f64(f).unwrap_or_default())
                            .unwrap_or_default();

                        if !price.is_zero() {
                            asks.push(OrderBookEntry { price, amount });
                        }
                    }
                }

                // Sort bids descending, asks ascending
                bids.sort_by(|a, b| b.price.cmp(&a.price));
                asks.sort_by(|a, b| a.price.cmp(&b.price));

                let orderbook = if is_snapshot {
                    // Full snapshot
                    
                    OrderBook {
                        symbol: symbol.clone(),
                        bids,
                        asks,
                        timestamp,
                        datetime: timestamp.map(|ts| {
                            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ts)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_default()
                        }),
                        nonce: None,
                    }
                } else {
                    // Delta update - merge with cache
                    let cache = orderbook_cache.read().await;
                    if let Some(cached) = cache.get(&symbol) {
                        let mut new_ob = cached.clone();
                        Self::apply_deltas(&mut new_ob.bids, &bids, true);
                        Self::apply_deltas(&mut new_ob.asks, &asks, false);
                        new_ob.timestamp = timestamp;
                        new_ob.datetime = timestamp.map(|ts| {
                            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ts)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_default()
                        });
                        new_ob
                    } else {
                        // No cache, treat as snapshot
                        OrderBook {
                            symbol: symbol.clone(),
                            bids,
                            asks,
                            timestamp,
                            datetime: timestamp.map(|ts| {
                                chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ts)
                                    .map(|dt| dt.to_rfc3339())
                                    .unwrap_or_default()
                            }),
                            nonce: None,
                        }
                    }
                };

                // Update cache
                {
                    let mut cache = orderbook_cache.write().await;
                    cache.insert(symbol.clone(), orderbook.clone());
                }

                let subs = subscriptions.read().await;
                if let Some(sender) = subs.get(&key) {
                    let _ = sender.send(WsMessage::OrderBook(WsOrderBookEvent {
                        symbol: symbol.clone(),
                        order_book: orderbook,
                        is_snapshot,
                    }));
                }
            }
        }
    }

    /// Apply delta updates to orderbook side
    fn apply_deltas(book_side: &mut Vec<OrderBookEntry>, updates: &Vec<OrderBookEntry>, is_bid: bool) {
        for update in updates {
            // Find existing entry at this price
            let pos = book_side.iter().position(|e| e.price == update.price);

            if update.amount.is_zero() {
                // Remove entry
                if let Some(idx) = pos {
                    book_side.remove(idx);
                }
            } else if let Some(idx) = pos {
                // Update existing entry
                book_side[idx].amount = update.amount;
            } else {
                // Insert new entry
                book_side.push(update.clone());
            }
        }

        // Re-sort
        if is_bid {
            book_side.sort_by(|a, b| b.price.cmp(&a.price));
        } else {
            book_side.sort_by(|a, b| a.price.cmp(&b.price));
        }
    }

    /// Static helper for market ID to symbol conversion
    fn market_id_to_symbol_static(market_id: &str) -> String {
        let parts: Vec<&str> = market_id.split('-').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            market_id.to_uppercase()
        }
    }
}

impl Default for IndependentReserveWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for IndependentReserveWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // IndependentReserve uses different URLs for different subscriptions
        // We'll connect when subscribing to specific channels
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let mut ws_guard = ws.write().await;
            ws_guard
                .close(None)
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL_BASE.to_string(),
                    message: format!("Failed to close WebSocket: {e}"),
                })?;
        }
        self.ws_stream = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        self.ws_stream.is_some()
    }

    async fn watch_ticker(&self, _symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "IndependentReserve WebSocket does not support ticker".to_string(),
        })
    }

    async fn watch_tickers(&self, _symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "IndependentReserve WebSocket does not support tickers".to_string(),
        })
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (base, quote) = self.parse_symbol_parts(symbol);
        let limit_str = limit.unwrap_or(100).to_string();

        // URL format: wss://websockets.independentreserve.com/orderbook/{limit}?subscribe={base}-{quote}
        let url = format!("{WS_URL_BASE}/orderbook/{limit_str}?subscribe={base}-{quote}");

        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("orderbook:{symbol}:{limit_str}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        // Connect to WebSocket
        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: format!("WebSocket connection failed: {e}"),
            })?;

        // Store the connection (note: this will overwrite previous connections)
        // In a production system, you'd want to manage multiple connections
        let ws_arc = Arc::new(RwLock::new(ws_stream));

        // Store only if we don't have a connection yet
        if self.ws_stream.is_none() {
            // This is a workaround - in reality we need better multi-connection management
            let subscriptions = self.subscriptions.clone();
            let orderbook_cache = self.orderbook_cache.clone();

            tokio::spawn(async move {
                loop {
                    let msg = {
                        let mut ws_guard = ws_arc.write().await;
                        ws_guard.next().await
                    };

                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(data) = serde_json::from_str::<Value>(&text) {
                                Self::handle_message_static(&data, &subscriptions, &orderbook_cache).await;
                            }
                        }
                        Some(Ok(Message::Ping(data))) => {
                            let mut ws_guard = ws_arc.write().await;
                            let _ = ws_guard.send(Message::Pong(data)).await;
                        }
                        Some(Ok(Message::Close(_))) => break,
                        Some(Err(_)) => break,
                        None => break,
                        _ => {}
                    }
                }
            });
        }

        Ok(rx)
    }

    async fn watch_order_book_for_symbols(
        &self,
        _symbols: &[&str],
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "IndependentReserve WebSocket does not support watching multiple orderbooks".to_string(),
        })
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (base, quote) = self.parse_symbol_parts(symbol);

        // URL format: wss://websockets.independentreserve.com?subscribe=ticker-{base}-{quote}
        let url = format!("{WS_URL_BASE}?subscribe=ticker-{base}-{quote}");

        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("trades:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        // Connect to WebSocket
        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: format!("WebSocket connection failed: {e}"),
            })?;

        let ws_arc = Arc::new(RwLock::new(ws_stream));

        // Start message loop for this connection
        let subscriptions = self.subscriptions.clone();
        let orderbook_cache = self.orderbook_cache.clone();

        tokio::spawn(async move {
            loop {
                let msg = {
                    let mut ws_guard = ws_arc.write().await;
                    ws_guard.next().await
                };

                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(data) = serde_json::from_str::<Value>(&text) {
                            Self::handle_message_static(&data, &subscriptions, &orderbook_cache).await;
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let mut ws_guard = ws_arc.write().await;
                        let _ = ws_guard.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) => break,
                    Some(Err(_)) => break,
                    None => break,
                    _ => {}
                }
            }
        });

        Ok(rx)
    }

    async fn watch_trades_for_symbols(
        &self,
        _symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "IndependentReserve WebSocket does not support watching multiple trades".to_string(),
        })
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "IndependentReserve WebSocket does not support OHLCV".to_string(),
        })
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        _symbols: &[&str],
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "IndependentReserve WebSocket does not support OHLCV".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_independentreserve_ws_creation() {
        let _ws = IndependentReserveWs::new();
    }

    #[test]
    fn test_parse_symbol_parts() {
        let ws = IndependentReserveWs::new();
        let (base, quote) = ws.parse_symbol_parts("BTC/AUD");
        assert_eq!(base, "btc");
        assert_eq!(quote, "aud");
    }

    #[test]
    fn test_market_id_to_symbol() {
        let ws = IndependentReserveWs::new();
        assert_eq!(ws.market_id_to_symbol("btc-aud"), "BTC/AUD");
        assert_eq!(ws.market_id_to_symbol("eth-usd"), "ETH/USD");
    }
}
