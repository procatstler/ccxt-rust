//! ApeX Pro WebSocket Implementation
//!
//! ApeX Pro (DEX) real-time data streaming via StarkEx Layer 2
//! API Documentation: <https://api-docs.pro.apex.exchange/>
//!
//! # Public Channels
//! - recentlyTrade - Recent trades
//! - depth - Order book depth
//! - ticker - Ticker updates
//! - kline - Candlestick/OHLCV data
//!
//! # Private Channels (requires authentication)
//! - orders - Order updates
//! - positions - Position updates
//! - asset - Balance updates

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

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    Balance, Balances, MarginMode, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, Position, PositionSide, Ticker, TimeInForce, Timeframe, Trade, WsBalanceEvent,
    WsExchange, WsMessage, WsOhlcvEvent, WsOrderBookEvent, WsOrderEvent, WsPositionEvent,
    WsTickerEvent, WsTradeEvent, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

const WS_PUBLIC_URL: &str = "wss://quote.pro.apex.exchange/realtime_public";
const WS_PRIVATE_URL: &str = "wss://quote.pro.apex.exchange/realtime_private";

/// ApeX Pro WebSocket client
pub struct ApexWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
}

impl ApexWs {
    /// Create new ApeX Pro WebSocket client
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: None,
            api_secret: None,
            passphrase: None,
        }
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: &str, api_secret: &str, passphrase: &str) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: Some(api_key.to_string()),
            api_secret: Some(api_secret.to_string()),
            passphrase: Some(passphrase.to_string()),
        }
    }

    /// Set API credentials
    pub fn set_credentials(&mut self, api_key: &str, api_secret: &str, passphrase: &str) {
        self.api_key = Some(api_key.to_string());
        self.api_secret = Some(api_secret.to_string());
        self.passphrase = Some(passphrase.to_string());
    }

    /// Get API key
    pub fn get_api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Generate authentication signature
    fn generate_auth_signature(api_secret: &str, timestamp: i64) -> String {
        let sign_str = format!("{timestamp}");
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(sign_str.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// Convert symbol to ApeX format (BTC/USDC:USDC -> BTC-USDC)
    fn format_symbol(symbol: &str) -> String {
        // Remove settlement suffix if present
        let base_symbol = symbol.split(':').next().unwrap_or(symbol);
        base_symbol.replace("/", "-")
    }

    /// Convert ApeX symbol to unified format (BTC-USDC -> BTC/USDC:USDC)
    fn to_unified_symbol(apex_symbol: &str) -> String {
        let parts: Vec<&str> = apex_symbol.split('-').collect();
        if parts.len() == 2 {
            format!("{}/{}:{}", parts[0], parts[1], parts[1])
        } else {
            apex_symbol.to_string()
        }
    }

    /// Convert timeframe to ApeX format
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1",
            Timeframe::Minute5 => "5",
            Timeframe::Minute15 => "15",
            Timeframe::Minute30 => "30",
            Timeframe::Hour1 => "60",
            Timeframe::Hour2 => "120",
            Timeframe::Hour4 => "240",
            Timeframe::Hour6 => "360",
            Timeframe::Hour12 => "720",
            Timeframe::Day1 => "D",
            Timeframe::Week1 => "W",
            Timeframe::Month1 => "M",
            _ => "1",
        }
    }

    /// Parse ticker message
    fn parse_ticker(data: &ApexTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: data
                .high_24h
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            low: data
                .low_24h
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: data
                .open_price_24h
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            close: data
                .last_price
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            last: data
                .last_price
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: data
                .price_change_24h
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            percentage: None,
            average: None,
            base_volume: data
                .volume_24h
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: data
                .turnover_24h
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            index_price: data
                .index_price
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            mark_price: data
                .mark_price
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// Parse order book message
    fn parse_order_book(
        data: &ApexOrderBookData,
        symbol: &str,
        is_snapshot: bool,
    ) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|b| {
                if b.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&b[0]).ok()?,
                        amount: Decimal::from_str(&b[1]).ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|a| {
                if a.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&a[0]).ok()?,
                        amount: Decimal::from_str(&a[1]).ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.update_id,
            bids,
            asks,
            checksum: None,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        }
    }

    /// Parse trade message
    fn parse_trades(data: &[ApexTradeData], symbol: &str) -> WsTradeEvent {
        let trades: Vec<Trade> = data
            .iter()
            .map(|t| {
                let timestamp = t.timestamp;
                let price = Decimal::from_str(&t.price).unwrap_or(Decimal::ZERO);
                let amount = Decimal::from_str(&t.amount).unwrap_or(Decimal::ZERO);

                let side = match t.side.to_uppercase().as_str() {
                    "BUY" => Some("buy".to_string()),
                    "SELL" => Some("sell".to_string()),
                    _ => None,
                };

                Trade {
                    id: t.id.clone(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side,
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: vec![],
                    info: serde_json::to_value(t).unwrap_or_default(),
                }
            })
            .collect();

        WsTradeEvent {
            symbol: symbol.to_string(),
            trades,
        }
    }

    /// Parse OHLCV message
    fn parse_ohlcv(
        data: &ApexKlineData,
        symbol: &str,
        timeframe: Timeframe,
    ) -> Option<WsOhlcvEvent> {
        let ohlcv = OHLCV {
            timestamp: data.timestamp,
            open: Decimal::from_str(&data.open).ok()?,
            high: Decimal::from_str(&data.high).ok()?,
            low: Decimal::from_str(&data.low).ok()?,
            close: Decimal::from_str(&data.close).ok()?,
            volume: Decimal::from_str(&data.volume).ok()?,
        };

        Some(WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        })
    }

    /// Parse order update message
    fn parse_order(data: &ApexOrderData) -> WsOrderEvent {
        let timestamp = data
            .created_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let symbol = Self::to_unified_symbol(&data.symbol);

        let status = match data.status.as_deref() {
            Some("OPEN") | Some("NEW") => OrderStatus::Open,
            Some("PENDING") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("CANCELED") | Some("CANCELLED") => OrderStatus::Canceled,
            Some("EXPIRED") => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            Some("STOP_LIMIT") => OrderType::StopLimit,
            Some("STOP_MARKET") => OrderType::StopMarket,
            Some("TAKE_PROFIT") => OrderType::TakeProfit,
            Some("TAKE_PROFIT_MARKET") => OrderType::TakeProfitMarket,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = match data.time_in_force.as_deref() {
            Some("GTC") => Some(TimeInForce::GTC),
            Some("IOC") => Some(TimeInForce::IOC),
            Some("FOK") => Some(TimeInForce::FOK),
            Some("GTT") => Some(TimeInForce::GTT),
            _ => None,
        };

        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let amount = data
            .size
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let filled = data
            .filled_size
            .as_ref()
            .and_then(|f| Decimal::from_str(f).ok())
            .unwrap_or(Decimal::ZERO);
        let remaining = amount - filled;

        let order = Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.updated_time,
            status,
            symbol,
            order_type,
            time_in_force,
            side,
            price,
            average: data
                .avg_fill_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            amount,
            filled,
            remaining: Some(remaining),
            stop_price: data
                .trigger_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            trigger_price: data
                .trigger_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            take_profit_price: data
                .take_profit_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            stop_loss_price: data
                .stop_loss_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            cost: data
                .filled_value
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            trades: vec![],
            fee: None,
            fees: vec![],
            reduce_only: data.reduce_only,
            post_only: data.post_only,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// Parse position update message
    fn parse_position(data: &ApexPositionData) -> WsPositionEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data
            .updated_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let size = data
            .size
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);

        let side = match data.side.as_deref() {
            Some("LONG") => Some(PositionSide::Long),
            Some("SHORT") => Some(PositionSide::Short),
            _ => {
                if size > Decimal::ZERO {
                    Some(PositionSide::Long)
                } else {
                    Some(PositionSide::Short)
                }
            },
        };

        let position = Position {
            symbol: symbol.clone(),
            id: data.position_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            contracts: Some(size.abs()),
            contract_size: Some(Decimal::ONE),
            side,
            notional: data.value.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            leverage: data
                .leverage
                .as_ref()
                .and_then(|l| Decimal::from_str(l).ok()),
            unrealized_pnl: data
                .unrealized_pnl
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            realized_pnl: data
                .realized_pnl
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            collateral: None,
            entry_price: data
                .entry_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            mark_price: data
                .mark_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            liquidation_price: data
                .liquidation_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            margin_mode: Some(MarginMode::Cross),
            hedged: Some(false),
            maintenance_margin: data
                .maintenance_margin
                .as_ref()
                .and_then(|m| Decimal::from_str(m).ok()),
            maintenance_margin_percentage: None,
            initial_margin: data
                .initial_margin
                .as_ref()
                .and_then(|m| Decimal::from_str(m).ok()),
            initial_margin_percentage: None,
            margin_ratio: None,
            last_update_timestamp: Some(timestamp),
            last_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            percentage: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsPositionEvent {
            positions: vec![position],
        }
    }

    /// Parse balance update message
    fn parse_balance(data: &ApexBalanceData) -> WsBalanceEvent {
        let mut currencies = HashMap::new();

        // Add USDC balance from perpetual account
        if let Some(available) = &data.available_balance {
            if let Ok(free) = Decimal::from_str(available) {
                let total_equity = data
                    .total_equity
                    .as_ref()
                    .and_then(|t| Decimal::from_str(t).ok())
                    .unwrap_or(free);
                let used = total_equity - free;

                currencies.insert(
                    "USDC".to_string(),
                    Balance {
                        free: Some(free),
                        used: Some(used),
                        total: Some(total_equity),
                        debt: None,
                    },
                );
            }
        }

        // Add spot balances if present
        if let Some(spot_balances) = &data.spot_balances {
            for bal in spot_balances {
                let currency = bal.currency.clone();
                let free = Decimal::from_str(&bal.available_balance).unwrap_or(Decimal::ZERO);
                let frozen = bal
                    .frozen_balance
                    .as_ref()
                    .and_then(|f| Decimal::from_str(f).ok())
                    .unwrap_or(Decimal::ZERO);
                let total = free + frozen;

                currencies.insert(
                    currency,
                    Balance {
                        free: Some(free),
                        used: Some(frozen),
                        total: Some(total),
                        debt: None,
                    },
                );
            }
        }

        let timestamp = Utc::now().timestamp_millis();
        let balances = Balances {
            currencies,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsBalanceEvent { balances }
    }

    /// Process incoming WebSocket message
    fn process_message(msg: &str, timeframe_hint: Option<Timeframe>) -> Option<WsMessage> {
        let json: ApexWsMessage = serde_json::from_str(msg).ok()?;

        // Handle subscription response
        if json.op.as_deref() == Some("subscribe") && json.success == Some(true) {
            return Some(WsMessage::Subscribed {
                channel: json.topic.clone().unwrap_or_default(),
                symbol: None,
            });
        }

        // Handle auth response
        if json.op.as_deref() == Some("auth") {
            if json.success == Some(true) {
                return Some(WsMessage::Authenticated);
            } else {
                return Some(WsMessage::Error(format!(
                    "Authentication failed: {}",
                    json.msg.unwrap_or_default()
                )));
            }
        }

        // Handle pong
        if json.op.as_deref() == Some("pong") {
            return None; // Silently ignore pong
        }

        // Handle data messages
        let topic = json.topic.as_deref()?;
        let data = json.data?;

        if topic.starts_with("recentlyTrade") {
            // Extract symbol from topic (e.g., "recentlyTrade.BTC-USDC")
            let symbol = topic.split('.').nth(1).unwrap_or("");
            let unified_symbol = Self::to_unified_symbol(symbol);

            let trades: Vec<ApexTradeData> = serde_json::from_value(data).ok()?;
            if !trades.is_empty() {
                return Some(WsMessage::Trade(Self::parse_trades(
                    &trades,
                    &unified_symbol,
                )));
            }
        } else if topic.starts_with("depth") {
            let symbol = topic.split('.').nth(1).unwrap_or("");
            let unified_symbol = Self::to_unified_symbol(symbol);

            let book: ApexOrderBookData = serde_json::from_value(data).ok()?;
            let is_snapshot = json.msg_type.as_deref() == Some("snapshot");
            return Some(WsMessage::OrderBook(Self::parse_order_book(
                &book,
                &unified_symbol,
                is_snapshot,
            )));
        } else if topic.starts_with("ticker") {
            let ticker: ApexTickerData = serde_json::from_value(data).ok()?;
            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker)));
        } else if topic.starts_with("kline") {
            let symbol = topic.split('.').nth(1).unwrap_or("");
            let unified_symbol = Self::to_unified_symbol(symbol);

            let kline: ApexKlineData = serde_json::from_value(data).ok()?;
            let tf = timeframe_hint.unwrap_or(Timeframe::Minute1);
            if let Some(ohlcv) = Self::parse_ohlcv(&kline, &unified_symbol, tf) {
                return Some(WsMessage::Ohlcv(ohlcv));
            }
        } else if topic == "orders" {
            let order: ApexOrderData = serde_json::from_value(data).ok()?;
            return Some(WsMessage::Order(Self::parse_order(&order)));
        } else if topic == "positions" {
            let position: ApexPositionData = serde_json::from_value(data).ok()?;
            return Some(WsMessage::Position(Self::parse_position(&position)));
        } else if topic == "asset" {
            let balance: ApexBalanceData = serde_json::from_value(data).ok()?;
            return Some(WsMessage::Balance(Self::parse_balance(&balance)));
        }

        None
    }

    /// Subscribe to a channel and return event stream
    async fn subscribe_stream(
        &mut self,
        topic: &str,
        requires_auth: bool,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let ws_url = if requires_auth {
            WS_PRIVATE_URL
        } else {
            WS_PUBLIC_URL
        };

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 20,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // Authenticate if required
        if requires_auth {
            if let (Some(api_key), Some(api_secret)) = (&self.api_key, &self.api_secret) {
                let timestamp = Utc::now().timestamp_millis();
                let sign = Self::generate_auth_signature(api_secret, timestamp);

                let auth_msg = serde_json::json!({
                    "op": "auth",
                    "args": [{
                        "apiKey": api_key,
                        "timestamp": timestamp.to_string(),
                        "signature": sign
                    }]
                });

                ws_client.send(&auth_msg.to_string())?;
            } else {
                return Err(crate::errors::CcxtError::AuthenticationError {
                    message: "API credentials required for private channels".to_string(),
                });
            }
        }

        // Build subscription message
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": [topic]
        });

        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // Save subscription
        {
            self.subscriptions
                .write()
                .await
                .insert(topic.to_string(), topic.to_string());
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
                        if let Some(ws_msg) = Self::process_message(&msg, None) {
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

    /// Subscribe to order updates (private channel)
    pub async fn watch_orders(&mut self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_stream("orders", true).await
    }

    /// Subscribe to position updates (private channel)
    pub async fn watch_positions(&mut self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_stream("positions", true).await
    }

    /// Subscribe to balance updates (private channel)
    pub async fn watch_balance(&mut self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_stream("asset", true).await
    }
}

impl Default for ApexWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ApexWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
            passphrase: self.passphrase.clone(),
        }
    }
}

