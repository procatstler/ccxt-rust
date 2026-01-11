//! NDAX WebSocket Implementation
//!
//! NDAX WebSocket API for real-time market data
//! URL: wss://api.ndax.io/WSGateway

#![allow(dead_code)]

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage,
    WsTickerEvent, WsTradeEvent, WsOrderBookEvent,
};

const WS_URL: &str = "wss://api.ndax.io/WSGateway";
const OMS_ID: u32 = 1;

/// NDAX WebSocket client
pub struct NdaxWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    request_id: Arc<AtomicU64>,
    symbol_to_instrument: Arc<RwLock<HashMap<String, u32>>>,
    instrument_to_symbol: Arc<RwLock<HashMap<u32, String>>>,
}

impl NdaxWs {
    /// Create a new NDAX WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            request_id: Arc::new(AtomicU64::new(1)),
            symbol_to_instrument: Arc::new(RwLock::new(HashMap::new())),
            instrument_to_symbol: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get next request ID
    fn next_request_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Convert unified symbol to NDAX format
    /// BTC/USDT -> BTCUSDT
    fn format_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// Convert NDAX symbol to unified format
    /// BTCUSDT -> BTC/USDT
    fn parse_symbol(&self, symbol: &str) -> String {
        // For proper parsing, we need market info
        // For now, use a simple heuristic
        if let Some(base) = symbol.strip_suffix("USDT") {
            format!("{base}/USDT")
        } else if let Some(base) = symbol.strip_suffix("USD") {
            format!("{base}/USD")
        } else if let Some(base) = symbol.strip_suffix("BTC") {
            format!("{base}/BTC")
        } else {
            symbol.to_string()
        }
    }

    /// Get instrument ID for symbol
    async fn get_instrument_id(&self, symbol: &str) -> Option<u32> {
        let map = self.symbol_to_instrument.read().await;
        map.get(symbol).copied()
    }

    /// Register instrument ID for symbol
    async fn register_instrument(&self, symbol: String, instrument_id: u32) {
        {
            let mut map = self.symbol_to_instrument.write().await;
            map.insert(symbol.clone(), instrument_id);
        }
        {
            let mut map = self.instrument_to_symbol.write().await;
            map.insert(instrument_id, symbol);
        }
    }

    /// Get symbol for instrument ID
    async fn get_symbol_for_instrument(&self, instrument_id: u32) -> Option<String> {
        let map = self.instrument_to_symbol.read().await;
        map.get(&instrument_id).cloned()
    }

