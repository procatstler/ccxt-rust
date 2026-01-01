//! Paradex WebSocket Implementation
//!
//! Paradex WebSocket API for real-time market data
//! URL: wss://ws.api.prod.paradex.trade/v1

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

const WS_URL: &str = "wss://ws.api.prod.paradex.trade/v1";

/// Paradex WebSocket client
pub struct ParadexWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
}

impl ParadexWs {
    /// Create a new Paradex WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Convert unified symbol to Paradex format
    /// BTC/USD:USD -> BTC-USD-PERP
    fn format_symbol(&self, symbol: &str) -> String {
        // Paradex uses format like BTC-USD-PERP
        if symbol.contains(":") {
            // Futures format: BTC/USD:USD -> BTC-USD-PERP
            let parts: Vec<&str> = symbol.split(':').collect();
            if let Some(base_quote) = parts.get(0) {
                let bq: Vec<&str> = base_quote.split('/').collect();
                if bq.len() == 2 {
                    return format!("{}-{}-PERP", bq[0], bq[1]);
                }
            }
        }
        // Spot or simple format
        symbol.replace("/", "-")
    }

    /// Convert Paradex symbol to unified format
    /// BTC-USD-PERP -> BTC/USD:USD
    fn parse_symbol(&self, symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('-').collect();
        if parts.len() >= 2 {
            if parts.len() >= 3 && parts[2] == "PERP" {
                // Perpetual: BTC-USD-PERP -> BTC/USD:USD
                return format!("{}/{}:{}", parts[0], parts[1], parts[1]);
            }
            // Spot: BTC-USD -> BTC/USD
            return format!("{}/{}", parts[0], parts[1]);
        }
        symbol.to_string()
    }

    /// Send a subscription message
    async fn subscribe(&self, channel: &str) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let msg = json!({
                "jsonrpc": "2.0",
                "method": "subscribe",
                "params": {
                    "channel": channel,
                },
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
        // Check for subscription method
        let method = data.get("method").and_then(|v| v.as_str()).unwrap_or("");

        if method != "subscription" {
            return;
        }

        // Get params and channel
        if let Some(params) = data.get("params") {
            let channel = params.get("channel").and_then(|v| v.as_str()).unwrap_or("");
            let parts: Vec<&str> = channel.split('.').collect();
            let channel_type = parts.get(0).copied().unwrap_or("");

            match channel_type {
                "trades" => {
                    Self::handle_trade(params, subscriptions).await;
                }
                "order_book" => {
                    Self::handle_orderbook(params, subscriptions, orderbook_cache).await;
                }
                "markets_summary" => {
                    Self::handle_ticker(params, subscriptions).await;
                }
                _ => {}
            }
        }
    }

    /// Handle trade message
    async fn handle_trade(
        params: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        if let Some(trade_data) = params.get("data") {
            let market_id = trade_data.get("market").and_then(|v| v.as_str()).unwrap_or("");
            let symbol = Self::parse_symbol_static(market_id);
            let key = format!("trades:{}", symbol);

            let timestamp = trade_data.get("created_at").and_then(|v| v.as_i64());
            let side = trade_data.get("side").and_then(|v| v.as_str())
                .map(|s| s.to_lowercase());

            let price: Decimal = trade_data.get("price").and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok()).unwrap_or_default();
            let amount: Decimal = trade_data.get("size").and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok()).unwrap_or_default();

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
            let trade = if let Some(ref s) = side {
                trade.with_side(s)
            } else {
                trade
            };

            let subs = subscriptions.read().await;
            if let Some(sender) = subs.get(&key) {
                let _ = sender.send(WsMessage::Trade(WsTradeEvent {
                    symbol: symbol.clone(),
                    trades: vec![trade.clone()],
                }));
            }

            // Also notify ALL channel subscribers
            let all_key = "trades:ALL".to_string();
            if let Some(sender) = subs.get(&all_key) {
                let _ = sender.send(WsMessage::Trade(WsTradeEvent {
                    symbol: symbol.clone(),
                    trades: vec![trade],
                }));
            }
        }
    }

    /// Handle orderbook message
    async fn handle_orderbook(
        params: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        if let Some(data) = params.get("data") {
            let market_id = data.get("market").and_then(|v| v.as_str()).unwrap_or("");
            let symbol = Self::parse_symbol_static(market_id);
            let key = format!("orderbook:{}", symbol);

            let timestamp = data.get("last_updated_at").and_then(|v| v.as_i64());
            let seq_no = data.get("seq_no").and_then(|v| v.as_i64());

            let mut bids: Vec<OrderBookEntry> = Vec::new();
            let mut asks: Vec<OrderBookEntry> = Vec::new();

            // Parse inserts
            if let Some(inserts) = data.get("inserts").and_then(|v| v.as_array()) {
                for insert in inserts {
                    let side = insert.get("side").and_then(|v| v.as_str()).unwrap_or("");
                    let price: Decimal = insert.get("price").and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok()).unwrap_or_default();
                    let size: Decimal = insert.get("size").and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok()).unwrap_or_default();

                    let entry = OrderBookEntry { price, amount: size };

                    if side == "BUY" {
                        bids.push(entry);
                    } else if side == "SELL" {
                        asks.push(entry);
                    }
                }
            }

            // Sort bids descending, asks ascending
            bids.sort_by(|a, b| b.price.cmp(&a.price));
            asks.sort_by(|a, b| a.price.cmp(&b.price));

            let orderbook = OrderBook {
                symbol: symbol.clone(),
                bids,
                asks,
                timestamp,
                datetime: None,
                nonce: seq_no,
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
                    is_snapshot: true,
                }));
            }
        }
    }

    /// Handle ticker message
    async fn handle_ticker(
        params: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        if let Some(data) = params.get("data") {
            let symbol_id = data.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let symbol = Self::parse_symbol_static(symbol_id);
            let key = format!("ticker:{}", symbol);

            let timestamp = data.get("created_at").and_then(|v| v.as_i64());

            let ticker = Ticker {
                symbol: symbol.clone(),
                bid: data.get("bid").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                ask: data.get("ask").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                last: data.get("last_traded_price").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                base_volume: data.get("volume_24h").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                timestamp,
                ..Default::default()
            };

            let subs = subscriptions.read().await;
            if let Some(sender) = subs.get(&key) {
                let _ = sender.send(WsMessage::Ticker(WsTickerEvent {
                    symbol: symbol.clone(),
                    ticker: ticker.clone(),
                }));
            }

            // Also notify markets_summary channel subscribers
            let summary_key = "ticker:markets_summary".to_string();
            if let Some(sender) = subs.get(&summary_key) {
                let _ = sender.send(WsMessage::Ticker(WsTickerEvent {
                    symbol: symbol.clone(),
                    ticker,
                }));
            }
        }
    }

    /// Static helper for parsing symbol
    fn parse_symbol_static(symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('-').collect();
        if parts.len() >= 2 {
            if parts.len() >= 3 && parts[2] == "PERP" {
                return format!("{}/{}:{}", parts[0], parts[1], parts[1]);
            }
            return format!("{}/{}", parts[0], parts[1]);
        }
        symbol.to_string()
    }
}

