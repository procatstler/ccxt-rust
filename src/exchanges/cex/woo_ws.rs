//! WOO X WebSocket Implementation
//!
//! WOO X 실시간 데이터 스트리밍 (Public & Private Streams)
//!
//! ## Private Channels
//! - `executionreport`: Order execution updates
//! - `balance`: Account balance updates (realtime push on update)
//! - `position`: Position updates for perpetual contracts
//!
//! ## Authentication
//! Private streams require HMAC-SHA256 authentication with:
//! - apikey: API key
//! - sign: HMAC-SHA256 signature of `|timestamp|`
//! - timestamp: Current Unix milliseconds

#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]

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
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, MarginMode, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, Position, PositionSide, TakerOrMaker, Ticker, Timeframe,
    Trade, OHLCV, WsBalanceEvent, WsExchange, WsMessage, WsMyTradeEvent, WsOhlcvEvent,
    WsOrderBookEvent, WsOrderEvent, WsPositionEvent, WsTickerEvent, WsTradeEvent,
};

use super::woo::Woo;

const WS_PUBLIC_URL: &str = "wss://wss.woo.org/ws/stream";
const WS_PRIVATE_URL: &str = "wss://wss.woox.io/v2/ws/private/stream";

/// WOO X WebSocket 클라이언트
pub struct WooWs {
    #[allow(dead_code)]
    rest: Woo,
    ws_client: Arc<RwLock<Option<WsClient>>>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Arc<RwLock<Option<mpsc::UnboundedSender<WsMessage>>>>,
    /// API key for private channels
    api_key: Option<String>,
    /// API secret for signing
    api_secret: Option<String>,
    /// Application ID for private WebSocket URL
    application_id: Option<String>,
}

impl WooWs {
    /// 새 WOO X WebSocket 클라이언트 생성
    pub fn new(rest: Woo) -> Self {
        Self {
            rest,
            ws_client: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: Arc::new(RwLock::new(None)),
            api_key: None,
            api_secret: None,
            application_id: None,
        }
    }

    /// Create WebSocket client with API credentials for private channels
    ///
    /// # Arguments
    /// * `rest` - WOO REST client
    /// * `api_key` - API key
    /// * `api_secret` - API secret
    /// * `application_id` - Application ID from WOO X console
    pub fn with_credentials(
        rest: Woo,
        api_key: &str,
        api_secret: &str,
        application_id: &str,
    ) -> Self {
        Self {
            rest,
            ws_client: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: Arc::new(RwLock::new(None)),
            api_key: Some(api_key.to_string()),
            api_secret: Some(api_secret.to_string()),
            application_id: Some(application_id.to_string()),
        }
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: &str, api_secret: &str, application_id: &str) {
        self.api_key = Some(api_key.to_string());
        self.api_secret = Some(api_secret.to_string());
        self.application_id = Some(application_id.to_string());
    }

