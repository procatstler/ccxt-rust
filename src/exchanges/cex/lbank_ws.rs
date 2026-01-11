//! LBank WebSocket Implementation
//!
//! LBank WebSocket API for real-time market data
//! URL: wss://www.lbkex.net/ws/V2/

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Ticker,
    Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsOhlcvEvent, WsOrderBookEvent,
    WsOrderEvent, WsTickerEvent, WsTradeEvent, OHLCV,
};

const WS_URL: &str = "wss://www.lbkex.net/ws/V2/";

/// Order update data from private WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LbankOrderUpdateData {
    #[serde(rename = "orderNo")]
    order_no: Option<String>,
    #[serde(rename = "customerID")]
    customer_id: Option<String>,
    symbol: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<String>,
    price: Option<String>,
    #[serde(rename = "origQty")]
    orig_qty: Option<String>,
    #[serde(rename = "executedQty")]
    executed_qty: Option<String>,
    status: Option<String>,
    #[serde(rename = "createTime")]
    create_time: Option<i64>,
    #[serde(rename = "updateTime")]
    update_time: Option<i64>,
}

/// Balance update data from private WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LbankBalanceUpdateData {
    asset: Option<String>,
    free: Option<String>,
    locked: Option<String>,
}

/// LBank WebSocket client
pub struct LbankWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    api_key: Option<String>,
    api_secret: Option<String>,
    request_id: AtomicI64,
}

impl LbankWs {
    /// Create a new LBank WebSocket client
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

