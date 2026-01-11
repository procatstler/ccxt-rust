//! Bitrue WebSocket Implementation
//!
//! Bitrue WebSocket API for real-time market data
//! Public URL: wss://ws.bitrue.com/market/ws
//! Private URL: wss://wsapi.bitrue.com
//!
//! Authentication: HMAC-SHA256 signature
//! Private channels: orders, balance

#![allow(dead_code)]

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
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
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Ticker, TimeInForce, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage,
    WsOrderEvent, WsOrderBookEvent, WsTickerEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://ws.bitrue.com/market/ws";
const WS_PRIVATE_URL: &str = "wss://wsapi.bitrue.com";

type HmacSha256 = Hmac<Sha256>;

/// Order update data from Bitrue private WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BitrueOrderUpdateData {
    #[serde(rename = "s")]
    symbol: Option<String>,
    #[serde(rename = "c")]
    client_order_id: Option<String>,
    #[serde(rename = "S")]
    side: Option<String>,
    #[serde(rename = "o")]
    order_type: Option<String>,
    #[serde(rename = "f")]
    time_in_force: Option<String>,
    #[serde(rename = "q")]
    quantity: Option<String>,
    #[serde(rename = "p")]
    price: Option<String>,
    #[serde(rename = "X")]
    status: Option<String>,
    #[serde(rename = "i")]
    order_id: Option<i64>,
    #[serde(rename = "z")]
    filled_quantity: Option<String>,
    #[serde(rename = "Z")]
    cumulative_quote_qty: Option<String>,
    #[serde(rename = "n")]
    commission: Option<String>,
    #[serde(rename = "N")]
    commission_asset: Option<String>,
    #[serde(rename = "T")]
    trade_time: Option<i64>,
    #[serde(rename = "t")]
    trade_id: Option<i64>,
    #[serde(rename = "E")]
    event_time: Option<i64>,
}

/// Balance update data from Bitrue private WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BitrueBalanceUpdateData {
    #[serde(rename = "a")]
    asset: Option<String>,
    #[serde(rename = "f")]
    free: Option<String>,
    #[serde(rename = "l")]
    locked: Option<String>,
}

/// Bitrue WebSocket client
pub struct BitrueWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    api_key: Option<String>,
    api_secret: Option<String>,
    request_id: AtomicI64,
}