    /// Send a WebSocket message
    async fn send_message(&self, msg: Value) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(msg.to_string()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to send message: {e}"),
                })?;
        }
        Ok(())
    }

    /// Subscribe to Level 2 order book
    async fn subscribe_level2(&self, symbol: &str, instrument_id: u32, depth: u32) -> CcxtResult<()> {
        let request_id = self.next_request_id();
        let payload = json!({
            "OMSId": OMS_ID,
            "InstrumentId": instrument_id,
            "Depth": depth
        });

        let msg = json!({
            "m": 0,
            "i": request_id,
            "n": "SubscribeLevel2",
            "o": payload.to_string()
        });

        self.send_message(msg).await?;
        self.register_instrument(symbol.to_string(), instrument_id).await;
        Ok(())
    }

    /// Subscribe to Level 1 ticker
    async fn subscribe_level1(&self, symbol: &str, instrument_id: u32) -> CcxtResult<()> {
        let request_id = self.next_request_id();
        let payload = json!({
            "OMSId": OMS_ID,
            "InstrumentId": instrument_id
        });

        let msg = json!({
            "m": 0,
            "i": request_id,
            "n": "SubscribeLevel1",
            "o": payload.to_string()
        });

        self.send_message(msg).await?;
        self.register_instrument(symbol.to_string(), instrument_id).await;
        Ok(())
    }

    /// Subscribe to trades
    async fn subscribe_trades(&self, symbol: &str, instrument_id: u32) -> CcxtResult<()> {
        let request_id = self.next_request_id();
        let payload = json!({
            "OMSId": OMS_ID,
            "InstrumentId": instrument_id,
            "IncludeLastCount": 100
        });

        let msg = json!({
            "m": 0,
            "i": request_id,
            "n": "SubscribeTrades",
            "o": payload.to_string()
        });

        self.send_message(msg).await?;
        self.register_instrument(symbol.to_string(), instrument_id).await;
        Ok(())
    }

    /// Subscribe to ticker
    async fn subscribe_ticker(&self, symbol: &str, instrument_id: u32) -> CcxtResult<()> {
        let request_id = self.next_request_id();
        let payload = json!({
            "OMSId": OMS_ID,
            "InstrumentId": instrument_id,
            "Interval": 60,
            "IncludeLastCount": 1
        });

        let msg = json!({
            "m": 0,
            "i": request_id,
            "n": "SubscribeTicker",
            "o": payload.to_string()
        });

        self.send_message(msg).await?;
        self.register_instrument(symbol.to_string(), instrument_id).await;
        Ok(())
    }

    /// Start message processing loop
    fn start_message_loop(&self) {
        let ws_stream = self.ws_stream.clone();
        let subscriptions = self.subscriptions.clone();
        let orderbook_cache = self.orderbook_cache.clone();
        let instrument_to_symbol = self.instrument_to_symbol.clone();

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
                                Self::handle_message_static(
                                    &data,
                                    &subscriptions,
                                    &orderbook_cache,
                                    &instrument_to_symbol,
                                ).await;
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
        instrument_to_symbol: &Arc<RwLock<HashMap<u32, String>>>,
    ) {
        let msg_type = data.get("m").and_then(|v| v.as_u64()).unwrap_or(0);
        let endpoint = data.get("n").and_then(|v| v.as_str()).unwrap_or("");

        match msg_type {
            // Message type 3 is event
            3 => {
                match endpoint {
                    "Level2UpdateEvent" => {
                        Self::handle_orderbook_update(data, subscriptions, orderbook_cache, instrument_to_symbol).await;
                    }
                    "Level1UpdateEvent" => {
                        Self::handle_level1_update(data, subscriptions, instrument_to_symbol).await;
                    }
                    "TradeDataUpdateEvent" => {
                        Self::handle_trade_update(data, subscriptions, instrument_to_symbol).await;
                    }
                    "TickerDataUpdateEvent" => {
                        Self::handle_ticker_update(data, subscriptions, instrument_to_symbol).await;
                    }
                    _ => {}
                }
            }
            // Message type 1 is response
            1 => {
                // Handle subscription responses if needed
            }
            _ => {}
        }
    }

    /// Handle Level 2 order book update
    async fn handle_orderbook_update(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
        instrument_to_symbol: &Arc<RwLock<HashMap<u32, String>>>,
    ) {
        if let Some(payload_str) = data.get("o").and_then(|v| v.as_str()) {
            if let Ok(payload) = serde_json::from_str::<Value>(payload_str) {
                if let Some(updates) = payload.as_array() {
                    for update_data in updates {
                        if let Some(update) = update_data.as_array() {
                            if update.len() >= 10 {
                                // [MDUpdateId, NumAccounts, ActionDateTime, ActionType, LastTradePrice, NumOrders, Price, ProductPairCode, Quantity, Side]
                                let instrument_id = update[7].as_u64().unwrap_or(0) as u32;
                                let symbol_opt = {
                                    let map = instrument_to_symbol.read().await;
                                    map.get(&instrument_id).cloned()
                                };

                                if let Some(symbol) = symbol_opt {
                                    let action_type = update[3].as_u64().unwrap_or(0);
                                    let price: Decimal = update[6].as_f64()
                                        .and_then(Decimal::from_f64)
                                        .unwrap_or_default();
                                    let quantity: Decimal = update[8].as_f64()
                                        .and_then(Decimal::from_f64)
                                        .unwrap_or_default();
                                    let side = update[9].as_u64().unwrap_or(0);

                                    let key = format!("orderbook:{symbol}");
                                    let is_snapshot = action_type == 2; // 2 = New, 0 = Update, 1 = Delete

                                    let mut cache = orderbook_cache.write().await;
                                    let orderbook = cache.entry(symbol.clone()).or_insert_with(|| {
                                        OrderBook {
                                            symbol: symbol.clone(),
                                            bids: Vec::new(),
                                            asks: Vec::new(),
                                            timestamp: None,
                                            datetime: None,
                                            nonce: None,
                                        }
                                    });

                                    // Update orderbook based on action type
                                    let book_side = if side == 0 { &mut orderbook.bids } else { &mut orderbook.asks };

                                    match action_type {
                                        0 | 2 => {
                                            // Update or New
                                            if quantity.is_zero() {
                                                // Remove entry
                                                book_side.retain(|e| e.price != price);
                                            } else {
                                                // Update or add entry
                                                if let Some(entry) = book_side.iter_mut().find(|e| e.price == price) {
                                                    entry.amount = quantity;
                                                } else {
                                                    book_side.push(OrderBookEntry { price, amount: quantity });
                                                }
                                            }
                                        }
                                        1 => {
                                            // Delete
                                            book_side.retain(|e| e.price != price);
                                        }
                                        _ => {}
                                    }

                                    // Sort
                                    if side == 0 {
                                        orderbook.bids.sort_by(|a, b| b.price.cmp(&a.price));
                                    } else {
                                        orderbook.asks.sort_by(|a, b| a.price.cmp(&b.price));
                                    }

                                    let orderbook_clone = orderbook.clone();
                                    drop(cache);

                                    let subs = subscriptions.read().await;
                                    if let Some(sender) = subs.get(&key) {
                                        let _ = sender.send(WsMessage::OrderBook(WsOrderBookEvent {
                                            symbol: symbol.clone(),
                                            order_book: orderbook_clone,
                                            is_snapshot,
                                        }));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Handle Level 1 ticker update
    async fn handle_level1_update(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        instrument_to_symbol: &Arc<RwLock<HashMap<u32, String>>>,
    ) {
        if let Some(payload_str) = data.get("o").and_then(|v| v.as_str()) {
            if let Ok(payload) = serde_json::from_str::<Value>(payload_str) {
                let instrument_id = payload.get("InstrumentId").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let symbol_opt = {
                    let map = instrument_to_symbol.read().await;
                    map.get(&instrument_id).cloned()
                };

                if let Some(symbol) = symbol_opt {
                    let key = format!("ticker:{symbol}");

                    let ticker = Ticker {
                        symbol: symbol.clone(),
                        last: payload.get("LastTradedPx").and_then(|v| v.as_f64())
                            .and_then(Decimal::from_f64),
                        bid: payload.get("BestBid").and_then(|v| v.as_f64())
                            .and_then(Decimal::from_f64),
                        ask: payload.get("BestOffer").and_then(|v| v.as_f64())
                            .and_then(Decimal::from_f64),
                        high: payload.get("High").and_then(|v| v.as_f64())
                            .and_then(Decimal::from_f64),
                        low: payload.get("Low").and_then(|v| v.as_f64())
                            .and_then(Decimal::from_f64),
                        base_volume: payload.get("Volume").and_then(|v| v.as_f64())
                            .and_then(Decimal::from_f64),
                        timestamp: payload.get("CurrentDayPxChangeTime").and_then(|v| v.as_i64()),
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
        }
    }

    /// Handle trade update
    async fn handle_trade_update(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        instrument_to_symbol: &Arc<RwLock<HashMap<u32, String>>>,
    ) {
        if let Some(payload_str) = data.get("o").and_then(|v| v.as_str()) {
            if let Ok(payload) = serde_json::from_str::<Value>(payload_str) {
                if let Some(trades_data) = payload.as_array() {
                    for trade_data in trades_data {
                        if let Some(trade_array) = trade_data.as_array() {
                            if trade_array.len() >= 11 {
                                // [TradeId, ProductPairCode, Quantity, Price, Order1, Order2, Tradetime, Direction, TakerSide, BlockTrade, ClientId]
                                let instrument_id = trade_array[1].as_u64().unwrap_or(0) as u32;
                                let symbol_opt = {
                                    let map = instrument_to_symbol.read().await;
                                    map.get(&instrument_id).cloned()
                                };

                                if let Some(symbol) = symbol_opt {
                                    let key = format!("trades:{symbol}");

                                    let trade_id = trade_array[0].as_u64().map(|id| id.to_string()).unwrap_or_default();
                                    let quantity: Decimal = trade_array[2].as_f64()
                                        .and_then(Decimal::from_f64)
                                        .unwrap_or_default();
                                    let price: Decimal = trade_array[3].as_f64()
                                        .and_then(Decimal::from_f64)
                                        .unwrap_or_default();
                                    let timestamp = trade_array[6].as_i64();
                                    let _direction = trade_array[7].as_u64().unwrap_or(0);
                                    let taker_side = trade_array[8].as_u64().unwrap_or(0);

                                    let side = if taker_side == 0 { "buy" } else { "sell" };

                                    let mut trade = Trade::new(trade_id, symbol.clone(), price, quantity);
                                    trade = trade.with_side(side);
                                    if let Some(ts) = timestamp {
                                        trade = trade.with_timestamp(ts);
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
                        }
                    }
                }
            }
        }
    }

    /// Handle ticker update
    async fn handle_ticker_update(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        instrument_to_symbol: &Arc<RwLock<HashMap<u32, String>>>,
    ) {
        if let Some(payload_str) = data.get("o").and_then(|v| v.as_str()) {
            if let Ok(payload) = serde_json::from_str::<Value>(payload_str) {
                if let Some(ticker_data) = payload.as_array() {
                    for ticker_array in ticker_data {
                        if let Some(ticker) = ticker_array.as_array() {
                            if ticker.len() >= 10 {
                                // Ticker data format: [timestamp, high, low, open, close, volume, ...]
                                let instrument_id = ticker.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                                let symbol_opt = {
                                    let map = instrument_to_symbol.read().await;
                                    map.get(&instrument_id).cloned()
                                };

                                if let Some(symbol) = symbol_opt {
                                    let key = format!("ticker:{symbol}");

                                    let timestamp = ticker[0].as_i64();
                                    let high: Option<Decimal> = ticker[2].as_f64().and_then(Decimal::from_f64);
                                    let low: Option<Decimal> = ticker[3].as_f64().and_then(Decimal::from_f64);
                                    let open: Option<Decimal> = ticker[4].as_f64().and_then(Decimal::from_f64);
                                    let close: Option<Decimal> = ticker[5].as_f64().and_then(Decimal::from_f64);
                                    let volume: Option<Decimal> = ticker[6].as_f64().and_then(Decimal::from_f64);

                                    let ticker_obj = Ticker {
                                        symbol: symbol.clone(),
                                        timestamp,
                                        high,
                                        low,
                                        open,
                                        last: close,
                                        close,
                                        base_volume: volume,
                                        ..Default::default()
                                    };

                                    let subs = subscriptions.read().await;
                                    if let Some(sender) = subs.get(&key) {
                                        let _ = sender.send(WsMessage::Ticker(WsTickerEvent {
                                            symbol: symbol.clone(),
                                            ticker: ticker_obj,
                                        }));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Static helper for parsing symbol
    fn parse_symbol_static(symbol: &str) -> String {
        if let Some(base) = symbol.strip_suffix("USDT") {
            format!("{base}/USDT")
        } else if let Some(base) = symbol.strip_suffix("USD") {
            format!("{base}/USD")
        } else if let Some(base) = symbol.strip_suffix("BTC") {
            format!("{base}/BTC")
        } else {
            symbol.to_string()
        }
    }
}

impl Default for NdaxWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for NdaxWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        let (ws_stream, _) = connect_async(WS_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: format!("WebSocket connection failed: {e}"),
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
                    message: format!("Failed to close WebSocket: {e}"),
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
        let key = format!("ticker:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        // For demo purposes, using instrument ID 8 for BTC/USDT
        // In production, you would need to fetch the actual instrument ID from the REST API
        let instrument_id = 8;
        self.subscribe_ticker(symbol, instrument_id).await?;

        Ok(rx)
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("ticker:{symbol}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }

            let instrument_id = 8; // Demo value
            self.subscribe_ticker(symbol, instrument_id).await?;
        }

        Ok(rx)
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("orderbook:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let depth = limit.unwrap_or(100);
        let instrument_id = 8; // Demo value
        self.subscribe_level2(symbol, instrument_id, depth).await?;

        Ok(rx)
    }

    async fn watch_order_book_for_symbols(
        &self,
        symbols: &[&str],
        limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("orderbook:{symbol}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }

            let depth = limit.unwrap_or(100);
            let instrument_id = 8; // Demo value
            self.subscribe_level2(symbol, instrument_id, depth).await?;
        }

        Ok(rx)
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("trades:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let instrument_id = 8; // Demo value
        self.subscribe_trades(symbol, instrument_id).await?;

        Ok(rx)
    }

    async fn watch_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("trades:{symbol}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }

            let instrument_id = 8; // Demo value
            self.subscribe_trades(symbol, instrument_id).await?;
        }

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // NDAX supports ticker data which includes OHLCV-like information
        // but not traditional OHLCV subscription
        Err(CcxtError::NotSupported {
            feature: "NDAX WebSocket does not support dedicated OHLCV streams".to_string(),
        })
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        _symbols: &[&str],
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "NDAX WebSocket does not support dedicated OHLCV streams".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ndax_ws_creation() {
        let _ws = NdaxWs::new();
    }

    #[test]
    fn test_format_symbol() {
        let ws = NdaxWs::new();
        assert_eq!(ws.format_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(ws.format_symbol("ETH/BTC"), "ETHBTC");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = NdaxWs::new();
        assert_eq!(ws.parse_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(ws.parse_symbol("ETHBTC"), "ETH/BTC");
        assert_eq!(ws.parse_symbol("BTCUSD"), "BTC/USD");
    }

    #[test]
    fn test_request_id_increment() {
        let ws = NdaxWs::new();
        let id1 = ws.next_request_id();
        let id2 = ws.next_request_id();
        assert_eq!(id1 + 1, id2);
    }
}