    /// Sign a message using MD5 (LBank uses MD5 for WebSocket signing)
    fn sign(&self, params: &str) -> CcxtResult<String> {
        let secret = self
            .api_secret
            .as_ref()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required for signing".into(),
            })?;

        // LBank uses MD5 signature with secret appended
        let sign_str = format!("{params}&secret_key={secret}");
        let digest = md5::compute(sign_str.as_bytes());
        Ok(format!("{digest:x}").to_uppercase())
    }

    /// Subscribe to private streams
    async fn subscribe_private_stream(
        &self,
        ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
        channel: &str,
    ) -> CcxtResult<()> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required for private channels".into(),
            })?;

        let timestamp = Utc::now().timestamp_millis();
        let params = format!("api_key={api_key}&timestamp={timestamp}");
        let signature = self.sign(&params)?;

        let auth_msg = json!({
            "action": "subscribe",
            "subscribe": channel,
            "api_key": api_key,
            "timestamp": timestamp,
            "sign": signature
        });

        ws.send(Message::Text(auth_msg.to_string()))
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: e.to_string(),
            })?;

        Ok(())
    }

    /// Process private WebSocket message
    fn process_private_message(&self, msg: &str) -> Option<WsMessage> {
        let parsed: Value = serde_json::from_str(msg).ok()?;

        let msg_type = parsed.get("type").and_then(|v| v.as_str())?;

        match msg_type {
            "orderUpdate" | "order" => {
                if let Some(data) = parsed.get("data") {
                    if let Ok(order_data) =
                        serde_json::from_value::<LbankOrderUpdateData>(data.clone())
                    {
                        return self.parse_order(&order_data);
                    }
                }
            },
            "balanceUpdate" | "balance" => {
                if let Some(data) = parsed.get("data") {
                    if let Some(balances_arr) = data.as_array() {
                        return self.parse_balance(balances_arr, &parsed);
                    }
                }
            },
            _ => {},
        }

        None
    }

    /// Parse order from private message
    fn parse_order(&self, data: &LbankOrderUpdateData) -> Option<WsMessage> {
        let symbol = data.symbol.as_ref().map(|s| self.parse_symbol(s))?;
        let order_id = data.order_no.clone()?;

        let status = match data.status.as_deref() {
            Some("0") | Some("open") => OrderStatus::Open,
            Some("1") | Some("partial") => OrderStatus::Open,
            Some("2") | Some("filled") => OrderStatus::Closed,
            Some("-1") | Some("canceled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let (side, order_type) = match data.order_type.as_deref() {
            Some("buy") | Some("buy_market") => (OrderSide::Buy, OrderType::Market),
            Some("sell") | Some("sell_market") => (OrderSide::Sell, OrderType::Market),
            Some("buy_maker") => (OrderSide::Buy, OrderType::Limit),
            Some("sell_maker") => (OrderSide::Sell, OrderType::Limit),
            _ => (OrderSide::Buy, OrderType::Limit),
        };

        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let amount = data
            .orig_qty
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok());
        let filled = data
            .executed_qty
            .as_ref()
            .and_then(|f| Decimal::from_str(f).ok());
        let remaining = match (amount, filled) {
            (Some(a), Some(f)) => Some(a - f),
            _ => None,
        };

        let order = Order {
            id: order_id,
            client_order_id: data.customer_id.clone(),
            timestamp: data.create_time,
            datetime: data.create_time.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: data.update_time,
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
            cost: match (price, filled) {
                (Some(p), Some(f)) => Some(p * f),
                _ => None,
            },
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
            if let Ok(balance_data) = serde_json::from_value::<LbankBalanceUpdateData>(bal.clone())
            {
                if let Some(asset) = &balance_data.asset {
                    let free = balance_data
                        .free
                        .as_ref()
                        .and_then(|f| Decimal::from_str(f).ok())
                        .unwrap_or_default();
                    let locked = balance_data
                        .locked
                        .as_ref()
                        .and_then(|l| Decimal::from_str(l).ok())
                        .unwrap_or_default();

                    currencies.insert(
                        asset.to_uppercase(),
                        Balance {
                            free: Some(free),
                            used: Some(locked),
                            total: Some(free + locked),
                            debt: None,
                        },
                    );
                }
            }
        }

        let timestamp = parsed
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .or_else(|| Some(Utc::now().timestamp_millis()));

        let balances = Balances {
            timestamp,
            datetime: None,
            currencies,
            info: parsed.clone(),
        };

        Some(WsMessage::Balance(WsBalanceEvent { balances }))
    }

    /// Convert unified symbol to LBank format
    /// BTC/USDT -> btc_usdt
    fn format_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "_").to_lowercase()
    }

    /// Convert LBank symbol to unified format
    /// btc_usdt -> BTC/USDT
    fn parse_symbol(&self, symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('_').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            symbol.to_uppercase()
        }
    }

    /// Convert timeframe to LBank format
    fn format_timeframe(&self, timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1min",
            Timeframe::Minute5 => "5min",
            Timeframe::Minute15 => "15min",
            Timeframe::Minute30 => "30min",
            Timeframe::Hour1 => "1hr",
            Timeframe::Hour4 => "4hr",
            Timeframe::Day1 => "day",
            Timeframe::Week1 => "week",
            Timeframe::Month1 => "month",
            _ => "1min",
        }
    }

    /// Send a subscription message
    async fn subscribe(
        &self,
        subscribe_type: &str,
        pair: &str,
        extra: Option<Value>,
    ) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let mut msg = json!({
                "action": "subscribe",
                "subscribe": subscribe_type,
                "pair": pair,
            });

            if let Some(Value::Object(map)) = extra {
                for (key, value) in map {
                    msg[key] = value;
                }
            }

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
                                    &ws,
                                )
                                .await;
                            }
                        },
                        Some(Ok(Message::Binary(bin))) => {
                            // LBank might send binary (gzipped) messages
                            if let Ok(text) = String::from_utf8(bin.to_vec()) {
                                if let Ok(data) = serde_json::from_str::<Value>(&text) {
                                    Self::handle_message_static(
                                        &data,
                                        &subscriptions,
                                        &orderbook_cache,
                                        &ws,
                                    )
                                    .await;
                                }
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
        ws: &Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>,
    ) {
        // Handle ping messages
        if let Some(action) = data.get("action").and_then(|v| v.as_str()) {
            if action == "ping" {
                if let Some(ping_id) = data.get("ping").and_then(|v| v.as_str()) {
                    let pong_msg = json!({
                        "action": "pong",
                        "pong": ping_id,
                    });
                    let mut ws_guard = ws.write().await;
                    let _ = ws_guard.send(Message::Text(pong_msg.to_string())).await;
                }
                return;
            }
        }

        let msg_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let pair = data.get("pair").and_then(|v| v.as_str()).unwrap_or("");

        match msg_type {
            "tick" => {
                if let Some(tick_data) = data.get("tick") {
                    Self::handle_ticker(pair, tick_data, subscriptions).await;
                }
            },
            "trade" => {
                if let Some(trade_data) = data.get("trade") {
                    Self::handle_trade(pair, trade_data, data, subscriptions).await;
                } else if let Some(trades) = data.get("trades") {
                    Self::handle_trades_array(pair, trades, subscriptions).await;
                }
            },
            "depth" => {
                Self::handle_orderbook(pair, data, subscriptions, orderbook_cache).await;
            },
            "kbar" => {
                if let Some(kbar_data) = data.get("kbar") {
                    let timeframe = kbar_data
                        .get("slot")
                        .and_then(|v| v.as_str())
                        .unwrap_or("1min");
                    Self::handle_ohlcv(pair, timeframe, kbar_data, subscriptions).await;
                } else if let Some(records) = data.get("records") {
                    let timeframe = data.get("kbar").and_then(|v| v.as_str()).unwrap_or("1min");
                    Self::handle_ohlcv_records(pair, timeframe, records, subscriptions).await;
                }
            },
            _ => {},
        }
    }

    /// Handle ticker message
    async fn handle_ticker(
        pair: &str,
        tick_data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let symbol = Self::parse_symbol_static(pair);
        let key = format!("ticker:{symbol}");

        let ticker = Ticker {
            symbol: symbol.clone(),
            high: tick_data
                .get("high")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Decimal>().ok()),
            low: tick_data
                .get("low")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Decimal>().ok()),
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: None,
            close: None,
            last: tick_data
                .get("latest")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Decimal>().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: tick_data
                .get("vol")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Decimal>().ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            timestamp: None,
            datetime: None,
            info: serde_json::Value::Null,
        };

        let event = WsTickerEvent {
            symbol: symbol.clone(),
            ticker,
        };

        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&key) {
            let _ = sender.send(WsMessage::Ticker(event));
        }
    }

    /// Handle single trade message
    async fn handle_trade(
        pair: &str,
        trade_data: &Value,
        _full_data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let symbol = Self::parse_symbol_static(pair);
        let key = format!("trades:{symbol}");

        let timestamp_str = trade_data.get("TS").and_then(|v| v.as_str());
        let timestamp = timestamp_str.and_then(|ts| {
            chrono::DateTime::parse_from_rfc3339(ts)
                .map(|dt| dt.timestamp_millis())
                .ok()
        });

        let direction = trade_data
            .get("direction")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let side = if direction.starts_with("buy") {
            Some("buy".to_string())
        } else {
            Some("sell".to_string())
        };

        let price = trade_data
            .get("price")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or_default();
        let amount = trade_data
            .get("volume")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or_default();

        let trade = Trade {
            id: String::new(),
            order: None,
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            symbol: symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: trade_data.clone(),
        };

        let event = WsTradeEvent {
            symbol: symbol.clone(),
            trades: vec![trade],
        };

        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&key) {
            let _ = sender.send(WsMessage::Trade(event));
        }
    }

    /// Handle trades array (from request)
    async fn handle_trades_array(
        pair: &str,
        trades: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let symbol = Self::parse_symbol_static(pair);
        let key = format!("trades:{symbol}");

        if let Some(trades_array) = trades.as_array() {
            let mut trade_list = Vec::new();
            for trade_data in trades_array {
                if let Some(arr) = trade_data.as_array() {
                    // [timestamp, price, volume, direction]
                    let direction = arr.get(3).and_then(|v| v.as_str()).unwrap_or("");
                    let side = if direction.starts_with("buy") {
                        Some("buy".to_string())
                    } else {
                        Some("sell".to_string())
                    };

                    let timestamp = arr.first().and_then(|v| v.as_i64());
                    let price = arr
                        .get(1)
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<Decimal>().ok())
                        .unwrap_or_default();
                    let amount = arr
                        .get(2)
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<Decimal>().ok())
                        .unwrap_or_default();

                    let trade = Trade {
                        id: String::new(),
                        order: None,
                        timestamp,
                        datetime: timestamp.and_then(|ts| {
                            chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                        }),
                        symbol: symbol.clone(),
                        trade_type: None,
                        side,
                        taker_or_maker: None,
                        price,
                        amount,
                        cost: Some(price * amount),
                        fee: None,
                        fees: Vec::new(),
                        info: trade_data.clone(),
                    };
                    trade_list.push(trade);
                }
            }

            if !trade_list.is_empty() {
                let event = WsTradeEvent {
                    symbol: symbol.clone(),
                    trades: trade_list,
                };

                let subs = subscriptions.read().await;
                if let Some(sender) = subs.get(&key) {
                    let _ = sender.send(WsMessage::Trade(event));
                }
            }
        }
    }

    /// Handle orderbook message
    async fn handle_orderbook(
        pair: &str,
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        let symbol = Self::parse_symbol_static(pair);
        let key = format!("orderbook:{symbol}");

        // Get orderbook data (could be nested under "depth" or at top level)
        let depth_data = data.get("depth").unwrap_or(data);

        let bids = Self::parse_orderbook_side(depth_data.get("bids"));
        let asks = Self::parse_orderbook_side(depth_data.get("asks"));

        let timestamp_str = data.get("TS").and_then(|v| v.as_str());
        let timestamp = timestamp_str.and_then(|ts| {
            chrono::DateTime::parse_from_rfc3339(ts)
                .map(|dt| dt.timestamp_millis())
                .ok()
        });

        let orderbook = OrderBook {
            symbol: symbol.clone(),
            bids,
            asks,
            checksum: None,
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            nonce: None,
        };

        // Update cache
        {
            let mut cache = orderbook_cache.write().await;
            cache.insert(symbol.clone(), orderbook.clone());
        }

        let event = WsOrderBookEvent {
            symbol: symbol.clone(),
            order_book: orderbook,
            is_snapshot: true,
        };

        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&key) {
            let _ = sender.send(WsMessage::OrderBook(event));
        }
    }

    /// Parse orderbook side
    fn parse_orderbook_side(data: Option<&Value>) -> Vec<OrderBookEntry> {
        let mut entries = Vec::new();
        if let Some(arr) = data.and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(entry_arr) = item.as_array() {
                    if entry_arr.len() >= 2 {
                        let price = entry_arr[0]
                            .as_str()
                            .and_then(|s| s.parse::<Decimal>().ok())
                            .or_else(|| {
                                entry_arr[0]
                                    .as_f64()
                                    .map(|f| Decimal::from_f64_retain(f).unwrap_or_default())
                            })
                            .unwrap_or_default();
                        let amount = entry_arr[1]
                            .as_str()
                            .and_then(|s| s.parse::<Decimal>().ok())
                            .or_else(|| {
                                entry_arr[1]
                                    .as_f64()
                                    .map(|f| Decimal::from_f64_retain(f).unwrap_or_default())
                            })
                            .unwrap_or_default();
                        entries.push(OrderBookEntry { price, amount });
                    }
                }
            }
        }
        entries
    }

    /// Handle OHLCV message (subscription)
    async fn handle_ohlcv(
        pair: &str,
        timeframe_str: &str,
        kbar_data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let symbol = Self::parse_symbol_static(pair);
        let key = format!("ohlcv:{symbol}:{timeframe_str}");

        let timestamp_str = kbar_data.get("t").and_then(|v| v.as_str());
        let timestamp = timestamp_str
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(0);

        let ohlcv = OHLCV {
            timestamp,
            open: kbar_data
                .get("o")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            high: kbar_data
                .get("h")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            low: kbar_data
                .get("l")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            close: kbar_data
                .get("c")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            volume: kbar_data
                .get("v")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
        };

        let event = WsOhlcvEvent {
            symbol: symbol.clone(),
            timeframe: Self::parse_timeframe(timeframe_str),
            ohlcv,
        };

        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&key) {
            let _ = sender.send(WsMessage::Ohlcv(event));
        }
    }

    /// Parse timeframe string to Timeframe enum
    fn parse_timeframe(tf: &str) -> Timeframe {
        match tf {
            "1min" => Timeframe::Minute1,
            "5min" => Timeframe::Minute5,
            "15min" => Timeframe::Minute15,
            "30min" => Timeframe::Minute30,
            "1hr" | "1hour" => Timeframe::Hour1,
            "4hr" | "4hour" => Timeframe::Hour4,
            "day" | "1day" => Timeframe::Day1,
            "week" | "1week" => Timeframe::Week1,
            "month" | "1month" => Timeframe::Month1,
            _ => Timeframe::Minute1,
        }
    }

    /// Handle OHLCV records (from request)
    async fn handle_ohlcv_records(
        pair: &str,
        timeframe: &str,
        records: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        use rust_decimal::prelude::*;

        let symbol = Self::parse_symbol_static(pair);
        let key = format!("ohlcv:{symbol}:{timeframe}");

        if let Some(records_array) = records.as_array() {
            for record in records_array {
                if let Some(arr) = record.as_array() {
                    // [timestamp, open, high, low, close, volume, turnover, count]
                    let ohlcv = OHLCV {
                        timestamp: arr.first().and_then(|v| v.as_i64()).unwrap_or(0) * 1000,
                        open: arr
                            .get(1)
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .or_else(|| {
                                arr.get(1)
                                    .and_then(|v| v.as_f64())
                                    .and_then(Decimal::from_f64)
                            })
                            .unwrap_or_default(),
                        high: arr
                            .get(2)
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .or_else(|| {
                                arr.get(2)
                                    .and_then(|v| v.as_f64())
                                    .and_then(Decimal::from_f64)
                            })
                            .unwrap_or_default(),
                        low: arr
                            .get(3)
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .or_else(|| {
                                arr.get(3)
                                    .and_then(|v| v.as_f64())
                                    .and_then(Decimal::from_f64)
                            })
                            .unwrap_or_default(),
                        close: arr
                            .get(4)
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .or_else(|| {
                                arr.get(4)
                                    .and_then(|v| v.as_f64())
                                    .and_then(Decimal::from_f64)
                            })
                            .unwrap_or_default(),
                        volume: arr
                            .get(5)
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .or_else(|| {
                                arr.get(5)
                                    .and_then(|v| v.as_f64())
                                    .and_then(Decimal::from_f64)
                            })
                            .unwrap_or_default(),
                    };

                    let subs = subscriptions.read().await;
                    if let Some(sender) = subs.get(&key) {
                        let _ = sender.send(WsMessage::Ohlcv(WsOhlcvEvent {
                            symbol: symbol.clone(),
                            timeframe: Self::parse_timeframe(timeframe),
                            ohlcv,
                        }));
                    }
                }
            }
        }
    }

    /// Static helper for parsing symbol
    fn parse_symbol_static(symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('_').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            symbol.to_uppercase()
        }
    }
}