impl BitrueWs {
    /// Create a new Bitrue WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: None,
            api_secret: None,
            request_id: AtomicI64::new(1),
        }
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: Some(api_key),
            api_secret: Some(api_secret),
            request_id: AtomicI64::new(1),
        }
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
    }

    /// Get next request ID
    fn next_id(&self) -> i64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Generate HMAC-SHA256 signature
    fn sign(&self, message: &str) -> CcxtResult<String> {
        let secret = self.api_secret.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret not set".to_string(),
        })?;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Failed to create HMAC: {e}"),
            })?;
        mac.update(message.as_bytes());
        let result = mac.finalize();
        Ok(hex::encode(result.into_bytes()))
    }

    /// Subscribe to private stream
    async fn subscribe_private_stream(
        &self,
        channel: &str,
        symbol: Option<&str>,
        tx: mpsc::UnboundedSender<WsMessage>,
    ) -> CcxtResult<()> {
        let api_key = self.api_key.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key not set".to_string(),
        })?;

        let (ws_stream, _) = connect_async(WS_PRIVATE_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_PRIVATE_URL.to_string(),
                message: format!("Private WebSocket connection failed: {e}"),
            })?;

        let (mut write, mut read) = ws_stream.split();

        // Generate timestamp and signature
        let timestamp = chrono::Utc::now().timestamp_millis();
        let sign_payload = format!("timestamp={timestamp}");
        let signature = self.sign(&sign_payload)?;

        // Send authentication message
        let auth_msg = json!({
            "method": "LOGIN",
            "params": {
                "apiKey": api_key,
                "timestamp": timestamp,
                "signature": signature
            },
            "id": self.next_id()
        });

        write
            .send(Message::Text(auth_msg.to_string()))
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_PRIVATE_URL.to_string(),
                message: format!("Failed to send auth message: {e}"),
            })?;

        // Wait for auth response
        if let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
                    if parsed.get("error").is_some() {
                        return Err(CcxtError::AuthenticationError {
                            message: format!("Authentication failed: {text}"),
                        });
                    }
                }
                Ok(Message::Binary(bin)) => {
                    if let Ok(text) = String::from_utf8(bin.to_vec()) {
                        let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
                        if parsed.get("error").is_some() {
                            return Err(CcxtError::AuthenticationError {
                                message: format!("Authentication failed: {text}"),
                            });
                        }
                    }
                }
                Err(e) => {
                    return Err(CcxtError::NetworkError {
                        url: WS_PRIVATE_URL.to_string(),
                        message: format!("Failed to receive auth response: {e}"),
                    });
                }
                _ => {}
            }
        }

        // Subscribe to channel
        let subscribe_msg = if let Some(sym) = symbol {
            let market_id = self.format_symbol(sym);
            json!({
                "method": "SUBSCRIBE",
                "params": [format!("{}@{}", market_id, channel)],
                "id": self.next_id()
            })
        } else {
            json!({
                "method": "SUBSCRIBE",
                "params": [channel],
                "id": self.next_id()
            })
        };

        write
            .send(Message::Text(subscribe_msg.to_string()))
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_PRIVATE_URL.to_string(),
                message: format!("Failed to send subscribe message: {e}"),
            })?;

        // Spawn message handler
        let subscriptions = Arc::clone(&self.subscriptions);
        let key = format!("private:{channel}");
        {
            let mut subs = subscriptions.write().await;
            subs.insert(key.clone(), tx);
        }

        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Some(ws_msg) = Self::process_private_message(&text) {
                            let subs = subscriptions.read().await;
                            if let Some(sender) = subs.get(&key) {
                                if sender.send(ws_msg).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Ok(Message::Binary(bin)) => {
                        if let Ok(text) = String::from_utf8(bin.to_vec()) {
                            if let Some(ws_msg) = Self::process_private_message(&text) {
                                let subs = subscriptions.read().await;
                                if let Some(sender) = subs.get(&key) {
                                    if sender.send(ws_msg).is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Ok(Message::Ping(data)) => {
                        let _ = write.send(Message::Pong(data)).await;
                    }
                    Ok(Message::Close(_)) => break,
                    Err(_) => break,
                    _ => {}
                }
            }
        });

        Ok(())
    }

    /// Process private WebSocket message
    fn process_private_message(text: &str) -> Option<WsMessage> {
        let parsed: Value = serde_json::from_str(text).ok()?;

        // Handle different event types
        let event_type = parsed.get("e").and_then(|v| v.as_str())
            .or_else(|| parsed.get("event").and_then(|v| v.as_str()))?;

        match event_type {
            "executionReport" | "ORDER_TRADE_UPDATE" => {
                if let Ok(data) = serde_json::from_value::<BitrueOrderUpdateData>(parsed.clone()) {
                    let order = Self::parse_order(&data);
                    return Some(WsMessage::Order(WsOrderEvent { order }));
                }
            }
            "outboundAccountPosition" | "ACCOUNT_UPDATE" => {
                if let Some(balances_arr) = parsed.get("B").and_then(|v| v.as_array()) {
                    let mut currencies = HashMap::new();
                    for b in balances_arr {
                        if let Ok(data) = serde_json::from_value::<BitrueBalanceUpdateData>(b.clone()) {
                            let balance = Self::parse_balance(&data);
                            if let Some(currency) = &data.asset {
                                currencies.insert(currency.clone(), balance);
                            }
                        }
                    }
                    let balances = Balances {
                        timestamp: parsed.get("E").and_then(|v| v.as_i64()),
                        datetime: None,
                        currencies,
                        info: parsed.clone(),
                    };
                    return Some(WsMessage::Balance(WsBalanceEvent { balances }));
                }
            }
            "balanceUpdate" => {
                if let Ok(data) = serde_json::from_value::<BitrueBalanceUpdateData>(parsed.clone()) {
                    let balance = Self::parse_balance(&data);
                    let mut currencies = HashMap::new();
                    if let Some(currency) = &data.asset {
                        currencies.insert(currency.clone(), balance);
                    }
                    let balances = Balances {
                        timestamp: parsed.get("E").and_then(|v| v.as_i64()),
                        datetime: None,
                        currencies,
                        info: parsed.clone(),
                    };
                    return Some(WsMessage::Balance(WsBalanceEvent { balances }));
                }
            }
            _ => {}
        }

        None
    }

    /// Parse order from update data
    fn parse_order(data: &BitrueOrderUpdateData) -> Order {
        let symbol = data.symbol.as_ref()
            .map(|s| Self::parse_symbol_static(s))
            .unwrap_or_default();

        let status = data.status.as_ref().map(|s| match s.as_str() {
            "NEW" => OrderStatus::Open,
            "PARTIALLY_FILLED" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" => OrderStatus::Canceled,
            "REJECTED" => OrderStatus::Rejected,
            "EXPIRED" => OrderStatus::Expired,
            _ => OrderStatus::Open,
        }).unwrap_or(OrderStatus::Open);

        let side = data.side.as_ref().map(|s| match s.as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        }).unwrap_or(OrderSide::Buy);

        let order_type = data.order_type.as_ref().map(|t| match t.as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            "STOP_LOSS" => OrderType::StopLoss,
            "STOP_LOSS_LIMIT" => OrderType::StopLossLimit,
            "TAKE_PROFIT" => OrderType::TakeProfit,
            "TAKE_PROFIT_LIMIT" => OrderType::TakeProfitLimit,
            _ => OrderType::Limit,
        }).unwrap_or(OrderType::Limit);

        let time_in_force = data.time_in_force.as_ref().and_then(|tif| match tif.as_str() {
            "GTC" => Some(TimeInForce::GTC),
            "IOC" => Some(TimeInForce::IOC),
            "FOK" => Some(TimeInForce::FOK),
            _ => None,
        });

        let price = data.price.as_ref()
            .and_then(|p| p.parse::<Decimal>().ok());
        let amount = data.quantity.as_ref()
            .and_then(|q| q.parse::<Decimal>().ok())
            .unwrap_or_default();
        let filled = data.filled_quantity.as_ref()
            .and_then(|f| f.parse::<Decimal>().ok())
            .unwrap_or_default();
        let remaining = amount - filled;
        let cost = data.cumulative_quote_qty.as_ref()
            .and_then(|c| c.parse::<Decimal>().ok());

        let timestamp = data.event_time.or(data.trade_time);

        Order {
            id: data.order_id.map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp,
            datetime: timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: data.trade_time,
            last_update_timestamp: data.event_time,
            status,
            symbol,
            order_type,
            time_in_force,
            side,
            price,
            stop_price: None,
            trigger_price: None,
            average: if filled > Decimal::ZERO {
                cost.map(|c| c / filled)
            } else {
                None
            },
            amount,
            filled,
            remaining: Some(remaining),
            cost,
            trades: vec![],
            fee: None,
            fees: vec![],
            info: serde_json::to_value(data).unwrap_or_default(),
            reduce_only: None,
            post_only: None,
            take_profit_price: None,
            stop_loss_price: None,
        }
    }

    /// Parse balance from update data
    fn parse_balance(data: &BitrueBalanceUpdateData) -> Balance {
        let free = data.free.as_ref()
            .and_then(|f| f.parse::<Decimal>().ok());
        let used = data.locked.as_ref()
            .and_then(|l| l.parse::<Decimal>().ok());
        let total = match (free, used) {
            (Some(f), Some(u)) => Some(f + u),
            (Some(f), None) => Some(f),
            (None, Some(u)) => Some(u),
            _ => None,
        };

        Balance {
            free,
            used,
            total,
            debt: None,
        }
    }

    /// Convert unified symbol to Bitrue format
    /// BTC/USDT -> btcusdt
    fn format_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "").to_lowercase()
    }

    /// Convert Bitrue symbol to unified format
    /// BTCUSDT -> BTC/USDT (for common quote currencies)
    fn parse_symbol(&self, symbol: &str) -> String {
        let symbol_upper = symbol.to_uppercase();
        let quote_currencies = ["USDT", "USDC", "BTC", "ETH", "BUSD", "BNB"];

        for quote in &quote_currencies {
            if symbol_upper.ends_with(quote) && symbol_upper.len() > quote.len() {
                let base = &symbol_upper[..symbol_upper.len() - quote.len()];
                return format!("{base}/{quote}");
            }
        }
        symbol_upper
    }

    /// Send a subscription message
    async fn subscribe(&self, channel: &str, cb_id: &str) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let msg = json!({
                "event": "sub",
                "params": {
                    "cb_id": cb_id,
                    "channel": channel,
                },
            });

            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(msg.to_string()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_PUBLIC_URL.to_string(),
                    message: format!("Failed to send subscribe: {e}"),
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
                                Self::handle_message_static(&data, &subscriptions, &orderbook_cache, &ws).await;
                            }
                        }
                        Some(Ok(Message::Binary(bin))) => {
                            // Try to parse as UTF-8 text
                            if let Ok(text) = String::from_utf8(bin.to_vec()) {
                                if let Ok(data) = serde_json::from_str::<Value>(&text) {
                                    Self::handle_message_static(&data, &subscriptions, &orderbook_cache, &ws).await;
                                }
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
        ws: &Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>,
    ) {
        // Handle ping messages
        if let Some(ping) = data.get("ping").and_then(|v| v.as_i64()) {
            let pong_msg = json!({
                "pong": ping,
            });
            let mut ws_guard = ws.write().await;
            let _ = ws_guard.send(Message::Text(pong_msg.to_string())).await;
            return;
        }

        // Handle channel messages
        if let Some(channel) = data.get("channel").and_then(|v| v.as_str()) {
            if channel.contains("_simple_depth_") {
                Self::handle_orderbook(data, subscriptions, orderbook_cache).await;
            } else if channel.contains("_trade_") {
                Self::handle_trade(data, subscriptions).await;
            } else if channel.contains("_ticker_") {
                Self::handle_ticker(data, subscriptions).await;
            }
        }
    }

    /// Handle orderbook message
    async fn handle_orderbook(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        let channel = data.get("channel").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = channel.split('_').collect();

        // Extract market id from channel: market_btcusdt_simple_depth_step0
        if parts.len() < 2 {
            return;
        }

        let market_id = parts[1].to_uppercase();
        let symbol = Self::parse_symbol_static(&market_id);
        let key = format!("orderbook:{symbol}");

        let timestamp = data.get("ts").and_then(|v| v.as_i64());

        if let Some(tick) = data.get("tick") {
            let bids = Self::parse_orderbook_side(tick.get("buys"));
            let asks = Self::parse_orderbook_side(tick.get("asks"));

            let orderbook = OrderBook {
                symbol: symbol.clone(),
                bids,
                asks,
                timestamp,
                datetime: None,
                nonce: None,
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

    /// Parse orderbook side
    fn parse_orderbook_side(data: Option<&Value>) -> Vec<OrderBookEntry> {
        let mut entries = Vec::new();
        if let Some(arr) = data.and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(entry_arr) = item.as_array() {
                    if entry_arr.len() >= 2 {
                        let price: Decimal = entry_arr[0].as_str()
                            .and_then(|s| s.parse().ok())
                            .or_else(|| entry_arr[0].as_f64().and_then(Decimal::from_f64))
                            .unwrap_or_default();
                        let amount: Decimal = entry_arr[1].as_str()
                            .and_then(|s| s.parse().ok())
                            .or_else(|| entry_arr[1].as_f64().and_then(Decimal::from_f64))
                            .unwrap_or_default();
                        entries.push(OrderBookEntry { price, amount });
                    }
                }
            }
        }
        entries
    }

    /// Handle trade message
    async fn handle_trade(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let channel = data.get("channel").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = channel.split('_').collect();

        if parts.len() < 2 {
            return;
        }

        let market_id = parts[1].to_uppercase();
        let symbol = Self::parse_symbol_static(&market_id);
        let key = format!("trades:{symbol}");

        if let Some(tick) = data.get("tick") {
            if let Some(trades) = tick.get("data").and_then(|v| v.as_array()) {
                for trade_data in trades {
                    let timestamp = trade_data.get("ts").and_then(|v| v.as_i64());
                    let side = trade_data.get("side").and_then(|v| v.as_str())
                        .map(|s| if s == "BUY" { "buy" } else { "sell" });

                    let price: Decimal = trade_data.get("price").and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok()).unwrap_or_default();
                    let amount: Decimal = trade_data.get("vol").and_then(|v| v.as_str())
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
                    let trade = if let Some(s) = side {
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
    }

    /// Handle ticker message
    async fn handle_ticker(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let channel = data.get("channel").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = channel.split('_').collect();

        if parts.len() < 2 {
            return;
        }

        let market_id = parts[1].to_uppercase();
        let symbol = Self::parse_symbol_static(&market_id);
        let key = format!("ticker:{symbol}");

        if let Some(tick) = data.get("tick") {
            let timestamp = data.get("ts").and_then(|v| v.as_i64());

            let ticker = Ticker {
                symbol: symbol.clone(),
                high: tick.get("high").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                low: tick.get("low").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                last: tick.get("close").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                bid: tick.get("bid").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                ask: tick.get("ask").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                base_volume: tick.get("vol").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
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

    /// Static helper for parsing symbol
    fn parse_symbol_static(symbol: &str) -> String {
        let symbol_upper = symbol.to_uppercase();
        let quote_currencies = ["USDT", "USDC", "BTC", "ETH", "BUSD", "BNB"];

        for quote in &quote_currencies {
            if symbol_upper.ends_with(quote) && symbol_upper.len() > quote.len() {
                let base = &symbol_upper[..symbol_upper.len() - quote.len()];
                return format!("{base}/{quote}");
            }
        }
        symbol_upper
    }
}

impl Default for BitrueWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BitrueWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        let (ws_stream, _) = connect_async(WS_PUBLIC_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_PUBLIC_URL.to_string(),
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
                    url: WS_PUBLIC_URL.to_string(),
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

        let market_id = self.format_symbol(symbol);
        let channel = format!("market_{market_id}_ticker");
        self.subscribe(&channel, &market_id).await?;

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

            let market_id = self.format_symbol(symbol);
            let channel = format!("market_{market_id}_ticker");
            self.subscribe(&channel, &market_id).await?;
        }

        Ok(rx)
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("orderbook:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        let channel = format!("market_{market_id}_simple_depth_step0");
        self.subscribe(&channel, &market_id).await?;

        Ok(rx)
    }

    async fn watch_order_book_for_symbols(
        &self,
        symbols: &[&str],
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("orderbook:{symbol}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }

            let market_id = self.format_symbol(symbol);
            let channel = format!("market_{market_id}_simple_depth_step0");
            self.subscribe(&channel, &market_id).await?;
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

        let market_id = self.format_symbol(symbol);
        let channel = format!("market_{market_id}_trade_ticker");
        self.subscribe(&channel, &market_id).await?;

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

            let market_id = self.format_symbol(symbol);
            let channel = format!("market_{market_id}_trade_ticker");
            self.subscribe(&channel, &market_id).await?;
        }

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Bitrue public WebSocket does not support OHLCV
        Err(CcxtError::NotSupported {
            feature: "Bitrue WebSocket does not support OHLCV".to_string(),
        })
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        _symbols: &[&str],
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Bitrue public WebSocket does not support OHLCV
        Err(CcxtError::NotSupported {
            feature: "Bitrue WebSocket does not support OHLCV".to_string(),
        })
    }

    // === Private Channel Methods ===

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribe_private_stream("executionReport", symbol, tx).await?;
        Ok(rx)
    }

    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        // Bitrue sends trade info as part of order execution reports
        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribe_private_stream("executionReport", symbol, tx).await?;
        Ok(rx)
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribe_private_stream("outboundAccountPosition", None, tx).await?;
        Ok(rx)
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials not set".into(),
            });
        }
        // Authentication is handled per-connection in subscribe_private_stream
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_bitrue_ws_creation() {
        let ws = BitrueWs::new();
        assert!(ws.api_key.is_none());
        assert!(ws.api_secret.is_none());
    }

    #[test]
    fn test_format_symbol() {
        let ws = BitrueWs::new();
        assert_eq!(ws.format_symbol("BTC/USDT"), "btcusdt");
        assert_eq!(ws.format_symbol("ETH/BTC"), "ethbtc");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = BitrueWs::new();
        assert_eq!(ws.parse_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(ws.parse_symbol("ETHBTC"), "ETH/BTC");
    }

    #[test]
    fn test_with_credentials() {
        let ws = BitrueWs::with_credentials("api_key".to_string(), "api_secret".to_string());
        assert_eq!(ws.api_key, Some("api_key".to_string()));
        assert_eq!(ws.api_secret, Some("api_secret".to_string()));
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = BitrueWs::new();
        ws.set_credentials("api_key".to_string(), "api_secret".to_string());
        assert_eq!(ws.api_key, Some("api_key".to_string()));
        assert_eq!(ws.api_secret, Some("api_secret".to_string()));
    }

    #[test]
    fn test_next_id() {
        let ws = BitrueWs::new();
        assert_eq!(ws.next_id(), 1);
        assert_eq!(ws.next_id(), 2);
        assert_eq!(ws.next_id(), 3);
    }

    #[test]
    fn test_sign() {
        let ws = BitrueWs::with_credentials("api_key".to_string(), "test_secret".to_string());
        let result = ws.sign("timestamp=1234567890").unwrap();
        // HMAC-SHA256 should produce a 64-character hex string
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn test_parse_order() {
        let data = BitrueOrderUpdateData {
            symbol: Some("BTCUSDT".to_string()),
            client_order_id: Some("test_client_id".to_string()),
            side: Some("BUY".to_string()),
            order_type: Some("LIMIT".to_string()),
            time_in_force: Some("GTC".to_string()),
            quantity: Some("1.5".to_string()),
            price: Some("50000.0".to_string()),
            status: Some("NEW".to_string()),
            order_id: Some(12345),
            filled_quantity: Some("0.5".to_string()),
            cumulative_quote_qty: Some("25000.0".to_string()),
            commission: None,
            commission_asset: None,
            trade_time: Some(1234567890000),
            trade_id: None,
            event_time: Some(1234567890000),
        };

        let order = BitrueWs::parse_order(&data);
        assert_eq!(order.id, "12345");
        assert_eq!(order.symbol, "BTC/USDT");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.status, OrderStatus::Open);
        assert_eq!(order.time_in_force, Some(TimeInForce::GTC));
        assert_eq!(order.price, Some(Decimal::from_str("50000.0").unwrap()));
        assert_eq!(order.amount, Decimal::from_str("1.5").unwrap());
        assert_eq!(order.filled, Decimal::from_str("0.5").unwrap());
        assert_eq!(order.remaining, Some(Decimal::from_str("1.0").unwrap()));
    }

    #[test]
    fn test_parse_order_status_mapping() {
        let test_cases = [
            ("NEW", OrderStatus::Open),
            ("PARTIALLY_FILLED", OrderStatus::Open),
            ("FILLED", OrderStatus::Closed),
            ("CANCELED", OrderStatus::Canceled),
            ("REJECTED", OrderStatus::Rejected),
            ("EXPIRED", OrderStatus::Expired),
        ];

        for (status_str, expected_status) in test_cases {
            let data = BitrueOrderUpdateData {
                symbol: Some("BTCUSDT".to_string()),
                client_order_id: None,
                side: Some("BUY".to_string()),
                order_type: Some("LIMIT".to_string()),
                time_in_force: None,
                quantity: Some("1.0".to_string()),
                price: Some("50000.0".to_string()),
                status: Some(status_str.to_string()),
                order_id: Some(1),
                filled_quantity: None,
                cumulative_quote_qty: None,
                commission: None,
                commission_asset: None,
                trade_time: None,
                trade_id: None,
                event_time: None,
            };

            let order = BitrueWs::parse_order(&data);
            assert_eq!(order.status, expected_status, "Status mismatch for {status_str}");
        }
    }

    #[test]
    fn test_parse_balance() {
        let data = BitrueBalanceUpdateData {
            asset: Some("BTC".to_string()),
            free: Some("1.5".to_string()),
            locked: Some("0.5".to_string()),
        };

        let balance = BitrueWs::parse_balance(&data);
        assert_eq!(balance.free, Some(Decimal::from_str("1.5").unwrap()));
        assert_eq!(balance.used, Some(Decimal::from_str("0.5").unwrap()));
        assert_eq!(balance.total, Some(Decimal::from_str("2.0").unwrap()));
    }

    #[test]
    fn test_parse_balance_partial() {
        let data = BitrueBalanceUpdateData {
            asset: Some("ETH".to_string()),
            free: Some("10.0".to_string()),
            locked: None,
        };

        let balance = BitrueWs::parse_balance(&data);
        assert_eq!(balance.free, Some(Decimal::from_str("10.0").unwrap()));
        assert_eq!(balance.used, None);
        assert_eq!(balance.total, Some(Decimal::from_str("10.0").unwrap()));
    }

    #[test]
    fn test_process_private_message_order() {
        let msg = r#"{
            "e": "executionReport",
            "s": "BTCUSDT",
            "c": "test_client",
            "S": "BUY",
            "o": "LIMIT",
            "f": "GTC",
            "q": "1.0",
            "p": "50000.0",
            "X": "NEW",
            "i": 12345,
            "z": "0.0",
            "Z": "0.0",
            "E": 1234567890000
        }"#;

        let result = BitrueWs::process_private_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "12345");
            assert_eq!(event.order.symbol, "BTC/USDT");
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_private_message_balance() {
        let msg = r#"{
            "e": "balanceUpdate",
            "a": "BTC",
            "f": "1.5",
            "l": "0.5"
        }"#;

        let result = BitrueWs::process_private_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("BTC"));
            let btc = event.balances.currencies.get("BTC").unwrap();
            assert_eq!(btc.free, Some(Decimal::from_str("1.5").unwrap()));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[tokio::test]
    async fn test_watch_orders_requires_credentials() {
        let ws = BitrueWs::new();
        let result = ws.watch_orders(None).await;
        assert!(result.is_err());
        if let Err(CcxtError::AuthenticationError { message }) = result {
            assert!(message.contains("credentials"));
        } else {
            panic!("Expected AuthenticationError");
        }
    }

    #[tokio::test]
    async fn test_watch_balance_requires_credentials() {
        let ws = BitrueWs::new();
        let result = ws.watch_balance().await;
        assert!(result.is_err());
        if let Err(CcxtError::AuthenticationError { message }) = result {
            assert!(message.contains("credentials"));
        } else {
            panic!("Expected AuthenticationError");
        }
    }

    #[tokio::test]
    async fn test_ws_authenticate_requires_credentials() {
        let mut ws = BitrueWs::new();
        let result = ws.ws_authenticate().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ws_authenticate_with_credentials() {
        let mut ws = BitrueWs::with_credentials("key".to_string(), "secret".to_string());
        let result = ws.ws_authenticate().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_default() {
        let ws = BitrueWs::default();
        assert!(ws.api_key.is_none());
    }
}