    /// Generate HMAC-SHA256 signature for authentication
    fn generate_signature(&self, timestamp: i64) -> CcxtResult<String> {
        let secret = self.api_secret.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required for private channels".to_string(),
        })?;

        // WOO signature format: |timestamp|
        let message = format!("|{timestamp}|");

        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Failed to create HMAC".to_string(),
            })?;

        mac.update(message.as_bytes());
        let signature = mac.finalize().into_bytes();

        Ok(hex::encode(signature))
    }

    /// Get private WebSocket URL
    fn get_private_ws_url(&self) -> CcxtResult<String> {
        let app_id = self.application_id.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Application ID required for private channels".to_string(),
        })?;
        Ok(format!("{WS_PRIVATE_URL}/{app_id}"))
    }

    /// 통합 심볼을 WOO 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        // WOO WebSocket uses BTC-USDT format
        symbol.replace("/", "-")
    }

    /// WOO 심볼을 통합 형식으로 변환 (BTC-USDT -> BTC/USDT, PERP_BTC_USDT -> PERP/BTC/USDT)
    fn to_unified_symbol(woo_symbol: &str) -> String {
        // Handle both spot (BTC-USDT) and perpetual (PERP_BTC_USDT) formats
        woo_symbol.replace("-", "/").replace("_", "/")
    }

    /// Timeframe을 WOO 인터벌로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1mon",
            _ => "1m",
        }
    }

    /// 타임프레임 문자열 파싱
    fn parse_timeframe(interval: &str) -> Timeframe {
        match interval {
            "1m" => Timeframe::Minute1,
            "5m" => Timeframe::Minute5,
            "15m" => Timeframe::Minute15,
            "30m" => Timeframe::Minute30,
            "1h" => Timeframe::Hour1,
            "4h" => Timeframe::Hour4,
            "12h" => Timeframe::Hour12,
            "1d" => Timeframe::Day1,
            "1w" => Timeframe::Week1,
            "1mon" => Timeframe::Month1,
            _ => Timeframe::Minute1,
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &WooTickerData, symbol: &str) -> WsTickerEvent {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: data.low.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: data.bid.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: data.bid_size.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask: data.ask.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: data.ask_size.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            close: data.close.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            last: data.close.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: data.amount.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent {
            symbol: symbol.to_string(),
            ticker,
        }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &WooOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            Some(OrderBookEntry {
                price: Decimal::from_str(&b.price).ok()?,
                amount: Decimal::from_str(&b.quantity).ok()?,
            })
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            Some(OrderBookEntry {
                price: Decimal::from_str(&a.price).ok()?,
                amount: Decimal::from_str(&a.quantity).ok()?,
            })
        }).collect();

        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            is_snapshot: true, // WOO sends full snapshots
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &WooTradeData, symbol: &str) -> WsTradeEvent {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = Decimal::from_str(&data.price).unwrap_or_default();
        let amount = Decimal::from_str(&data.size).unwrap_or_default();

        let trades = vec![Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.side.as_ref().map(|s| s.to_lowercase()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent {
            symbol: symbol.to_string(),
            trades,
        }
    }

    /// OHLCV 메시지 파싱
    fn parse_ohlcv(data: &WooOhlcvData, symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let timestamp = data.start_timestamp?;

        let ohlcv = OHLCV::new(
            timestamp,
            Decimal::from_str(&data.open).ok()?,
            Decimal::from_str(&data.high).ok()?,
            Decimal::from_str(&data.low).ok()?,
            Decimal::from_str(&data.close).ok()?,
            Decimal::from_str(&data.volume).ok()?,
        );

        Some(WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        })
    }

    // === Private Channel Parse Functions ===

    /// Parse balance update from private stream
    fn parse_balance_update(data: &WooBalanceData) -> WsBalanceEvent {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let mut currencies = HashMap::new();

        for holding in &data.holding {
            let free = Decimal::from_str(&holding.holding).unwrap_or_default();
            let used = Decimal::from_str(&holding.frozen).unwrap_or_default();
            let total = free + used;

            currencies.insert(
                holding.token.clone(),
                Balance {
                    free: Some(free),
                    used: Some(used),
                    total: Some(total),
                    debt: None,
                },
            );
        }

        let balances = Balances {
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            currencies,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsBalanceEvent { balances }
    }

    /// Parse execution report (order update) from private stream
    fn parse_execution_report(data: &WooExecutionReport) -> Vec<WsMessage> {
        let mut messages = Vec::new();
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        // Parse order status
        let status = match data.status.as_deref() {
            Some("NEW") => OrderStatus::Open,
            Some("PARTIAL_FILLED") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("CANCELLED") | Some("CANCELED") => OrderStatus::Canceled,
            Some("REJECTED") => OrderStatus::Rejected,
            Some("EXPIRED") => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        // Parse order type
        let order_type = match data.order_type.as_deref() {
            Some("MARKET") => OrderType::Market,
            Some("LIMIT") => OrderType::Limit,
            Some("STOP_LIMIT") => OrderType::StopLimit,
            Some("STOP_MARKET") => OrderType::StopMarket,
            Some("POST_ONLY") => OrderType::Limit,
            _ => OrderType::Limit,
        };

        // Parse side
        let side = match data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price = data.price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let amount = data.quantity.as_ref()
            .and_then(|q| Decimal::from_str(q).ok())
            .unwrap_or_default();
        let filled = data.executed_quantity.as_ref()
            .and_then(|q| Decimal::from_str(q).ok())
            .unwrap_or_default();
        let remaining = amount - filled;
        let average = data.average_executed_price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let cost = average.map(|avg| avg * filled);

        let fee = data.fee.as_ref().and_then(|f| {
            Decimal::from_str(f).ok().map(|f_val| Fee {
                cost: Some(f_val),
                currency: data.fee_asset.clone(),
                rate: None,
            })
        });

        let order = Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: data.executed_timestamp,
            last_update_timestamp: Some(timestamp),
            symbol: symbol.clone(),
            order_type,
            time_in_force: None,
            post_only: Some(data.order_type.as_deref() == Some("POST_ONLY")),
            reduce_only: data.reduce_only,
            side,
            price,
            trigger_price: None,
            amount,
            cost,
            average,
            filled,
            remaining: Some(remaining),
            status,
            fee: fee.clone(),
            trades: Vec::new(),
            fees: Vec::new(),
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        messages.push(WsMessage::Order(WsOrderEvent { order }));

        // If this is a fill, also emit a MyTrade event
        if data.executed_quantity.is_some() && filled > Decimal::ZERO {
            if let Some(exec_price) = data.executed_price.as_ref().and_then(|p| Decimal::from_str(p).ok()) {
                let exec_qty = data.last_executed_quantity.as_ref()
                    .and_then(|q| Decimal::from_str(q).ok())
                    .unwrap_or(filled);

                let trade = Trade {
                    id: data.trade_id.clone().unwrap_or_default(),
                    order: data.order_id.clone(),
                    timestamp: data.executed_timestamp,
                    datetime: data.executed_timestamp.map(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
                    symbol: symbol.clone(),
                    trade_type: None,
                    side: Some(if side == OrderSide::Buy { "buy".to_string() } else { "sell".to_string() }),
                    taker_or_maker: data.maker.map(|m| if m { TakerOrMaker::Maker } else { TakerOrMaker::Taker }),
                    price: exec_price,
                    amount: exec_qty,
                    cost: Some(exec_price * exec_qty),
                    fee: fee.clone(),
                    fees: Vec::new(),
                    info: serde_json::to_value(data).unwrap_or_default(),
                };

                messages.push(WsMessage::MyTrade(WsMyTradeEvent {
                    symbol,
                    trades: vec![trade],
                }));
            }
        }

        messages
    }

    /// Parse position update from private stream
    fn parse_position_update(data: &WooPositionData) -> WsPositionEvent {
        let positions: Vec<Position> = data.positions.iter().map(|p| {
            let symbol = Self::to_unified_symbol(&p.symbol);
            let contracts = Decimal::from_str(&p.position_qty).ok();
            let entry_price = p.average_open_price.as_ref().and_then(|ep| Decimal::from_str(ep).ok());
            let mark_price = p.mark_price.as_ref().and_then(|mp| Decimal::from_str(mp).ok());
            let unrealized_pnl = p.unrealized_pnl.as_ref().and_then(|pnl| Decimal::from_str(pnl).ok());
            let liquidation_price = p.est_liq_price.as_ref().and_then(|lp| Decimal::from_str(lp).ok());

            let side = contracts.and_then(|c| {
                if c > Decimal::ZERO {
                    Some(PositionSide::Long)
                } else if c < Decimal::ZERO {
                    Some(PositionSide::Short)
                } else {
                    None
                }
            });

            let margin_mode = match p.margin_mode.as_deref() {
                Some("CROSS") => Some(MarginMode::Cross),
                Some("ISOLATED") => Some(MarginMode::Isolated),
                _ => None,
            };

            Position {
                id: None,
                symbol,
                timestamp: p.timestamp,
                datetime: p.timestamp.map(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                hedged: Some(false),
                side,
                contracts: contracts.map(|c| c.abs()),
                contract_size: Some(Decimal::ONE),
                entry_price,
                mark_price,
                notional: None,
                leverage: p.leverage.and_then(|l| Decimal::from_str(&l.to_string()).ok()),
                collateral: None,
                initial_margin: None,
                maintenance_margin: None,
                initial_margin_percentage: None,
                maintenance_margin_percentage: None,
                unrealized_pnl,
                liquidation_price,
                margin_mode,
                margin_ratio: None,
                percentage: None,
                info: serde_json::to_value(p).unwrap_or_default(),
                realized_pnl: None,
                last_update_timestamp: p.timestamp,
                last_price: mark_price,
                stop_loss_price: None,
                take_profit_price: None,
            }
        }).collect();

        WsPositionEvent { positions }
    }

    /// WebSocket 메시지 처리
    fn process_message(message: &str) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<WooWsResponse>(message) {
            // 구독 확인 메시지
            if let Some(event) = response.event.as_deref() {
                if event == "subscribe" {
                    if let Some(topic) = response.topic {
                        let parts: Vec<&str> = topic.split('@').collect();
                        let symbol = if !parts.is_empty() {
                            Some(Self::to_unified_symbol(parts[0]))
                        } else {
                            None
                        };

                        return Some(WsMessage::Subscribed {
                            channel: topic,
                            symbol,
                        });
                    }
                    return None;
                }
            }

            // 데이터 메시지
            if let Some(topic) = response.topic {
                let parts: Vec<&str> = topic.split('@').collect();
                if parts.len() < 2 {
                    return None;
                }

                let woo_symbol = parts[0];
                let channel = parts[1];
                let symbol = Self::to_unified_symbol(woo_symbol);

                if let Some(data) = response.data {
                    match channel {
                        "ticker" => {
                            if let Ok(ticker_data) = serde_json::from_value::<WooTickerData>(data) {
                                return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data, &symbol)));
                            }
                        }
                        "orderbook" | "orderbookupdate" => {
                            if let Ok(ob_data) = serde_json::from_value::<WooOrderBookData>(data) {
                                return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol)));
                            }
                        }
                        "trade" => {
                            if let Ok(trade_data) = serde_json::from_value::<WooTradeData>(data) {
                                return Some(WsMessage::Trade(Self::parse_trade(&trade_data, &symbol)));
                            }
                        }
                        ch if ch.starts_with("kline_") => {
                            // Extract timeframe from channel (e.g., "kline_1m")
                            let interval = ch.strip_prefix("kline_").unwrap_or("1m");
                            let timeframe = Self::parse_timeframe(interval);

                            if let Ok(ohlcv_data) = serde_json::from_value::<WooOhlcvData>(data) {
                                if let Some(event) = Self::parse_ohlcv(&ohlcv_data, &symbol, timeframe) {
                                    return Some(WsMessage::Ohlcv(event));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        None
    }

    /// Process private WebSocket messages (returns multiple messages)
    fn process_private_message(message: &str) -> Vec<WsMessage> {
        let mut messages = Vec::new();

        if let Ok(response) = serde_json::from_str::<WooPrivateWsResponse>(message) {
            // Handle authentication response
            if response.event.as_deref() == Some("auth") {
                if response.success.unwrap_or(false) {
                    messages.push(WsMessage::Authenticated);
                } else {
                    let error_msg = response.message.unwrap_or_else(|| "Authentication failed".to_string());
                    messages.push(WsMessage::Error(error_msg));
                }
                return messages;
            }

            // Handle subscription confirmation
            if response.event.as_deref() == Some("subscribe") {
                if let Some(topic) = response.topic {
                    messages.push(WsMessage::Subscribed {
                        channel: topic,
                        symbol: None,
                    });
                }
                return messages;
            }

            // Handle data messages
            if let Some(topic) = response.topic {
                if let Some(data) = response.data {
                    match topic.as_str() {
                        "balance" => {
                            if let Ok(balance_data) = serde_json::from_value::<WooBalanceData>(data) {
                                messages.push(WsMessage::Balance(Self::parse_balance_update(&balance_data)));
                            }
                        }
                        "executionreport" => {
                            if let Ok(exec_data) = serde_json::from_value::<WooExecutionReport>(data) {
                                messages.extend(Self::parse_execution_report(&exec_data));
                            }
                        }
                        "position" => {
                            if let Ok(pos_data) = serde_json::from_value::<WooPositionData>(data) {
                                messages.push(WsMessage::Position(Self::parse_position_update(&pos_data)));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        messages
    }

    /// Subscribe to private stream with authentication
    async fn subscribe_private_stream(
        &self,
        topic: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        *self.event_tx.write().await = Some(event_tx.clone());

        let ws_url = self.get_private_ws_url()?;
        let api_key = self.api_key.clone().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required for private channels".to_string(),
        })?;

        let timestamp = Utc::now().timestamp_millis();
        let signature = self.generate_signature(timestamp)?;

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Send authentication message
        let auth_msg = serde_json::json!({
            "event": "auth",
            "params": {
                "apikey": api_key,
                "sign": signature,
                "timestamp": timestamp.to_string()
            }
        });

        ws_client.send(&auth_msg.to_string())?;

        // Wait briefly for auth to process, then subscribe to topic
        let subscribe_msg = serde_json::json!({
            "event": "subscribe",
            "topic": topic
        });

        ws_client.send(&subscribe_msg.to_string())?;

        *self.ws_client.write().await = Some(ws_client);

        // Store subscription
        self.subscriptions.write().await.insert(topic.to_string(), topic.to_string());

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
                        let messages = Self::process_private_message(&msg);
                        for ws_msg in messages {
                            let _ = tx.send(ws_msg);
                        }
                    }
                    WsEvent::Error(e) => {
                        let _ = tx.send(WsMessage::Error(e));
                    }
                    _ => {}
                }
            }
        });

        Ok(event_rx)
    }

    /// 채널 구독 및 이벤트 스트림 반환
    async fn subscribe_stream(&self, topic: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        *self.event_tx.write().await = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        let subscribe_msg = WooSubscribeMessage {
            event: "subscribe".to_string(),
            topic: topic.to_string(),
        };

        let msg_json = serde_json::to_string(&subscribe_msg).unwrap();
        ws_client.send(&msg_json)?;

        *self.ws_client.write().await = Some(ws_client);

        // 구독 저장
        self.subscriptions.write().await.insert(topic.to_string(), topic.to_string());

        // 이벤트 처리 태스크
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
                    WsEvent::Error(e) => {
                        let _ = tx.send(WsMessage::Error(e));
                    }
                    _ => {}
                }
            }
        });

        Ok(event_rx)
    }
}

