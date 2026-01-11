//! AscendEX WebSocket Implementation
//!
//! AscendEX real-time data streaming (Public + Private)

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Ticker, Timeframe, Trade, OHLCV, WsBalanceEvent, WsExchange, WsMessage, WsOhlcvEvent,
    WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_BASE_URL: &str = "wss://ascendex.com:443/api/pro/v2/stream";
const REST_BASE_URL: &str = "https://ascendex.com";

/// AscendEX WebSocket client
pub struct AscendexWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    private_ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    account_group: Arc<RwLock<Option<String>>>,
}

impl AscendexWs {
    /// Create new AscendEX WebSocket client
    pub fn new() -> Self {
        Self {
            config: None,
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            account_group: Arc::new(RwLock::new(None)),
        }
    }

    /// Create AscendEX WebSocket client with config
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            account_group: Arc::new(RwLock::new(None)),
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

    /// Get account group ID
    async fn get_account_group(&self) -> CcxtResult<String> {
        // Check if already cached
        {
            let ag = self.account_group.read().await;
            if let Some(group) = ag.as_ref() {
                return Ok(group.clone());
            }
        }

        // Fetch account info from REST API
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let url = format!("{REST_BASE_URL}/api/pro/v1/info");
        let timestamp = Utc::now().timestamp_millis().to_string();
        let path = "/api/pro/v1/info";
        let signature = self.sign_request(&timestamp, path)?;

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .header("x-auth-key", api_key)
            .header("x-auth-timestamp", &timestamp)
            .header("x-auth-signature", &signature)
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(CcxtError::ExchangeError {
                message: format!("Failed to get account info: {error_text}"),
            });
        }

        let data: AscendexAccountInfoResponse = response.json().await.map_err(|e| CcxtError::ParseError {
            data_type: "AscendexAccountInfoResponse".to_string(),
            message: e.to_string(),
        })?;

        let group_id = data.data.account_group.to_string();

        // Cache the account group
        *self.account_group.write().await = Some(group_id.clone());

