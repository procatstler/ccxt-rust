//! WhiteBit WebSocket Implementation
//!
//! WhiteBit real-time data streaming

#![allow(clippy::manual_strip)]
#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Ticker, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsOrderEvent,
    WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent, OHLCV,
};

const WS_PUBLIC_URL: &str = "wss://api.whitebit.com/ws";

#[allow(dead_code)]
type HmacSha512 = Hmac<Sha512>;

/// Order update data from private WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WhitebitOrderUpdateData {
    id: Option<u64>,
    #[serde(rename = "clientOrderId")]
    client_order_id: Option<String>,
    market: Option<String>,
    side: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<String>,
    price: Option<String>,
    amount: Option<String>,
    #[serde(rename = "executedAmount")]
    executed_amount: Option<String>,
    #[serde(rename = "dealFee")]
    deal_fee: Option<String>,
    status: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: Option<f64>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<f64>,
}

/// Balance update data from private WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WhitebitBalanceUpdateData {
    currency: Option<String>,
    available: Option<String>,
    freeze: Option<String>,
}

/// WhiteBit WebSocket client
pub struct WhitebitWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    next_id: Arc<RwLock<i64>>,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl WhitebitWs {
    /// Create new WhiteBit WebSocket client
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            next_id: Arc::new(RwLock::new(1)),
            api_key: None,
            api_secret: None,
        }
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            next_id: Arc::new(RwLock::new(1)),
            api_key: Some(api_key),
            api_secret: Some(api_secret),
        }
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
    }

    /// Get next request ID
    async fn get_next_id(&self) -> i64 {
        let mut id = self.next_id.write().await;
        let current = *id;
        *id += 1;
        current
    }

    /// Convert symbol to WhiteBit format (BTC/USDT -> BTC_USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Convert timeframe to WhiteBit interval (in seconds)
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "60",
            Timeframe::Minute5 => "300",
            Timeframe::Minute15 => "900",
            Timeframe::Minute30 => "1800",
            Timeframe::Hour1 => "3600",
            Timeframe::Hour4 => "14400",
            Timeframe::Hour8 => "28800",
            Timeframe::Day1 => "86400",
            Timeframe::Week1 => "604800",
            _ => "60",
        }
    }

    /// Sign request for authentication (WhiteBIT uses HMAC-SHA512)
    fn sign(&self, payload: &str) -> CcxtResult<String> {
        let secret = self.api_secret.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required for signing".into(),
        })?;

        let mut mac = HmacSha512::new_from_slice(secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            })?;

        mac.update(payload.as_bytes());
        let result = mac.finalize();
        Ok(hex::encode(result.into_bytes()))
    }

    /// Convert WhiteBIT symbol to unified format (BTC_USDT -> BTC/USDT)
    fn to_unified_symbol(symbol: &str) -> String {
        symbol.replace("_", "/")
    }

    /// Parse order from private message
    fn parse_order(&self, data: &WhitebitOrderUpdateData) -> Option<WsMessage> {
        let symbol = data.market.as_ref().map(|s| Self::to_unified_symbol(s))?;
        let order_id = data.id.map(|id| id.to_string())?;

        let status = match data.status.as_deref() {
            Some("pending") | Some("new") => OrderStatus::Open,
            Some("filled") => OrderStatus::Closed,
            Some("canceled") | Some("cancelled") | Some("rejected") => OrderStatus::Canceled,
            Some("partiallyFilled") => OrderStatus::Open,
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
            Some("stop limit") | Some("stop_limit") => OrderType::Limit,
            Some("stop market") | Some("stop_market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let amount = data.amount.as_ref().and_then(|a| Decimal::from_str(a).ok());
        let filled = data.executed_amount.as_ref().and_then(|f| Decimal::from_str(f).ok());
        let remaining = match (amount, filled) {
            (Some(a), Some(f)) => Some(a - f),
            _ => None,
        };

        let timestamp = data.created_at.map(|t| (t * 1000.0) as i64);

        let order = Order {
            id: order_id,
            client_order_id: data.client_order_id.clone(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: data.updated_at.map(|t| (t * 1000.0) as i64),
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
            cost: match (price, filled) { (Some(p), Some(f)) => Some(p * f), _ => None },
            reduce_only: None,
            post_only: None,
            trades: vec![],
            fee: None,
            fees: vec![],
            info: serde_json::to_value(data).unwrap_or(serde_json::Value::Null),
        };

        Some(WsMessage::Order(WsOrderEvent { order }))
    }

    /// Parse balance from private message
    fn parse_balance(&self, balances_data: &[WhitebitBalanceUpdateData]) -> Option<WsMessage> {
        let mut currencies: HashMap<String, Balance> = HashMap::new();

        for bal in balances_data {
            if let Some(currency) = &bal.currency {
                let available = bal.available.as_ref()
                    .and_then(|a| Decimal::from_str(a).ok())
                    .unwrap_or_default();
                let freeze = bal.freeze.as_ref()
                    .and_then(|f| Decimal::from_str(f).ok())
                    .unwrap_or_default();
                let total = available + freeze;

                currencies.insert(currency.to_uppercase(), Balance {
                    free: Some(available),
                    used: Some(freeze),
                    total: Some(total),
                    debt: None,
                });
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
            info: serde_json::Value::Null,
        };

        Some(WsMessage::Balance(WsBalanceEvent { balances }))
    }

    /// Parse ticker message
    fn parse_ticker(data: &WhitebitTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.market);
        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|v| v.parse().ok()),
            low: data.low.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: None,
            ask: data.ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.deal.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// Parse order book message
    fn parse_order_book(data: &WhitebitOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: b[0].parse().ok()?,
                    amount: b[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: a[0].parse().ok()?,
                    amount: a[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let timestamp = data.timestamp
            .map(|t| (t * 1000.0) as i64)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        }
    }

    /// Parse trade message
    #[allow(dead_code)]
    fn parse_trade(data: &WhitebitTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.market);
        let timestamp = data.time
            .map(|t| (t * 1000.0) as i64)
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.amount.parse().unwrap_or_default();

        let trades = vec![Trade {
            id: data.id.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(data.trade_type.clone()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent { symbol, trades }
    }

    /// Parse candle message
    fn parse_candle(data: &[serde_json::Value], symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        if data.len() < 6 {
            return None;
        }

        let timestamp = data[0].as_i64()?;
        let ohlcv = OHLCV::new(
            timestamp * 1000, // Convert to milliseconds
            data[1].as_str()?.parse().ok()?,
            data[4].as_str()?.parse().ok()?, // close
            data[2].as_str()?.parse().ok()?, // high
            data[3].as_str()?.parse().ok()?, // low
            data[5].as_str()?.parse().ok()?, // volume
        );

        Some(WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        })
    }

    /// Process incoming WebSocket message
    fn process_message(msg: &str) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<WhitebitWsResponse>(msg) {
            // Check for errors
            if let Some(error) = &response.error {
                return Some(WsMessage::Error(format!("{}: {}", error.code, error.message)));
            }

            // Handle result (subscription confirmation)
            if response.result.is_some() && response.id.is_some() {
                return Some(WsMessage::Subscribed {
                    channel: response.method.clone().unwrap_or_default(),
                    symbol: None,
                });
            }

            // Process updates based on method
            if let Some(method) = &response.method {
                match method.as_str() {
                    // Ticker update
                    "ticker_update" => {
                        if let Some(params) = &response.params {
                            if let Ok(ticker_data) = serde_json::from_value::<WhitebitTickerData>(params[0].clone()) {
                                return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                            }
                        }
                    }
                    // Depth update
                    "depth_update" => {
                        if let Some(params) = &response.params {
                            if params.len() >= 3 {
                                let is_snapshot = params[0].as_bool().unwrap_or(false);
                                if let Ok(ob_data) = serde_json::from_value::<WhitebitOrderBookData>(params[1].clone()) {
                                    let market = params[2].as_str().unwrap_or("");
                                    let symbol = Self::to_unified_symbol(market);
                                    return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol, is_snapshot)));
                                }
                            }
                        }
                    }
                    // Trades update
                    "trades_update" => {
                        if let Some(params) = &response.params {
                            if params.len() >= 2 {
                                let market = params[0].as_str().unwrap_or("");
                                if let Ok(trades_arr) = serde_json::from_value::<Vec<WhitebitTradeData>>(params[1].clone()) {
                                    let symbol = Self::to_unified_symbol(market);
                                    let trades: Vec<Trade> = trades_arr.iter().map(|t| {
                                        let timestamp = t.time.map(|time| (time * 1000.0) as i64)
                                            .unwrap_or_else(|| Utc::now().timestamp_millis());
                                        let price: Decimal = t.price.parse().unwrap_or_default();
                                        let amount: Decimal = t.amount.parse().unwrap_or_default();

                                        Trade {
                                            id: t.id.to_string(),
                                            order: None,
                                            timestamp: Some(timestamp),
                                            datetime: Some(
                                                chrono::DateTime::from_timestamp_millis(timestamp)
                                                    .map(|dt| dt.to_rfc3339())
                                                    .unwrap_or_default(),
                                            ),
                                            symbol: symbol.clone(),
                                            trade_type: None,
                                            side: Some(t.trade_type.clone()),
                                            taker_or_maker: None,
                                            price,
                                            amount,
                                            cost: Some(price * amount),
                                            fee: None,
                                            fees: Vec::new(),
                                            info: serde_json::to_value(t).unwrap_or_default(),
                                        }
                                    }).collect();

                                    return Some(WsMessage::Trade(WsTradeEvent { symbol, trades }));
                                }
                            }
                        }
                    }
                    // Candles update
                    "candles_update" => {
                        if let Some(params) = &response.params {
                            if let Some(candle_arr) = params.first() {
                                if let Ok(candle_data) = serde_json::from_value::<Vec<serde_json::Value>>(candle_arr.clone()) {
                                    if candle_data.len() >= 8 {
                                        let market = candle_data[7].as_str().unwrap_or("");
                                        let symbol = Self::to_unified_symbol(market);
                                        // Infer timeframe from interval - for now default to 1m
                                        // In production, you'd track subscriptions to know the timeframe
                                        let timeframe = Timeframe::Minute1;
                                        if let Some(event) = Self::parse_candle(&candle_data[..7], &symbol, timeframe) {
                                            return Some(WsMessage::Ohlcv(event));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        None
    }

    /// Subscribe to a stream and return event receiver
    async fn subscribe_stream(
        &mut self,
        method: &str,
        params: Vec<serde_json::Value>,
        channel: &str,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Send subscription message
        let id = self.get_next_id().await;
        let subscribe_msg = serde_json::json!({
            "id": id,
            "method": method,
            "params": params
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // Store subscription
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, method.to_string());
        }

        // Spawn event processing task
        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    }
                    WsEvent::Disconnected => {
                        let _ = tx.send(WsMessage::Disconnected);
                    }
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_message(&msg) {
                            let _ = tx.send(ws_msg);
                        }
                    }
                    WsEvent::Error(err) => {
                        let _ = tx.send(WsMessage::Error(err));
                    }
                    _ => {}
                }
            }
        });

        Ok(event_rx)
    }
}

impl Default for WhitebitWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for WhitebitWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let params = vec![serde_json::json!(market_id)];
        client.subscribe_stream("ticker_subscribe", params, "ticker", Some(symbol)).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let depth = limit.unwrap_or(100);
        let price_interval = "0"; // No price interval grouping
        let params = vec![
            serde_json::json!(market_id),
            serde_json::json!(depth),
            serde_json::json!(price_interval),
            serde_json::json!(true), // Allow multiple subscriptions
        ];
        client.subscribe_stream("depth_subscribe", params, "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let params = vec![serde_json::json!(market_id)];
        client.subscribe_stream("trades_subscribe", params, "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe).parse::<i64>().unwrap_or(60);
        let params = vec![
            serde_json::json!(market_id),
            serde_json::json!(interval),
        ];
        client.subscribe_stream("candles_subscribe", params, "ohlcv", Some(symbol)).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // WhiteBit connects automatically on subscription
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = &self.ws_client {
            client.close()?;
        }
        self.ws_client = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        if let Some(client) = &self.ws_client {
            client.is_connected().await
        } else {
            false
        }
    }

    // === Private Channel Methods ===

    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        // WhiteBIT private channels require API v4 implementation
        // Full implementation would:
        // 1. Connect to WebSocket
        // 2. Authenticate with HMAC-SHA512 signature
        // 3. Subscribe to "orders_pending_update" channel
        // 4. Process messages through parse_order
        Err(CcxtError::NotSupported {
            feature: "watch_orders - WhiteBIT private WebSocket requires API v4 implementation".to_string(),
        })
    }

    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        // WhiteBIT private channels require API v4 implementation
        Err(CcxtError::NotSupported {
            feature: "watch_my_trades - WhiteBIT private WebSocket requires API v4 implementation".to_string(),
        })
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        // WhiteBIT private channels require API v4 implementation
        Err(CcxtError::NotSupported {
            feature: "watch_balance - WhiteBIT private WebSocket requires API v4 implementation".to_string(),
        })
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required".into(),
            });
        }

        // WhiteBIT authentication would involve:
        // 1. Generate nonce/timestamp
        // 2. Create signature using HMAC-SHA512
        // 3. Send authorization message to WebSocket
        Ok(())
    }
}

