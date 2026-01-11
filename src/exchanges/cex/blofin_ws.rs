//! Blofin WebSocket Implementation
//!
//! Blofin 실시간 데이터 스트리밍 (Public & Private Streams)
//!
//! ## Private Channels
//! - `orders`: Real-time order updates
//! - `positions`: Position information and changes
//! - `account`: Account balance and equity updates
//!
//! ## Authentication
//! Private streams require HMAC-SHA256 authentication with:
//! - apiKey: API key
//! - passphrase: API passphrase
//! - timestamp: Current Unix milliseconds
//! - sign: Base64(Hex(HMAC-SHA256("/users/self/verify" + "GET" + timestamp + nonce, secret)))
//! - nonce: Unique identifier

#![allow(clippy::manual_strip)]

use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
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

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, MarginMode, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, Position, PositionSide, Ticker, Timeframe, Trade, OHLCV,
    WsBalanceEvent, WsExchange, WsMessage, WsMyTradeEvent, WsOhlcvEvent, WsOrderBookEvent,
    WsOrderEvent, WsPositionEvent, WsTickerEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://openapi.blofin.com/ws/public";
const WS_PRIVATE_URL: &str = "wss://openapi.blofin.com/ws/private";

/// Blofin WebSocket 클라이언트
pub struct BlofinWs {
    ws_client: Arc<RwLock<Option<WsClient>>>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Arc<RwLock<Option<mpsc::UnboundedSender<WsMessage>>>>,
    /// API key for private channels
    api_key: Option<String>,
    /// API secret for signing
    api_secret: Option<String>,
    /// API passphrase
    passphrase: Option<String>,
}