        Ok(group_id)
    }

    /// Sign request with HMAC SHA256
    fn sign_request(&self, timestamp: &str, path: &str) -> CcxtResult<String> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Config required".into(),
        })?;

        let secret = config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;

        let message = format!("{timestamp}+{path}");

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("HMAC error: {e}"),
            })?;
        mac.update(message.as_bytes());
        let signature = mac.finalize().into_bytes();

        Ok(BASE64.encode(signature))
    }

    /// Subscribe to private stream
    async fn subscribe_private_stream(&mut self, channel: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let account_group = self.get_account_group().await?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // Private WebSocket URL
        let url = format!("wss://ascendex.com:443/{account_group}/api/pro/v2/stream");

        // WebSocket client
        let mut ws_client = WsClient::new(WsConfig {
            url: url.clone(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.private_ws_client = Some(ws_client);

        // Store subscription
        {
            let key = format!("private:{channel}");
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // Authenticate
        let timestamp = Utc::now().timestamp_millis().to_string();
        let path = format!("/{account_group}/api/pro/v2/stream");
        let version = "v2";
        let auth = format!("{timestamp}+{version}/{path}");
        let signature = {
            let config = self.config.as_ref().unwrap();
            let secret = config.secret().unwrap();
            let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
            mac.update(auth.as_bytes());
            BASE64.encode(mac.finalize().into_bytes())
        };

        let auth_msg = serde_json::json!({
            "op": "auth",
            "id": Utc::now().timestamp_millis().to_string(),
            "t": timestamp,
            "key": self.config.as_ref().unwrap().api_key().unwrap(),
            "sig": signature,
        });

        // Send auth message
        if let Some(ref client) = self.private_ws_client {
            client.send(&auth_msg.to_string())?;
        }

        // Subscribe to channel
        let sub_msg = serde_json::json!({
            "op": "sub",
            "id": Utc::now().timestamp_millis().to_string(),
            "ch": channel,
        });

        if let Some(ref client) = self.private_ws_client {
            client.send(&sub_msg.to_string())?;
        }

        // Event processing task
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
                        if let Some(ws_msg) = Self::process_private_message(&msg) {
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

    /// Process private messages
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;
        let message_type = json.get("m").and_then(|v| v.as_str())?;

        match message_type {
            "order" | "futures-order" => {
                if let Ok(data) = serde_json::from_str::<AscendexOrderMessage>(msg) {
                    return Some(WsMessage::Order(Self::parse_order_message(&data)));
                }
            }
            "balance" | "futures-account-update" => {
                if let Ok(data) = serde_json::from_str::<AscendexBalanceMessage>(msg) {
                    return Some(WsMessage::Balance(Self::parse_balance_message(&data)));
                }
            }
            _ => {}
        }

        None
    }

    /// Parse order message
    fn parse_order_message(msg: &AscendexOrderMessage) -> WsOrderEvent {
        let data = &msg.data;
        let symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.st.as_str() {
            "New" | "PartiallyFilled" => OrderStatus::Open,
            "Filled" => OrderStatus::Closed,
            "Canceled" => OrderStatus::Canceled,
            "Rejected" => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let side = match data.sd.as_str() {
            "Buy" => OrderSide::Buy,
            "Sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.ot.as_str() {
            "Limit" => OrderType::Limit,
            "Market" => OrderType::Market,
            "StopMarket" => OrderType::StopMarket,
            "StopLimit" => OrderType::StopLimit,
            _ => OrderType::Limit,
        };

        let price: Decimal = data.p.parse().unwrap_or_default();
        let amount: Decimal = data.q.parse().unwrap_or_default();
        let filled: Decimal = data.cfq.parse().unwrap_or_default();
        let remaining = amount - filled;
        let average: Decimal = data.ap.parse().unwrap_or_default();
        let cost = if filled > Decimal::ZERO && average > Decimal::ZERO {
            Some(average * filled)
        } else {
            None
        };

        let order = Order {
            id: data.order_id.clone(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: data.t,
            last_update_timestamp: None,
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force: None,
            side,
            price: Some(price),
            average: Some(average),
            amount,
            filled,
            remaining: Some(remaining),
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
            stop_price: data.sp.as_ref().and_then(|v| v.parse().ok()),
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        };

        WsOrderEvent { order }
    }

    /// Parse balance message
    fn parse_balance_message(msg: &AscendexBalanceMessage) -> WsBalanceEvent {
        let mut balances = Balances {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            ..Default::default()
        };

        if let Some(ref data) = msg.data {
            let currency = data.a.clone();
            let total: Decimal = data.tb.parse().unwrap_or_default();
            let free: Decimal = data.ab.parse().unwrap_or_default();
            let used = total - free;

            balances.currencies.insert(
                currency,
                Balance {
                    free: Some(free),
                    used: Some(used),
                    total: Some(total),
                    debt: None,
                },
            );
        }

        balances.info = serde_json::to_value(msg).unwrap_or_default();
        WsBalanceEvent { balances }
    }

    /// Convert symbol to AscendEX format (BTC/USDT -> BTC/USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.to_string()
    }

    /// Convert AscendEX symbol to unified format
    fn to_unified_symbol(ascendex_symbol: &str) -> String {
        ascendex_symbol.to_string()
    }

    /// Parse ticker message
    fn parse_ticker(data: &AscendexBboMessage) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: None,
            low: None,
            bid: data.data.bid.first().and_then(|v| v.parse().ok()),
            bid_volume: data.data.bid.get(1).and_then(|v| v.parse().ok()),
            ask: data.data.ask.first().and_then(|v| v.parse().ok()),
            ask_volume: data.data.ask.get(1).and_then(|v| v.parse().ok()),
            vwap: None,
            open: None,
            close: None,
            last: None,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: None,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// Parse order book message
    fn parse_order_book(data: &AscendexDepthMessage, symbol: &str) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: b[0].parse().ok()?,
                    amount: b[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: a[0].parse().ok()?,
                    amount: a[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let timestamp = data.data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.data.seqnum,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot: false,
        }
    }

    /// Parse trade message
    fn parse_trade(data: &AscendexTradesMessage) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let mut trades = Vec::new();

        for trade_data in &data.data {
            let timestamp = trade_data.ts;
            let price: Decimal = trade_data.p.parse().unwrap_or_default();
            let amount: Decimal = trade_data.q.parse().unwrap_or_default();

            let trade = Trade {
                id: trade_data.seqnum.to_string(),
                order: None,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol.clone(),
                trade_type: None,
                side: if trade_data.bm { Some("sell".into()) } else { Some("buy".into()) },
                taker_or_maker: None,
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(trade_data).unwrap_or_default(),
            };

            trades.push(trade);
        }

        WsTradeEvent { symbol, trades }
    }

    /// Parse OHLCV message
    fn parse_ohlcv(data: &AscendexBarMessage, timeframe: Timeframe) -> WsOhlcvEvent {
        let symbol = Self::to_unified_symbol(&data.s);

        let ohlcv = OHLCV {
            timestamp: data.data.ts,
            open: data.data.o.parse().unwrap_or_default(),
            high: data.data.h.parse().unwrap_or_default(),
            low: data.data.l.parse().unwrap_or_default(),
            close: data.data.c.parse().unwrap_or_default(),
            volume: data.data.v.parse().unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        }
    }

    /// Process public message
    fn process_message(msg: &str) -> Option<WsMessage> {
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;
        let message_type = json.get("m").and_then(|v| v.as_str())?;

        match message_type {
            "bbo" => {
                if let Ok(data) = serde_json::from_str::<AscendexBboMessage>(msg) {
                    return Some(WsMessage::Ticker(Self::parse_ticker(&data)));
                }
            }
            "depth" => {
                if let Ok(data) = serde_json::from_str::<AscendexDepthMessage>(msg) {
                    let symbol = Self::to_unified_symbol(&data.symbol);
                    return Some(WsMessage::OrderBook(Self::parse_order_book(&data, &symbol)));
                }
            }
            "trades" => {
                if let Ok(data) = serde_json::from_str::<AscendexTradesMessage>(msg) {
                    return Some(WsMessage::Trade(Self::parse_trade(&data)));
                }
            }
            "bar" => {
                if let Ok(data) = serde_json::from_str::<AscendexBarMessage>(msg) {
                    // Extract interval from data
                    let interval = &data.data.i;
                    let timeframe = match interval.as_str() {
                        "1" => Timeframe::Minute1,
                        "5" => Timeframe::Minute5,
                        "15" => Timeframe::Minute15,
                        "30" => Timeframe::Minute30,
                        "60" => Timeframe::Hour1,
                        "240" => Timeframe::Hour4,
                        "1d" => Timeframe::Day1,
                        "1w" => Timeframe::Week1,
                        "1m" => Timeframe::Month1,
                        _ => Timeframe::Minute1,
                    };
                    return Some(WsMessage::Ohlcv(Self::parse_ohlcv(&data, timeframe)));
                }
            }
            _ => {}
        }

        None
    }

    /// Subscribe to stream
    async fn subscribe_stream(&mut self, channel: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket URL
        let url = WS_BASE_URL.to_string();

        // WebSocket client
        let mut ws_client = WsClient::new(WsConfig {
            url: url.clone(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

        // Store subscription
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // Subscribe message
        let sub_msg = serde_json::json!({
            "op": "sub",
            "id": Utc::now().timestamp_millis().to_string(),
            "ch": channel,
        });

        if let Some(ref client) = self.ws_client {
            client.send(&sub_msg.to_string())?;
        }

        // Event processing task
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

impl Default for AscendexWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for AscendexWs {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            account_group: Arc::clone(&self.account_group),
        }
    }
}

#[async_trait]
impl WsExchange for AscendexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let channel = format!("bbo:{}", Self::format_symbol(symbol));
        client.subscribe_stream(&channel, Some(symbol)).await
    }

    async fn watch_tickers(&self, _symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watch_tickers not supported by AscendEX".into(),
        })
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let channel = format!("depth:{}", Self::format_symbol(symbol));
        client.subscribe_stream(&channel, Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let channel = format!("trades:{}", Self::format_symbol(symbol));
        client.subscribe_stream(&channel, Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let interval = match timeframe {
            Timeframe::Minute1 => "1",
            Timeframe::Minute5 => "5",
            Timeframe::Minute15 => "15",
            Timeframe::Minute30 => "30",
            Timeframe::Hour1 => "60",
            Timeframe::Hour4 => "240",
            Timeframe::Day1 => "1d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1m",
            _ => "1",
        };
        let channel = format!("bar:{}:{}", interval, Self::format_symbol(symbol));
        client.subscribe_stream(&channel, Some(symbol)).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // AscendEX connects on subscription
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = &self.ws_client {
            client.close()?;
        }
        if let Some(client) = &self.private_ws_client {
            client.close()?;
        }
        self.ws_client = None;
        self.private_ws_client = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        if let Some(client) = &self.ws_client {
            return client.is_connected().await;
        }
        if let Some(client) = &self.private_ws_client {
            return client.is_connected().await;
        }
        false
    }

    // Private streams
    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("order:CASH").await
    }

    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("order:CASH").await
    }

    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watch_my_trades not directly supported by AscendEX".into(),
        })
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        // Authentication happens in subscribe_private_stream
        self.get_account_group().await?;
        Ok(())
    }
}