impl Default for ParadexWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for ParadexWs {
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

        // Subscribe to markets_summary channel
        self.subscribe("markets_summary").await?;

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
        }

        // Subscribe to markets_summary channel
        self.subscribe("markets_summary").await?;

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
        let channel = format!("order_book.{}.snapshot@15@100ms", market_id);
        self.subscribe(&channel).await?;

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
            let channel = format!("order_book.{}.snapshot@15@100ms", market_id);
            self.subscribe(&channel).await?;
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
        let channel = format!("trades.{}", market_id);
        self.subscribe(&channel).await?;

        Ok(rx)
    }

    async fn watch_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        // Subscribe to ALL trades channel
        {
            let mut subs = self.subscriptions.write().await;
            subs.insert("trades:ALL".to_string(), tx.clone());

            // Also register specific symbols
            for symbol in symbols {
                let key = format!("trades:{}", symbol);
                subs.insert(key, tx.clone());
            }
        }

        self.subscribe("trades.ALL").await?;

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Paradex does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "Paradex WebSocket does not support OHLCV".to_string(),
        })
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        _symbols: &[&str],
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Paradex does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "Paradex WebSocket does not support OHLCV".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paradex_ws_creation() {
        let _ws = ParadexWs::new();
    }

    #[test]
    fn test_format_symbol() {
        let ws = ParadexWs::new();
        assert_eq!(ws.format_symbol("BTC/USD:USD"), "BTC-USD-PERP");
        assert_eq!(ws.format_symbol("ETH/USD"), "ETH-USD");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = ParadexWs::new();
        assert_eq!(ws.parse_symbol("BTC-USD-PERP"), "BTC/USD:USD");
        assert_eq!(ws.parse_symbol("ETH-USD"), "ETH/USD");
    }
}