#[async_trait]
impl WsExchange for WooWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let woo_symbol = Self::format_symbol(symbol);
        let _topic = format!("{woo_symbol}@ticker");

        // Need to use interior mutability for subscribe_stream
        // This is a simplified placeholder - proper implementation would require Arc<RwLock<Self>>
        let (_tx, rx) = mpsc::unbounded_channel();
        Ok(rx)
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let woo_symbol = Self::format_symbol(symbol);
        let _topic = format!("{woo_symbol}@orderbook");

        let (_tx, rx) = mpsc::unbounded_channel();
        Ok(rx)
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let woo_symbol = Self::format_symbol(symbol);
        let _topic = format!("{woo_symbol}@trade");

        let (_tx, rx) = mpsc::unbounded_channel();
        Ok(rx)
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let woo_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let _topic = format!("{woo_symbol}@kline_{interval}");

        let (_tx, rx) = mpsc::unbounded_channel();
        Ok(rx)
    }

    // === Private Streams ===

    /// Watch order updates
    ///
    /// Subscribes to `executionreport` channel for real-time order status updates.
    ///
    /// # Requirements
    /// - API credentials must be set via `with_credentials()` or `set_credentials()`
    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_private_stream("executionreport").await
    }

    /// Watch user trade fills
    ///
    /// Subscribes to `executionreport` channel for real-time fill notifications.
    /// Uses the same channel as watch_orders since WOO combines execution data.
    ///
    /// # Requirements
    /// - API credentials must be set via `with_credentials()` or `set_credentials()`
    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_private_stream("executionreport").await
    }

    /// Watch balance updates
    ///
    /// Subscribes to `balance` channel for real-time account balance updates.
    /// Updates are pushed immediately when balance changes occur.
    ///
    /// # Requirements
    /// - API credentials must be set via `with_credentials()` or `set_credentials()`
    ///
    /// # Example
    /// ```ignore
    /// let ws = WooWs::with_credentials(rest, "api_key", "api_secret", "app_id");
    /// let mut rx = ws.watch_balance().await?;
    /// while let Some(msg) = rx.recv().await {
    ///     if let WsMessage::Balance(event) = msg {
    ///         for (token, balance) in &event.balances.currencies {
    ///             println!("{}: {:?}", token, balance.free);
    ///         }
    ///     }
    /// }
    /// ```
    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_private_stream("balance").await
    }

    /// Watch position updates
    ///
    /// Subscribes to `position` channel for real-time position updates.
    /// Provides position data for perpetual contracts.
    ///
    /// # Requirements
    /// - API credentials must be set via `with_credentials()` or `set_credentials()`
    async fn watch_positions(&self, _symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_private_stream("position").await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.read().await.is_some() {
            return Ok(());
        }

        let (event_tx, _) = mpsc::unbounded_channel();
        *self.event_tx.write().await = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        *self.ws_client.write().await = Some(ws_client);

        // 메시지 처리 태스크 시작
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
                    WsEvent::Error(e) => {
                        let _ = tx.send(WsMessage::Error(e));
                    }
                    _ => {}
                }
            }
        });

        let _ = event_tx.send(WsMessage::Connected);
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = self.ws_client.write().await.take() {
            client.close()?;
        }

        if let Some(tx) = self.event_tx.read().await.as_ref() {
            let _ = tx.send(WsMessage::Disconnected);
        }

        self.subscriptions.write().await.clear();
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        match self.ws_client.read().await.as_ref() {
            Some(c) => c.is_connected().await,
            None => false,
        }
    }
}