impl Default for LbankWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for LbankWs {
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

        let pair = self.format_symbol(symbol);
        self.subscribe("tick", &pair, None).await?;

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

            let pair = self.format_symbol(symbol);
            self.subscribe("tick", &pair, None).await?;
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

        let pair = self.format_symbol(symbol);
        let depth = limit.unwrap_or(100);
        self.subscribe("depth", &pair, Some(json!({"depth": depth})))
            .await?;

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

            let pair = self.format_symbol(symbol);
            let depth = limit.unwrap_or(100);
            self.subscribe("depth", &pair, Some(json!({"depth": depth})))
                .await?;
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

        let pair = self.format_symbol(symbol);
        self.subscribe("trade", &pair, None).await?;

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

            let pair = self.format_symbol(symbol);
            self.subscribe("trade", &pair, None).await?;
        }

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let tf = self.format_timeframe(timeframe);
        let key = format!("ohlcv:{symbol}:{tf}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let pair = self.format_symbol(symbol);
        self.subscribe("kbar", &pair, Some(json!({"kbar": tf})))
            .await?;

        Ok(rx)
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        symbols: &[&str],
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let tf = self.format_timeframe(timeframe);

        for symbol in symbols {
            let key = format!("ohlcv:{symbol}:{tf}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }

            let pair = self.format_symbol(symbol);
            self.subscribe("kbar", &pair, Some(json!({"kbar": tf})))
                .await?;
        }

        Ok(rx)
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

        let (tx, rx) = mpsc::unbounded_channel();

        let (mut ws, _) = connect_async(WS_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: e.to_string(),
            })?;

        // Subscribe to order updates
        self.subscribe_private_stream(&mut ws, "orderUpdate")
            .await?;

        let _ = tx.send(WsMessage::Connected);

        let api_key = self.api_key.clone();
        let api_secret = self.api_secret.clone();

        tokio::spawn(async move {
            let parse_symbol = |symbol: &str| -> String {
                let parts: Vec<&str> = symbol.split('_').collect();
                if parts.len() == 2 {
                    format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
                } else {
                    symbol.to_uppercase()
                }
            };

            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                            let msg_type = parsed.get("type").and_then(|v| v.as_str());
                            if msg_type == Some("orderUpdate") || msg_type == Some("order") {
                                if let Some(data) = parsed.get("data") {
                                    if let Ok(order_data) =
                                        serde_json::from_value::<LbankOrderUpdateData>(data.clone())
                                    {
                                        if let Some(symbol_raw) = &order_data.symbol {
                                            let symbol = parse_symbol(symbol_raw);
                                            let order_id =
                                                order_data.order_no.clone().unwrap_or_default();

                                            let status = match order_data.status.as_deref() {
                                                Some("0") | Some("open") => OrderStatus::Open,
                                                Some("1") | Some("partial") => OrderStatus::Open,
                                                Some("2") | Some("filled") => OrderStatus::Closed,
                                                Some("-1") | Some("canceled") => {
                                                    OrderStatus::Canceled
                                                },
                                                _ => OrderStatus::Open,
                                            };

                                            let (side, order_type) =
                                                match order_data.order_type.as_deref() {
                                                    Some("buy") | Some("buy_market") => {
                                                        (OrderSide::Buy, OrderType::Market)
                                                    },
                                                    Some("sell") | Some("sell_market") => {
                                                        (OrderSide::Sell, OrderType::Market)
                                                    },
                                                    Some("buy_maker") => {
                                                        (OrderSide::Buy, OrderType::Limit)
                                                    },
                                                    Some("sell_maker") => {
                                                        (OrderSide::Sell, OrderType::Limit)
                                                    },
                                                    _ => (OrderSide::Buy, OrderType::Limit),
                                                };

                                            let price = order_data
                                                .price
                                                .as_ref()
                                                .and_then(|p| Decimal::from_str(p).ok());
                                            let amount = order_data
                                                .orig_qty
                                                .as_ref()
                                                .and_then(|q| Decimal::from_str(q).ok());
                                            let filled = order_data
                                                .executed_qty
                                                .as_ref()
                                                .and_then(|f| Decimal::from_str(f).ok());

                                            let order = Order {
                                                id: order_id,
                                                client_order_id: order_data.customer_id.clone(),
                                                timestamp: order_data.create_time,
                                                datetime: order_data.create_time.map(|t| {
                                                    chrono::DateTime::from_timestamp_millis(t)
                                                        .map(|dt| dt.to_rfc3339())
                                                        .unwrap_or_default()
                                                }),
                                                last_trade_timestamp: None,
                                                last_update_timestamp: order_data.update_time,
                                                status,
                                                symbol,
                                                order_type,
                                                time_in_force: None,
                                                side,
                                                price,
                                                average: None,
                                                amount: amount.unwrap_or_default(),
                                                filled: filled.unwrap_or_default(),
                                                remaining: match (amount, filled) {
                                                    (Some(a), Some(f)) => Some(a - f),
                                                    _ => None,
                                                },
                                                stop_price: None,
                                                trigger_price: None,
                                                take_profit_price: None,
                                                stop_loss_price: None,
                                                cost: match (price, filled) {
                                                    (Some(p), Some(f)) => Some(p * f),
                                                    _ => None,
                                                },
                                                reduce_only: None,
                                                post_only: None,
                                                trades: vec![],
                                                fee: None,
                                                fees: vec![],
                                                info: serde_json::to_value(&order_data)
                                                    .unwrap_or(Value::Null),
                                            };

                                            let _ =
                                                tx.send(WsMessage::Order(WsOrderEvent { order }));
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    },
                    Err(_) => break,
                    _ => {},
                }
            }
            drop(api_key);
            drop(api_secret);
        });

        Ok(rx)
    }

    async fn watch_my_trades(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // LBank sends trade info within order updates
        self.watch_orders(_symbol).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        let (tx, rx) = mpsc::unbounded_channel();

        let (mut ws, _) = connect_async(WS_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: e.to_string(),
            })?;

        // Subscribe to balance updates
        self.subscribe_private_stream(&mut ws, "balanceUpdate")
            .await?;

        let _ = tx.send(WsMessage::Connected);

        tokio::spawn(async move {
            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                            let msg_type = parsed.get("type").and_then(|v| v.as_str());
                            if msg_type == Some("balanceUpdate") || msg_type == Some("balance") {
                                if let Some(data) = parsed.get("data") {
                                    if let Some(balances_arr) = data.as_array() {
                                        let mut currencies: HashMap<String, Balance> =
                                            HashMap::new();

                                        for bal in balances_arr {
                                            if let Ok(balance_data) =
                                                serde_json::from_value::<LbankBalanceUpdateData>(
                                                    bal.clone(),
                                                )
                                            {
                                                if let Some(asset) = &balance_data.asset {
                                                    let free = balance_data
                                                        .free
                                                        .as_ref()
                                                        .and_then(|f| Decimal::from_str(f).ok())
                                                        .unwrap_or_default();
                                                    let locked = balance_data
                                                        .locked
                                                        .as_ref()
                                                        .and_then(|l| Decimal::from_str(l).ok())
                                                        .unwrap_or_default();

                                                    currencies.insert(
                                                        asset.to_uppercase(),
                                                        Balance {
                                                            free: Some(free),
                                                            used: Some(locked),
                                                            total: Some(free + locked),
                                                            debt: None,
                                                        },
                                                    );
                                                }
                                            }
                                        }

                                        let timestamp = parsed
                                            .get("timestamp")
                                            .and_then(|v| v.as_i64())
                                            .or_else(|| Some(Utc::now().timestamp_millis()));

                                        let balances = Balances {
                                            timestamp,
                                            datetime: None,
                                            currencies,
                                            info: parsed.clone(),
                                        };

                                        let _ = tx
                                            .send(WsMessage::Balance(WsBalanceEvent { balances }));
                                    }
                                }
                            }
                        }
                    },
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    },
                    Err(_) => break,
                    _ => {},
                }
            }
        });

        Ok(rx)
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for authentication".into(),
            });
        }

        let (mut ws, _) = connect_async(WS_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: e.to_string(),
            })?;

        let api_key = self.api_key.as_ref().unwrap();
        let timestamp = Utc::now().timestamp_millis();
        let params = format!("api_key={api_key}&timestamp={timestamp}");
        let signature = self.sign(&params)?;

        let auth_msg = json!({
            "action": "auth",
            "api_key": api_key,
            "timestamp": timestamp,
            "sign": signature
        });

        ws.send(Message::Text(auth_msg.to_string()))
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: e.to_string(),
            })?;

        self.ws_stream = Some(Arc::new(RwLock::new(ws)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lbank_ws_creation() {
        let _ws = LbankWs::new();
    }

    #[test]
    fn test_format_symbol() {
        let ws = LbankWs::new();
        assert_eq!(ws.format_symbol("BTC/USDT"), "btc_usdt");
        assert_eq!(ws.format_symbol("ETH/BTC"), "eth_btc");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = LbankWs::new();
        assert_eq!(ws.parse_symbol("btc_usdt"), "BTC/USDT");
        assert_eq!(ws.parse_symbol("eth_btc"), "ETH/BTC");
    }

    #[test]
    fn test_format_timeframe() {
        let ws = LbankWs::new();
        assert_eq!(ws.format_timeframe(Timeframe::Minute1), "1min");
        assert_eq!(ws.format_timeframe(Timeframe::Hour1), "1hr");
        assert_eq!(ws.format_timeframe(Timeframe::Day1), "day");
    }

    #[test]
    fn test_with_credentials() {
        let _ws = LbankWs::with_credentials("api_key".to_string(), "api_secret".to_string());
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = LbankWs::new();
        ws.set_credentials("api_key".to_string(), "api_secret".to_string());
    }

    #[test]
    fn test_sign() {
        let ws = LbankWs::with_credentials("api_key".to_string(), "test_secret".to_string());
        let result = ws.sign("api_key=api_key&timestamp=1234567890");
        assert!(result.is_ok());
        let signature = result.unwrap();
        // MD5 signatures are 32 hex characters uppercase
        assert_eq!(signature.len(), 32);
        assert!(signature.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_parse_order() {
        let ws = LbankWs::new();

        let order_data = LbankOrderUpdateData {
            order_no: Some("order123".to_string()),
            customer_id: Some("client456".to_string()),
            symbol: Some("btc_usdt".to_string()),
            order_type: Some("buy".to_string()),
            price: Some("50000.00".to_string()),
            orig_qty: Some("1.5".to_string()),
            executed_qty: Some("0.5".to_string()),
            status: Some("1".to_string()), // partial filled
            create_time: Some(1704067200000),
            update_time: Some(1704067300000),
        };

        let result = ws.parse_order(&order_data);
        assert!(result.is_some());

        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "order123".to_string());
            assert_eq!(event.order.client_order_id, Some("client456".to_string()));
            assert_eq!(event.order.symbol, "BTC/USDT");
            assert_eq!(event.order.side, OrderSide::Buy);
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
        let ws = LbankWs::new();

        let balances_arr = vec![serde_json::json!({
            "asset": "BTC",
            "free": "1.5",
            "locked": "0.5"
        })];
        let parsed = serde_json::json!({});

        let result = ws.parse_balance(&balances_arr, &parsed);
        assert!(result.is_some());

        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("BTC"));
            let btc_balance = event.balances.currencies.get("BTC").unwrap();
            assert_eq!(btc_balance.free, Some(Decimal::from_str("1.5").unwrap()));
            assert_eq!(btc_balance.used, Some(Decimal::from_str("0.5").unwrap()));
            assert_eq!(btc_balance.total, Some(Decimal::from_str("2.0").unwrap()));
        } else {
            panic!("Expected Balance event");
        }
    }

    #[test]
    fn test_parse_order_status() {
        let ws = LbankWs::new();

        // Test different order statuses
        // status -1: cancelled, 0: pending, 1: partial filled, 2: filled
        let pending_data = LbankOrderUpdateData {
            order_no: Some("order1".to_string()),
            customer_id: None,
            symbol: Some("btc_usdt".to_string()),
            order_type: Some("buy".to_string()),
            price: Some("50000".to_string()),
            orig_qty: Some("1".to_string()),
            executed_qty: Some("0".to_string()),
            status: Some("0".to_string()),
            create_time: None,
            update_time: None,
        };
        if let Some(WsMessage::Order(event)) = ws.parse_order(&pending_data) {
            assert_eq!(event.order.status, OrderStatus::Open);
        } else {
            panic!("Expected Order event");
        }

        let filled_data = LbankOrderUpdateData {
            order_no: Some("order2".to_string()),
            customer_id: None,
            symbol: Some("btc_usdt".to_string()),
            order_type: Some("sell".to_string()),
            price: Some("50000".to_string()),
            orig_qty: Some("1".to_string()),
            executed_qty: Some("1".to_string()),
            status: Some("2".to_string()),
            create_time: None,
            update_time: None,
        };
        if let Some(WsMessage::Order(event)) = ws.parse_order(&filled_data) {
            assert_eq!(event.order.status, OrderStatus::Closed);
        } else {
            panic!("Expected Order event");
        }

        let cancelled_data = LbankOrderUpdateData {
            order_no: Some("order3".to_string()),
            customer_id: None,
            symbol: Some("btc_usdt".to_string()),
            order_type: Some("buy".to_string()),
            price: Some("50000".to_string()),
            orig_qty: Some("1".to_string()),
            executed_qty: Some("0".to_string()),
            status: Some("-1".to_string()),
            create_time: None,
            update_time: None,
        };
        if let Some(WsMessage::Order(event)) = ws.parse_order(&cancelled_data) {
            assert_eq!(event.order.status, OrderStatus::Canceled);
        } else {
            panic!("Expected Order event");
        }
    }

    #[test]
    fn test_next_id() {
        let ws = LbankWs::new();
        let id1 = ws.next_id();
        let id2 = ws.next_id();
        let id3 = ws.next_id();

        assert_eq!(id2, id1 + 1);
        assert_eq!(id3, id2 + 1);
    }
}