impl BlofinWs {
    /// 새 Blofin WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: Arc::new(RwLock::new(None)),
            api_key: None,
            api_secret: None,
            passphrase: None,
        }
    }

    /// Create WebSocket client with API credentials for private channels
    ///
    /// # Arguments
    /// * `api_key` - API key
    /// * `api_secret` - API secret
    /// * `passphrase` - API passphrase
    pub fn with_credentials(api_key: &str, api_secret: &str, passphrase: &str) -> Self {
        Self {
            ws_client: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: Arc::new(RwLock::new(None)),
            api_key: Some(api_key.to_string()),
            api_secret: Some(api_secret.to_string()),
            passphrase: Some(passphrase.to_string()),
        }
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: &str, api_secret: &str, passphrase: &str) {
        self.api_key = Some(api_key.to_string());
        self.api_secret = Some(api_secret.to_string());
        self.passphrase = Some(passphrase.to_string());
    }

    /// Generate signature for WebSocket authentication
    /// Format: Base64(Hex(HMAC-SHA256("/users/self/verify" + "GET" + timestamp + nonce, secret)))
    fn generate_signature(&self, timestamp: i64, nonce: &str) -> CcxtResult<String> {
        let secret = self.api_secret.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required for private channels".to_string(),
        })?;

        // Build the message: path + method + timestamp + nonce
        let message = format!("/users/self/verifyGET{timestamp}{nonce}");

        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Invalid secret key: {e}"),
            })?;
        mac.update(message.as_bytes());
        let result = mac.finalize();

        // Convert to hex, then base64 encode
        let hex_sig = hex::encode(result.into_bytes());
        Ok(general_purpose::STANDARD.encode(hex_sig.as_bytes()))
    }

    /// 심볼을 Blofin 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Timeframe을 Blofin candle로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1H",
            Timeframe::Hour2 => "2H",
            Timeframe::Hour4 => "4H",
            Timeframe::Hour6 => "6H",
            Timeframe::Hour12 => "12H",
            Timeframe::Day1 => "1D",
            Timeframe::Week1 => "1W",
            Timeframe::Month1 => "1M",
            _ => "1m",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BlofinTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open24h.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.vol24h.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.vol_ccy24h.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BlofinOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
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

        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.seq_id,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &BlofinTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.px.parse().unwrap_or_default();
        let amount: Decimal = data.sz.parse().unwrap_or_default();

        let trades = vec![Trade {
            id: data.trade_id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(data.side.to_lowercase()),
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

    /// OHLCV 메시지 파싱
    fn parse_candle(data: &[String], symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        if data.len() < 6 {
            return None;
        }

        let timestamp = data[0].parse::<i64>().ok()?;
        let ohlcv = OHLCV::new(
            timestamp,
            data[1].parse().ok()?,
            data[2].parse().ok()?,
            data[3].parse().ok()?,
            data[4].parse().ok()?,
            data[5].parse().ok()?,
        );

        Some(WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        })
    }

    /// Blofin 심볼을 통합 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(blofin_symbol: &str) -> String {
        blofin_symbol.replace("-", "/")
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<BlofinWsResponse>(msg) {
            // 에러 체크
            if response.code.as_ref().map(|c| c != "0").unwrap_or(false) {
                return Some(WsMessage::Error(response.msg.unwrap_or_default()));
            }

            // 이벤트 처리
            if let Some(event) = &response.event {
                if event == "subscribe" {
                    return Some(WsMessage::Subscribed {
                        channel: response.arg.as_ref()
                            .and_then(|a| a.channel.clone())
                            .unwrap_or_default(),
                        symbol: response.arg.as_ref().and_then(|a| a.inst_id.clone()),
                    });
                }
            }

            // 데이터 처리
            if let (Some(arg), Some(data_arr)) = (&response.arg, &response.data) {
                let channel = arg.channel.as_deref().unwrap_or("");

                // Ticker
                if channel == "tickers" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(ticker_data) = serde_json::from_value::<BlofinTickerData>(first.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }
                }

                // OrderBook
                if channel.starts_with("books") {
                    if let Some(first) = data_arr.first() {
                        if let Ok(ob_data) = serde_json::from_value::<BlofinOrderBookData>(first.clone()) {
                            let symbol = arg.inst_id.as_ref()
                                .map(|s| Self::to_unified_symbol(s))
                                .unwrap_or_default();
                            let is_snapshot = response.action.as_deref() == Some("snapshot");
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol, is_snapshot)));
                        }
                    }
                }

                // Trade
                if channel == "trades" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(trade_data) = serde_json::from_value::<BlofinTradeData>(first.clone()) {
                            return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                        }
                    }
                }

                // Candle
                if channel.starts_with("candle") {
                    if let Some(first) = data_arr.first() {
                        if let Ok(candle_arr) = serde_json::from_value::<Vec<String>>(first.clone()) {
                            let symbol = arg.inst_id.as_ref()
                                .map(|s| Self::to_unified_symbol(s))
                                .unwrap_or_default();
                            // Extract timeframe from channel (e.g., "candle1m")
                            let timeframe = match &channel[6..] {
                                "1m" => Timeframe::Minute1,
                                "3m" => Timeframe::Minute3,
                                "5m" => Timeframe::Minute5,
                                "15m" => Timeframe::Minute15,
                                "30m" => Timeframe::Minute30,
                                "1H" => Timeframe::Hour1,
                                "2H" => Timeframe::Hour2,
                                "4H" => Timeframe::Hour4,
                                "6H" => Timeframe::Hour6,
                                "12H" => Timeframe::Hour12,
                                "1D" => Timeframe::Day1,
                                "1W" => Timeframe::Week1,
                                "1M" => Timeframe::Month1,
                                _ => Timeframe::Minute1,
                            };
                            if let Some(event) = Self::parse_candle(&candle_arr, &symbol, timeframe) {
                                return Some(WsMessage::Ohlcv(event));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Process private channel messages
    fn process_private_message(msg: &str) -> Vec<WsMessage> {
        let mut messages = Vec::new();

        if let Ok(response) = serde_json::from_str::<BlofinWsResponse>(msg) {
            // Handle login response
            if let Some(event) = &response.event {
                if event == "login" {
                    if response.code.as_ref().map(|c| c == "0").unwrap_or(false) {
                        messages.push(WsMessage::Authenticated);
                    } else {
                        messages.push(WsMessage::Error(
                            response.msg.clone().unwrap_or_else(|| "Login failed".to_string())
                        ));
                    }
                    return messages;
                }
            }

            // Handle data messages
            if let (Some(arg), Some(data_arr)) = (&response.arg, &response.data) {
                let channel = arg.channel.as_deref().unwrap_or("");

                // Account channel (balance updates)
                if channel == "account" {
                    for data in data_arr {
                        if let Ok(account_data) = serde_json::from_value::<BlofinAccountData>(data.clone()) {
                            messages.push(WsMessage::Balance(Self::parse_account_update(&account_data)));
                        }
                    }
                }

                // Orders channel
                if channel == "orders" {
                    for data in data_arr {
                        if let Ok(order_data) = serde_json::from_value::<BlofinOrderData>(data.clone()) {
                            let (order_event, trade_event) = Self::parse_order_update(&order_data);
                            messages.push(WsMessage::Order(order_event));
                            if let Some(trade) = trade_event {
                                messages.push(WsMessage::MyTrade(trade));
                            }
                        }
                    }
                }

                // Positions channel
                if channel == "positions" {
                    for data in data_arr {
                        if let Ok(position_data) = serde_json::from_value::<BlofinPositionData>(data.clone()) {
                            messages.push(WsMessage::Position(Self::parse_position_update(&position_data)));
                        }
                    }
                }
            }
        }

        messages
    }

    /// Parse account update from private stream
    fn parse_account_update(data: &BlofinAccountData) -> WsBalanceEvent {
        let timestamp = data.u_time.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let mut currencies = HashMap::new();

        for detail in &data.details {
            let free = detail.available_equity.as_ref()
                .and_then(|v| Decimal::from_str(v).ok());
            let used = detail.frozen_balance.as_ref()
                .and_then(|v| Decimal::from_str(v).ok());
            let total = detail.equity.as_ref()
                .and_then(|v| Decimal::from_str(v).ok());

            currencies.insert(detail.currency.clone(), Balance {
                free,
                used,
                total,
                debt: None,
            });
        }

        let balances = Balances {
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()),
            currencies,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsBalanceEvent { balances }
    }

    /// Parse order update from private stream
    fn parse_order_update(data: &BlofinOrderData) -> (WsOrderEvent, Option<WsMyTradeEvent>) {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.u_time.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = match data.side.as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_str() {
            "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            "post_only" => OrderType::Limit,
            "fok" => OrderType::Limit,
            "ioc" => OrderType::Limit,
            _ => OrderType::Limit,
        };

        let status = match data.state.as_str() {
            "live" => OrderStatus::Open,
            "partially_filled" => OrderStatus::Open,
            "canceled" => OrderStatus::Canceled,
            "filled" => OrderStatus::Closed,
            "mmp_canceled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let amount = data.size.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let filled = data.filled_size.as_ref().and_then(|f| Decimal::from_str(f).ok());
        let average = data.average_price.as_ref().and_then(|a| Decimal::from_str(a).ok());
        let remaining = match (amount, filled) {
            (Some(a), Some(f)) => Some(a - f),
            _ => None,
        };

        let fee = data.fee.as_ref().and_then(|f| {
            Decimal::from_str(f).ok().map(|cost| Fee {
                cost: Some(cost.abs()),
                currency: data.fee_currency.clone(),
                rate: None,
            })
        });

        let order = Order {
            id: data.order_id.clone(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()),
            last_trade_timestamp: None,
            last_update_timestamp: Some(timestamp),
            symbol: symbol.clone(),
            order_type,
            time_in_force: None,
            post_only: Some(data.order_type == "post_only"),
            reduce_only: data.reduce_only.as_ref().map(|r| r == "true"),
            side,
            price,
            trigger_price: None,
            amount: amount.unwrap_or_default(),
            cost: None,
            average,
            filled: filled.unwrap_or_default(),
            remaining,
            status,
            fee: fee.clone(),
            trades: Vec::new(),
            fees: Vec::new(),
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        let order_event = WsOrderEvent { order };

        // If there's a fill, create a trade event
        let trade_event = if filled.is_some() && filled.unwrap_or_default() > Decimal::ZERO {
            if let (Some(avg_price), Some(fill_qty)) = (average, filled) {
                let trade = Trade {
                    id: data.trade_id.clone().unwrap_or_default(),
                    order: Some(data.order_id.clone()),
                    timestamp: Some(timestamp),
                    datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()),
                    symbol: symbol.clone(),
                    trade_type: None,
                    side: Some(if side == OrderSide::Buy { "buy".to_string() } else { "sell".to_string() }),
                    taker_or_maker: None,
                    price: avg_price,
                    amount: fill_qty,
                    cost: Some(avg_price * fill_qty),
                    fee,
                    fees: Vec::new(),
                    info: serde_json::to_value(data).unwrap_or_default(),
                };
                Some(WsMyTradeEvent {
                    symbol,
                    trades: vec![trade],
                })
            } else {
                None
            }
        } else {
            None
        };

        (order_event, trade_event)
    }

    /// Parse position update from private stream
    fn parse_position_update(data: &BlofinPositionData) -> WsPositionEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.u_time.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let contracts = data.position.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let entry_price = data.average_price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let mark_price = data.mark_price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let unrealized_pnl = data.upl.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let liquidation_price = data.liquidation_price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let leverage = data.leverage.as_ref()
            .and_then(|l| Decimal::from_str(l).ok());
        let margin = data.margin.as_ref()
            .and_then(|m| Decimal::from_str(m).ok());

        let side = match data.position_side.as_deref() {
            Some("long") => Some(PositionSide::Long),
            Some("short") => Some(PositionSide::Short),
            Some("net") => contracts.map(|c| {
                if c > Decimal::ZERO { PositionSide::Long }
                else if c < Decimal::ZERO { PositionSide::Short }
                else { PositionSide::Long }
            }),
            _ => None,
        };

        let margin_mode = match data.margin_mode.as_deref() {
            Some("cross") => Some(MarginMode::Cross),
            Some("isolated") => Some(MarginMode::Isolated),
            _ => None,
        };

        let position = Position {
            id: None,
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()),
            hedged: None,
            side,
            contracts: contracts.map(|c| c.abs()),
            contract_size: None,
            entry_price,
            mark_price,
            notional: None,
            leverage,
            collateral: margin,
            initial_margin: None,
            maintenance_margin: None,
            initial_margin_percentage: None,
            maintenance_margin_percentage: None,
            unrealized_pnl,
            realized_pnl: None,
            liquidation_price,
            margin_mode,
            margin_ratio: None,
            percentage: None,
            last_update_timestamp: Some(timestamp),
            last_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsPositionEvent {
            positions: vec![position],
        }
    }

    /// Subscribe to private stream with authentication
    async fn subscribe_private_stream(
        &self,
        channel: &str,
        inst_id: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        *self.event_tx.write().await = Some(event_tx.clone());

        let api_key = self.api_key.clone().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required for private channels".to_string(),
        })?;
        let passphrase = self.passphrase.clone().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Passphrase required for private channels".to_string(),
        })?;

        let timestamp = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = self.generate_signature(timestamp, &nonce)?;

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PRIVATE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Send login message
        let login_msg = serde_json::json!({
            "op": "login",
            "args": [{
                "apiKey": api_key,
                "passphrase": passphrase,
                "timestamp": timestamp.to_string(),
                "sign": signature,
                "nonce": nonce
            }]
        });

        ws_client.send(&login_msg.to_string())?;

        // Subscribe to channel after a brief delay for auth
        let subscribe_args = if let Some(id) = inst_id {
            serde_json::json!([{
                "channel": channel,
                "instId": Self::format_symbol(id)
            }])
        } else {
            serde_json::json!([{
                "channel": channel
            }])
        };

        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": subscribe_args
        });

        ws_client.send(&subscribe_msg.to_string())?;

        *self.ws_client.write().await = Some(ws_client);

        // Store subscription
        let key = format!("{}:{}", channel, inst_id.unwrap_or(""));
        self.subscriptions.write().await.insert(key, channel.to_string());

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

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&self, args: Vec<serde_json::Value>, channel: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
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
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": args
        });
        ws_client.send(&subscribe_msg.to_string())?;

        *self.ws_client.write().await = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

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

