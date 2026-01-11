//! Bitstamp WebSocket Implementation
//!
//! Bitstamp real-time data streaming

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    TakerOrMaker, Ticker, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsMyTradeEvent,
    WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_PUBLIC_URL: &str = "wss://ws.bitstamp.net";

/// Bitstamp WebSocket client
pub struct BitstampWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    private_ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    authenticated: Arc<RwLock<bool>>,
}

impl Clone for BitstampWs {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::clone(&self.subscriptions),
            event_tx: self.event_tx.clone(),
            authenticated: Arc::clone(&self.authenticated),
        }
    }
}

impl BitstampWs {
    /// Create new Bitstamp WebSocket client
    pub fn new() -> Self {
        Self {
            config: None,
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            authenticated: Arc::new(RwLock::new(false)),
        }
    }

    /// Create new Bitstamp WebSocket client with config
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            authenticated: Arc::new(RwLock::new(false)),
        }
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        let config = ExchangeConfig::default()
            .with_api_key(api_key)
            .with_api_secret(api_secret);
        Self::with_config(config)
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        let config = ExchangeConfig::default()
            .with_api_key(api_key)
            .with_api_secret(api_secret);
        self.config = Some(config);
    }

    /// Generate authentication signature for WebSocket
    /// Bitstamp WebSocket uses nonce-based HMAC-SHA256 signature
    fn generate_auth_signature(
        api_key: &str,
        api_secret: &str,
        nonce: &str,
        timestamp: &str,
    ) -> CcxtResult<String> {
        // Bitstamp WebSocket signature format: "BITSTAMP {api_key}" + message parts
        let message = format!("BITSTAMP {api_key}{nonce}{timestamp}websocket_token");

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(message.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()).to_uppercase())
    }

    /// Subscribe to private stream with authentication
    async fn subscribe_private_stream(
        &mut self,
        channel: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Config required for private channels".into(),
            })?;

        let api_key = config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let api_secret = config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 15,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // Generate nonce and timestamp for authentication
        let nonce = Uuid::new_v4().to_string();
        let timestamp = Utc::now().timestamp_millis().to_string();
        let signature = Self::generate_auth_signature(api_key, api_secret, &nonce, &timestamp)?;

        // First, authenticate by subscribing to private channel with auth params
        // Bitstamp uses private channel names like "private-my_orders", "private-my_trades"
        let auth_msg = serde_json::json!({
            "event": "bts:subscribe",
            "data": {
                "channel": channel,
                "auth": {
                    "key": api_key,
                    "signature": signature,
                    "nonce": nonce,
                    "timestamp": timestamp
                }
            }
        });

        ws_client.send(&auth_msg.to_string())?;
        self.private_ws_client = Some(ws_client);
        *self.authenticated.write().await = true;

        // Event processing task
        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    },
                    WsEvent::Disconnected => {
                        let _ = tx.send(WsMessage::Disconnected);
                    },
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_private_message(&msg) {
                            let _ = tx.send(ws_msg);
                        }
                    },
                    WsEvent::Error(err) => {
                        let _ = tx.send(WsMessage::Error(err));
                    },
                    _ => {},
                }
            }
        });

        Ok(event_rx)
    }

    /// Process private channel messages
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        let json: BitstampWsMessage = serde_json::from_str(msg).ok()?;

        match json.event.as_str() {
            "bts:subscription_succeeded" => Some(WsMessage::Subscribed {
                channel: json.channel.clone(),
                symbol: None,
            }),
            "data" => {
                let channel = json.channel.as_str();

                if channel.contains("my_orders") {
                    if let Some(data) = &json.data {
                        if let Ok(order_data) =
                            serde_json::from_value::<BitstampOrderData>(data.clone())
                        {
                            return Some(WsMessage::Order(Self::parse_order_update(&order_data)));
                        }
                    }
                } else if channel.contains("my_trades") {
                    if let Some(data) = &json.data {
                        if let Ok(trade_data) =
                            serde_json::from_value::<BitstampMyTradeData>(data.clone())
                        {
                            return Some(WsMessage::MyTrade(Self::parse_my_trade(&trade_data)));
                        }
                    }
                } else if channel.contains("balance") {
                    if let Some(data) = &json.data {
                        if let Ok(balance_data) =
                            serde_json::from_value::<BitstampBalanceData>(data.clone())
                        {
                            return Some(WsMessage::Balance(Self::parse_balance_update(
                                &balance_data,
                            )));
                        }
                    }
                }

                None
            },
            _ => None,
        }
    }

    /// Parse balance update
    fn parse_balance_update(data: &BitstampBalanceData) -> WsBalanceEvent {
        let mut currencies = HashMap::new();

        if let Some(currency) = &data.currency {
            let balance = Balance {
                free: data
                    .available
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok()),
                used: data
                    .reserved
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok()),
                total: data
                    .balance
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok()),
                debt: None,
            };
            currencies.insert(currency.to_uppercase(), balance);
        }

        WsBalanceEvent {
            balances: Balances {
                info: serde_json::to_value(data).unwrap_or_default(),
                currencies,
                timestamp: Some(Utc::now().timestamp_millis()),
                datetime: Some(Utc::now().to_rfc3339()),
            },
        }
    }

    /// Parse order update
    fn parse_order_update(data: &BitstampOrderData) -> WsOrderEvent {
        let symbol = data
            .currency_pair
            .as_ref()
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let side = match data.order_type.as_deref() {
            Some("0") | Some("buy") => OrderSide::Buy,
            Some("1") | Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy, // Default to Buy if unknown
        };

        let status = match data.order_status.as_deref() {
            Some("0") | Some("open") => OrderStatus::Open,
            Some("1") | Some("filled") => OrderStatus::Closed,
            Some("2") | Some("partial") => OrderStatus::Open,
            Some("3") | Some("canceled") => OrderStatus::Canceled,
            _ => OrderStatus::Open, // Default to Open if unknown
        };

        let timestamp = data.datetime.as_ref().and_then(|dt| {
            chrono::DateTime::parse_from_rfc3339(dt)
                .ok()
                .map(|d| d.timestamp_millis())
        });

        let order = Order {
            id: data.id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            symbol: symbol.clone(),
            order_type: OrderType::Limit,
            side,
            price: data.price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            amount: data
                .amount
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok())
                .unwrap_or_default(),
            cost: None,
            average: data
                .avg_price
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            filled: data
                .amount_filled
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok())
                .unwrap_or_default(),
            remaining: data
                .amount_remaining
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            stop_price: None,
            status,
            fee: None,
            fees: Vec::new(),
            trades: Vec::new(),
            timestamp,
            datetime: data.datetime.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            time_in_force: None,
            post_only: None,
            reduce_only: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// Parse my trade (private trade)
    fn parse_my_trade(data: &BitstampMyTradeData) -> WsMyTradeEvent {
        let symbol = data
            .currency_pair
            .as_ref()
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let timestamp = data.microtimestamp.map(|mt| mt / 1000);

        let price = data
            .price
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();
        let amount = data
            .amount
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let fee = data.fee.as_ref().and_then(|v| Decimal::from_str(v).ok());
        let fee_obj = fee.map(|f| Fee {
            cost: Some(f),
            currency: data.fee_currency.clone(),
            rate: None,
        });

        let trade = Trade {
            id: data.id.clone().unwrap_or_default(),
            order: data.order_id.clone(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            symbol: symbol.clone(),
            trade_type: data.trade_type.clone(),
            side: match data.trade_type.as_deref() {
                Some("0") | Some("buy") => Some("buy".to_string()),
                Some("1") | Some("sell") => Some("sell".to_string()),
                _ => None,
            },
            taker_or_maker: data.taker_or_maker.as_ref().map(|t| {
                if t == "taker" {
                    TakerOrMaker::Taker
                } else {
                    TakerOrMaker::Maker
                }
            }),
            price,
            amount,
            cost: Some(price * amount),
            fee: fee_obj,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsMyTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    /// Convert symbol to Bitstamp WebSocket format (BTC/USD -> btcusd)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "").to_lowercase()
    }

    /// Convert Bitstamp symbol to unified format (btcusd -> BTC/USD)
    fn to_unified_symbol(bitstamp_symbol: &str) -> String {
        // Simple heuristic: split into base/quote based on common patterns
        let upper = bitstamp_symbol.to_uppercase();

        // Common quote currencies
        if upper.ends_with("USD") {
            let base = &upper[..upper.len() - 3];
            return format!("{base}/USD");
        } else if upper.ends_with("USDT") {
            let base = &upper[..upper.len() - 4];
            return format!("{base}/USDT");
        } else if upper.ends_with("EUR") {
            let base = &upper[..upper.len() - 3];
            return format!("{base}/EUR");
        } else if upper.ends_with("BTC") {
            let base = &upper[..upper.len() - 3];
            return format!("{base}/BTC");
        } else if upper.ends_with("ETH") {
            let base = &upper[..upper.len() - 3];
            return format!("{base}/ETH");
        }

        bitstamp_symbol.to_uppercase()
    }

    /// Parse ticker message
    fn parse_ticker(data: &BitstampTickerData, symbol: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: data.low.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: data.bid.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: None,
            ask: data.ask.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: None,
            vwap: data.vwap.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            open: data.open.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            close: None,
            last: data.last.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent {
            symbol: unified_symbol,
            ticker,
        }
    }

    /// Parse order book message
    fn parse_order_book(data: &BitstampOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data
            .microtimestamp
            .map(|mt| mt / 1000)
            .or_else(|| Some(Utc::now().timestamp_millis()))
            .unwrap();

        let parse_entries = |entries: &Vec<Vec<String>>| -> Vec<OrderBookEntry> {
            entries
                .iter()
                .filter_map(|e| {
                    if e.len() >= 2 {
                        Some(OrderBookEntry {
                            price: Decimal::from_str(&e[0]).ok()?,
                            amount: Decimal::from_str(&e[1]).ok()?,
                        })
                    } else {
                        None
                    }
                })
                .collect()
        };

        let bids = parse_entries(&data.bids);
        let asks = parse_entries(&data.asks);

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
            bids,
            asks,
            checksum: None,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot: true,
        }
    }

    /// Parse diff order book message
    fn parse_order_book_diff(data: &BitstampOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let mut event = Self::parse_order_book(data, symbol);
        event.is_snapshot = false;
        event
    }

    /// Parse trade message
    fn parse_trade(data: &BitstampTradeData, symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data
            .microtimestamp
            .map(|mt| mt / 1000)
            .or_else(|| Some(Utc::now().timestamp_millis()))
            .unwrap();

        let price = Decimal::from_str(&data.price.to_string()).unwrap_or_default();
        let amount = Decimal::from_str(&data.amount.to_string()).unwrap_or_default();

        let trade = Trade {
            id: data.id.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
            trade_type: Some(match data.trade_type {
                0 => "buy".to_string(),
                1 => "sell".to_string(),
                _ => "unknown".to_string(),
            }),
            side: Some(match data.trade_type {
                0 => "buy".to_string(),
                1 => "sell".to_string(),
                _ => "unknown".to_string(),
            }),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTradeEvent {
            symbol: unified_symbol,
            trades: vec![trade],
        }
    }

    /// Process incoming WebSocket messages
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Parse the JSON message
        let json: BitstampWsMessage = serde_json::from_str(msg).ok()?;

        match json.event.as_str() {
            "bts:subscription_succeeded" => Some(WsMessage::Subscribed {
                channel: json.channel.clone(),
                symbol: None,
            }),
            "data" => {
                let channel = json.channel.as_str();

                // Extract symbol from channel name
                // Format: "live_ticker_btcusd", "order_book_btcusd", "live_trades_btcusd"
                let symbol = if let Some(idx) = channel.rfind('_') {
                    &channel[idx + 1..]
                } else {
                    ""
                };

                if channel.starts_with("live_ticker") {
                    if let Some(data) = &json.data {
                        if let Ok(ticker_data) =
                            serde_json::from_value::<BitstampTickerData>(data.clone())
                        {
                            return Some(WsMessage::Ticker(Self::parse_ticker(
                                &ticker_data,
                                symbol,
                            )));
                        }
                    }
                } else if channel.starts_with("order_book") {
                    if let Some(data) = &json.data {
                        if let Ok(ob_data) =
                            serde_json::from_value::<BitstampOrderBookData>(data.clone())
                        {
                            return Some(WsMessage::OrderBook(Self::parse_order_book(
                                &ob_data, symbol,
                            )));
                        }
                    }
                } else if channel.starts_with("diff_order_book") {
                    if let Some(data) = &json.data {
                        if let Ok(ob_data) =
                            serde_json::from_value::<BitstampOrderBookData>(data.clone())
                        {
                            return Some(WsMessage::OrderBook(Self::parse_order_book_diff(
                                &ob_data, symbol,
                            )));
                        }
                    }
                } else if channel.starts_with("live_trades") {
                    if let Some(data) = &json.data {
                        if let Ok(trade_data) =
                            serde_json::from_value::<BitstampTradeData>(data.clone())
                        {
                            return Some(WsMessage::Trade(Self::parse_trade(&trade_data, symbol)));
                        }
                    }
                }

                None
            },
            _ => None,
        }
    }

    /// Subscribe to a channel and return event stream
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        symbol: &str,
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
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // Build subscription message
        let formatted_symbol = Self::format_symbol(symbol);
        let channel_name = format!("{channel}_{formatted_symbol}");

        let subscribe_msg = BitstampSubscribeMessage {
            event: "bts:subscribe".to_string(),
            data: BitstampSubscribeData {
                channel: channel_name.clone(),
            },
        };

        ws_client.send(&serde_json::to_string(&subscribe_msg).unwrap())?;

        self.ws_client = Some(ws_client);

        // Save subscription
        {
            let key = format!("{channel}:{formatted_symbol}");
            self.subscriptions.write().await.insert(key, channel_name);
        }

        // Event processing task
        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    },
                    WsEvent::Disconnected => {
                        let _ = tx.send(WsMessage::Disconnected);
                    },
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_message(&msg) {
                            let _ = tx.send(ws_msg);
                        }
                    },
                    WsEvent::Error(err) => {
                        let _ = tx.send(WsMessage::Error(err));
                    },
                    _ => {},
                }
            }
        });

        Ok(event_rx)
    }
}

