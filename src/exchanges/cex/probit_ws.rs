//! ProBit WebSocket Implementation
//!
//! ProBit WebSocket API for real-time market data
//! URL: wss://api.probit.com/api/exchange/v1/ws

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

use rust_decimal::Decimal;

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Ticker,
    Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsOrderBookEvent, WsOrderEvent,
    WsTickerEvent, WsTradeEvent,
};

const WS_URL: &str = "wss://api.probit.com/api/exchange/v1/ws";

/// Order update data from private WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProbitOrderUpdateData {
    id: Option<String>,
    user_id: Option<String>,
    market_id: Option<String>,
    side: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<String>,
    limit_price: Option<String>,
    quantity: Option<String>,
    filled_quantity: Option<String>,
    filled_cost: Option<String>,
    time_in_force: Option<String>,
    status: Option<String>,
    client_order_id: Option<String>,
    time: Option<String>,
}

/// Balance update data from private WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProbitBalanceUpdateData {
    currency_id: Option<String>,
    total: Option<String>,
    available: Option<String>,
}

/// ProBit WebSocket client
pub struct ProbitWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    api_key: Option<String>,
    api_secret: Option<String>,
    access_token: Arc<RwLock<Option<String>>>,
}

impl ProbitWs {
    /// Create a new ProBit WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: None,
            api_secret: None,
            access_token: Arc::new(RwLock::new(None)),
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
            access_token: Arc::new(RwLock::new(None)),
        }
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
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

    /// Get access token using Basic Auth (ProBit uses OAuth2-style authentication)
    async fn get_access_token(&self) -> CcxtResult<String> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let api_secret =
            self.api_secret
                .as_ref()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API secret required".into(),
                })?;

        // Check if we already have a valid token
        if let Some(token) = self.access_token.read().await.as_ref() {
            return Ok(token.clone());
        }

        // Request new token using Basic Auth
        let credentials = format!("{api_key}:{api_secret}");
        let auth_header = format!("Basic {}", STANDARD.encode(credentials.as_bytes()));

        let client = reqwest::Client::new();
        let response = client
            .post("https://accounts.probit.com/token")
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "grant_type": "client_credentials"
            }))
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: "https://accounts.probit.com/token".to_string(),
                message: format!("Failed to get access token: {e}"),
            })?;

        let body: Value = response.json().await.map_err(|e| CcxtError::ParseError {
            data_type: "JSON".to_string(),
            message: format!("Failed to parse token response: {e}"),
        })?;

        let token = body["access_token"]
            .as_str()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "No access_token in response".into(),
            })?
            .to_string();

        // Store the token
        *self.access_token.write().await = Some(token.clone());

        Ok(token)
    }

    /// Authenticate WebSocket connection
    async fn ws_auth(&self, ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>) -> CcxtResult<()> {
        let token = self.get_access_token().await?;

        let auth_msg = json!({
            "type": "authorization",
            "token": token
        });

        ws.send(Message::Text(auth_msg.to_string()))
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: format!("Failed to send auth message: {e}"),
            })?;

        Ok(())
    }

    /// Subscribe to private streams
    async fn subscribe_private_stream(
        &self,
        ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
        channel: &str,
    ) -> CcxtResult<()> {
        let subscribe_msg = json!({
            "type": "subscribe",
            "channel": channel
        });

        ws.send(Message::Text(subscribe_msg.to_string()))
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: format!("Failed to subscribe to {channel}: {e}"),
            })?;

        Ok(())
    }

    /// Process private WebSocket message
    fn process_private_message(&self, data: &Value) -> Option<WsMessage> {
        let channel = data["channel"].as_str()?;

        match channel {
            "open_order" | "order_history" => {
                // Order update
                if let Some(order_data_arr) = data["data"].as_array() {
                    for order_data in order_data_arr {
                        if let Ok(order_update) =
                            serde_json::from_value::<ProbitOrderUpdateData>(order_data.clone())
                        {
                            return self.parse_order(&order_update);
                        }
                    }
                }
                None
            },
            "balance" => {
                // Balance update
                if let Some(balance_data_arr) = data["data"].as_array() {
                    return self.parse_balance(balance_data_arr, data);
                }
                None
            },
            _ => None,
        }
    }

    /// Parse order from private message
    fn parse_order(&self, data: &ProbitOrderUpdateData) -> Option<WsMessage> {
        let symbol = data.market_id.as_ref().map(|s| self.parse_symbol(s))?;
        let order_id = data.id.clone()?;

        let status = match data.status.as_deref() {
            Some("open") => OrderStatus::Open,
            Some("filled") => OrderStatus::Closed,
            Some("cancelled") | Some("canceled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let price = data
            .limit_price
            .as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let amount = data
            .quantity
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok());
        let filled = data
            .filled_quantity
            .as_ref()
            .and_then(|f| Decimal::from_str(f).ok());
        let remaining = match (amount, filled) {
            (Some(a), Some(f)) => Some(a - f),
            _ => None,
        };

        let timestamp = data.time.as_ref().and_then(|t| {
            chrono::DateTime::parse_from_rfc3339(t)
                .ok()
                .map(|dt| dt.timestamp_millis())
        });

        let order = Order {
            id: order_id,
            client_order_id: data.client_order_id.clone(),
            timestamp,
            datetime: data.time.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount: amount.unwrap_or_default(),
            filled: filled.unwrap_or_default(),
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: data
                .filled_cost
                .as_ref()
                .and_then(|c| Decimal::from_str(c).ok()),
            reduce_only: None,
            post_only: None,
            trades: vec![],
            fee: None,
            fees: vec![],
            info: serde_json::to_value(data).unwrap_or(Value::Null),
        };

        Some(WsMessage::Order(WsOrderEvent { order }))
    }

    /// Parse balance from private message
    fn parse_balance(&self, balances_arr: &[Value], parsed: &Value) -> Option<WsMessage> {
        let mut currencies: HashMap<String, Balance> = HashMap::new();

        for bal in balances_arr {
            if let Ok(balance_data) = serde_json::from_value::<ProbitBalanceUpdateData>(bal.clone())
            {
                if let Some(currency) = &balance_data.currency_id {
                    let total = balance_data
                        .total
                        .as_ref()
                        .and_then(|t| Decimal::from_str(t).ok())
                        .unwrap_or_default();
                    let available = balance_data
                        .available
                        .as_ref()
                        .and_then(|a| Decimal::from_str(a).ok())
                        .unwrap_or_default();
                    let used = total - available;

                    currencies.insert(
                        currency.to_uppercase(),
                        Balance {
                            free: Some(available),
                            used: Some(used),
                            total: Some(total),
                            debt: None,
                        },
                    );
                }
            }
        }

        if currencies.is_empty() {
            return None;
        }

        let now = Utc::now();
        let balances = Balances {
            timestamp: Some(now.timestamp_millis()),
            datetime: Some(now.to_rfc3339()),
            currencies,
            info: parsed.clone(),
        };

        Some(WsMessage::Balance(WsBalanceEvent { balances }))
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
                .send(Message::Text(msg.to_string()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
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
                                Self::handle_message_static(
                                    &data,
                                    &subscriptions,
                                    &orderbook_cache,
                                )
                                .await;
                            }
                        },
                        Some(Ok(Message::Ping(data))) => {
                            let mut ws_guard = ws.write().await;
                            let _ = ws_guard.send(Message::Pong(data)).await;
                        },
                        Some(Ok(Message::Close(_))) => break,
                        Some(Err(_)) => break,
                        None => break,
                        _ => {},
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

        if channel == "marketdata" {
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
    }

    /// Handle ticker message
    async fn handle_ticker(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let market_id = data.get("market_id").and_then(|v| v.as_str()).unwrap_or("");
        let symbol = Self::parse_symbol_static(market_id);
        let key = format!("ticker:{symbol}");

        if let Some(ticker_data) = data.get("ticker") {
            let time_str = ticker_data.get("time").and_then(|v| v.as_str());
            let timestamp = time_str.and_then(|ts| {
                chrono::DateTime::parse_from_rfc3339(ts)
                    .map(|dt| dt.timestamp_millis())
                    .ok()
            });

            let ticker = Ticker {
                symbol: symbol.clone(),
                high: ticker_data
                    .get("high")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
                low: ticker_data
                    .get("low")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
                last: ticker_data
                    .get("last")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
                base_volume: ticker_data
                    .get("base_volume")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
                quote_volume: ticker_data
                    .get("quote_volume")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
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
        let key = format!("trades:{symbol}");

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

                let price = trade_data
                    .get("price")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();
                let amount = trade_data
                    .get("quantity")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();

                let trade = Trade::new(
                    trade_data
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or_default(),
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
        let key = format!("orderbook:{symbol}");

        // Get orderbook data from any of the possible keys
        let order_book_data = data
            .get("order_books")
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
                let price: Decimal = entry
                    .get("price")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();
                let quantity: Decimal = entry
                    .get("quantity")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();

                let ob_entry = OrderBookEntry {
                    price,
                    amount: quantity,
                };

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
                    checksum: None,
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
                        checksum: None,
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
    fn apply_deltas(
        book_side: &mut Vec<OrderBookEntry>,
        updates: &Vec<OrderBookEntry>,
        is_bid: bool,
    ) {
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

        let market_id = self.format_symbol(symbol);
        self.subscribe(&market_id, vec!["ticker"]).await?;

        Ok(rx)
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("ticker:{symbol}");
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
        let key = format!("orderbook:{symbol}");

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
            let key = format!("orderbook:{symbol}");
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
        let key = format!("trades:{symbol}");

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
            let key = format!("trades:{symbol}");
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

    // === Private Channel Methods ===

    async fn watch_orders(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        let (mut ws_stream, _) =
            connect_async(WS_URL)
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to connect: {e}"),
                })?;

        // Authenticate
        self.ws_auth(&mut ws_stream).await?;

        // Subscribe to open orders
        self.subscribe_private_stream(&mut ws_stream, "open_order")
            .await?;

        let (tx, rx) = mpsc::unbounded_channel();
        let subscriptions = self.subscriptions.clone();
        subscriptions.write().await.insert("orders".to_string(), tx);

        let subs = subscriptions.clone();
        let self_clone = ProbitWs {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
            access_token: self.access_token.clone(),
        };

        tokio::spawn(async move {
            while let Some(msg) = ws_stream.next().await {
                if let Ok(Message::Text(text)) = msg {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                        if let Some(ws_msg) = self_clone.process_private_message(&parsed) {
                            if let Some(sender) = subs.read().await.get("orders") {
                                let _ = sender.send(ws_msg);
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }

    async fn watch_my_trades(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        let (mut ws_stream, _) =
            connect_async(WS_URL)
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to connect: {e}"),
                })?;

        // Authenticate
        self.ws_auth(&mut ws_stream).await?;

        // Subscribe to order history for trade information
        self.subscribe_private_stream(&mut ws_stream, "order_history")
            .await?;

        let (tx, rx) = mpsc::unbounded_channel();
        let subscriptions = self.subscriptions.clone();
        subscriptions
            .write()
            .await
            .insert("my_trades".to_string(), tx);

        let subs = subscriptions.clone();
        let self_clone = ProbitWs {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
            access_token: self.access_token.clone(),
        };

        tokio::spawn(async move {
            while let Some(msg) = ws_stream.next().await {
                if let Ok(Message::Text(text)) = msg {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                        if let Some(ws_msg) = self_clone.process_private_message(&parsed) {
                            if let Some(sender) = subs.read().await.get("my_trades") {
                                let _ = sender.send(ws_msg);
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        let (mut ws_stream, _) =
            connect_async(WS_URL)
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to connect: {e}"),
                })?;

        // Authenticate
        self.ws_auth(&mut ws_stream).await?;

        // Subscribe to balance updates
        self.subscribe_private_stream(&mut ws_stream, "balance")
            .await?;

        let (tx, rx) = mpsc::unbounded_channel();
        let subscriptions = self.subscriptions.clone();
        subscriptions
            .write()
            .await
            .insert("balance".to_string(), tx);

        let subs = subscriptions.clone();
        let self_clone = ProbitWs {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
            access_token: self.access_token.clone(),
        };

        tokio::spawn(async move {
            while let Some(msg) = ws_stream.next().await {
                if let Ok(Message::Text(text)) = msg {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                        if let Some(ws_msg) = self_clone.process_private_message(&parsed) {
                            if let Some(sender) = subs.read().await.get("balance") {
                                let _ = sender.send(ws_msg);
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        // ProBit uses OAuth2 token-based authentication
        // The actual auth happens in watch_* methods via ws_auth
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required".into(),
            });
        }
        Ok(())
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

    #[test]
    fn test_with_credentials() {
        let _ws = ProbitWs::with_credentials("api_key".to_string(), "api_secret".to_string());
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = ProbitWs::new();
        ws.set_credentials("api_key".to_string(), "api_secret".to_string());
    }

    #[test]
    fn test_parse_order() {
        let ws = ProbitWs::new();

        let order_data = ProbitOrderUpdateData {
            id: Some("order123".to_string()),
            user_id: Some("user456".to_string()),
            market_id: Some("BTC-USDT".to_string()),
            side: Some("buy".to_string()),
            order_type: Some("limit".to_string()),
            limit_price: Some("50000.00".to_string()),
            quantity: Some("1.5".to_string()),
            filled_quantity: Some("0.5".to_string()),
            filled_cost: Some("25000.00".to_string()),
            time_in_force: Some("gtc".to_string()),
            status: Some("open".to_string()),
            client_order_id: Some("client789".to_string()),
            time: Some("2024-01-01T00:00:00Z".to_string()),
        };

        let result = ws.parse_order(&order_data);
        assert!(result.is_some());

        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "order123");
            assert_eq!(event.order.client_order_id, Some("client789".to_string()));
            assert_eq!(event.order.symbol, "BTC/USDT");
            assert_eq!(event.order.side, OrderSide::Buy);
            assert_eq!(event.order.order_type, OrderType::Limit);
            assert_eq!(
                event.order.price,
                Some(Decimal::from_str("50000.00").unwrap())
            );
            assert_eq!(event.order.amount, Decimal::from_str("1.5").unwrap());
            assert_eq!(event.order.filled, Decimal::from_str("0.5").unwrap());
            assert_eq!(
                event.order.remaining,
                Some(Decimal::from_str("1.0").unwrap())
            );
            assert_eq!(event.order.status, OrderStatus::Open);
        } else {
            panic!("Expected Order event");
        }
    }

    #[test]
    fn test_parse_balance() {
        let ws = ProbitWs::new();

        let balances_arr = vec![
            serde_json::json!({
                "currency_id": "BTC",
                "total": "2.0",
                "available": "1.5"
            }),
            serde_json::json!({
                "currency_id": "USDT",
                "total": "1000.0",
                "available": "800.0"
            }),
        ];
        let parsed = serde_json::json!({});

        let result = ws.parse_balance(&balances_arr, &parsed);
        assert!(result.is_some());

        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("BTC"));
            assert!(event.balances.currencies.contains_key("USDT"));

            let btc_balance = event.balances.currencies.get("BTC").unwrap();
            assert_eq!(btc_balance.free, Some(Decimal::from_str("1.5").unwrap()));
            assert_eq!(btc_balance.used, Some(Decimal::from_str("0.5").unwrap()));
            assert_eq!(btc_balance.total, Some(Decimal::from_str("2.0").unwrap()));

            let usdt_balance = event.balances.currencies.get("USDT").unwrap();
            assert_eq!(usdt_balance.free, Some(Decimal::from_str("800.0").unwrap()));
            assert_eq!(usdt_balance.used, Some(Decimal::from_str("200.0").unwrap()));
            assert_eq!(
                usdt_balance.total,
                Some(Decimal::from_str("1000.0").unwrap())
            );
        } else {
            panic!("Expected Balance event");
        }
    }

    #[test]
    fn test_parse_order_status() {
        let ws = ProbitWs::new();

        // Test open status
        let open_data = ProbitOrderUpdateData {
            id: Some("order1".to_string()),
            user_id: None,
            market_id: Some("BTC-USDT".to_string()),
            side: Some("buy".to_string()),
            order_type: Some("limit".to_string()),
            limit_price: Some("50000".to_string()),
            quantity: Some("1".to_string()),
            filled_quantity: Some("0".to_string()),
            filled_cost: None,
            time_in_force: None,
            status: Some("open".to_string()),
            client_order_id: None,
            time: None,
        };
        if let Some(WsMessage::Order(event)) = ws.parse_order(&open_data) {
            assert_eq!(event.order.status, OrderStatus::Open);
        } else {
            panic!("Expected Order event");
        }

        // Test filled status
        let filled_data = ProbitOrderUpdateData {
            id: Some("order2".to_string()),
            user_id: None,
            market_id: Some("BTC-USDT".to_string()),
            side: Some("sell".to_string()),
            order_type: Some("market".to_string()),
            limit_price: None,
            quantity: Some("1".to_string()),
            filled_quantity: Some("1".to_string()),
            filled_cost: Some("50000".to_string()),
            time_in_force: None,
            status: Some("filled".to_string()),
            client_order_id: None,
            time: None,
        };
        if let Some(WsMessage::Order(event)) = ws.parse_order(&filled_data) {
            assert_eq!(event.order.status, OrderStatus::Closed);
            assert_eq!(event.order.order_type, OrderType::Market);
            assert_eq!(event.order.side, OrderSide::Sell);
        } else {
            panic!("Expected Order event");
        }

        // Test cancelled status
        let cancelled_data = ProbitOrderUpdateData {
            id: Some("order3".to_string()),
            user_id: None,
            market_id: Some("BTC-USDT".to_string()),
            side: Some("buy".to_string()),
            order_type: Some("limit".to_string()),
            limit_price: Some("50000".to_string()),
            quantity: Some("1".to_string()),
            filled_quantity: Some("0".to_string()),
            filled_cost: None,
            time_in_force: None,
            status: Some("cancelled".to_string()),
            client_order_id: None,
            time: None,
        };
        if let Some(WsMessage::Order(event)) = ws.parse_order(&cancelled_data) {
            assert_eq!(event.order.status, OrderStatus::Canceled);
        } else {
            panic!("Expected Order event");
        }
    }
}