impl Default for BlofinWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BlofinWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let args = vec![serde_json::json!({
            "channel": "tickers",
            "instId": Self::format_symbol(symbol)
        })];
        self.subscribe_stream(args, "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let args: Vec<serde_json::Value> = symbols
            .iter()
            .map(|s| serde_json::json!({
                "channel": "tickers",
                "instId": Self::format_symbol(s)
            }))
            .collect();
        self.subscribe_stream(args, "tickers", None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let channel = match limit.unwrap_or(400) {
            1..=5 => "books5",
            6..=50 => "books50-l2-tbt",
            _ => "books",
        };
        let args = vec![serde_json::json!({
            "channel": channel,
            "instId": Self::format_symbol(symbol)
        })];
        self.subscribe_stream(args, "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let args = vec![serde_json::json!({
            "channel": "trades",
            "instId": Self::format_symbol(symbol)
        })];
        self.subscribe_stream(args, "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let interval = Self::format_interval(timeframe);
        let args = vec![serde_json::json!({
            "channel": format!("candle{}", interval),
            "instId": Self::format_symbol(symbol)
        })];
        self.subscribe_stream(args, "ohlcv", Some(symbol)).await
    }

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_private_stream("orders", symbol).await
    }

    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Blofin sends trade updates via the orders channel when orders are filled
        self.subscribe_private_stream("orders", symbol).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_private_stream("account", None).await
    }

    async fn watch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Blofin positions channel can filter by instId, but we use first symbol if provided
        let inst_id = symbols.and_then(|s| s.first().copied());
        self.subscribe_private_stream("positions", inst_id).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = self.ws_client.read().await.as_ref() {
            client.close()?;
        }
        *self.ws_client.write().await = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        if let Some(client) = self.ws_client.read().await.as_ref() {
            client.is_connected().await
        } else {
            false
        }
    }
}

