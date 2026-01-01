//! ProBit WebSocket Implementation
//!
//! ProBit WebSocket API for real-time market data
//! URL: wss://api.probit.com/api/exchange/v1/ws

#![allow(dead_code)]

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

use rust_decimal::Decimal;

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage,
    WsTickerEvent, WsTradeEvent, WsOrderBookEvent,
};

const WS_URL: &str = "wss://api.probit.com/api/exchange/v1/ws";

/// ProBit WebSocket client
pub struct ProbitWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
}

impl ProbitWs {
    /// Create a new ProBit WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Convert unified symbol to ProBit format
    /// BTC/USDT -> BTC-USDT
    fn format_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Convert ProBit symbol to unified format
    /// BTC-USDT -> BTC/USDT
    fn parse_symbol(&self, symbol: &str) -> String {
        symbol.replace("-", "/")
    }

    /// Send a subscription message
    async fn subscribe(&self, market_id: &str, filters: Vec<&str>) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let msg = json!({
                "type": "subscribe",
                "channel": "marketdata",
                "market_id": market_id,
                "filter": filters,
                "interval": 100,
            });

            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(msg.to_string().into()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to send subscribe: {}", e),
                })?;
        }
        Ok(())
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
        let channel = data.get("channel").and_then(|v| v.as_str()).unwrap_or("");

        match channel {
            "marketdata" => {
                // Check what data is present
                if data.get("ticker").is_some() {
                    Self::handle_ticker(data, subscriptions).await;
                }
                if let Some(trades) = data.get("recent_trades") {
                    if trades.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                        Self::handle_trades(data, subscriptions).await;
                    }
                }
                if data.get("order_books").is_some()
                    || data.get("order_books_l2").is_some()
                    || data.get("order_books_l1").is_some()
                {
                    Self::handle_orderbook(data, subscriptions, orderbook_cache).await;
                }
            }
            _ => {}
        }
    }

    /// Handle ticker message
    async fn handle_ticker(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let market_id = data.get("market_id").and_then(|v| v.as_str()).unwrap_or("");
        let symbol = Self::parse_symbol_static(market_id);
        let key = format!("ticker:{}", symbol);

        if let Some(ticker_data) = data.get("ticker") {
            let time_str = ticker_data.get("time").and_then(|v| v.as_str());
            let timestamp = time_str.and_then(|ts| {
                chrono::DateTime::parse_from_rfc3339(ts)
                    .map(|dt| dt.timestamp_millis())
                    .ok()
            });

            let ticker = Ticker {
                symbol: symbol.clone(),
                high: ticker_data.get("high").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                low: ticker_data.get("low").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                last: ticker_data.get("last").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                base_volume: ticker_data.get("base_volume").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                quote_volume: ticker_data.get("quote_volume").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                timestamp,
                ..Default::default()
            };

            let subs = subscriptions.read().await;
            if let Some(sender) = subs.get(&key) {
                let _ = sender.send(WsMessage::Ticker(WsTickerEvent {
                    symbol: symbol.clone(),
                    ticker,
                }));
            }
        }
    }

    /// Handle trades message
    async fn handle_trades(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let market_id = data.get("market_id").and_then(|v| v.as_str()).unwrap_or("");
        let symbol = Self::parse_symbol_static(market_id);
        let key = format!("trades:{}", symbol);

        // Skip reset messages (initial snapshot)
        if data.get("reset").and_then(|v| v.as_bool()).unwrap_or(false) {
            return;
        }

        if let Some(trades) = data.get("recent_trades").and_then(|v| v.as_array()) {
            for trade_data in trades {
                let time_str = trade_data.get("time").and_then(|v| v.as_str());
                let timestamp = time_str.and_then(|ts| {
                    chrono::DateTime::parse_from_rfc3339(ts)
                        .map(|dt| dt.timestamp_millis())
                        .ok()
                });

                let price = trade_data.get("price").and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();
                let amount = trade_data.get("quantity").and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();

                let trade = Trade::new(
                    trade_data.get("id").and_then(|v| v.as_str())
                        .map(String::from).unwrap_or_default(),
                    symbol.clone(),
                    price,
                    amount,
                );
                let trade = if let Some(ts) = timestamp {
                    trade.with_timestamp(ts)
                } else {
                    trade
                };
                let trade = if let Some(s) = trade_data.get("side").and_then(|v| v.as_str()) {
                    trade.with_side(s)
                } else {
                    trade
                };

                let subs = subscriptions.read().await;
                if let Some(sender) = subs.get(&key) {
                    let _ = sender.send(WsMessage::Trade(WsTradeEvent {
                        symbol: symbol.clone(),
                        trades: vec![trade],
                    }));
                }
            }
        }
    }

    /// Handle orderbook message
    async fn handle_orderbook(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        let market_id = data.get("market_id").and_then(|v| v.as_str()).unwrap_or("");
        let symbol = Self::parse_symbol_static(market_id);
        let key = format!("orderbook:{}", symbol);

        // Get orderbook data from any of the possible keys
        let order_book_data = data.get("order_books")
            .or_else(|| data.get("order_books_l2"))
            .or_else(|| data.get("order_books_l1"))
            .or_else(|| data.get("order_books_l3"))
            .or_else(|| data.get("order_books_l4"));

        if let Some(entries) = order_book_data.and_then(|v| v.as_array()) {
            let reset = data.get("reset").and_then(|v| v.as_bool()).unwrap_or(false);

            let mut bids: Vec<OrderBookEntry> = Vec::new();
            let mut asks: Vec<OrderBookEntry> = Vec::new();

            for entry in entries {
                let side = entry.get("side").and_then(|v| v.as_str()).unwrap_or("");
                let price: Decimal = entry.get("price").and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()).unwrap_or_default();
                let quantity: Decimal = entry.get("quantity").and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()).unwrap_or_default();

                let ob_entry = OrderBookEntry { price, amount: quantity };

                if side == "buy" {
                    bids.push(ob_entry);
                } else if side == "sell" {
                    asks.push(ob_entry);
                }
            }

            // Sort bids descending, asks ascending
            bids.sort_by(|a, b| b.price.cmp(&a.price));
            asks.sort_by(|a, b| a.price.cmp(&b.price));

            let orderbook = if reset {
                OrderBook {
                    symbol: symbol.clone(),
                    bids,
                    asks,
                    timestamp: None,
                    datetime: None,
                    nonce: None,
                }
            } else {
                // For incremental updates, merge with cache
                let cache = orderbook_cache.read().await;
                if let Some(cached) = cache.get(&symbol) {
                    let mut new_ob = cached.clone();
                    Self::apply_deltas(&mut new_ob.bids, &bids, true);
                    Self::apply_deltas(&mut new_ob.asks, &asks, false);
                    new_ob
                } else {
                    OrderBook {
                        symbol: symbol.clone(),
                        bids,
                        asks,
                        timestamp: None,
                        datetime: None,
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
                    is_snapshot: reset,
                }));
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

    /// Static helper for parsing symbol
    fn parse_symbol_static(symbol: &str) -> String {
        symbol.replace("-", "/")
    }
}