#[async_trait]
impl WsExchange for BitstampWs {
    /// Subscribe to ticker updates
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = BitstampWs::new();
        ws.subscribe_stream("live_ticker", symbol).await
    }

    /// Subscribe to order book updates
    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = BitstampWs::new();
        ws.subscribe_stream("order_book", symbol).await
    }

    /// Subscribe to trade updates
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = BitstampWs::new();
        ws.subscribe_stream("live_trades", symbol).await
    }

    /// Subscribe to OHLCV updates (not supported by Bitstamp WebSocket API)
    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOhlcv".into(),
        })
    }

    /// Connect to WebSocket
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let mut ws_client = WsClient::new(WsConfig {
                url: WS_PUBLIC_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 30,
                connect_timeout_secs: 30,
                ..Default::default()
            });

            ws_client.connect().await?;
            self.ws_client = Some(ws_client);
        }
        Ok(())
    }

    /// Close WebSocket connection
    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws_client) = &self.ws_client {
            ws_client.close()?;
            self.ws_client = None;
        }
        if let Some(ws_client) = &self.private_ws_client {
            ws_client.close()?;
        }
        self.private_ws_client = None;
        Ok(())
    }

    /// Check if WebSocket is connected
    async fn ws_is_connected(&self) -> bool {
        match &self.ws_client {
            Some(c) => c.is_connected().await,
            None => false,
        }
    }

    // === Private Streams ===

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("private-my_balance").await
    }

    async fn watch_orders(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("private-my_orders").await
    }

    async fn watch_my_trades(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("private-my_trades").await
    }

    async fn watch_positions(
        &self,
        _symbols: Option<&[&str]>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Bitstamp is a spot exchange, doesn't support positions
        Err(CcxtError::NotSupported {
            feature: "watchPositions".into(),
        })
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        // Authentication happens during subscription for Bitstamp
        *self.authenticated.write().await = true;
        Ok(())
    }
}

