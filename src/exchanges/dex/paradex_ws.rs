//! Paradex WebSocket Implementation
//!
//! Paradex WebSocket API for real-time market data and private channels
//!
//! # Public Channels
//! - `trades.{market}` - Trade updates
//! - `order_book.{market}.snapshot` - Order book snapshots
//! - `markets_summary` - Market ticker updates
//!
//! # Private Channels (requires JWT authentication)
//! - `orders.{market}` - Order updates (new, filled, canceled)
//! - `fills.{market}` - Fill/trade execution updates
//! - `tradebusts.{market}` - Trade bust notifications
//! - `positions` - Position updates
//! - `account` - Account balance updates
//!
//! # Authentication
//! Paradex uses JWT token authentication for private channels.
//! Use `ParadexWs::with_jwt()` to create an authenticated client.

#![allow(dead_code)]

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Fee, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Position, PositionSide,
    MarginMode, Ticker, Timeframe, Trade, TakerOrMaker, WsExchange, WsMessage,
    WsTickerEvent, WsTradeEvent, WsOrderBookEvent, WsOrderEvent, WsPositionEvent, WsMyTradeEvent,
};

const WS_URL: &str = "wss://ws.api.prod.paradex.trade/v1";
const WS_TESTNET_URL: &str = "wss://ws.api.testnet.paradex.trade/v1";

/// Paradex WebSocket client
pub struct ParadexWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    /// JWT token for private channel authentication
    jwt_token: Option<String>,
    /// Whether to use testnet
    testnet: bool,
    /// Whether authenticated
    authenticated: Arc<RwLock<bool>>,
}

// ===== Private Channel Data Structures =====

/// Order update from Paradex WebSocket
#[derive(Debug, Clone, Deserialize)]
pub struct ParadexOrderUpdate {
    pub id: String,
    pub market: String,
    pub side: String,
    pub r#type: String,
    pub size: String,
    pub price: Option<String>,
    pub status: String,
    pub filled_size: Option<String>,
    pub avg_fill_price: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub client_id: Option<String>,
    pub reduce_only: Option<bool>,
    pub post_only: Option<bool>,
}

/// Fill update from Paradex WebSocket
#[derive(Debug, Clone, Deserialize)]
pub struct ParadexFillUpdate {
    pub id: String,
    pub order_id: String,
    pub market: String,
    pub side: String,
    pub size: String,
    pub price: String,
    pub fee: Option<String>,
    pub fee_currency: Option<String>,
    pub liquidity: Option<String>,
    pub created_at: Option<i64>,
}

/// Position update from Paradex WebSocket
#[derive(Debug, Clone, Deserialize)]
pub struct ParadexPositionUpdate {
    pub market: String,
    pub side: String,
    pub size: String,
    pub entry_price: Option<String>,
    pub mark_price: Option<String>,
    pub unrealized_pnl: Option<String>,
    pub realized_pnl: Option<String>,
    pub liquidation_price: Option<String>,
    pub leverage: Option<String>,
    pub updated_at: Option<i64>,
}