impl Default for ProbitWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for ProbitWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        let (ws_stream, _) = connect_async(WS_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: format!("WebSocket connection failed: {}", e),
            })?;

        self.ws_stream = Some(Arc::new(RwLock::new(ws_stream)));
        self.start_message_loop();

        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let mut ws_guard = ws.write().await;
            ws_guard
                .close(None)
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to close WebSocket: {}", e),
                })?;
        }
        self.ws_stream = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        self.ws_stream.is_some()
    }

    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("ticker:{}", symbol);

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        self.subscribe(&market_id, vec!["ticker"]).await?;

        Ok(rx)
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("ticker:{}", symbol);
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }

            let market_id = self.format_symbol(symbol);
            self.subscribe(&market_id, vec!["ticker"]).await?;
        }

        Ok(rx)
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("orderbook:{}", symbol);

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        self.subscribe(&market_id, vec!["order_books_l2"]).await?;

        Ok(rx)
    }

    async fn watch_order_book_for_symbols(
        &self,
        symbols: &[&str],
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("orderbook:{}", symbol);
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }

            let market_id = self.format_symbol(symbol);
            self.subscribe(&market_id, vec!["order_books_l2"]).await?;
        }

        Ok(rx)
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("trades:{}", symbol);

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        self.subscribe(&market_id, vec!["recent_trades"]).await?;

        Ok(rx)
    }

    async fn watch_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("trades:{}", symbol);
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }

            let market_id = self.format_symbol(symbol);
            self.subscribe(&market_id, vec!["recent_trades"]).await?;
        }

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // ProBit does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "ProBit WebSocket does not support OHLCV".to_string(),
        })
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        _symbols: &[&str],
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // ProBit does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "ProBit WebSocket does not support OHLCV".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_probit_ws_creation() {
        let _ws = ProbitWs::new();
    }

    #[test]
    fn test_format_symbol() {
        let ws = ProbitWs::new();
        assert_eq!(ws.format_symbol("BTC/USDT"), "BTC-USDT");
        assert_eq!(ws.format_symbol("ETH/BTC"), "ETH-BTC");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = ProbitWs::new();
        assert_eq!(ws.parse_symbol("BTC-USDT"), "BTC/USDT");
        assert_eq!(ws.parse_symbol("ETH-BTC"), "ETH/BTC");
    }
}