// === AscendEX WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct AscendexAccountInfoResponse {
    data: AscendexAccountInfo,
}

#[derive(Debug, Deserialize)]
struct AscendexAccountInfo {
    #[serde(rename = "accountGroup")]
    account_group: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct AscendexBboMessage {
    m: String,
    symbol: String,
    data: AscendexBboData,
}

#[derive(Debug, Deserialize, Serialize)]
struct AscendexBboData {
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    bid: Vec<String>,
    #[serde(default)]
    ask: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AscendexDepthMessage {
    m: String,
    symbol: String,
    data: AscendexDepthData,
}

#[derive(Debug, Deserialize, Serialize)]
struct AscendexDepthData {
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    seqnum: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AscendexTradesMessage {
    m: String,
    symbol: String,
    data: Vec<AscendexTradeData>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AscendexTradeData {
    p: String,
    q: String,
    ts: i64,
    bm: bool,
    seqnum: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct AscendexBarMessage {
    m: String,
    s: String,
    data: AscendexBarData,
}

#[derive(Debug, Deserialize, Serialize)]
struct AscendexBarData {
    i: String,
    ts: i64,
    o: String,
    c: String,
    h: String,
    l: String,
    v: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct AscendexOrderMessage {
    m: String,
    #[serde(rename = "accountId")]
    account_id: String,
    ac: String,
    data: AscendexOrderData,
}

#[derive(Debug, Deserialize, Serialize)]
struct AscendexOrderData {
    #[serde(rename = "orderId")]
    order_id: String,
    s: String,
    ot: String,
    #[serde(default)]
    t: Option<i64>,
    p: String,
    q: String,
    sd: String,
    st: String,
    ap: String,
    cfq: String,
    #[serde(default)]
    sp: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AscendexBalanceMessage {
    m: String,
    #[serde(rename = "accountId")]
    account_id: String,
    ac: String,
    #[serde(default)]
    data: Option<AscendexBalanceData>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AscendexBalanceData {
    a: String,
    tb: String,
    ab: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(AscendexWs::format_symbol("BTC/USDT"), "BTC/USDT");
        assert_eq!(AscendexWs::format_symbol("ETH/USDT"), "ETH/USDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(AscendexWs::to_unified_symbol("BTC/USDT"), "BTC/USDT");
        assert_eq!(AscendexWs::to_unified_symbol("ETH/USDT"), "ETH/USDT");
    }

    #[test]
    fn test_with_credentials() {
        let ws = AscendexWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
        let config = ws.config.unwrap();
        assert_eq!(config.api_key(), Some("test_key"));
        assert_eq!(config.secret(), Some("test_secret"));
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = AscendexWs::new();
        assert!(ws.config.is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
        let config = ws.config.unwrap();
        assert_eq!(config.api_key(), Some("test_key"));
        assert_eq!(config.secret(), Some("test_secret"));
    }
}