// === Blofin WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct BlofinWsResponse {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    arg: Option<BlofinWsArg>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    data: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlofinWsArg {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    inst_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlofinTickerData {
    inst_id: String,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    last_sz: Option<String>,
    #[serde(default)]
    ask_price: Option<String>,
    #[serde(default)]
    ask_size: Option<String>,
    #[serde(default)]
    bid_price: Option<String>,
    #[serde(default)]
    bid_size: Option<String>,
    #[serde(default)]
    open24h: Option<String>,
    #[serde(default)]
    high24h: Option<String>,
    #[serde(default)]
    low24h: Option<String>,
    #[serde(default)]
    vol_ccy24h: Option<String>,
    #[serde(default)]
    vol24h: Option<String>,
    #[serde(default)]
    ts: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlofinOrderBookData {
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    seq_id: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlofinTradeData {
    inst_id: String,
    trade_id: String,
    px: String,
    sz: String,
    side: String,
    #[serde(default)]
    ts: Option<String>,
}

// === Private Channel Data Structures ===

/// Account data for balance updates
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlofinAccountData {
    /// Account update time
    #[serde(default)]
    u_time: Option<String>,
    /// Total equity in USD
    #[serde(default)]
    total_equity: Option<String>,
    /// Isolated margin equity
    #[serde(default)]
    iso_equity: Option<String>,
    /// Available margin
    #[serde(default)]
    available_margin: Option<String>,
    /// Per-currency details
    #[serde(default)]
    details: Vec<BlofinAccountDetail>,
}

/// Account detail for each currency
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlofinAccountDetail {
    /// Currency code (e.g., "USDT")
    #[serde(default)]
    currency: String,
    /// Available equity
    #[serde(default)]
    available_equity: Option<String>,
    /// Equity
    #[serde(default)]
    equity: Option<String>,
    /// Frozen balance
    #[serde(default)]
    frozen_balance: Option<String>,
    /// Cash balance
    #[serde(default)]
    cash_balance: Option<String>,
    /// Unrealized PnL
    #[serde(default)]
    upl: Option<String>,
}

/// Order data for order updates
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlofinOrderData {
    /// Instrument ID (e.g., "BTC-USDT")
    inst_id: String,
    /// Order ID
    order_id: String,
    /// Client order ID
    #[serde(default)]
    client_order_id: Option<String>,
    /// Order side: buy, sell
    side: String,
    /// Order type: market, limit, post_only, fok, ioc
    order_type: String,
    /// Order price
    #[serde(default)]
    price: Option<String>,
    /// Order size
    #[serde(default)]
    size: Option<String>,
    /// Filled size
    #[serde(default)]
    filled_size: Option<String>,
    /// Average fill price
    #[serde(default)]
    average_price: Option<String>,
    /// Order state: live, partially_filled, canceled, filled, mmp_canceled
    state: String,
    /// Fee amount (negative for rebates)
    #[serde(default)]
    fee: Option<String>,
    /// Fee currency
    #[serde(default)]
    fee_currency: Option<String>,
    /// Trade ID (for filled orders)
    #[serde(default)]
    trade_id: Option<String>,
    /// Reduce only flag
    #[serde(default)]
    reduce_only: Option<String>,
    /// Position side: long, short, net
    #[serde(default)]
    position_side: Option<String>,
    /// Creation time
    #[serde(default)]
    c_time: Option<String>,
    /// Update time
    #[serde(default)]
    u_time: Option<String>,
}

/// Position data for position updates
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlofinPositionData {
    /// Instrument ID (e.g., "BTC-USDT")
    inst_id: String,
    /// Position side: long, short, net
    #[serde(default)]
    position_side: Option<String>,
    /// Position quantity (positive for long, negative for short in net mode)
    #[serde(default)]
    position: Option<String>,
    /// Average entry price
    #[serde(default)]
    average_price: Option<String>,
    /// Mark price
    #[serde(default)]
    mark_price: Option<String>,
    /// Unrealized PnL
    #[serde(default)]
    upl: Option<String>,
    /// Unrealized PnL ratio
    #[serde(default)]
    upl_ratio: Option<String>,
    /// Liquidation price
    #[serde(default)]
    liquidation_price: Option<String>,
    /// Leverage
    #[serde(default)]
    leverage: Option<String>,
    /// Margin mode: cross, isolated
    #[serde(default)]
    margin_mode: Option<String>,
    /// Margin amount
    #[serde(default)]
    margin: Option<String>,
    /// Update time
    #[serde(default)]
    u_time: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BlofinWs::format_symbol("BTC/USDT"), "BTC-USDT");
        assert_eq!(BlofinWs::format_symbol("ETH/BTC"), "ETH-BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BlofinWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
        assert_eq!(BlofinWs::to_unified_symbol("ETH-BTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(BlofinWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(BlofinWs::format_interval(Timeframe::Hour1), "1H");
        assert_eq!(BlofinWs::format_interval(Timeframe::Day1), "1D");
    }

    #[test]
    fn test_with_credentials() {
        let client = BlofinWs::with_credentials("test_key", "test_secret", "test_pass");
        assert_eq!(client.api_key, Some("test_key".to_string()));
        assert_eq!(client.api_secret, Some("test_secret".to_string()));
        assert_eq!(client.passphrase, Some("test_pass".to_string()));
    }

    #[test]
    fn test_set_credentials() {
        let mut client = BlofinWs::new();
        assert!(client.api_key.is_none());

        client.set_credentials("key", "secret", "pass");
        assert_eq!(client.api_key, Some("key".to_string()));
        assert_eq!(client.api_secret, Some("secret".to_string()));
        assert_eq!(client.passphrase, Some("pass".to_string()));
    }

    #[test]
    fn test_generate_signature() {
        let client = BlofinWs::with_credentials("key", "test_secret", "pass");
        let sig = client.generate_signature(1700000000000, "test-nonce");
        assert!(sig.is_ok());
        // Signature should be base64 encoded hex
        let sig_str = sig.unwrap();
        assert!(!sig_str.is_empty());
    }

    #[test]
    fn test_parse_account_update() {
        let data = BlofinAccountData {
            u_time: Some("1700000000000".to_string()),
            total_equity: Some("10000.0".to_string()),
            iso_equity: None,
            available_margin: Some("5000.0".to_string()),
            details: vec![
                BlofinAccountDetail {
                    currency: "USDT".to_string(),
                    available_equity: Some("5000.0".to_string()),
                    equity: Some("10000.0".to_string()),
                    frozen_balance: Some("5000.0".to_string()),
                    cash_balance: Some("10000.0".to_string()),
                    upl: Some("100.0".to_string()),
                },
            ],
        };

        let event = BlofinWs::parse_account_update(&data);
        assert!(event.balances.currencies.contains_key("USDT"));
        let usdt = event.balances.currencies.get("USDT").unwrap();
        assert_eq!(usdt.free, Some(Decimal::from_str("5000.0").unwrap()));
        assert_eq!(usdt.used, Some(Decimal::from_str("5000.0").unwrap()));
        assert_eq!(usdt.total, Some(Decimal::from_str("10000.0").unwrap()));
    }

    #[test]
    fn test_parse_order_update() {
        let data = BlofinOrderData {
            inst_id: "BTC-USDT".to_string(),
            order_id: "123456".to_string(),
            client_order_id: Some("client123".to_string()),
            side: "buy".to_string(),
            order_type: "limit".to_string(),
            price: Some("50000.0".to_string()),
            size: Some("1.0".to_string()),
            filled_size: Some("0.5".to_string()),
            average_price: Some("49900.0".to_string()),
            state: "partially_filled".to_string(),
            fee: Some("-0.001".to_string()),
            fee_currency: Some("BTC".to_string()),
            trade_id: Some("trade123".to_string()),
            reduce_only: None,
            position_side: None,
            c_time: Some("1700000000000".to_string()),
            u_time: Some("1700000001000".to_string()),
        };

        let (order_event, trade_event) = BlofinWs::parse_order_update(&data);

        assert_eq!(order_event.order.id, "123456");
        assert_eq!(order_event.order.symbol, "BTC/USDT");
        assert_eq!(order_event.order.side, OrderSide::Buy);
        assert_eq!(order_event.order.order_type, OrderType::Limit);
        assert_eq!(order_event.order.status, OrderStatus::Open);
        assert_eq!(order_event.order.amount, Decimal::from_str("1.0").unwrap());
        assert_eq!(order_event.order.filled, Decimal::from_str("0.5").unwrap());

        // Should have a trade event since there's a fill
        assert!(trade_event.is_some());
        let trade = trade_event.unwrap();
        assert_eq!(trade.symbol, "BTC/USDT");
        assert_eq!(trade.trades.len(), 1);
    }

    #[test]
    fn test_parse_position_update() {
        let data = BlofinPositionData {
            inst_id: "BTC-USDT".to_string(),
            position_side: Some("long".to_string()),
            position: Some("1.5".to_string()),
            average_price: Some("50000.0".to_string()),
            mark_price: Some("50500.0".to_string()),
            upl: Some("750.0".to_string()),
            upl_ratio: Some("0.01".to_string()),
            liquidation_price: Some("45000.0".to_string()),
            leverage: Some("10".to_string()),
            margin_mode: Some("cross".to_string()),
            margin: Some("5000.0".to_string()),
            u_time: Some("1700000000000".to_string()),
        };

        let event = BlofinWs::parse_position_update(&data);

        assert_eq!(event.positions.len(), 1);
        let position = &event.positions[0];
        assert_eq!(position.symbol, "BTC/USDT");
        assert_eq!(position.side, Some(PositionSide::Long));
        assert_eq!(position.contracts, Some(Decimal::from_str("1.5").unwrap()));
        assert_eq!(position.entry_price, Some(Decimal::from_str("50000.0").unwrap()));
        assert_eq!(position.mark_price, Some(Decimal::from_str("50500.0").unwrap()));
        assert_eq!(position.unrealized_pnl, Some(Decimal::from_str("750.0").unwrap()));
        assert_eq!(position.liquidation_price, Some(Decimal::from_str("45000.0").unwrap()));
        assert_eq!(position.leverage, Some(Decimal::from_str("10").unwrap()));
        assert_eq!(position.margin_mode, Some(MarginMode::Cross));
        assert_eq!(position.collateral, Some(Decimal::from_str("5000.0").unwrap()));
    }

    #[test]
    fn test_process_private_message_login_success() {
        let msg = r#"{"event":"login","code":"0","msg":"Success"}"#;
        let messages = BlofinWs::process_private_message(msg);
        assert_eq!(messages.len(), 1);
        assert!(matches!(messages[0], WsMessage::Authenticated));
    }

    #[test]
    fn test_process_private_message_login_failure() {
        let msg = r#"{"event":"login","code":"60001","msg":"Invalid API key"}"#;
        let messages = BlofinWs::process_private_message(msg);
        assert_eq!(messages.len(), 1);
        match &messages[0] {
            WsMessage::Error(e) => assert!(e.contains("Invalid API key")),
            _ => panic!("Expected error message"),
        }
    }

    #[test]
    fn test_process_private_message_account() {
        let msg = r#"{
            "arg": {"channel": "account"},
            "data": [{
                "uTime": "1700000000000",
                "totalEquity": "10000.0",
                "details": [{
                    "currency": "USDT",
                    "availableEquity": "5000.0",
                    "equity": "10000.0",
                    "frozenBalance": "5000.0"
                }]
            }]
        }"#;

        let messages = BlofinWs::process_private_message(msg);
        assert_eq!(messages.len(), 1);
        match &messages[0] {
            WsMessage::Balance(event) => {
                assert!(event.balances.currencies.contains_key("USDT"));
            }
            _ => panic!("Expected balance message"),
        }
    }

    #[test]
    fn test_process_private_message_orders() {
        let msg = r#"{
            "arg": {"channel": "orders"},
            "data": [{
                "instId": "BTC-USDT",
                "orderId": "123456",
                "side": "buy",
                "orderType": "limit",
                "price": "50000.0",
                "size": "1.0",
                "filledSize": "0.0",
                "state": "live",
                "uTime": "1700000000000"
            }]
        }"#;

        let messages = BlofinWs::process_private_message(msg);
        assert!(!messages.is_empty());
        match &messages[0] {
            WsMessage::Order(event) => {
                assert_eq!(event.order.symbol, "BTC/USDT");
                assert_eq!(event.order.id, "123456");
            }
            _ => panic!("Expected order message"),
        }
    }

    #[test]
    fn test_process_private_message_positions() {
        let msg = r#"{
            "arg": {"channel": "positions"},
            "data": [{
                "instId": "BTC-USDT",
                "positionSide": "long",
                "position": "1.0",
                "averagePrice": "50000.0",
                "markPrice": "50100.0",
                "upl": "100.0",
                "leverage": "10",
                "marginMode": "cross",
                "uTime": "1700000000000"
            }]
        }"#;

        let messages = BlofinWs::process_private_message(msg);
        assert_eq!(messages.len(), 1);
        match &messages[0] {
            WsMessage::Position(event) => {
                assert_eq!(event.positions.len(), 1);
                assert_eq!(event.positions[0].symbol, "BTC/USDT");
            }
            _ => panic!("Expected position message"),
        }
    }
}
