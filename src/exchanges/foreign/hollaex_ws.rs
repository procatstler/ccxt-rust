//! HollaEx WebSocket Implementation
//!
//! HollaEx WebSocket API for real-time market data
//! URL: wss://api.hollaex.com/stream

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
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
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage,
    WsOrderEvent, WsOrderBookEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_URL: &str = "wss://api.hollaex.com/stream";

/// HollaEx WebSocket client
pub struct HollaexWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    private_ws_stream: Arc<RwLock<Option<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    private_subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    api_key: Option<String>,
    api_secret: Option<String>,
    is_authenticated: Arc<RwLock<bool>>,
}

impl HollaexWs {
    /// Create a new HollaEx WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            private_ws_stream: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            private_subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: None,
            api_secret: None,
            is_authenticated: Arc::new(RwLock::new(false)),
        }
    }

    /// Create a new HollaEx WebSocket client with credentials
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        Self {
            ws_stream: None,
            private_ws_stream: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            private_subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: Some(api_key),
            api_secret: Some(api_secret),
            is_authenticated: Arc::new(RwLock::new(false)),
        }
    }

    /// Generate authenticated WebSocket URL
    fn get_authenticated_url(&self) -> CcxtResult<String> {
        let api_key = self.api_key.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required for private streams".to_string(),
        })?;
        let api_secret = self.api_secret.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required for private streams".to_string(),
        })?;

        // Calculate expiration (60 seconds from now)
        let expires = Utc::now().timestamp() + 60;
        let expires_str = expires.to_string();

        // Build authentication string: CONNECT/stream{expires}
        let auth_str = format!("CONNECT/stream{}", expires_str);

        // Create HMAC-SHA256 signature
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(auth_str.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        // Build authenticated URL with query parameters
        Ok(format!(
            "{}?api-key={}&api-signature={}&api-expires={}",
            WS_URL, api_key, signature, expires_str
        ))
    }

    /// Connect to private WebSocket stream with authentication
    async fn connect_private(&self) -> CcxtResult<()> {
        // Check if already connected
        {
            let ws_guard = self.private_ws_stream.read().await;
            if ws_guard.is_some() {
                return Ok(());
            }
        }

        let url = self.get_authenticated_url()?;

        let (ws, _) = connect_async(&url).await.map_err(|e| CcxtError::NetworkError {
            url: WS_URL.to_string(),
            message: format!("Failed to connect to private WebSocket: {}", e),
        })?;

        {
            let mut ws_guard = self.private_ws_stream.write().await;
            *ws_guard = Some(ws);
        }

        {
            let mut auth_guard = self.is_authenticated.write().await;
            *auth_guard = true;
        }

        self.start_private_message_loop();

        Ok(())
    }

    /// Subscribe to a private channel
    async fn subscribe_private(&self, channel: &str) -> CcxtResult<()> {
        let msg = json!({
            "op": "subscribe",
            "args": [channel]
        });

        let mut ws_guard = self.private_ws_stream.write().await;
        if let Some(ref mut ws) = *ws_guard {
            ws.send(Message::Text(msg.to_string()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to send private subscribe: {}", e),
                })?;
        }
        Ok(())
    }

    /// Start private message processing loop
    fn start_private_message_loop(&self) {
        let ws_clone = self.private_ws_stream.clone();
        let subscriptions = self.private_subscriptions.clone();

        tokio::spawn(async move {
            loop {
                let msg = {
                    let mut ws_guard = ws_clone.write().await;
                    if let Some(ref mut ws) = *ws_guard {
                        ws.next().await
                    } else {
                        break;
                    }
                };

                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(data) = serde_json::from_str::<Value>(&text) {
                            Self::handle_private_message(&data, &subscriptions).await;
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let mut ws_guard = ws_clone.write().await;
                        if let Some(ref mut ws) = *ws_guard {
                            let _ = ws.send(Message::Pong(data)).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) => break,
                    Some(Err(_)) => break,
                    None => break,
                    _ => {}
                }
            }
        });
    }

    /// Handle private WebSocket messages
    async fn handle_private_message(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let topic = data.get("topic").and_then(|v| v.as_str()).unwrap_or("");

        match topic {
            "wallet" => {
                Self::handle_balance(data, subscriptions).await;
            }
            "order" => {
                Self::handle_order(data, subscriptions).await;
            }
            _ => {}
        }
    }

    /// Handle balance updates
    async fn handle_balance(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let message_hash = "wallet";
        let balance_data = data.get("data");

        if let Some(balance_data) = balance_data {
            let timestamp = data.get("time")
                .and_then(|v| v.as_i64())
                .map(|t| t * 1000); // Convert to milliseconds

            let mut currencies = HashMap::new();

            if let Some(obj) = balance_data.as_object() {
                for (key, value) in obj {
                    // Keys are like "btc_balance", "btc_available"
                    let parts: Vec<&str> = key.split('_').collect();
                    if parts.len() >= 2 {
                        let currency = parts[0].to_uppercase();
                        let field_type = parts[1];

                        let balance = currencies.entry(currency).or_insert_with(Balance::default);

                        if let Some(amount) = value.as_f64() {
                            let decimal_amount = Decimal::from_f64(amount);
                            match field_type {
                                "available" => balance.free = decimal_amount,
                                "balance" => balance.total = decimal_amount,
                                _ => {}
                            }
                        }
                    }
                }
            }

            // Calculate used = total - free for each balance
            for balance in currencies.values_mut() {
                if let (Some(total), Some(free)) = (balance.total, balance.free) {
                    balance.used = Some(total - free);
                }
            }

            let balances = Balances {
                timestamp,
                datetime: timestamp.map(|t| Utc.timestamp_millis_opt(t).single().map(|dt| dt.to_rfc3339())).flatten(),
                currencies,
                info: balance_data.clone(),
            };

            let event = WsBalanceEvent { balances };

            let subs = subscriptions.read().await;
            if let Some(tx) = subs.get(message_hash) {
                let _ = tx.send(WsMessage::Balance(event));
            }
        }
    }

    /// Handle order updates
    async fn handle_order(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let message_hash = "order";
        let order_data = data.get("data");

        if let Some(order_data) = order_data {
            // Handle both single order and array of orders
            let orders: Vec<&Value> = if order_data.is_array() {
                order_data.as_array().map(|a| a.iter().collect()).unwrap_or_default()
            } else {
                vec![order_data]
            };

            for order in orders {
                let symbol = order.get("symbol")
                    .and_then(|v| v.as_str())
                    .map(|s| s.replace("-", "/").to_uppercase())
                    .unwrap_or_default();

                let order_id = order.get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let side = match order.get("side").and_then(|v| v.as_str()) {
                    Some("buy") => OrderSide::Buy,
                    Some("sell") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                let order_type = match order.get("type").and_then(|v| v.as_str()) {
                    Some("limit") => OrderType::Limit,
                    Some("market") => OrderType::Market,
                    _ => OrderType::Limit,
                };

                let status = match order.get("status").and_then(|v| v.as_str()) {
                    Some("new") | Some("pending") => OrderStatus::Open,
                    Some("filled") => OrderStatus::Closed,
                    Some("canceled") | Some("cancelled") => OrderStatus::Canceled,
                    Some("partially_filled") => OrderStatus::Open,
                    _ => OrderStatus::Open,
                };

                let price = order.get("price")
                    .and_then(|v| v.as_f64())
                    .and_then(|f| Decimal::from_f64(f));

                let amount = order.get("size")
                    .and_then(|v| v.as_f64())
                    .and_then(|f| Decimal::from_f64(f))
                    .unwrap_or(Decimal::ZERO);

                let filled = order.get("filled")
                    .and_then(|v| v.as_f64())
                    .and_then(|f| Decimal::from_f64(f))
                    .unwrap_or(Decimal::ZERO);

                let remaining = if amount > filled {
                    Some(amount - filled)
                } else {
                    Some(Decimal::ZERO)
                };

                let timestamp = order.get("created_at")
                    .and_then(|v| v.as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis());

                let event = WsOrderEvent {
                    order: Order {
                        id: order_id,
                        client_order_id: None,
                        symbol,
                        side,
                        order_type,
                        status,
                        price,
                        amount,
                        filled,
                        remaining,
                        cost: None,
                        average: None,
                        timestamp,
                        datetime: order.get("created_at").and_then(|v| v.as_str()).map(String::from),
                        last_trade_timestamp: None,
                        last_update_timestamp: None,
                        time_in_force: None,
                        stop_price: None,
                        trigger_price: None,
                        take_profit_price: None,
                        stop_loss_price: None,
                        fee: None,
                        fees: Vec::new(),
                        trades: Vec::new(),
                        post_only: None,
                        reduce_only: None,
                        info: order.clone(),
                    },
                };

                // Compute symbol_hash before sending event to avoid borrow-after-move
                let symbol_hash = format!("order:{}", event.order.symbol.to_lowercase().replace("/", "-"));

                let subs = subscriptions.read().await;

                // Send to symbol-specific subscription first (if exists)
                if let Some(tx) = subs.get(&symbol_hash) {
                    let _ = tx.send(WsMessage::Order(event.clone()));
                }

                // Send to general order subscription
                if let Some(tx) = subs.get(message_hash) {
                    let _ = tx.send(WsMessage::Order(event));
                }
            }
        }
    }

    /// Convert unified symbol to HollaEx format
    /// BTC/USDT -> btc-usdt
    fn format_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "-").to_lowercase()
    }

    /// Convert HollaEx symbol to unified format
    /// btc-usdt -> BTC/USDT
    fn parse_symbol(&self, symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('-').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            symbol.to_uppercase()
        }
    }

    /// Send a subscription message
    async fn subscribe(&self, channel: &str, symbol: &str) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let msg = json!({
                "op": "subscribe",
                "args": [format!("{}:{}", channel, symbol)]
            });

            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(msg.to_string()))
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
                                // Handle ping/pong
                                if data.get("op").and_then(|v| v.as_str()) == Some("ping") {
                                    let pong = json!({"op": "pong"});
                                    let mut ws_guard = ws.write().await;
                                    let _ = ws_guard.send(Message::Text(pong.to_string())).await;
                                    continue;
                                }

                                // Handle data messages
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
        let topic = data.get("topic").and_then(|v| v.as_str()).unwrap_or("");
        let action = data.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let symbol = data.get("symbol").and_then(|v| v.as_str()).unwrap_or("");

        match topic {
            "trade" => {
                Self::handle_trades(data, subscriptions, symbol).await;
            }
            "orderbook" => {
                Self::handle_orderbook(data, subscriptions, orderbook_cache, symbol, action).await;
            }
            _ => {}
        }
    }

    /// Handle trades message
    async fn handle_trades(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        market_id: &str,
    ) {
        let symbol = Self::parse_symbol_static(market_id);
        let key = format!("trades:{}", symbol);

        if let Some(trades_data) = data.get("data").and_then(|v| v.as_array()) {
            for trade_data in trades_data {
                let timestamp_str = trade_data.get("timestamp").and_then(|v| v.as_str());
                let timestamp = timestamp_str.and_then(|ts| {
                    chrono::DateTime::parse_from_rfc3339(ts)
                        .map(|dt| dt.timestamp_millis())
                        .ok()
                });

                let price = trade_data.get("price")
                    .and_then(|v| {
                        if v.is_string() {
                            v.as_str().and_then(|s| s.parse::<Decimal>().ok())
                        } else if v.is_f64() {
                            v.as_f64().and_then(|f| Decimal::from_f64(f))
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();

                let amount = trade_data.get("size")
                    .and_then(|v| {
                        if v.is_string() {
                            v.as_str().and_then(|s| s.parse::<Decimal>().ok())
                        } else if v.is_f64() {
                            v.as_f64().and_then(|f| Decimal::from_f64(f))
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();

                let trade_id = timestamp
                    .map(|ts| ts.to_string())
                    .unwrap_or_else(|| chrono::Utc::now().timestamp_millis().to_string());

                let mut trade = Trade::new(
                    trade_id,
                    symbol.clone(),
                    price,
                    amount,
                );

                if let Some(ts) = timestamp {
                    trade = trade.with_timestamp(ts);
                }

                if let Some(side) = trade_data.get("side").and_then(|v| v.as_str()) {
                    trade = trade.with_side(side);
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

    /// Handle orderbook message
    async fn handle_orderbook(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
        market_id: &str,
        action: &str,
    ) {
        let symbol = Self::parse_symbol_static(market_id);
        let key = format!("orderbook:{}", symbol);

        if let Some(ob_data) = data.get("data") {
            let is_snapshot = action == "partial";

            let mut bids: Vec<OrderBookEntry> = Vec::new();
            let mut asks: Vec<OrderBookEntry> = Vec::new();

            // Parse bids
            if let Some(bids_data) = ob_data.get("bids").and_then(|v| v.as_array()) {
                for bid in bids_data {
                    if let Some(bid_arr) = bid.as_array() {
                        if bid_arr.len() >= 2 {
                            let price = Self::parse_decimal(&bid_arr[0]);
                            let amount = Self::parse_decimal(&bid_arr[1]);
                            if !price.is_zero() || !amount.is_zero() {
                                bids.push(OrderBookEntry { price, amount });
                            }
                        }
                    }
                }
            }

            // Parse asks
            if let Some(asks_data) = ob_data.get("asks").and_then(|v| v.as_array()) {
                for ask in asks_data {
                    if let Some(ask_arr) = ask.as_array() {
                        if ask_arr.len() >= 2 {
                            let price = Self::parse_decimal(&ask_arr[0]);
                            let amount = Self::parse_decimal(&ask_arr[1]);
                            if !price.is_zero() || !amount.is_zero() {
                                asks.push(OrderBookEntry { price, amount });
                            }
                        }
                    }
                }
            }

            // Sort bids descending, asks ascending
            bids.sort_by(|a, b| b.price.cmp(&a.price));
            asks.sort_by(|a, b| a.price.cmp(&b.price));

            // Parse timestamp
            let timestamp = ob_data.get("timestamp").and_then(|v| {
                if let Some(ts_str) = v.as_str() {
                    chrono::DateTime::parse_from_rfc3339(ts_str)
                        .map(|dt| dt.timestamp_millis())
                        .ok()
                } else {
                    None
                }
            });

            let orderbook = if is_snapshot {
                OrderBook {
                    symbol: symbol.clone(),
                    bids,
                    asks,
                    timestamp,
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
                    new_ob.timestamp = timestamp;
                    new_ob
                } else {
                    OrderBook {
                        symbol: symbol.clone(),
                        bids,
                        asks,
                        timestamp,
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
                    is_snapshot,
                }));
            }
        }
    }

    /// Parse Value to Decimal
    fn parse_decimal(value: &Value) -> Decimal {
        if let Some(s) = value.as_str() {
            s.parse::<Decimal>().unwrap_or_default()
        } else if let Some(f) = value.as_f64() {
            Decimal::from_f64(f).unwrap_or_default()
        } else if let Some(i) = value.as_i64() {
            Decimal::from_i64(i).unwrap_or_default()
        } else {
            Decimal::ZERO
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
        let parts: Vec<&str> = symbol.split('-').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            symbol.to_uppercase()
        }
    }
}

impl Default for HollaexWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for HollaexWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        let url = if self.api_key.is_some() && self.api_secret.is_some() {
            // For authenticated connections, add auth params to URL
            // Note: Full HMAC authentication would be implemented here
            WS_URL.to_string()
        } else {
            WS_URL.to_string()
        };

        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
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

    async fn watch_ticker(&self, _symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // HollaEx does not support ticker via WebSocket
        Err(CcxtError::NotSupported {
            feature: "HollaEx WebSocket does not support ticker".to_string(),
        })
    }

    async fn watch_tickers(&self, _symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // HollaEx does not support ticker via WebSocket
        Err(CcxtError::NotSupported {
            feature: "HollaEx WebSocket does not support ticker".to_string(),
        })
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
        self.subscribe("orderbook", &market_id).await?;

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
            self.subscribe("orderbook", &market_id).await?;
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
        self.subscribe("trade", &market_id).await?;

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
            self.subscribe("trade", &market_id).await?;
        }

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // HollaEx does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "HollaEx WebSocket does not support OHLCV".to_string(),
        })
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        _symbols: &[&str],
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // HollaEx does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "HollaEx WebSocket does not support OHLCV".to_string(),
        })
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Private streams require authentication
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private streams".to_string(),
            });
        }

        // Connect to private WebSocket if not connected
        self.connect_private().await?;

        let message_hash = "wallet".to_string();

        // Create channel for balance updates
        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut subs = self.private_subscriptions.write().await;
            subs.insert(message_hash.clone(), tx);
        }

        // Subscribe to wallet channel
        self.subscribe_private(&message_hash).await?;

        Ok(rx)
    }

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Private streams require authentication
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private streams".to_string(),
            });
        }

        // Connect to private WebSocket if not connected
        self.connect_private().await?;

        let message_hash = if let Some(sym) = symbol {
            format!("order:{}", self.format_symbol(sym))
        } else {
            "order".to_string()
        };

        // Create channel for order updates
        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut subs = self.private_subscriptions.write().await;
            subs.insert(message_hash.clone(), tx);
        }

        // Subscribe to order channel
        self.subscribe_private("order").await?;

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hollaex_ws_creation() {
        let _ws = HollaexWs::new();
    }

    #[test]
    fn test_hollaex_ws_with_credentials() {
        let _ws = HollaexWs::with_credentials("test_key".into(), "test_secret".into());
    }

    #[test]
    fn test_format_symbol() {
        let ws = HollaexWs::new();
        assert_eq!(ws.format_symbol("BTC/USDT"), "btc-usdt");
        assert_eq!(ws.format_symbol("ETH/BTC"), "eth-btc");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = HollaexWs::new();
        assert_eq!(ws.parse_symbol("btc-usdt"), "BTC/USDT");
        assert_eq!(ws.parse_symbol("eth-btc"), "ETH/BTC");
    }

    #[test]
    fn test_parse_decimal() {
        assert_eq!(HollaexWs::parse_decimal(&json!("123.45")), Decimal::new(12345, 2));
        assert_eq!(HollaexWs::parse_decimal(&json!(123.45)), Decimal::from_f64(123.45).unwrap());
        assert_eq!(HollaexWs::parse_decimal(&json!(100)), Decimal::from(100));
    }
}
