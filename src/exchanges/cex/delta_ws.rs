//! Delta Exchange WebSocket Implementation
//!
//! Delta Exchange WebSocket API for real-time market data
//! URL: wss://socket.india.delta.exchange (India) or wss://socket.delta.exchange (Global)

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

use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use sha2::Sha256;

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Ticker, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsOrderEvent,
    WsTickerEvent, WsTradeEvent, WsOrderBookEvent, WsOhlcvEvent, OHLCV,
};
use std::str::FromStr;

/// Delta order update data from WebSocket
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DeltaOrderUpdateData {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub product_id: Option<i64>,
    #[serde(default)]
    pub product_symbol: Option<String>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub unfilled_size: Option<String>,
    #[serde(default)]
    pub limit_price: Option<String>,
    #[serde(default)]
    pub side: Option<String>,
    #[serde(default)]
    pub order_type: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub client_order_id: Option<String>,
}

/// Delta balance update data from WebSocket
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DeltaBalanceUpdateData {
    #[serde(default)]
    pub asset_symbol: Option<String>,
    #[serde(default)]
    pub balance: Option<String>,
    #[serde(default)]
    pub available_balance: Option<String>,
    #[serde(default)]
    pub blocked_margin: Option<String>,
    #[serde(default)]
    pub pending_referral_bonus: Option<String>,
}

type HmacSha256 = Hmac<Sha256>;

const WS_URL: &str = "wss://socket.india.delta.exchange";
const WS_URL_GLOBAL: &str = "wss://socket.delta.exchange";

/// Delta Exchange WebSocket client
pub struct DeltaWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    api_key: Option<String>,
    api_secret: Option<String>,
    use_global: bool,
    markets_by_id: Arc<RwLock<HashMap<String, String>>>,
}

impl DeltaWs {
    /// Create a new Delta WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: None,
            api_secret: None,
            use_global: false,
            markets_by_id: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new Delta WebSocket client with credentials
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: Some(api_key),
            api_secret: Some(api_secret),
            use_global: false,
            markets_by_id: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new Delta WebSocket client for global endpoint
    pub fn global() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: None,
            api_secret: None,
            use_global: true,
            markets_by_id: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get WebSocket URL
    fn ws_url(&self) -> &str {
        if self.use_global {
            WS_URL_GLOBAL
        } else {
            WS_URL
        }
    }

    /// Convert unified symbol to Delta format
    /// BTC/USD -> BTCUSD
    fn format_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// Convert Delta symbol to unified format
    /// BTCUSD -> BTC/USD
    fn parse_symbol(&self, market_id: &str) -> String {
        // Try to find from cache first
        if let Ok(cache) = self.markets_by_id.try_read() {
            if let Some(symbol) = cache.get(market_id) {
                return symbol.clone();
            }
        }

        // Common patterns for Delta (derivatives exchange)
        if let Some(base) = market_id.strip_suffix("USD") {
            return format!("{base}/USD");
        } else if let Some(base) = market_id.strip_suffix("USDT") {
            return format!("{base}/USDT");
        }

        market_id.to_string()
    }