#[async_trait]
impl WsExchange for ApexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let apex_symbol = Self::format_symbol(symbol);
        let topic = format!("ticker.{apex_symbol}");
        ws.subscribe_stream(&topic, false).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let apex_symbol = Self::format_symbol(symbol);
        let topic = format!("depth.{apex_symbol}");
        ws.subscribe_stream(&topic, false).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let apex_symbol = Self::format_symbol(symbol);
        let topic = format!("recentlyTrade.{apex_symbol}");
        ws.subscribe_stream(&topic, false).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let apex_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let topic = format!("kline.{apex_symbol}.{interval}");
        ws.subscribe_stream(&topic, false).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let mut ws_client = WsClient::new(WsConfig {
                url: WS_PUBLIC_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 20,
                connect_timeout_secs: 30,
                ..Default::default()
            });

            ws_client.connect().await?;
            self.ws_client = Some(ws_client);
        }
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws_client) = &self.ws_client {
            ws_client.close()?;
            self.ws_client = None;
        }
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        match &self.ws_client {
            Some(c) => c.is_connected().await,
            None => false,
        }
    }
}

// ===== WebSocket Message Structures =====

#[derive(Debug, Deserialize)]
struct ApexWsMessage {
    #[serde(default)]
    op: Option<String>,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default, rename = "type")]
    msg_type: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApexTickerData {
    #[serde(default)]
    symbol: String,
    #[serde(default, rename = "lastPrice")]
    last_price: Option<String>,
    #[serde(default, rename = "markPrice")]
    mark_price: Option<String>,
    #[serde(default, rename = "indexPrice")]
    index_price: Option<String>,
    #[serde(default, rename = "high24h")]
    high_24h: Option<String>,
    #[serde(default, rename = "low24h")]
    low_24h: Option<String>,
    #[serde(default, rename = "volume24h")]
    volume_24h: Option<String>,
    #[serde(default, rename = "turnover24h")]
    turnover_24h: Option<String>,
    #[serde(default, rename = "openPrice24h")]
    open_price_24h: Option<String>,
    #[serde(default, rename = "priceChange24h")]
    price_change_24h: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApexOrderBookData {
    #[serde(default, rename = "a")]
    asks: Vec<Vec<String>>,
    #[serde(default, rename = "b")]
    bids: Vec<Vec<String>>,
    #[serde(default, rename = "u")]
    update_id: Option<i64>,
    #[serde(default, rename = "t")]
    timestamp: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApexTradeData {
    #[serde(rename = "i")]
    id: String,
    #[serde(rename = "T")]
    timestamp: i64,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "v")]
    amount: String,
    #[serde(rename = "S")]
    side: String,
}

