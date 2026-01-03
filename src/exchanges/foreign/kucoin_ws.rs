//! Kucoin WebSocket Implementation
//!
//! Kucoin 실시간 데이터 스트리밍
//! Note: Kucoin requires a token from REST API before connecting to WebSocket

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, TakerOrMaker, Ticker, Timeframe, TimeInForce, Trade, OHLCV,
    WsBalanceEvent, WsExchange, WsMessage, WsMyTradeEvent, WsOrderBookEvent,
    WsOrderEvent, WsOhlcvEvent, WsTickerEvent, WsTradeEvent,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::str::FromStr;
use base64::{Engine as _, engine::general_purpose};

/// Kucoin WebSocket 클라이언트
pub struct KucoinWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    connect_id: String,
    /// API credentials for private channels
    api_key: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
}

impl KucoinWs {
    /// 새 Kucoin WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            connect_id: uuid::Uuid::new_v4().to_string(),
            api_key: None,
            api_secret: None,
            passphrase: None,
        }
    }

    /// 자격 증명으로 클라이언트 생성
    pub fn with_credentials(api_key: &str, api_secret: &str, passphrase: &str) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            connect_id: uuid::Uuid::new_v4().to_string(),
            api_key: Some(api_key.to_string()),
            api_secret: Some(api_secret.to_string()),
            passphrase: Some(passphrase.to_string()),
        }
    }

    /// 자격 증명 설정
    pub fn set_credentials(&mut self, api_key: &str, api_secret: &str, passphrase: &str) {
        self.api_key = Some(api_key.to_string());
        self.api_secret = Some(api_secret.to_string());
        self.passphrase = Some(passphrase.to_string());
    }

    /// API 키 반환
    pub fn get_api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// 심볼을 Kucoin 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Kucoin 심볼을 통합 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(kucoin_symbol: &str) -> String {
        kucoin_symbol.replace("-", "/")
    }

    /// Timeframe을 Kucoin 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1min",
            Timeframe::Minute3 => "3min",
            Timeframe::Minute5 => "5min",
            Timeframe::Minute15 => "15min",
            Timeframe::Minute30 => "30min",
            Timeframe::Hour1 => "1hour",
            Timeframe::Hour2 => "2hour",
            Timeframe::Hour4 => "4hour",
            Timeframe::Hour6 => "6hour",
            Timeframe::Hour8 => "8hour",
            Timeframe::Hour12 => "12hour",
            Timeframe::Day1 => "1day",
            Timeframe::Week1 => "1week",
            _ => "1min",
        }
    }

    /// Public 토큰 가져오기 (WebSocket 연결에 필요)
    async fn get_public_token() -> CcxtResult<KucoinWsToken> {
        let url = "https://api.kucoin.com/api/v1/bullet-public";
        let client = reqwest::Client::new();
        let response = client
            .post(url)
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError { url: url.to_string(), message: e.to_string() })?;

        let token_response: KucoinTokenResponse = response
            .json()
            .await
            .map_err(|e| CcxtError::ParseError { data_type: "KucoinTokenResponse".to_string(), message: e.to_string() })?;

        token_response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to get WebSocket token".into(),
        })
    }

    /// HMAC-SHA256 서명 생성
    fn sign(secret: &str, message: &str) -> String {
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(message.as_bytes());
        let result = mac.finalize();
        general_purpose::STANDARD.encode(result.into_bytes())
    }

    /// Passphrase 암호화
    fn encrypt_passphrase(secret: &str, passphrase: &str) -> String {
        Self::sign(secret, passphrase)
    }

    /// Private 토큰 가져오기 (인증된 WebSocket 연결에 필요)
    async fn get_private_token(&self) -> CcxtResult<KucoinWsToken> {
        let api_key = self.api_key.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required for private channels".into(),
        })?;
        let api_secret = self.api_secret.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required for private channels".into(),
        })?;
        let passphrase = self.passphrase.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Passphrase required for private channels".into(),
        })?;

        let url = "https://api.kucoin.com/api/v1/bullet-private";
        let timestamp = Utc::now().timestamp_millis().to_string();
        let str_to_sign = format!("{timestamp}POST/api/v1/bullet-private");
        let signature = Self::sign(api_secret, &str_to_sign);
        let encrypted_passphrase = Self::encrypt_passphrase(api_secret, passphrase);

        let client = reqwest::Client::new();
        let response = client
            .post(url)
            .header("KC-API-KEY", api_key)
            .header("KC-API-SIGN", signature)
            .header("KC-API-TIMESTAMP", &timestamp)
            .header("KC-API-PASSPHRASE", encrypted_passphrase)
            .header("KC-API-KEY-VERSION", "2")
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError { url: url.to_string(), message: e.to_string() })?;

        let token_response: KucoinTokenResponse = response
            .json()
            .await
            .map_err(|e| CcxtError::ParseError { data_type: "KucoinTokenResponse".to_string(), message: e.to_string() })?;

        token_response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to get private WebSocket token".into(),
        })
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &KucoinTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            bid: data.best_bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.best_bid_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.best_ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.best_ask_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.change_price.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.change_rate.as_ref().and_then(|v| {
                v.parse::<Decimal>().ok().map(|d| d * Decimal::from(100))
            }),
            average: None,
            base_volume: data.vol.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.vol_value.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &KucoinOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
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

        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.sequence,
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
    fn parse_trade(data: &KucoinTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        // Kucoin time is in nanoseconds
        let timestamp = data.time.map(|t| t / 1_000_000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.size.parse().unwrap_or_default();

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
            taker_or_maker: data.taker_order_id.as_ref().map(|_| crate::types::TakerOrMaker::Taker),
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
    fn parse_candle(data: &KucoinCandleData, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let candles = &data.candles;
        if candles.len() < 7 {
            return None;
        }

        let timestamp = candles[0].parse::<i64>().ok()? * 1000;
        let ohlcv = OHLCV::new(
            timestamp,
            candles[1].parse().ok()?,
            candles[3].parse().ok()?,
            candles[4].parse().ok()?,
            candles[2].parse().ok()?,
            candles[5].parse().ok()?,
        );

        let symbol = Self::to_unified_symbol(&data.symbol);

        Some(WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        })
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Ping/pong handling
        if msg.contains("\"type\":\"pong\"") || msg.contains("\"type\":\"welcome\"") {
            return None;
        }

        if let Ok(response) = serde_json::from_str::<KucoinWsResponse>(msg) {
            // Ack message (subscription confirmation)
            if response.msg_type.as_deref() == Some("ack") {
                return Some(WsMessage::Subscribed {
                    channel: "subscription".to_string(),
                    symbol: None,
                });
            }

            // Message data
            if response.msg_type.as_deref() == Some("message") {
                if let (Some(topic), Some(data)) = (&response.topic, &response.data) {
                    // Ticker
                    if topic.starts_with("/market/ticker:") {
                        if let Ok(ticker_data) = serde_json::from_value::<KucoinTickerData>(data.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }

                    // Level2 OrderBook
                    if topic.starts_with("/market/level2:") {
                        if let Ok(ob_data) = serde_json::from_value::<KucoinOrderBookData>(data.clone()) {
                            let symbol_part = topic.trim_start_matches("/market/level2:");
                            let symbol = Self::to_unified_symbol(symbol_part);
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol, false)));
                        }
                    }

                    // Match (trades)
                    if topic.starts_with("/market/match:") {
                        if let Ok(trade_data) = serde_json::from_value::<KucoinTradeData>(data.clone()) {
                            return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                        }
                    }

                    // Candles
                    if topic.starts_with("/market/candles:") {
                        if let Ok(candle_data) = serde_json::from_value::<KucoinCandleData>(data.clone()) {
                            // Extract timeframe from topic (e.g., "/market/candles:BTC-USDT_1hour")
                            let parts: Vec<&str> = topic.split('_').collect();
                            let timeframe = if parts.len() >= 2 {
                                match parts[1] {
                                    "1min" => Timeframe::Minute1,
                                    "3min" => Timeframe::Minute3,
                                    "5min" => Timeframe::Minute5,
                                    "15min" => Timeframe::Minute15,
                                    "30min" => Timeframe::Minute30,
                                    "1hour" => Timeframe::Hour1,
                                    "2hour" => Timeframe::Hour2,
                                    "4hour" => Timeframe::Hour4,
                                    "6hour" => Timeframe::Hour6,
                                    "8hour" => Timeframe::Hour8,
                                    "12hour" => Timeframe::Hour12,
                                    "1day" => Timeframe::Day1,
                                    "1week" => Timeframe::Week1,
                                    _ => Timeframe::Minute1,
                                }
                            } else {
                                Timeframe::Minute1
                            };

                            if let Some(event) = Self::parse_candle(&candle_data, timeframe) {
                                return Some(WsMessage::Ohlcv(event));
                            }
                        }
                    }

                    // === Private Channels ===

                    // Order updates (/spotMarket/tradeOrders)
                    if topic.starts_with("/spotMarket/tradeOrders") {
                        if let Ok(order_data) = serde_json::from_value::<KucoinOrderData>(data.clone()) {
                            // Check if this is a trade (match) event
                            if order_data.order_type.as_deref() == Some("match") {
                                if let Some(trade_event) = Self::parse_my_trade(&order_data) {
                                    return Some(WsMessage::MyTrade(trade_event));
                                }
                            }
                            // Always return order update
                            if let Some(order_event) = Self::parse_order_update(&order_data) {
                                return Some(WsMessage::Order(order_event));
                            }
                        }
                    }

                    // Balance updates (/account/balance)
                    if topic.starts_with("/account/balance") {
                        if let Ok(balance_data) = serde_json::from_value::<KucoinBalanceData>(data.clone()) {
                            if let Some(balance_event) = Self::parse_balance_update(&balance_data) {
                                return Some(WsMessage::Balance(balance_event));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// 주문 업데이트 파싱
    fn parse_order_update(data: &KucoinOrderData) -> Option<WsOrderEvent> {
        let symbol = Self::to_unified_symbol(&data.symbol);
        // Kucoin timestamp is in nanoseconds, convert to milliseconds
        let timestamp_ms = data.ts.map(|t| t / 1_000_000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = match data.side.as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type_str.as_deref().unwrap_or("limit") {
            "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            "stop" => OrderType::StopLimit,
            "stop_limit" => OrderType::StopLimit,
            _ => OrderType::Limit,
        };

        let status = match data.status.as_deref() {
            Some("open") => OrderStatus::Open,
            Some("match") => OrderStatus::Open, // Partial fill
            Some("done") => {
                // Check if canceled or filled
                if data.remain_size.as_ref().and_then(|s| Decimal::from_str(s).ok()).unwrap_or_default() > Decimal::ZERO {
                    OrderStatus::Canceled
                } else {
                    OrderStatus::Closed
                }
            }
            Some("canceled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let time_in_force = match data.time_in_force.as_deref() {
            Some("GTC") => Some(TimeInForce::GTC),
            Some("IOC") => Some(TimeInForce::IOC),
            Some("FOK") => Some(TimeInForce::FOK),
            Some("GTX") => Some(TimeInForce::PO),
            _ => Some(TimeInForce::GTC),
        };

        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let amount = data.size.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let filled = data.filled_size.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let remaining = data.remain_size.as_ref().and_then(|s| Decimal::from_str(s).ok());

        let average = if let (Some(amt), Some(fill)) = (amount, filled) {
            if fill > Decimal::ZERO && amt > Decimal::ZERO {
                // Kucoin doesn't provide average directly, we'd need fills_price
                data.fill_price.as_ref().and_then(|p| Decimal::from_str(p).ok())
            } else {
                None
            }
        } else {
            None
        };

        let cost = if let (Some(fill), Some(avg)) = (filled, average) {
            Some(fill * avg)
        } else {
            None
        };

        let fee = data.fee.as_ref().and_then(|f| Decimal::from_str(f).ok()).map(|fee_amount| {
            Fee {
                currency: data.fee_currency.clone(),
                cost: Some(fee_amount),
                rate: None,
            }
        });

        let is_post_only = data.time_in_force.as_deref() == Some("GTX");

        let order = Order {
            id: data.order_id.clone(),
            client_order_id: data.client_oid.clone(),
            timestamp: Some(timestamp_ms),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp_ms)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: Some(timestamp_ms),
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force,
            side,
            price,
            trigger_price: None,
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            average,
            amount: amount.unwrap_or_default(),
            filled: filled.unwrap_or_default(),
            remaining,
            cost,
            trades: Vec::new(),
            reduce_only: None,
            post_only: Some(is_post_only),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsOrderEvent { order })
    }

    /// 내 체결 파싱
    fn parse_my_trade(data: &KucoinOrderData) -> Option<WsMyTradeEvent> {
        // Only process if there's a match
        if data.match_size.is_none() || data.match_price.is_none() {
            return None;
        }

        let symbol = Self::to_unified_symbol(&data.symbol);
        // Kucoin timestamp is in nanoseconds, convert to milliseconds
        let timestamp_ms = data.ts.map(|t| t / 1_000_000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let match_size = data.match_size.as_ref().and_then(|s| Decimal::from_str(s).ok())?;
        let match_price = data.match_price.as_ref().and_then(|p| Decimal::from_str(p).ok())?;

        if match_size == Decimal::ZERO {
            return None;
        }

        let side = Some(data.side.to_lowercase());
        let taker_or_maker = if data.liquidity.as_deref() == Some("taker") {
            Some(TakerOrMaker::Taker)
        } else {
            Some(TakerOrMaker::Maker)
        };

        let fee = data.fee.as_ref().and_then(|f| Decimal::from_str(f).ok()).map(|fee_amount| {
            Fee {
                currency: data.fee_currency.clone(),
                cost: Some(fee_amount),
                rate: None,
            }
        });

        let trade = Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: Some(data.order_id.clone()),
            timestamp: Some(timestamp_ms),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp_ms)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker,
            price: match_price,
            amount: match_size,
            cost: Some(match_size * match_price),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsMyTradeEvent {
            symbol,
            trades: vec![trade],
        })
    }

    /// 잔고 업데이트 파싱
    fn parse_balance_update(data: &KucoinBalanceData) -> Option<WsBalanceEvent> {
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());
        let mut currencies: HashMap<String, Balance> = HashMap::new();

        let total = Decimal::from_str(&data.total).unwrap_or_default();
        let available = Decimal::from_str(&data.available).unwrap_or_default();
        let hold = Decimal::from_str(&data.hold).unwrap_or_default();

        let balance = Balance {
            free: Some(available),
            used: Some(hold),
            total: Some(total),
            debt: None,
        };

        currencies.insert(data.currency.clone(), balance);

        Some(WsBalanceEvent {
            balances: Balances {
                currencies,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                info: serde_json::to_value(data).unwrap_or_default(),
            },
        })
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, topics: Vec<String>, channel: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // Get WebSocket token
        let token_data = Self::get_public_token().await?;
        let server = token_data.instance_servers.first()
            .ok_or_else(|| CcxtError::ExchangeError { message: "No WebSocket server available".into() })?;

        let ws_url = format!("{}?token={}&connectId={}", server.endpoint, token_data.token, self.connect_id);

        let ping_interval = (server.ping_interval.unwrap_or(18000) / 1000) as u64;

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: ping_interval,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        let id = uuid::Uuid::new_v4().to_string();
        let subscribe_msg = serde_json::json!({
            "id": id,
            "type": "subscribe",
            "topic": topics.join(","),
            "privateChannel": false,
            "response": true
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

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

    /// Private 채널 구독 (인증 필요)
    async fn subscribe_private_stream(&mut self, topics: Vec<String>, channel: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // Get private WebSocket token (requires authentication)
        let token_data = self.get_private_token().await?;
        let server = token_data.instance_servers.first()
            .ok_or_else(|| CcxtError::ExchangeError { message: "No WebSocket server available".into() })?;

        let ws_url = format!("{}?token={}&connectId={}", server.endpoint, token_data.token, self.connect_id);

        let ping_interval = (server.ping_interval.unwrap_or(18000) / 1000) as u64;

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: ping_interval,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Subscribe to private topics
        for topic in &topics {
            let id = uuid::Uuid::new_v4().to_string();
            let subscribe_msg = serde_json::json!({
                "id": id,
                "type": "subscribe",
                "topic": topic,
                "privateChannel": true,
                "response": true
            });
            ws_client.send(&subscribe_msg.to_string())?;
        }

        self.ws_client = Some(ws_client);

        // Store subscription
        {
            let key = format!("private:{}", channel);
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // Send authenticated message
        let _ = event_tx.send(WsMessage::Authenticated);

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

impl Default for KucoinWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for KucoinWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let topics = vec![format!("/market/ticker:{}", market_id)];
        client.subscribe_stream(topics, "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_ids: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        let topics = vec![format!("/market/ticker:{}", market_ids.join(","))];
        client.subscribe_stream(topics, "tickers", None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        // Use level2Depth5 for smaller updates, level2:50 for more depth
        let topics = vec![format!("/market/level2:{}", market_id)];
        client.subscribe_stream(topics, "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let topics = vec![format!("/market/match:{}", market_id)];
        client.subscribe_stream(topics, "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let topics = vec![format!("/market/candles:{}_{}", market_id, interval)];
        client.subscribe_stream(topics, "ohlcv", Some(symbol)).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
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

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        if self.api_key.is_none() || self.api_secret.is_none() || self.passphrase.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API key, secret, and passphrase required for private channels".into(),
            });
        }
        // Authentication is done via token request, not WebSocket message
        Ok(())
    }

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let (Some(key), Some(secret), Some(pass)) =
            (self.api_key.as_ref(), self.api_secret.as_ref(), self.passphrase.as_ref()) {
            Self::with_credentials(key, secret, pass)
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for watch_orders".into(),
            });
        };

        let topic = if let Some(sym) = symbol {
            format!("/spotMarket/tradeOrders:{}", Self::format_symbol(sym))
        } else {
            "/spotMarket/tradeOrders".to_string()
        };

        client.subscribe_private_stream(vec![topic], "orders").await
    }

    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let (Some(key), Some(secret), Some(pass)) =
            (self.api_key.as_ref(), self.api_secret.as_ref(), self.passphrase.as_ref()) {
            Self::with_credentials(key, secret, pass)
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for watch_my_trades".into(),
            });
        };

        // Use the same topic as orders - trades come from order updates
        let topic = if let Some(sym) = symbol {
            format!("/spotMarket/tradeOrders:{}", Self::format_symbol(sym))
        } else {
            "/spotMarket/tradeOrders".to_string()
        };

        client.subscribe_private_stream(vec![topic], "myTrades").await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let (Some(key), Some(secret), Some(pass)) =
            (self.api_key.as_ref(), self.api_secret.as_ref(), self.passphrase.as_ref()) {
            Self::with_credentials(key, secret, pass)
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for watch_balance".into(),
            });
        };

        let topic = "/account/balance".to_string();
        client.subscribe_private_stream(vec![topic], "balance").await
    }
}

// === Kucoin WebSocket Types ===

#[derive(Debug, Deserialize)]
struct KucoinTokenResponse {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    data: Option<KucoinWsToken>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KucoinWsToken {
    token: String,
    instance_servers: Vec<KucoinWsServer>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KucoinWsServer {
    endpoint: String,
    #[serde(default)]
    ping_interval: Option<i64>,
    #[serde(default)]
    ping_timeout: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KucoinWsResponse {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinTickerData {
    symbol: String,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    best_bid: Option<String>,
    #[serde(default)]
    best_bid_size: Option<String>,
    #[serde(default)]
    best_ask: Option<String>,
    #[serde(default)]
    best_ask_size: Option<String>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    vol: Option<String>,
    #[serde(default)]
    vol_value: Option<String>,
    #[serde(default)]
    change_price: Option<String>,
    #[serde(default)]
    change_rate: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinOrderBookData {
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    sequence: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinTradeData {
    symbol: String,
    trade_id: String,
    price: String,
    size: String,
    side: String,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    taker_order_id: Option<String>,
    #[serde(default)]
    maker_order_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinCandleData {
    symbol: String,
    candles: Vec<String>,
    #[serde(default)]
    time: Option<i64>,
}

// === Private Channel Data Structures ===

/// 주문 업데이트 데이터
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinOrderData {
    symbol: String,
    order_id: String,
    side: String,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default, rename = "orderType")]
    order_type_str: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    filled_size: Option<String>,
    #[serde(default, rename = "remainSize")]
    remain_size: Option<String>,
    #[serde(default)]
    client_oid: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    order_time: Option<i64>,
    #[serde(default)]
    time_in_force: Option<String>,
    // Match (trade) specific fields
    #[serde(default)]
    match_price: Option<String>,
    #[serde(default)]
    match_size: Option<String>,
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    liquidity: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    fee_currency: Option<String>,
    #[serde(default)]
    fill_price: Option<String>,
}

/// 잔고 업데이트 데이터
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinBalanceData {
    currency: String,
    total: String,
    available: String,
    hold: String,
    #[serde(default)]
    available_change: Option<String>,
    #[serde(default)]
    hold_change: Option<String>,
    #[serde(default)]
    relation_event: Option<String>,
    #[serde(default)]
    relation_event_id: Option<String>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    account_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(KucoinWs::format_symbol("BTC/USDT"), "BTC-USDT");
        assert_eq!(KucoinWs::format_symbol("ETH/BTC"), "ETH-BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(KucoinWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
        assert_eq!(KucoinWs::to_unified_symbol("ETH-BTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(KucoinWs::format_interval(Timeframe::Minute1), "1min");
        assert_eq!(KucoinWs::format_interval(Timeframe::Hour1), "1hour");
        assert_eq!(KucoinWs::format_interval(Timeframe::Day1), "1day");
    }

    #[test]
    fn test_with_credentials() {
        let client = KucoinWs::with_credentials("test_key", "test_secret", "test_pass");
        assert_eq!(client.get_api_key(), Some("test_key"));
        assert!(client.api_secret.is_some());
        assert!(client.passphrase.is_some());
    }

    #[test]
    fn test_set_credentials() {
        let mut client = KucoinWs::new();
        assert!(client.get_api_key().is_none());
        client.set_credentials("api_key", "api_secret", "passphrase");
        assert_eq!(client.get_api_key(), Some("api_key"));
    }

    #[test]
    fn test_sign() {
        let signature = KucoinWs::sign("secret", "message");
        assert!(!signature.is_empty());
        // Verify it's base64 encoded
        assert!(general_purpose::STANDARD.decode(&signature).is_ok());
    }

    #[test]
    fn test_parse_order_update() {
        let json = r#"{"symbol":"BTC-USDT","orderId":"5c52423054d74a0001a8d123","side":"buy","orderType":"limit","type":"open","price":"10000","size":"0.1","filledSize":"0","remainSize":"0.1","clientOid":"test123","ts":1609459200000000000,"status":"open","timeInForce":"GTC"}"#;
        let data: KucoinOrderData = serde_json::from_str(json).unwrap();
        let event = KucoinWs::parse_order_update(&data);
        assert!(event.is_some());
        let order_event = event.unwrap();
        assert_eq!(order_event.order.symbol, "BTC/USDT");
        assert_eq!(order_event.order.side, OrderSide::Buy);
        assert_eq!(order_event.order.order_type, OrderType::Limit);
        assert_eq!(order_event.order.status, OrderStatus::Open);
    }

    #[test]
    fn test_parse_my_trade() {
        let json = r#"{"symbol":"BTC-USDT","orderId":"5c52423054d74a0001a8d123","side":"buy","orderType":"limit","type":"match","price":"10000","size":"0.1","filledSize":"0.05","remainSize":"0.05","matchPrice":"10000","matchSize":"0.05","tradeId":"trade123","liquidity":"taker","fee":"0.0001","feeCurrency":"USDT","ts":1609459200000000000}"#;
        let data: KucoinOrderData = serde_json::from_str(json).unwrap();
        let event = KucoinWs::parse_my_trade(&data);
        assert!(event.is_some());
        let trade_event = event.unwrap();
        assert_eq!(trade_event.symbol, "BTC/USDT");
        assert!(!trade_event.trades.is_empty());
        assert_eq!(trade_event.trades[0].amount, Decimal::from_str("0.05").unwrap());
    }

    #[test]
    fn test_parse_balance_update() {
        let json = r#"{"currency":"USDT","total":"10000.00","available":"9000.00","hold":"1000.00","availableChange":"100.00","holdChange":"-100.00","relationEvent":"trade.setted","time":1609459200000}"#;
        let data: KucoinBalanceData = serde_json::from_str(json).unwrap();
        let event = KucoinWs::parse_balance_update(&data);
        assert!(event.is_some());
        let balance_event = event.unwrap();
        assert!(balance_event.balances.currencies.contains_key("USDT"));
        let usdt_balance = balance_event.balances.currencies.get("USDT").unwrap();
        assert_eq!(usdt_balance.free, Some(Decimal::from_str("9000.00").unwrap()));
        assert_eq!(usdt_balance.used, Some(Decimal::from_str("1000.00").unwrap()));
        assert_eq!(usdt_balance.total, Some(Decimal::from_str("10000.00").unwrap()));
    }

    #[test]
    fn test_process_message_order_update() {
        let json = r#"{"type":"message","topic":"/spotMarket/tradeOrders","data":{"symbol":"BTC-USDT","orderId":"order123","side":"sell","orderType":"market","type":"open","status":"open","price":"50000","size":"0.01","ts":1609459200000000000}}"#;
        let msg = KucoinWs::process_message(json);
        assert!(msg.is_some());
        if let Some(WsMessage::Order(event)) = msg {
            assert_eq!(event.order.symbol, "BTC/USDT");
            assert_eq!(event.order.side, OrderSide::Sell);
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_message_balance_update() {
        let json = r#"{"type":"message","topic":"/account/balance","data":{"currency":"BTC","total":"1.5","available":"1.0","hold":"0.5","time":1609459200000}}"#;
        let msg = KucoinWs::process_message(json);
        assert!(msg.is_some());
        if let Some(WsMessage::Balance(event)) = msg {
            assert!(event.balances.currencies.contains_key("BTC"));
        } else {
            panic!("Expected Balance message");
        }
    }
}