    /// Map timeframe to Delta resolution
    fn timeframe_to_resolution(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Day1 => "1d",
            Timeframe::Week1 => "1w",
            _ => "1h",
        }
    }

    /// Send a subscription message
    async fn subscribe(&self, channel: &str, symbols: &[&str]) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            // Delta subscription format
            let msg = json!({
                "type": "subscribe",
                "payload": {
                    "channels": [{
                        "name": channel,
                        "symbols": symbols
                    }]
                }
            });

            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(msg.to_string()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: self.ws_url().to_string(),
                    message: format!("Failed to send subscribe: {e}"),
                })?;
        }
        Ok(())
    }

    /// Authenticate for private channels
    async fn authenticate(&self) -> CcxtResult<()> {
        let api_key = self.api_key.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = self.api_secret.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let timestamp = Utc::now().timestamp().to_string();

        // Signature: HMAC-SHA256(GET + timestamp + /live)
        let signature_payload = format!("GET{timestamp}/live");

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError { message: "Invalid secret key".into() })?;
        mac.update(signature_payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let auth_msg = json!({
            "type": "key-auth",
            "payload": {
                "api-key": api_key,
                "signature": signature,
                "timestamp": timestamp
            }
        });

        if let Some(ws) = &self.ws_stream {
            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(auth_msg.to_string()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: self.ws_url().to_string(),
                    message: format!("Failed to authenticate: {e}"),
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
        let msg_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match msg_type {
            "v2/ticker" => {
                Self::handle_ticker(data, subscriptions).await;
            }
            "l2_orderbook" | "l2_updates" => {
                Self::handle_orderbook(data, subscriptions, orderbook_cache).await;
            }
            "all_trades" => {
                Self::handle_trades(data, subscriptions).await;
            }
            "candlestick" => {
                Self::handle_ohlcv(data, subscriptions).await;
            }
            "orders" => {
                Self::handle_orders(data, subscriptions).await;
            }
            "margins" | "portfolio_margins" => {
                Self::handle_balance(data, subscriptions).await;
            }
            _ => {
                // Check if it's a channel message with different structure
                if let Some(channel) = data.get("channel").and_then(|v| v.as_str()) {
                    match channel {
                        ch if ch.starts_with("v2/ticker") => {
                            Self::handle_ticker(data, subscriptions).await;
                        }
                        ch if ch.starts_with("l2_orderbook") || ch.starts_with("l2_updates") => {
                            Self::handle_orderbook(data, subscriptions, orderbook_cache).await;
                        }
                        ch if ch.starts_with("all_trades") => {
                            Self::handle_trades(data, subscriptions).await;
                        }
                        ch if ch.starts_with("candlestick") => {
                            Self::handle_ohlcv(data, subscriptions).await;
                        }
                        ch if ch.starts_with("orders") => {
                            Self::handle_orders(data, subscriptions).await;
                        }
                        ch if ch.starts_with("margins") || ch.starts_with("portfolio_margins") => {
                            Self::handle_balance(data, subscriptions).await;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Handle ticker messages
    async fn handle_ticker(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let market_id = data.get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let symbol = Self::parse_symbol_static(market_id);
        let key = format!("ticker:{symbol}");

        let ticker_data = data.get("data").unwrap_or(data);

        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: ticker_data.get("high").and_then(Self::parse_decimal_opt),
            low: ticker_data.get("low").and_then(Self::parse_decimal_opt),
            bid: ticker_data.get("best_bid").and_then(Self::parse_decimal_opt),
            bid_volume: None,
            ask: ticker_data.get("best_ask").and_then(Self::parse_decimal_opt),
            ask_volume: None,
            vwap: None,
            open: ticker_data.get("open").and_then(Self::parse_decimal_opt),
            close: ticker_data.get("close").and_then(Self::parse_decimal_opt),
            last: ticker_data.get("close").and_then(Self::parse_decimal_opt),
            previous_close: None,
            change: None,
            percentage: ticker_data.get("price_change_percent_24h")
                .and_then(Self::parse_decimal_opt),
            average: None,
            base_volume: ticker_data.get("volume").and_then(Self::parse_decimal_opt),
            quote_volume: ticker_data.get("turnover").and_then(Self::parse_decimal_opt),
            index_price: ticker_data.get("spot_price").and_then(Self::parse_decimal_opt),
            mark_price: ticker_data.get("mark_price").and_then(Self::parse_decimal_opt),
            info: data.clone(),
        };

        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&key) {
            let _ = sender.send(WsMessage::Ticker(WsTickerEvent {
                symbol,
                ticker,
            }));
        }
    }

    /// Handle trades messages
    async fn handle_trades(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let market_id = data.get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let symbol = Self::parse_symbol_static(market_id);
        let key = format!("trades:{symbol}");

        let trades_array = if let Some(arr) = data.get("data").and_then(|v| v.as_array()) {
            arr.clone()
        } else if let Some(arr) = data.get("recent_trades").and_then(|v| v.as_array()) {
            arr.clone()
        } else if data.get("data").is_some() {
            vec![data.get("data").unwrap().clone()]
        } else {
            return;
        };

        let mut trades = Vec::new();
        for trade_data in &trades_array {
            let price = Self::parse_decimal(trade_data.get("price").unwrap_or(&json!(0)));
            let amount = Self::parse_decimal(trade_data.get("size").unwrap_or(&json!(0)));

            let timestamp = trade_data.get("timestamp")
                .and_then(|v| v.as_i64())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            let trade_id = trade_data.get("id")
                .and_then(|v| {
                    if v.is_i64() {
                        Some(v.as_i64().unwrap().to_string())
                    } else if v.is_string() {
                        Some(v.as_str().unwrap().to_string())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| timestamp.to_string());

            let mut trade = Trade::new(
                trade_id,
                symbol.clone(),
                price,
                amount,
            );

            trade = trade.with_timestamp(timestamp);

            // Delta uses buyer_role to indicate side
            if let Some(buyer_role) = trade_data.get("buyer_role").and_then(|v| v.as_str()) {
                let side = if buyer_role == "taker" { "buy" } else { "sell" };
                trade = trade.with_side(side);
            }

            trades.push(trade);
        }

        if !trades.is_empty() {
            let subs = subscriptions.read().await;
            if let Some(sender) = subs.get(&key) {
                let _ = sender.send(WsMessage::Trade(WsTradeEvent {
                    symbol,
                    trades,
                }));
            }
        }
    }

    /// Handle orderbook messages
    async fn handle_orderbook(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        let market_id = data.get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let symbol = Self::parse_symbol_static(market_id);
        let key = format!("orderbook:{symbol}");

        let msg_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let is_snapshot = msg_type == "l2_orderbook";

        let ob_data = data.get("data").unwrap_or(data);

        let mut bids: Vec<OrderBookEntry> = Vec::new();
        let mut asks: Vec<OrderBookEntry> = Vec::new();

        // Parse bids (buy orders)
        if let Some(buy_data) = ob_data.get("buy").and_then(|v| v.as_array()) {
            for entry in buy_data {
                let price = Self::parse_decimal(entry.get("price").unwrap_or(&json!(0)));
                let amount = Self::parse_decimal(entry.get("size").unwrap_or(&json!(0)));
                if !price.is_zero() {
                    bids.push(OrderBookEntry { price, amount });
                }
            }
        }

        // Parse asks (sell orders)
        if let Some(sell_data) = ob_data.get("sell").and_then(|v| v.as_array()) {
            for entry in sell_data {
                let price = Self::parse_decimal(entry.get("price").unwrap_or(&json!(0)));
                let amount = Self::parse_decimal(entry.get("size").unwrap_or(&json!(0)));
                if !price.is_zero() {
                    asks.push(OrderBookEntry { price, amount });
                }
            }
        }

        // Sort bids descending, asks ascending
        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

        let timestamp = Some(Utc::now().timestamp_millis());

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
                symbol,
                order_book: orderbook,
                is_snapshot,
            }));
        }
    }

    /// Handle OHLCV/candlestick messages
    async fn handle_ohlcv(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let market_id = data.get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let symbol = Self::parse_symbol_static(market_id);

        // Extract resolution from channel name or message
        let resolution = data.get("resolution")
            .and_then(|v| v.as_str())
            .unwrap_or("1h");

        let key = format!("ohlcv:{symbol}:{resolution}");

        let candle_data = data.get("data").unwrap_or(data);

        let timestamp = candle_data.get("time")
            .or_else(|| candle_data.get("timestamp"))
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| Utc::now().timestamp()) * 1000;

        let ohlcv = OHLCV {
            timestamp,
            open: Self::parse_decimal(candle_data.get("open").unwrap_or(&json!(0))),
            high: Self::parse_decimal(candle_data.get("high").unwrap_or(&json!(0))),
            low: Self::parse_decimal(candle_data.get("low").unwrap_or(&json!(0))),
            close: Self::parse_decimal(candle_data.get("close").unwrap_or(&json!(0))),
            volume: Self::parse_decimal(candle_data.get("volume").unwrap_or(&json!(0))),
        };

        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&key) {
            let _ = sender.send(WsMessage::Ohlcv(WsOhlcvEvent {
                symbol,
                timeframe: Self::resolution_to_timeframe(resolution),
                ohlcv,
            }));
        }
    }

    /// Convert resolution string to Timeframe
    fn resolution_to_timeframe(resolution: &str) -> Timeframe {
        match resolution {
            "1m" => Timeframe::Minute1,
            "5m" => Timeframe::Minute5,
            "15m" => Timeframe::Minute15,
            "30m" => Timeframe::Minute30,
            "1h" => Timeframe::Hour1,
            "2h" => Timeframe::Hour2,
            "4h" => Timeframe::Hour4,
            "6h" => Timeframe::Hour6,
            "1d" => Timeframe::Day1,
            "1w" => Timeframe::Week1,
            _ => Timeframe::Hour1,
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

    /// Parse Value to Option<Decimal>
    fn parse_decimal_opt(value: &Value) -> Option<Decimal> {
        if let Some(s) = value.as_str() {
            s.parse::<Decimal>().ok()
        } else if let Some(f) = value.as_f64() {
            Decimal::from_f64(f)
        } else if let Some(i) = value.as_i64() {
            Decimal::from_i64(i)
        } else {
            None
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
    fn parse_symbol_static(market_id: &str) -> String {
        // Common patterns for Delta (derivatives exchange)
        if let Some(base) = market_id.strip_suffix("USD") {
            return format!("{base}/USD");
        } else if let Some(base) = market_id.strip_suffix("USDT") {
            return format!("{base}/USDT");
        }

        market_id.to_string()
    }

    /// Generate signature for authentication
    pub fn sign(&self, message: &str) -> CcxtResult<String> {
        let api_secret = self.api_secret.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required for signing".into(),
        })?;

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError { message: "Invalid secret key".into() })?;
        mac.update(message.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Parse order update data to unified Order type
    pub fn parse_order(&self, data: &DeltaOrderUpdateData) -> Option<WsMessage> {
        let order_id = data.id.map(|id| id.to_string())?;
        let symbol = data.product_symbol.as_ref()
            .map(|s| Self::parse_symbol_static(s))
            .unwrap_or_default();

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit_order") => OrderType::Limit,
            Some("market_order") => OrderType::Market,
            Some("stop_limit_order") => OrderType::StopLimit,
            Some("stop_market_order") => OrderType::StopMarket,
            _ => OrderType::Limit,
        };

        let status = match data.state.as_deref() {
            Some("open") => OrderStatus::Open,
            Some("pending") => OrderStatus::Open,
            Some("closed") => OrderStatus::Closed,
            Some("cancelled") => OrderStatus::Canceled,
            Some("partially_filled") => OrderStatus::Open,
            _ => OrderStatus::Open,
        };

        let amount = data.size.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();
        let unfilled = data.unfilled_size.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();
        let filled = amount - unfilled;
        let price = data.limit_price.as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let timestamp = data.created_at.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order = Order {
            id: order_id,
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: data.created_at.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: data.updated_at.as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis()),
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force: None,
            side,
            price,
            trigger_price: None,
            average: None,
            amount,
            filled,
            remaining: Some(unfilled),
            cost: None,
            trades: vec![],
            fee: None,
            fees: vec![],
            info: serde_json::json!(data),
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        };

        Some(WsMessage::Order(WsOrderEvent { order }))
    }

    /// Parse balance update data to unified Balances type
    pub fn parse_balance(&self, data: &DeltaBalanceUpdateData) -> Option<WsMessage> {
        let currency = data.asset_symbol.as_ref()?.to_uppercase();

        let total = data.balance.as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let free = data.available_balance.as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let used = match (total, free) {
            (Some(t), Some(f)) => Some(t - f),
            _ => None,
        };

        let balance = Balance {
            free,
            used,
            total,
            debt: None,
        };

        let mut balances = Balances {
            info: serde_json::json!(data),
            ..Default::default()
        };
        balances.currencies.insert(currency.clone(), balance);

        Some(WsMessage::Balance(WsBalanceEvent {
            balances,
        }))
    }

    /// Handle orders messages
    async fn handle_orders(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let order_data = data.get("data").unwrap_or(data);

        if let Ok(order_update) = serde_json::from_value::<DeltaOrderUpdateData>(order_data.clone()) {
            let symbol = order_update.product_symbol.as_ref()
                .map(|s| Self::parse_symbol_static(s))
                .unwrap_or_default();

            // Create dummy self for parse_order
            let ws = DeltaWs::new();
            if let Some(ws_message) = ws.parse_order(&order_update) {
                let subs = subscriptions.read().await;

                // Try symbol-specific subscription first
                let key = format!("orders:{symbol}");
                if let Some(sender) = subs.get(&key) {
                    let _ = sender.send(ws_message.clone());
                }

                // Also send to "all" subscription
                if let Some(sender) = subs.get("orders:all") {
                    let _ = sender.send(ws_message);
                }
            }
        }
    }

    /// Handle balance/margins messages
    async fn handle_balance(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let balance_data = data.get("data").unwrap_or(data);

        if let Ok(balance_update) = serde_json::from_value::<DeltaBalanceUpdateData>(balance_data.clone()) {
            let ws = DeltaWs::new();
            if let Some(ws_message) = ws.parse_balance(&balance_update) {
                let subs = subscriptions.read().await;
                if let Some(sender) = subs.get("balance") {
                    let _ = sender.send(ws_message);
                }
            }
        }
    }
}

impl Default for DeltaWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for DeltaWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        let url = self.ws_url();

        let (ws_stream, _) = connect_async(url)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.to_string(),
                message: format!("WebSocket connection failed: {e}"),
            })?;

        self.ws_stream = Some(Arc::new(RwLock::new(ws_stream)));
        self.start_message_loop();

        // Authenticate if credentials are available
        if self.api_key.is_some() && self.api_secret.is_some() {
            self.authenticate().await?;
        }

        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let mut ws_guard = ws.write().await;
            ws_guard
                .close(None)
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: self.ws_url().to_string(),
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
        self.subscribe("v2/ticker", &[&market_id]).await?;

        Ok(rx)
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        let market_ids: Vec<String> = symbols.iter()
            .map(|s| self.format_symbol(s))
            .collect();

        for symbol in symbols {
            let key = format!("ticker:{symbol}");
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx.clone());
        }

        let market_id_refs: Vec<&str> = market_ids.iter().map(|s| s.as_str()).collect();
        self.subscribe("v2/ticker", &market_id_refs).await?;

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
        self.subscribe("l2_orderbook", &[&market_id]).await?;

        Ok(rx)
    }

    async fn watch_order_book_for_symbols(
        &self,
        symbols: &[&str],
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        let market_ids: Vec<String> = symbols.iter()
            .map(|s| self.format_symbol(s))
            .collect();

        for symbol in symbols {
            let key = format!("orderbook:{symbol}");
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx.clone());
        }

        let market_id_refs: Vec<&str> = market_ids.iter().map(|s| s.as_str()).collect();
        self.subscribe("l2_orderbook", &market_id_refs).await?;

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
        self.subscribe("all_trades", &[&market_id]).await?;

        Ok(rx)
    }

    async fn watch_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        let market_ids: Vec<String> = symbols.iter()
            .map(|s| self.format_symbol(s))
            .collect();

        for symbol in symbols {
            let key = format!("trades:{symbol}");
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx.clone());
        }

        let market_id_refs: Vec<&str> = market_ids.iter().map(|s| s.as_str()).collect();
        self.subscribe("all_trades", &market_id_refs).await?;

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let resolution = Self::timeframe_to_resolution(timeframe);
        let key = format!("ohlcv:{symbol}:{resolution}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        let channel = format!("candlestick_{resolution}");
        self.subscribe(&channel, &[&market_id]).await?;

        Ok(rx)
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        symbols: &[&str],
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let resolution = Self::timeframe_to_resolution(timeframe);

        let market_ids: Vec<String> = symbols.iter()
            .map(|s| self.format_symbol(s))
            .collect();

        for symbol in symbols {
            let key = format!("ohlcv:{symbol}:{resolution}");
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx.clone());
        }

        let channel = format!("candlestick_{resolution}");
        let market_id_refs: Vec<&str> = market_ids.iter().map(|s| s.as_str()).collect();
        self.subscribe(&channel, &market_id_refs).await?;

        Ok(rx)
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private streams".to_string(),
            });
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let key = "balance".to_string();

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        // Subscribe to margins channel for balance updates
        self.subscribe("margins", &["all"]).await?;

        Ok(rx)
    }

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private streams".to_string(),
            });
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let key = if let Some(s) = symbol {
            format!("orders:{s}")
        } else {
            "orders:all".to_string()
        };

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        // Subscribe to orders channel
        let symbols = if let Some(s) = symbol {
            vec![self.format_symbol(s)]
        } else {
            vec!["all".to_string()]
        };
        let symbol_refs: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
        self.subscribe("orders", &symbol_refs).await?;

        Ok(rx)
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        self.authenticate().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_ws_creation() {
        let _ws = DeltaWs::new();
    }

    #[test]
    fn test_delta_ws_global() {
        let ws = DeltaWs::global();
        assert!(ws.use_global);
        assert_eq!(ws.ws_url(), WS_URL_GLOBAL);
    }

    #[test]
    fn test_delta_ws_with_credentials() {
        let ws = DeltaWs::with_credentials("test_key".into(), "test_secret".into());
        assert!(ws.api_key.is_some());
        assert!(ws.api_secret.is_some());
    }

    #[test]
    fn test_format_symbol() {
        let ws = DeltaWs::new();
        assert_eq!(ws.format_symbol("BTC/USD"), "BTCUSD");
        assert_eq!(ws.format_symbol("ETH/USDT"), "ETHUSDT");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = DeltaWs::new();
        assert_eq!(ws.parse_symbol("BTCUSD"), "BTC/USD");
        assert_eq!(ws.parse_symbol("ETHUSDT"), "ETH/USDT");
    }

    #[test]
    fn test_parse_decimal() {
        assert_eq!(DeltaWs::parse_decimal(&json!("123.45")), Decimal::new(12345, 2));
        assert_eq!(DeltaWs::parse_decimal(&json!(100)), Decimal::from(100));
    }

    #[test]
    fn test_timeframe_resolution() {
        assert_eq!(DeltaWs::timeframe_to_resolution(Timeframe::Minute1), "1m");
        assert_eq!(DeltaWs::timeframe_to_resolution(Timeframe::Hour1), "1h");
        assert_eq!(DeltaWs::timeframe_to_resolution(Timeframe::Day1), "1d");
    }

    #[test]
    fn test_sign() {
        let ws = DeltaWs::with_credentials("test_key".into(), "test_secret".into());
        let signature = ws.sign("GET1234567890/live").unwrap();
        assert!(!signature.is_empty());
        assert_eq!(signature.len(), 64); // SHA256 hex is 64 chars
    }

    #[test]
    fn test_parse_order() {
        let ws = DeltaWs::new();
        let order_data = DeltaOrderUpdateData {
            id: Some(12345),
            product_symbol: Some("BTCUSD".to_string()),
            size: Some("1.5".to_string()),
            unfilled_size: Some("0.5".to_string()),
            limit_price: Some("50000".to_string()),
            side: Some("buy".to_string()),
            order_type: Some("limit_order".to_string()),
            state: Some("open".to_string()),
            created_at: Some("2025-01-01T00:00:00Z".to_string()),
            ..Default::default()
        };

        let result = ws.parse_order(&order_data);
        assert!(result.is_some());

        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "12345");
            assert_eq!(event.order.symbol, "BTC/USD");
            assert_eq!(event.order.side, OrderSide::Buy);
            assert_eq!(event.order.order_type, OrderType::Limit);
            assert_eq!(event.order.status, OrderStatus::Open);
            assert_eq!(event.order.amount, Decimal::new(15, 1));
            assert_eq!(event.order.filled, Decimal::new(10, 1));
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_parse_balance() {
        let ws = DeltaWs::new();
        let balance_data = DeltaBalanceUpdateData {
            asset_symbol: Some("USD".to_string()),
            balance: Some("10000".to_string()),
            available_balance: Some("8000".to_string()),
            blocked_margin: Some("2000".to_string()),
            ..Default::default()
        };

        let result = ws.parse_balance(&balance_data);
        assert!(result.is_some());

        if let Some(WsMessage::Balance(event)) = result {
            let usd_balance = event.balances.currencies.get("USD");
            assert!(usd_balance.is_some());
            let balance = usd_balance.unwrap();
            assert_eq!(balance.total, Some(Decimal::from(10000)));
            assert_eq!(balance.free, Some(Decimal::from(8000)));
            assert_eq!(balance.used, Some(Decimal::from(2000)));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_order_status_parsing() {
        let ws = DeltaWs::new();

        let test_cases = vec![
            ("open", OrderStatus::Open),
            ("pending", OrderStatus::Open),
            ("closed", OrderStatus::Closed),
            ("cancelled", OrderStatus::Canceled),
            ("partially_filled", OrderStatus::Open),
        ];

        for (state, expected_status) in test_cases {
            let order_data = DeltaOrderUpdateData {
                id: Some(1),
                product_symbol: Some("BTCUSD".to_string()),
                state: Some(state.to_string()),
                ..Default::default()
            };

            if let Some(WsMessage::Order(event)) = ws.parse_order(&order_data) {
                assert_eq!(event.order.status, expected_status, "Failed for state: {state}");
            }
        }
    }

    #[test]
    fn test_order_type_parsing() {
        let ws = DeltaWs::new();

        let test_cases = vec![
            ("limit_order", OrderType::Limit),
            ("market_order", OrderType::Market),
            ("stop_limit_order", OrderType::StopLimit),
            ("stop_market_order", OrderType::StopMarket),
        ];

        for (order_type, expected_type) in test_cases {
            let order_data = DeltaOrderUpdateData {
                id: Some(1),
                product_symbol: Some("BTCUSD".to_string()),
                order_type: Some(order_type.to_string()),
                ..Default::default()
            };

            if let Some(WsMessage::Order(event)) = ws.parse_order(&order_data) {
                assert_eq!(event.order.order_type, expected_type, "Failed for type: {order_type}");
            }
        }
    }
}