impl ParadexWs {
    /// Create a new Paradex WebSocket client (mainnet, public only)
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            jwt_token: None,
            testnet: false,
            authenticated: Arc::new(RwLock::new(false)),
        }
    }

    /// Create a Paradex WebSocket client for testnet
    pub fn testnet() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            jwt_token: None,
            testnet: true,
            authenticated: Arc::new(RwLock::new(false)),
        }
    }

    /// Create an authenticated Paradex WebSocket client with JWT token
    ///
    /// The JWT token can be obtained from Paradex REST API authentication.
    /// Use this for private channels (orders, fills, positions).
    ///
    /// # Example
    /// ```rust,ignore
    /// let paradex = Paradex::from_starknet_key(config, "0x...", false)?;
    /// let jwt_token = paradex.authenticate().await?;
    /// let ws = ParadexWs::with_jwt(&jwt_token, false);
    /// ```
    pub fn with_jwt(jwt_token: &str, testnet: bool) -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            jwt_token: Some(jwt_token.to_string()),
            testnet,
            authenticated: Arc::new(RwLock::new(false)),
        }
    }

    /// Get WebSocket URL based on network
    fn get_ws_url(&self) -> &str {
        if self.testnet {
            WS_TESTNET_URL
        } else {
            WS_URL
        }
    }

    /// Check if this client has JWT token for private channels
    pub fn has_jwt(&self) -> bool {
        self.jwt_token.is_some()
    }

    /// Check if authenticated
    pub async fn is_authenticated(&self) -> bool {
        *self.authenticated.read().await
    }

    /// Convert unified symbol to Paradex format
    /// BTC/USD:USD -> BTC-USD-PERP
    fn format_symbol(&self, symbol: &str) -> String {
        // Paradex uses format like BTC-USD-PERP
        if symbol.contains(":") {
            // Futures format: BTC/USD:USD -> BTC-USD-PERP
            let parts: Vec<&str> = symbol.split(':').collect();
            if let Some(base_quote) = parts.first() {
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
                .send(Message::Text(msg.to_string()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: self.get_ws_url().to_string(),
                    message: format!("Failed to send subscribe: {e}"),
                })?;
        }
        Ok(())
    }

    /// Send authentication message with JWT token
    async fn send_auth(&self) -> CcxtResult<()> {
        let jwt_token = self.jwt_token.as_ref().ok_or_else(|| {
            CcxtError::AuthenticationError {
                message: "JWT token required for private channels. Use ParadexWs::with_jwt()".into(),
            }
        })?;

        if let Some(ws) = &self.ws_stream {
            let msg = json!({
                "jsonrpc": "2.0",
                "method": "auth",
                "params": {
                    "bearer": jwt_token,
                },
            });

            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(msg.to_string()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: self.get_ws_url().to_string(),
                    message: format!("Failed to send auth: {e}"),
                })?;
        }
        Ok(())
    }

    /// Subscribe to a private channel (requires authentication)
    async fn subscribe_private(&self, channel: &str) -> CcxtResult<()> {
        // Ensure authenticated
        if !*self.authenticated.read().await {
            self.send_auth().await?;
            // Wait for auth response (handled in message loop)
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        self.subscribe(channel).await
    }

    /// Start message processing loop
    fn start_message_loop(&self) {
        let ws_stream = self.ws_stream.clone();
        let subscriptions = self.subscriptions.clone();
        let orderbook_cache = self.orderbook_cache.clone();
        let authenticated = self.authenticated.clone();

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
                                Self::handle_message_static(&data, &subscriptions, &orderbook_cache, &authenticated).await;
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
        authenticated: &Arc<RwLock<bool>>,
    ) {
        let method = data.get("method").and_then(|v| v.as_str()).unwrap_or("");

        // Handle auth response
        if method == "auth" {
            if let Some(result) = data.get("result") {
                if result.get("authenticated").and_then(|v| v.as_bool()).unwrap_or(false) {
                    *authenticated.write().await = true;
                    // Notify subscribers of successful authentication
                    let subs = subscriptions.read().await;
                    if let Some(sender) = subs.get("private:auth") {
                        let _ = sender.send(WsMessage::Authenticated);
                    }
                }
            }
            return;
        }

        // Only process subscription messages
        if method != "subscription" {
            return;
        }

        // Get params and channel
        if let Some(params) = data.get("params") {
            let channel = params.get("channel").and_then(|v| v.as_str()).unwrap_or("");
            let parts: Vec<&str> = channel.split('.').collect();
            let channel_type = parts.first().copied().unwrap_or("");

            match channel_type {
                // Public channels
                "trades" => {
                    Self::handle_trade(params, subscriptions).await;
                }
                "order_book" => {
                    Self::handle_orderbook(params, subscriptions, orderbook_cache).await;
                }
                "markets_summary" => {
                    Self::handle_ticker(params, subscriptions).await;
                }
                // Private channels
                "orders" => {
                    Self::handle_orders(params, subscriptions).await;
                }
                "fills" => {
                    Self::handle_fills(params, subscriptions).await;
                }
                "positions" => {
                    Self::handle_positions(params, subscriptions).await;
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
            let key = format!("trades:{symbol}");

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
            let key = format!("orderbook:{symbol}");

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
            let key = format!("ticker:{symbol}");

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

    /// Handle orders update (private channel)
    async fn handle_orders(
        params: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        if let Some(data) = params.get("data") {
            let market_id = data.get("market").and_then(|v| v.as_str()).unwrap_or("");
            let symbol = Self::parse_symbol_static(market_id);

            // Parse order status
            let status_str = data.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let status = match status_str.to_uppercase().as_str() {
                "NEW" | "OPEN" => OrderStatus::Open,
                "FILLED" => OrderStatus::Closed,
                "PARTIALLY_FILLED" => OrderStatus::Open,
                "CANCELED" | "CANCELLED" => OrderStatus::Canceled,
                "REJECTED" => OrderStatus::Rejected,
                "EXPIRED" => OrderStatus::Expired,
                _ => OrderStatus::Open,
            };

            // Parse order side
            let side_str = data.get("side").and_then(|v| v.as_str()).unwrap_or("");
            let side = match side_str.to_uppercase().as_str() {
                "BUY" => OrderSide::Buy,
                "SELL" => OrderSide::Sell,
                _ => OrderSide::Buy,
            };

            // Parse order type
            let type_str = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let order_type = match type_str.to_uppercase().as_str() {
                "LIMIT" => OrderType::Limit,
                "MARKET" => OrderType::Market,
                "STOP_LIMIT" => OrderType::StopLimit,
                "STOP_MARKET" => OrderType::StopMarket,
                _ => OrderType::Limit,
            };

            let price = data.get("price")
                .and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str(s).ok());
            let amount = data.get("size")
                .and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default();
            let filled = data.get("filled_size")
                .and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default();
            let average = data.get("avg_fill_price")
                .and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str(s).ok());
            let timestamp = data.get("updated_at").and_then(|v| v.as_i64())
                .or_else(|| data.get("created_at").and_then(|v| v.as_i64()));

            let order = Order {
                id: data.get("id").and_then(|v| v.as_str()).map(String::from).unwrap_or_default(),
                client_order_id: data.get("client_id").and_then(|v| v.as_str()).map(String::from),
                symbol: symbol.clone(),
                order_type,
                side,
                price,
                amount,
                filled,
                remaining: Some(amount - filled),
                average,
                status,
                timestamp,
                datetime: timestamp.map(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts)
                        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
                        .unwrap_or_default()
                }),
                reduce_only: data.get("reduce_only").and_then(|v| v.as_bool()),
                post_only: data.get("post_only").and_then(|v| v.as_bool()),
                ..Default::default()
            };

            let subs = subscriptions.read().await;
            // Notify symbol-specific subscribers
            let key = format!("orders:{symbol}");
            if let Some(sender) = subs.get(&key) {
                let _ = sender.send(WsMessage::Order(WsOrderEvent { order: order.clone() }));
            }
            // Notify all-orders subscribers
            if let Some(sender) = subs.get("orders:ALL") {
                let _ = sender.send(WsMessage::Order(WsOrderEvent { order }));
            }
        }
    }

    /// Handle fills update (private channel)
    async fn handle_fills(
        params: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        if let Some(data) = params.get("data") {
            let market_id = data.get("market").and_then(|v| v.as_str()).unwrap_or("");
            let symbol = Self::parse_symbol_static(market_id);

            let side_str = data.get("side").and_then(|v| v.as_str()).unwrap_or("");
            let side = match side_str.to_uppercase().as_str() {
                "BUY" => Some("buy".to_string()),
                "SELL" => Some("sell".to_string()),
                _ => None,
            };

            let liquidity = data.get("liquidity").and_then(|v| v.as_str()).unwrap_or("");
            let taker_or_maker = match liquidity.to_uppercase().as_str() {
                "MAKER" => Some(TakerOrMaker::Maker),
                "TAKER" => Some(TakerOrMaker::Taker),
                _ => None,
            };

            let price = data.get("price")
                .and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default();
            let amount = data.get("size")
                .and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default();
            let timestamp = data.get("created_at").and_then(|v| v.as_i64());

            // Build Fee struct
            let fee_cost = data.get("fee")
                .and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str(s).ok());
            let fee_currency = data.get("fee_currency")
                .and_then(|v| v.as_str())
                .map(String::from);
            let fee = if fee_cost.is_some() || fee_currency.is_some() {
                Some(Fee {
                    cost: fee_cost,
                    currency: fee_currency,
                    rate: None,
                })
            } else {
                None
            };

            let mut trade = Trade::new(
                data.get("id").and_then(|v| v.as_str()).map(String::from).unwrap_or_default(),
                symbol.clone(),
                price,
                amount,
            );

            if let Some(ts) = timestamp {
                trade = trade.with_timestamp(ts);
            }
            trade.side = side;
            trade.taker_or_maker = taker_or_maker;
            trade.order = data.get("order_id").and_then(|v| v.as_str()).map(String::from);
            trade.fee = fee;

            let subs = subscriptions.read().await;
            // Notify symbol-specific subscribers
            let key = format!("my_trades:{symbol}");
            if let Some(sender) = subs.get(&key) {
                let _ = sender.send(WsMessage::MyTrade(WsMyTradeEvent {
                    symbol: symbol.clone(),
                    trades: vec![trade.clone()],
                }));
            }
            // Notify all-trades subscribers
            if let Some(sender) = subs.get("my_trades:ALL") {
                let _ = sender.send(WsMessage::MyTrade(WsMyTradeEvent {
                    symbol: symbol.clone(),
                    trades: vec![trade],
                }));
            }
        }
    }

    /// Handle positions update (private channel)
    async fn handle_positions(
        params: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        if let Some(data) = params.get("data") {
            let positions = if data.is_array() {
                data.as_array().cloned().unwrap_or_default()
            } else {
                vec![data.clone()]
            };

            let mut parsed_positions = Vec::new();
            for pos_data in positions {
                let market_id = pos_data.get("market").and_then(|v| v.as_str()).unwrap_or("");
                let symbol = Self::parse_symbol_static(market_id);

                let side_str = pos_data.get("side").and_then(|v| v.as_str()).unwrap_or("");
                let side = match side_str.to_uppercase().as_str() {
                    "LONG" | "BUY" => Some(PositionSide::Long),
                    "SHORT" | "SELL" => Some(PositionSide::Short),
                    _ => Some(PositionSide::Long),
                };

                let contracts = pos_data.get("size")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok());
                let entry_price = pos_data.get("entry_price")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok());
                let mark_price = pos_data.get("mark_price")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok());
                let unrealized_pnl = pos_data.get("unrealized_pnl")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok());
                let liquidation_price = pos_data.get("liquidation_price")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok());
                let leverage = pos_data.get("leverage")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok());
                let timestamp = pos_data.get("updated_at").and_then(|v| v.as_i64());

                let mut position = Position::new(symbol.clone());
                position.side = side;
                position.contracts = contracts;
                position.entry_price = entry_price;
                position.mark_price = mark_price;
                position.unrealized_pnl = unrealized_pnl;
                position.liquidation_price = liquidation_price;
                position.leverage = leverage;
                position.margin_mode = Some(MarginMode::Cross); // Paradex uses cross margin
                position.timestamp = timestamp;
                position.datetime = timestamp.map(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts)
                        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
                        .unwrap_or_default()
                });

                parsed_positions.push(position);
            }

            if !parsed_positions.is_empty() {
                let subs = subscriptions.read().await;
                if let Some(sender) = subs.get("positions") {
                    let _ = sender.send(WsMessage::Position(WsPositionEvent {
                        positions: parsed_positions,
                    }));
                }
            }
        }
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
        let ws_url = self.get_ws_url();
        let (ws_stream, _) = connect_async(ws_url)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: ws_url.to_string(),
                message: format!("WebSocket connection failed: {e}"),
            })?;

        self.ws_stream = Some(Arc::new(RwLock::new(ws_stream)));
        self.start_message_loop();

        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        let ws_url = self.get_ws_url();
        if let Some(ws) = &self.ws_stream {
            let mut ws_guard = ws.write().await;
            ws_guard
                .close(None)
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: ws_url.to_string(),
                    message: format!("Failed to close WebSocket: {e}"),
                })?;
        }
        self.ws_stream = None;
        *self.authenticated.write().await = false;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        self.ws_stream.is_some()
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        if !self.ws_is_connected().await {
            return Err(CcxtError::NetworkError {
                url: self.get_ws_url().to_string(),
                message: "WebSocket not connected. Call ws_connect() first.".into(),
            });
        }

        self.send_auth().await?;

        // Wait for authentication response
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        if *self.authenticated.read().await {
            Ok(())
        } else {
            Err(CcxtError::AuthenticationError {
                message: "WebSocket authentication failed".into(),
            })
        }
    }

    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("ticker:{symbol}");

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
            let key = format!("ticker:{symbol}");
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
        let key = format!("orderbook:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        let channel = format!("order_book.{market_id}.snapshot@15@100ms");
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
            let key = format!("orderbook:{symbol}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }

            let market_id = self.format_symbol(symbol);
            let channel = format!("order_book.{market_id}.snapshot@15@100ms");
            self.subscribe(&channel).await?;
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
        let channel = format!("trades.{market_id}");
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
                let key = format!("trades:{symbol}");
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

    // === Private Channels ===

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if !self.has_jwt() {
            return Err(CcxtError::AuthenticationError {
                message: "JWT token required for watch_orders. Use ParadexWs::with_jwt()".into(),
            });
        }

        let (tx, rx) = mpsc::unbounded_channel();

        if let Some(sym) = symbol {
            let key = format!("orders:{sym}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx);
            }
            let market_id = self.format_symbol(sym);
            let channel = format!("orders.{market_id}");
            self.subscribe_private(&channel).await?;
        } else {
            // Subscribe to all orders
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert("orders:ALL".to_string(), tx);
            }
            // Paradex may require subscribing to specific markets or use a wildcard
            self.subscribe_private("orders").await?;
        }

        Ok(rx)
    }

    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if !self.has_jwt() {
            return Err(CcxtError::AuthenticationError {
                message: "JWT token required for watch_my_trades. Use ParadexWs::with_jwt()".into(),
            });
        }

        let (tx, rx) = mpsc::unbounded_channel();

        if let Some(sym) = symbol {
            let key = format!("my_trades:{sym}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx);
            }
            let market_id = self.format_symbol(sym);
            let channel = format!("fills.{market_id}");
            self.subscribe_private(&channel).await?;
        } else {
            // Subscribe to all fills
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert("my_trades:ALL".to_string(), tx);
            }
            self.subscribe_private("fills").await?;
        }

        Ok(rx)
    }

    async fn watch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if !self.has_jwt() {
            return Err(CcxtError::AuthenticationError {
                message: "JWT token required for watch_positions. Use ParadexWs::with_jwt()".into(),
            });
        }

        let _ = symbols; // Paradex positions channel returns all positions

        let (tx, rx) = mpsc::unbounded_channel();

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert("positions".to_string(), tx);
        }

        // Paradex positions channel
        self.subscribe_private("positions").await?;

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paradex_ws_creation() {
        let ws = ParadexWs::new();
        assert!(!ws.has_jwt());
        assert!(!ws.testnet);
    }

    #[test]
    fn test_paradex_ws_testnet() {
        let ws = ParadexWs::testnet();
        assert!(!ws.has_jwt());
        assert!(ws.testnet);
        assert_eq!(ws.get_ws_url(), WS_TESTNET_URL);
    }

    #[test]
    fn test_paradex_ws_with_jwt() {
        let ws = ParadexWs::with_jwt("test_jwt_token", false);
        assert!(ws.has_jwt());
        assert!(!ws.testnet);
        assert_eq!(ws.get_ws_url(), WS_URL);
    }

    #[test]
    fn test_paradex_ws_with_jwt_testnet() {
        let ws = ParadexWs::with_jwt("test_jwt_token", true);
        assert!(ws.has_jwt());
        assert!(ws.testnet);
        assert_eq!(ws.get_ws_url(), WS_TESTNET_URL);
    }

    #[test]
    fn test_format_symbol() {
        let ws = ParadexWs::new();
        assert_eq!(ws.format_symbol("BTC/USD:USD"), "BTC-USD-PERP");
        assert_eq!(ws.format_symbol("ETH/USD:USD"), "ETH-USD-PERP");
        assert_eq!(ws.format_symbol("ETH/USD"), "ETH-USD");
        assert_eq!(ws.format_symbol("SOL/USDC"), "SOL-USDC");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = ParadexWs::new();
        assert_eq!(ws.parse_symbol("BTC-USD-PERP"), "BTC/USD:USD");
        assert_eq!(ws.parse_symbol("ETH-USD-PERP"), "ETH/USD:USD");
        assert_eq!(ws.parse_symbol("ETH-USD"), "ETH/USD");
        assert_eq!(ws.parse_symbol("SOL-USDC"), "SOL/USDC");
    }

    #[test]
    fn test_parse_symbol_static() {
        assert_eq!(ParadexWs::parse_symbol_static("BTC-USD-PERP"), "BTC/USD:USD");
        assert_eq!(ParadexWs::parse_symbol_static("ETH-USD"), "ETH/USD");
        assert_eq!(ParadexWs::parse_symbol_static("UNKNOWN"), "UNKNOWN");
    }

    #[tokio::test]
    async fn test_authenticated_flag() {
        let ws = ParadexWs::new();
        assert!(!ws.is_authenticated().await);
    }

    #[tokio::test]
    async fn test_watch_orders_requires_jwt() {
        let ws = ParadexWs::new();
        let result = ws.watch_orders(Some("BTC/USD:USD")).await;
        assert!(result.is_err());
        if let Err(CcxtError::AuthenticationError { message }) = result {
            assert!(message.contains("JWT token required"));
        }
    }

    #[tokio::test]
    async fn test_watch_my_trades_requires_jwt() {
        let ws = ParadexWs::new();
        let result = ws.watch_my_trades(Some("BTC/USD:USD")).await;
        assert!(result.is_err());
        if let Err(CcxtError::AuthenticationError { message }) = result {
            assert!(message.contains("JWT token required"));
        }
    }

    #[tokio::test]
    async fn test_watch_positions_requires_jwt() {
        let ws = ParadexWs::new();
        let result = ws.watch_positions(None).await;
        assert!(result.is_err());
        if let Err(CcxtError::AuthenticationError { message }) = result {
            assert!(message.contains("JWT token required"));
        }
    }

    #[test]
    fn test_order_update_parsing() {
        let order_json = serde_json::json!({
            "id": "123",
            "market": "BTC-USD-PERP",
            "side": "BUY",
            "type": "LIMIT",
            "size": "0.1",
            "price": "50000",
            "status": "OPEN",
            "filled_size": "0",
            "created_at": 1700000000000i64
        });

        let order: ParadexOrderUpdate = serde_json::from_value(order_json).unwrap();
        assert_eq!(order.id, "123");
        assert_eq!(order.market, "BTC-USD-PERP");
        assert_eq!(order.side, "BUY");
        assert_eq!(order.r#type, "LIMIT");
        assert_eq!(order.size, "0.1");
        assert_eq!(order.price, Some("50000".to_string()));
        assert_eq!(order.status, "OPEN");
    }

    #[test]
    fn test_fill_update_parsing() {
        let fill_json = serde_json::json!({
            "id": "fill_123",
            "order_id": "order_456",
            "market": "ETH-USD-PERP",
            "side": "SELL",
            "size": "1.5",
            "price": "2000",
            "fee": "0.5",
            "fee_currency": "USD",
            "liquidity": "MAKER",
            "created_at": 1700000000000i64
        });

        let fill: ParadexFillUpdate = serde_json::from_value(fill_json).unwrap();
        assert_eq!(fill.id, "fill_123");
        assert_eq!(fill.order_id, "order_456");
        assert_eq!(fill.market, "ETH-USD-PERP");
        assert_eq!(fill.side, "SELL");
        assert_eq!(fill.size, "1.5");
        assert_eq!(fill.price, "2000");
        assert_eq!(fill.fee, Some("0.5".to_string()));
        assert_eq!(fill.liquidity, Some("MAKER".to_string()));
    }

    #[test]
    fn test_position_update_parsing() {
        let position_json = serde_json::json!({
            "market": "BTC-USD-PERP",
            "side": "LONG",
            "size": "0.5",
            "entry_price": "45000",
            "mark_price": "46000",
            "unrealized_pnl": "500",
            "liquidation_price": "40000",
            "leverage": "10",
            "updated_at": 1700000000000i64
        });

        let position: ParadexPositionUpdate = serde_json::from_value(position_json).unwrap();
        assert_eq!(position.market, "BTC-USD-PERP");
        assert_eq!(position.side, "LONG");
        assert_eq!(position.size, "0.5");
        assert_eq!(position.entry_price, Some("45000".to_string()));
        assert_eq!(position.mark_price, Some("46000".to_string()));
        assert_eq!(position.unrealized_pnl, Some("500".to_string()));
        assert_eq!(position.leverage, Some("10".to_string()));
    }
}