impl Default for BitstampWs {
    fn default() -> Self {
        Self::new()
    }
}

// ===== Bitstamp WebSocket Message Structures =====

#[derive(Debug, Deserialize)]
struct BitstampWsMessage {
    event: String,
    channel: String,
    data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct BitstampSubscribeMessage {
    event: String,
    data: BitstampSubscribeData,
}

#[derive(Debug, Serialize)]
struct BitstampSubscribeData {
    channel: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct BitstampTickerData {
    timestamp: Option<i64>,
    last: Option<String>,
    high: Option<String>,
    low: Option<String>,
    vwap: Option<String>,
    volume: Option<String>,
    bid: Option<String>,
    ask: Option<String>,
    open: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BitstampOrderBookData {
    microtimestamp: Option<i64>,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BitstampTradeData {
    id: u64,
    microtimestamp: Option<i64>,
    amount: f64,
    price: f64,
    #[serde(rename = "type")]
    trade_type: i32, // 0 = buy, 1 = sell
}

// === Private Channel Response Types ===

#[derive(Debug, Default, Serialize, Deserialize)]
struct BitstampBalanceData {
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    reserved: Option<String>,
    #[serde(default)]
    balance: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BitstampOrderData {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    currency_pair: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    order_status: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    avg_price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    amount_filled: Option<String>,
    #[serde(default)]
    amount_remaining: Option<String>,
    #[serde(default)]
    datetime: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BitstampMyTradeData {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    currency_pair: Option<String>,
    #[serde(default)]
    trade_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    fee_currency: Option<String>,
    #[serde(default)]
    microtimestamp: Option<i64>,
    #[serde(default)]
    taker_or_maker: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BitstampWs::format_symbol("BTC/USD"), "btcusd");
        assert_eq!(BitstampWs::format_symbol("ETH/USDT"), "ethusdt");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BitstampWs::to_unified_symbol("btcusd"), "BTC/USD");
        assert_eq!(BitstampWs::to_unified_symbol("ethusdt"), "ETH/USDT");
        assert_eq!(BitstampWs::to_unified_symbol("ethbtc"), "ETH/BTC");
    }

    #[test]
    fn test_ws_client_creation() {
        let ws = BitstampWs::new();
        assert!(ws.ws_client.is_none());
    }

    #[test]
    fn test_with_config() {
        let config = ExchangeConfig::new()
            .with_api_key("test_key")
            .with_api_secret("test_secret");
        let ws = BitstampWs::with_config(config);
        assert!(ws.config.is_some());
        assert!(ws.private_ws_client.is_none());
    }

    #[test]
    fn test_clone() {
        let config = ExchangeConfig::new().with_api_key("test_key");
        let ws = BitstampWs::with_config(config);
        let cloned = ws.clone();
        assert!(cloned.config.is_some());
        assert!(cloned.ws_client.is_none());
    }

    #[test]
    fn test_generate_auth_signature() {
        let result = BitstampWs::generate_auth_signature(
            "test_api_key",
            "test_secret",
            "test_nonce",
            "1234567890",
        );
        assert!(result.is_ok());
        let signature = result.unwrap();
        assert!(!signature.is_empty());
        assert!(signature.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_parse_balance_update() {
        let data = BitstampBalanceData {
            currency: Some("USD".to_string()),
            available: Some("10000.00".to_string()),
            reserved: Some("500.00".to_string()),
            balance: Some("10500.00".to_string()),
        };

        let event = BitstampWs::parse_balance_update(&data);
        assert!(event.balances.currencies.contains_key("USD"));
        let usd_balance = event.balances.currencies.get("USD").unwrap();
        assert_eq!(
            usd_balance.free,
            Some(Decimal::from_str("10000.00").unwrap())
        );
        assert_eq!(usd_balance.used, Some(Decimal::from_str("500.00").unwrap()));
        assert_eq!(
            usd_balance.total,
            Some(Decimal::from_str("10500.00").unwrap())
        );
    }

    #[test]
    fn test_parse_order_update() {
        let data = BitstampOrderData {
            id: Some("12345".to_string()),
            client_order_id: Some("client123".to_string()),
            currency_pair: Some("btcusd".to_string()),
            order_type: Some("buy".to_string()),
            order_status: Some("open".to_string()),
            price: Some("50000.00".to_string()),
            avg_price: None,
            amount: Some("0.1".to_string()),
            amount_filled: Some("0.0".to_string()),
            amount_remaining: Some("0.1".to_string()),
            datetime: None,
        };

        let event = BitstampWs::parse_order_update(&data);
        assert_eq!(event.order.symbol, "BTC/USD");
        assert_eq!(event.order.id, "12345");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.status, OrderStatus::Open);
        assert_eq!(
            event.order.price,
            Some(Decimal::from_str("50000.00").unwrap())
        );
    }

    #[test]
    fn test_parse_my_trade() {
        let data = BitstampMyTradeData {
            id: Some("trade123".to_string()),
            order_id: Some("order456".to_string()),
            currency_pair: Some("btcusd".to_string()),
            trade_type: Some("buy".to_string()),
            price: Some("50000.00".to_string()),
            amount: Some("0.01".to_string()),
            fee: Some("5.00".to_string()),
            fee_currency: Some("USD".to_string()),
            microtimestamp: Some(1700000000000000),
            taker_or_maker: Some("taker".to_string()),
        };

        let event = BitstampWs::parse_my_trade(&data);
        assert_eq!(event.symbol, "BTC/USD");
        assert_eq!(event.trades.len(), 1);
        let trade = &event.trades[0];
        assert_eq!(trade.id, "trade123");
        assert_eq!(trade.order, Some("order456".to_string()));
        assert_eq!(trade.price, Decimal::from_str("50000.00").unwrap());
    }

    #[test]
    fn test_process_private_message_balance() {
        let json = r#"{
            "event": "data",
            "channel": "private-my_balance",
            "data": {
                "currency": "USD",
                "available": "10000.00",
                "reserved": "500.00",
                "balance": "10500.00"
            }
        }"#;

        let result = BitstampWs::process_private_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("USD"));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_process_private_message_order() {
        let json = r#"{
            "event": "data",
            "channel": "private-my_orders",
            "data": {
                "id": "12345",
                "currency_pair": "btcusd",
                "order_type": "buy",
                "order_status": "open",
                "price": "50000.00",
                "amount": "0.1"
            }
        }"#;

        let result = BitstampWs::process_private_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.symbol, "BTC/USD");
            assert_eq!(event.order.id, "12345");
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_private_message_trade() {
        let json = r#"{
            "event": "data",
            "channel": "private-my_trades",
            "data": {
                "id": "trade123",
                "order_id": "order456",
                "currency_pair": "etheur",
                "trade_type": "sell",
                "price": "2000.00",
                "amount": "1.0"
            }
        }"#;

        let result = BitstampWs::process_private_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::MyTrade(event)) = result {
            assert_eq!(event.symbol, "ETH/EUR");
            assert_eq!(event.trades.len(), 1);
        } else {
            panic!("Expected MyTrade message");
        }
    }

    #[test]
    fn test_with_credentials() {
        let ws = BitstampWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
        let config = ws.config.unwrap();
        assert_eq!(config.api_key(), Some("test_key"));
        assert_eq!(config.secret(), Some("test_secret"));
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = BitstampWs::new();
        assert!(ws.config.is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
        let config = ws.config.unwrap();
        assert_eq!(config.api_key(), Some("test_key"));
        assert_eq!(config.secret(), Some("test_secret"));
    }
}