#[derive(Debug, Deserialize)]
struct ApexKlineData {
    #[serde(rename = "t")]
    timestamp: i64,
    #[serde(rename = "o")]
    open: String,
    #[serde(rename = "h")]
    high: String,
    #[serde(rename = "l")]
    low: String,
    #[serde(rename = "c")]
    close: String,
    #[serde(rename = "v")]
    volume: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApexOrderData {
    #[serde(default)]
    symbol: String,
    #[serde(default, rename = "orderId")]
    order_id: Option<String>,
    #[serde(default, rename = "clientOrderId")]
    client_order_id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "orderType")]
    order_type: Option<String>,
    #[serde(default, rename = "timeInForce")]
    time_in_force: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "filledSize")]
    filled_size: Option<String>,
    #[serde(default, rename = "avgFillPrice")]
    avg_fill_price: Option<String>,
    #[serde(default, rename = "filledValue")]
    filled_value: Option<String>,
    #[serde(default, rename = "triggerPrice")]
    trigger_price: Option<String>,
    #[serde(default, rename = "takeProfitPrice")]
    take_profit_price: Option<String>,
    #[serde(default, rename = "stopLossPrice")]
    stop_loss_price: Option<String>,
    #[serde(default, rename = "reduceOnly")]
    reduce_only: Option<bool>,
    #[serde(default, rename = "postOnly")]
    post_only: Option<bool>,
    #[serde(default, rename = "createdTime")]
    created_time: Option<i64>,
    #[serde(default, rename = "updatedTime")]
    updated_time: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApexPositionData {
    #[serde(default)]
    symbol: String,
    #[serde(default, rename = "positionId")]
    position_id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default, rename = "entryPrice")]
    entry_price: Option<String>,
    #[serde(default, rename = "markPrice")]
    mark_price: Option<String>,
    #[serde(default, rename = "liquidationPrice")]
    liquidation_price: Option<String>,
    #[serde(default)]
    leverage: Option<String>,
    #[serde(default, rename = "unrealizedPnl")]
    unrealized_pnl: Option<String>,
    #[serde(default, rename = "realizedPnl")]
    realized_pnl: Option<String>,
    #[serde(default, rename = "initialMargin")]
    initial_margin: Option<String>,
    #[serde(default, rename = "maintenanceMargin")]
    maintenance_margin: Option<String>,
    #[serde(default, rename = "updatedTime")]
    updated_time: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApexBalanceData {
    #[serde(default, rename = "totalEquityValue")]
    total_equity: Option<String>,
    #[serde(default, rename = "availableBalance")]
    available_balance: Option<String>,
    #[serde(default, rename = "spotBalances")]
    spot_balances: Option<Vec<ApexSpotBalance>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApexSpotBalance {
    currency: String,
    #[serde(rename = "availableBalance")]
    available_balance: String,
    #[serde(default, rename = "frozenBalance")]
    frozen_balance: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_client() {
        let ws = ApexWs::new();
        assert!(ws.ws_client.is_none());
        assert!(ws.api_key.is_none());
    }

    #[test]
    fn test_create_with_credentials() {
        let ws = ApexWs::with_credentials("key", "secret", "pass");
        assert_eq!(ws.get_api_key(), Some("key"));
    }

    #[test]
    fn test_format_symbol() {
        assert_eq!(ApexWs::format_symbol("BTC/USDC:USDC"), "BTC-USDC");
        assert_eq!(ApexWs::format_symbol("ETH/USDC"), "ETH-USDC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(ApexWs::to_unified_symbol("BTC-USDC"), "BTC/USDC:USDC");
        assert_eq!(ApexWs::to_unified_symbol("ETH-USDC"), "ETH/USDC:USDC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(ApexWs::format_interval(Timeframe::Minute1), "1");
        assert_eq!(ApexWs::format_interval(Timeframe::Hour1), "60");
        assert_eq!(ApexWs::format_interval(Timeframe::Day1), "D");
    }

    #[test]
    fn test_parse_ticker() {
        let data = ApexTickerData {
            symbol: "BTC-USDC".to_string(),
            last_price: Some("42000.00".to_string()),
            mark_price: Some("42001.00".to_string()),
            index_price: Some("41999.00".to_string()),
            high_24h: Some("42500.00".to_string()),
            low_24h: Some("41500.00".to_string()),
            volume_24h: Some("1000.00".to_string()),
            turnover_24h: Some("42000000.00".to_string()),
            open_price_24h: Some("41800.00".to_string()),
            price_change_24h: Some("200.00".to_string()),
        };

        let event = ApexWs::parse_ticker(&data);
        assert_eq!(event.symbol, "BTC/USDC:USDC");
        assert_eq!(
            event.ticker.last,
            Some(Decimal::from_str("42000.00").unwrap())
        );
        assert_eq!(
            event.ticker.mark_price,
            Some(Decimal::from_str("42001.00").unwrap())
        );
    }

    #[test]
    fn test_parse_order_book() {
        let data = ApexOrderBookData {
            bids: vec![
                vec!["42000.00".to_string(), "1.5".to_string()],
                vec!["41999.00".to_string(), "2.0".to_string()],
            ],
            asks: vec![
                vec!["42001.00".to_string(), "1.0".to_string()],
                vec!["42002.00".to_string(), "3.0".to_string()],
            ],
            update_id: Some(12345),
            timestamp: Some(1704067200000),
        };

        let event = ApexWs::parse_order_book(&data, "BTC/USDC:USDC", true);
        assert_eq!(event.symbol, "BTC/USDC:USDC");
        assert!(event.is_snapshot);
        assert_eq!(event.order_book.bids.len(), 2);
        assert_eq!(event.order_book.asks.len(), 2);
    }

    #[test]
    fn test_parse_trades() {
        let data = vec![ApexTradeData {
            id: "123".to_string(),
            timestamp: 1704067200000,
            price: "42000.00".to_string(),
            amount: "0.1".to_string(),
            side: "BUY".to_string(),
        }];

        let event = ApexWs::parse_trades(&data, "BTC/USDC:USDC");
        assert_eq!(event.symbol, "BTC/USDC:USDC");
        assert_eq!(event.trades.len(), 1);
        assert_eq!(event.trades[0].id, "123");
        assert_eq!(
            event.trades[0].price,
            Decimal::from_str("42000.00").unwrap()
        );
    }

    #[test]
    fn test_clone() {
        let ws = ApexWs::with_credentials("key", "secret", "pass");
        let cloned = ws.clone();
        assert_eq!(cloned.api_key, Some("key".to_string()));
        assert!(cloned.ws_client.is_none());
    }

    #[test]
    fn test_default() {
        let ws = ApexWs::default();
        assert!(ws.api_key.is_none());
    }

    #[test]
    fn test_generate_auth_signature() {
        let sig = ApexWs::generate_auth_signature("secret123", 1704067200000);
        assert!(!sig.is_empty());
    }
}