// === WOO WebSocket Response Types ===

#[derive(Debug, Deserialize)]
struct WooWsResponse {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct WooSubscribeMessage {
    event: String,
    topic: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct WooTickerData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    bid: Option<String>,
    #[serde(rename = "bidSize", default)]
    bid_size: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(rename = "askSize", default)]
    ask_size: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct WooOrderBookData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    bids: Vec<WooOrderBookLevel>,
    #[serde(default)]
    asks: Vec<WooOrderBookLevel>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct WooOrderBookLevel {
    #[serde(default)]
    price: String,
    #[serde(default)]
    quantity: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct WooTradeData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    price: String,
    #[serde(default)]
    size: String,
    #[serde(default)]
    side: Option<String>,
    #[serde(rename = "tradeId", default)]
    trade_id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct WooOhlcvData {
    #[serde(rename = "startTimestamp", default)]
    start_timestamp: Option<i64>,
    #[serde(default)]
    open: String,
    #[serde(default)]
    high: String,
    #[serde(default)]
    low: String,
    #[serde(default)]
    close: String,
    #[serde(default)]
    volume: String,
}

// === Private Channel Message Types ===

#[derive(Debug, Deserialize)]
struct WooPrivateWsResponse {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// Balance data from private stream
#[derive(Debug, Deserialize, Serialize)]
struct WooBalanceData {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    holding: Vec<WooHolding>,
}

/// Individual token holding
#[derive(Debug, Deserialize, Serialize)]
struct WooHolding {
    #[serde(default)]
    token: String,
    #[serde(default)]
    holding: String,
    #[serde(default)]
    frozen: String,
    #[serde(default)]
    pending_short_qty: Option<String>,
}

/// Execution report (order update) from private stream
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WooExecutionReport {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(rename = "clientOrderId", default)]
    client_order_id: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(rename = "executedQuantity", default)]
    executed_quantity: Option<String>,
    #[serde(rename = "lastExecutedQuantity", default)]
    last_executed_quantity: Option<String>,
    #[serde(rename = "executedPrice", default)]
    executed_price: Option<String>,
    #[serde(rename = "averageExecutedPrice", default)]
    average_executed_price: Option<String>,
    #[serde(rename = "executedTimestamp", default)]
    executed_timestamp: Option<i64>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(rename = "feeAsset", default)]
    fee_asset: Option<String>,
    #[serde(default)]
    maker: Option<bool>,
    #[serde(rename = "reduceOnly", default)]
    reduce_only: Option<bool>,
    #[serde(rename = "tradeId", default)]
    trade_id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

/// Position data from private stream
#[derive(Debug, Deserialize, Serialize)]
struct WooPositionData {
    #[serde(default)]
    positions: Vec<WooPositionItem>,
}

/// Individual position item
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WooPositionItem {
    #[serde(default)]
    symbol: String,
    #[serde(rename = "positionQty", default)]
    position_qty: String,
    #[serde(rename = "averageOpenPrice", default)]
    average_open_price: Option<String>,
    #[serde(rename = "markPrice", default)]
    mark_price: Option<String>,
    #[serde(rename = "unrealizedPnl", default)]
    unrealized_pnl: Option<String>,
    #[serde(rename = "estLiqPrice", default)]
    est_liq_price: Option<String>,
    #[serde(rename = "marginMode", default)]
    margin_mode: Option<String>,
    #[serde(default)]
    leverage: Option<i32>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(WooWs::format_symbol("BTC/USDT"), "BTC-USDT");
        assert_eq!(WooWs::format_symbol("ETH/USDT"), "ETH-USDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        // Spot symbols (dash format)
        assert_eq!(WooWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
        assert_eq!(WooWs::to_unified_symbol("ETH-USDT"), "ETH/USDT");
        // Perpetual symbols (underscore format)
        assert_eq!(WooWs::to_unified_symbol("PERP_BTC_USDT"), "PERP/BTC/USDT");
        assert_eq!(WooWs::to_unified_symbol("PERP_ETH_USDT"), "PERP/ETH/USDT");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(WooWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(WooWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(WooWs::format_interval(Timeframe::Day1), "1d");
    }

    #[test]
    fn test_parse_timeframe() {
        assert_eq!(WooWs::parse_timeframe("1m"), Timeframe::Minute1);
        assert_eq!(WooWs::parse_timeframe("1h"), Timeframe::Hour1);
        assert_eq!(WooWs::parse_timeframe("1d"), Timeframe::Day1);
    }

    // === Private Channel Tests ===

    #[test]
    fn test_parse_balance_update() {
        let data = WooBalanceData {
            timestamp: Some(1710125266669),
            holding: vec![
                WooHolding {
                    token: "USDT".to_string(),
                    holding: "1000.50".to_string(),
                    frozen: "50.25".to_string(),
                    pending_short_qty: None,
                },
                WooHolding {
                    token: "BTC".to_string(),
                    holding: "0.5".to_string(),
                    frozen: "0.1".to_string(),
                    pending_short_qty: None,
                },
            ],
        };

        let event = WooWs::parse_balance_update(&data);

        assert!(event.balances.currencies.contains_key("USDT"));
        assert!(event.balances.currencies.contains_key("BTC"));

        let usdt = event.balances.currencies.get("USDT").unwrap();
        assert_eq!(usdt.free, Some(Decimal::from_str("1000.50").unwrap()));
        assert_eq!(usdt.used, Some(Decimal::from_str("50.25").unwrap()));
        assert_eq!(usdt.total, Some(Decimal::from_str("1050.75").unwrap()));

        let btc = event.balances.currencies.get("BTC").unwrap();
        assert_eq!(btc.free, Some(Decimal::from_str("0.5").unwrap()));
        assert_eq!(btc.used, Some(Decimal::from_str("0.1").unwrap()));
        assert_eq!(btc.total, Some(Decimal::from_str("0.6").unwrap()));
    }

    #[test]
    fn test_parse_execution_report_new_order() {
        let data = WooExecutionReport {
            symbol: "BTC-USDT".to_string(),
            order_id: Some("12345".to_string()),
            client_order_id: Some("client-1".to_string()),
            order_type: Some("LIMIT".to_string()),
            side: Some("BUY".to_string()),
            status: Some("NEW".to_string()),
            price: Some("50000.0".to_string()),
            quantity: Some("0.1".to_string()),
            executed_quantity: Some("0".to_string()),
            last_executed_quantity: None,
            executed_price: None,
            average_executed_price: None,
            executed_timestamp: None,
            fee: None,
            fee_asset: None,
            maker: None,
            reduce_only: None,
            trade_id: None,
            timestamp: Some(1710125266669),
        };

        let messages = WooWs::parse_execution_report(&data);

        assert_eq!(messages.len(), 1);
        if let WsMessage::Order(event) = &messages[0] {
            assert_eq!(event.order.id, "12345");
            assert_eq!(event.order.symbol, "BTC/USDT");
            assert_eq!(event.order.side, OrderSide::Buy);
            assert_eq!(event.order.status, OrderStatus::Open);
            assert_eq!(event.order.order_type, OrderType::Limit);
            assert_eq!(event.order.price, Some(Decimal::from_str("50000.0").unwrap()));
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_parse_execution_report_filled() {
        let data = WooExecutionReport {
            symbol: "ETH-USDT".to_string(),
            order_id: Some("67890".to_string()),
            client_order_id: None,
            order_type: Some("MARKET".to_string()),
            side: Some("SELL".to_string()),
            status: Some("FILLED".to_string()),
            price: None,
            quantity: Some("1.0".to_string()),
            executed_quantity: Some("1.0".to_string()),
            last_executed_quantity: Some("1.0".to_string()),
            executed_price: Some("3500.0".to_string()),
            average_executed_price: Some("3500.0".to_string()),
            executed_timestamp: Some(1710125266670),
            fee: Some("3.5".to_string()),
            fee_asset: Some("USDT".to_string()),
            maker: Some(false),
            reduce_only: None,
            trade_id: Some("trade-123".to_string()),
            timestamp: Some(1710125266669),
        };

        let messages = WooWs::parse_execution_report(&data);

        assert_eq!(messages.len(), 2); // Order + MyTrade

        let mut has_order = false;
        let mut has_trade = false;

        for msg in &messages {
            match msg {
                WsMessage::Order(event) => {
                    assert_eq!(event.order.id, "67890");
                    assert_eq!(event.order.status, OrderStatus::Closed);
                    assert_eq!(event.order.filled, Decimal::from_str("1.0").unwrap());
                    has_order = true;
                }
                WsMessage::MyTrade(event) => {
                    assert_eq!(event.symbol, "ETH/USDT");
                    assert_eq!(event.trades.len(), 1);
                    assert_eq!(event.trades[0].id, "trade-123");
                    assert_eq!(event.trades[0].taker_or_maker, Some(TakerOrMaker::Taker));
                    has_trade = true;
                }
                _ => {}
            }
        }

        assert!(has_order, "Expected Order message");
        assert!(has_trade, "Expected MyTrade message");
    }

    #[test]
    fn test_parse_position_update() {
        let data = WooPositionData {
            positions: vec![
                WooPositionItem {
                    symbol: "PERP_BTC_USDT".to_string(),
                    position_qty: "0.5".to_string(),
                    average_open_price: Some("50000.0".to_string()),
                    mark_price: Some("51000.0".to_string()),
                    unrealized_pnl: Some("500.0".to_string()),
                    est_liq_price: Some("45000.0".to_string()),
                    margin_mode: Some("CROSS".to_string()),
                    leverage: Some(10),
                    timestamp: Some(1710125266669),
                },
            ],
        };

        let event = WooWs::parse_position_update(&data);

        assert_eq!(event.positions.len(), 1);
        let pos = &event.positions[0];
        assert_eq!(pos.symbol, "PERP/BTC/USDT");
        assert_eq!(pos.side, Some(PositionSide::Long));
        assert_eq!(pos.contracts, Some(Decimal::from_str("0.5").unwrap()));
        assert_eq!(pos.entry_price, Some(Decimal::from_str("50000.0").unwrap()));
        assert_eq!(pos.mark_price, Some(Decimal::from_str("51000.0").unwrap()));
        assert_eq!(pos.unrealized_pnl, Some(Decimal::from_str("500.0").unwrap()));
        assert_eq!(pos.margin_mode, Some(MarginMode::Cross));
    }

    #[test]
    fn test_parse_position_short() {
        let data = WooPositionData {
            positions: vec![
                WooPositionItem {
                    symbol: "PERP_ETH_USDT".to_string(),
                    position_qty: "-2.0".to_string(),
                    average_open_price: Some("3500.0".to_string()),
                    mark_price: Some("3400.0".to_string()),
                    unrealized_pnl: Some("200.0".to_string()),
                    est_liq_price: None,
                    margin_mode: Some("ISOLATED".to_string()),
                    leverage: Some(5),
                    timestamp: Some(1710125266669),
                },
            ],
        };

        let event = WooWs::parse_position_update(&data);

        assert_eq!(event.positions.len(), 1);
        let pos = &event.positions[0];
        assert_eq!(pos.side, Some(PositionSide::Short));
        assert_eq!(pos.contracts, Some(Decimal::from_str("2.0").unwrap())); // abs value
        assert_eq!(pos.margin_mode, Some(MarginMode::Isolated));
    }

    #[test]
    fn test_process_private_message_balance() {
        let json = r#"{
            "topic": "balance",
            "data": {
                "timestamp": 1710125266669,
                "holding": [
                    {"token": "USDT", "holding": "500.0", "frozen": "0"}
                ]
            }
        }"#;

        let messages = WooWs::process_private_message(json);

        assert_eq!(messages.len(), 1);
        if let WsMessage::Balance(event) = &messages[0] {
            assert!(event.balances.currencies.contains_key("USDT"));
            let usdt = event.balances.currencies.get("USDT").unwrap();
            assert_eq!(usdt.free, Some(Decimal::from_str("500.0").unwrap()));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_process_private_message_auth_success() {
        let json = r#"{
            "event": "auth",
            "success": true
        }"#;

        let messages = WooWs::process_private_message(json);

        assert_eq!(messages.len(), 1);
        assert!(matches!(&messages[0], WsMessage::Authenticated));
    }

    #[test]
    fn test_process_private_message_auth_failure() {
        let json = r#"{
            "event": "auth",
            "success": false,
            "message": "Invalid API key"
        }"#;

        let messages = WooWs::process_private_message(json);

        assert_eq!(messages.len(), 1);
        if let WsMessage::Error(msg) = &messages[0] {
            assert_eq!(msg, "Invalid API key");
        } else {
            panic!("Expected Error message");
        }
    }
}