// === WhiteBit WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct WhitebitWsResponse {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<WhitebitError>,
}

#[derive(Debug, Deserialize)]
struct WhitebitError {
    code: i32,
    message: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct WhitebitTickerData {
    #[serde(default)]
    market: String,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    bid: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    deal: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WhitebitOrderBookData {
    #[serde(default)]
    timestamp: Option<f64>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WhitebitTradeData {
    id: i64,
    #[serde(default)]
    price: String,
    #[serde(default)]
    amount: String,
    #[serde(rename = "type")]
    #[serde(default)]
    trade_type: String,
    #[serde(default)]
    time: Option<f64>,
    #[serde(default)]
    market: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(WhitebitWs::format_symbol("BTC/USDT"), "BTC_USDT");
        assert_eq!(WhitebitWs::format_symbol("ETH/USDT"), "ETH_USDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(WhitebitWs::to_unified_symbol("BTC_USDT"), "BTC/USDT");
        assert_eq!(WhitebitWs::to_unified_symbol("ETH_USDT"), "ETH/USDT");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(WhitebitWs::format_interval(Timeframe::Minute1), "60");
        assert_eq!(WhitebitWs::format_interval(Timeframe::Hour1), "3600");
        assert_eq!(WhitebitWs::format_interval(Timeframe::Day1), "86400");
    }

    #[test]
    fn test_with_credentials() {
        let _ws = WhitebitWs::with_credentials("api_key".to_string(), "api_secret".to_string());
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = WhitebitWs::new();
        ws.set_credentials("api_key".to_string(), "api_secret".to_string());
    }

    #[test]
    fn test_sign() {
        let ws = WhitebitWs::with_credentials("api_key".to_string(), "test_secret".to_string());
        let result = ws.sign("test_payload");
        assert!(result.is_ok());
        let signature = result.unwrap();
        // SHA512 signatures are 128 hex characters
        assert_eq!(signature.len(), 128);
        assert!(signature.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_parse_order() {
        let ws = WhitebitWs::new();

        let order_data = WhitebitOrderUpdateData {
            id: Some(12345),
            client_order_id: Some("client789".to_string()),
            market: Some("BTC_USDT".to_string()),
            side: Some("buy".to_string()),
            order_type: Some("limit".to_string()),
            price: Some("50000.00".to_string()),
            amount: Some("1.5".to_string()),
            executed_amount: Some("0.5".to_string()),
            deal_fee: Some("0.001".to_string()),
            status: Some("pending".to_string()),
            created_at: Some(1704067200.0),
            updated_at: Some(1704067300.0),
        };

        let result = ws.parse_order(&order_data);
        assert!(result.is_some());

        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "12345");
            assert_eq!(event.order.client_order_id, Some("client789".to_string()));
            assert_eq!(event.order.symbol, "BTC/USDT");
            assert_eq!(event.order.side, OrderSide::Buy);
            assert_eq!(event.order.order_type, OrderType::Limit);
            assert_eq!(event.order.price, Some(Decimal::from_str("50000.00").unwrap()));
            assert_eq!(event.order.amount, Decimal::from_str("1.5").unwrap());
            assert_eq!(event.order.filled, Decimal::from_str("0.5").unwrap());
            assert_eq!(event.order.remaining, Some(Decimal::from_str("1.0").unwrap()));
            assert_eq!(event.order.status, OrderStatus::Open);
        } else {
            panic!("Expected Order event");
        }
    }

    #[test]
    fn test_parse_balance() {
        let ws = WhitebitWs::new();

        let balances_data = vec![
            WhitebitBalanceUpdateData {
                currency: Some("BTC".to_string()),
                available: Some("1.5".to_string()),
                freeze: Some("0.5".to_string()),
            },
            WhitebitBalanceUpdateData {
                currency: Some("USDT".to_string()),
                available: Some("800.0".to_string()),
                freeze: Some("200.0".to_string()),
            },
        ];

        let result = ws.parse_balance(&balances_data);
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
            assert_eq!(usdt_balance.total, Some(Decimal::from_str("1000.0").unwrap()));
        } else {
            panic!("Expected Balance event");
        }
    }

    #[test]
    fn test_parse_order_status() {
        let ws = WhitebitWs::new();

        // Test pending status
        let pending_data = WhitebitOrderUpdateData {
            id: Some(1),
            client_order_id: None,
            market: Some("BTC_USDT".to_string()),
            side: Some("buy".to_string()),
            order_type: Some("limit".to_string()),
            price: Some("50000".to_string()),
            amount: Some("1".to_string()),
            executed_amount: Some("0".to_string()),
            deal_fee: None,
            status: Some("pending".to_string()),
            created_at: None,
            updated_at: None,
        };
        if let Some(WsMessage::Order(event)) = ws.parse_order(&pending_data) {
            assert_eq!(event.order.status, OrderStatus::Open);
        } else {
            panic!("Expected Order event");
        }

        // Test filled status
        let filled_data = WhitebitOrderUpdateData {
            id: Some(2),
            client_order_id: None,
            market: Some("BTC_USDT".to_string()),
            side: Some("sell".to_string()),
            order_type: Some("market".to_string()),
            price: None,
            amount: Some("1".to_string()),
            executed_amount: Some("1".to_string()),
            deal_fee: Some("0.001".to_string()),
            status: Some("filled".to_string()),
            created_at: None,
            updated_at: None,
        };
        if let Some(WsMessage::Order(event)) = ws.parse_order(&filled_data) {
            assert_eq!(event.order.status, OrderStatus::Closed);
            assert_eq!(event.order.order_type, OrderType::Market);
            assert_eq!(event.order.side, OrderSide::Sell);
        } else {
            panic!("Expected Order event");
        }

        // Test canceled status
        let canceled_data = WhitebitOrderUpdateData {
            id: Some(3),
            client_order_id: None,
            market: Some("BTC_USDT".to_string()),
            side: Some("buy".to_string()),
            order_type: Some("limit".to_string()),
            price: Some("50000".to_string()),
            amount: Some("1".to_string()),
            executed_amount: Some("0".to_string()),
            deal_fee: None,
            status: Some("canceled".to_string()),
            created_at: None,
            updated_at: None,
        };
        if let Some(WsMessage::Order(event)) = ws.parse_order(&canceled_data) {
            assert_eq!(event.order.status, OrderStatus::Canceled);
        } else {
            panic!("Expected Order event");
        }
    }
}
