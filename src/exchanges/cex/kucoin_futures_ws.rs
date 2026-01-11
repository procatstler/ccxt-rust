//! KuCoin Futures WebSocket Implementation
//!
//! KuCoin Futures 실시간 데이터 스트리밍
//! Note: KuCoin Futures requires a token from REST API before connecting to WebSocket

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
    Balance, Balances, Fee, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage,
    WsMyTradeEvent, WsOhlcvEvent, WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent,
    OHLCV,
};
use base64::{engine::general_purpose, Engine as _};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::str::FromStr;

const BASE_URL: &str = "https://api-futures.kucoin.com";

/// KuCoin Futures WebSocket 클라이언트
pub struct KucoinFuturesWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    connect_id: String,
    /// API credentials for private channels
    api_key: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
}

impl KucoinFuturesWs {
    /// 새 KuCoin Futures WebSocket 클라이언트 생성
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

    /// 심볼을 KuCoin Futures 형식으로 변환 (BTC/USDT:USDT -> XBTUSDTM)
    fn format_symbol(symbol: &str) -> String {
        // Remove settlement currency suffix
        let base_symbol = symbol.split(':').next().unwrap_or(symbol);
        // Replace / and add M suffix for perpetual
        let formatted = base_symbol.replace("/", "");
        if formatted.ends_with('M') {
            formatted
        } else {
            format!("{formatted}M")
        }
    }

    /// KuCoin Futures 심볼을 통합 심볼로 변환 (XBTUSDTM -> BTC/USDT:USDT)
    fn to_unified_symbol(kucoin_symbol: &str) -> String {
        // Remove M suffix
        let base = kucoin_symbol.trim_end_matches('M');
        // Find common quote currencies
        for quote in &["USDT", "USD", "USDC", "BTC", "ETH"] {
            if let Some(base_currency) = base.strip_suffix(quote) {
                // Handle XBT -> BTC conversion
                let base_currency = if base_currency == "XBT" {
                    "BTC"
                } else {
                    base_currency
                };
                return format!("{base_currency}/{quote}:{quote}");
            }
        }
        format!("{base}/USDT:USDT")
    }

    /// Timeframe을 KuCoin Futures 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1",
            Timeframe::Minute5 => "5",
            Timeframe::Minute15 => "15",
            Timeframe::Minute30 => "30",
            Timeframe::Hour1 => "60",
            Timeframe::Hour2 => "120",
            Timeframe::Hour4 => "240",
            Timeframe::Hour8 => "480",
            Timeframe::Hour12 => "720",
            Timeframe::Day1 => "1440",
            Timeframe::Week1 => "10080",
            _ => "1",
        }
    }

    /// Public 토큰 가져오기 (WebSocket 연결에 필요)
    async fn get_public_token() -> CcxtResult<KucoinFuturesWsToken> {
        let url = format!("{BASE_URL}/api/v1/bullet-public");
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?;

        let token_response: KucoinFuturesTokenResponse =
            response.json().await.map_err(|e| CcxtError::ParseError {
                data_type: "KucoinFuturesTokenResponse".to_string(),
                message: e.to_string(),
            })?;

        token_response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to get WebSocket token".into(),
        })
    }

    /// HMAC-SHA256 서명 생성
    fn sign(secret: &str, message: &str) -> String {
        type HmacSha256 = Hmac<Sha256>;
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(message.as_bytes());
        let result = mac.finalize();
        general_purpose::STANDARD.encode(result.into_bytes())
    }

    /// Passphrase 암호화
    fn encrypt_passphrase(secret: &str, passphrase: &str) -> String {
        Self::sign(secret, passphrase)
    }

    /// Private 토큰 가져오기 (인증된 WebSocket 연결에 필요)
    async fn get_private_token(&self) -> CcxtResult<KucoinFuturesWsToken> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required for private channels".into(),
            })?;
        let api_secret =
            self.api_secret
                .as_ref()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API secret required for private channels".into(),
                })?;
        let passphrase =
            self.passphrase
                .as_ref()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "Passphrase required for private channels".into(),
                })?;

        let url = format!("{BASE_URL}/api/v1/bullet-private");
        let timestamp = Utc::now().timestamp_millis().to_string();
        let str_to_sign = format!("{timestamp}POST/api/v1/bullet-private");
        let signature = Self::sign(api_secret, &str_to_sign);
        let encrypted_passphrase = Self::encrypt_passphrase(api_secret, passphrase);

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("KC-API-KEY", api_key)
            .header("KC-API-SIGN", signature)
            .header("KC-API-TIMESTAMP", &timestamp)
            .header("KC-API-PASSPHRASE", encrypted_passphrase)
            .header("KC-API-KEY-VERSION", "2")
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?;

        let token_response: KucoinFuturesTokenResponse =
            response.json().await.map_err(|e| CcxtError::ParseError {
                data_type: "KucoinFuturesTokenResponse".to_string(),
                message: e.to_string(),
            })?;

        token_response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to get private WebSocket token".into(),
        })
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &KucoinFuturesTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data
            .ts
            .map(|t| t / 1_000_000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

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
            bid: data.best_bid_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.best_bid_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.best_ask_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.best_ask_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: None,
            close: data.price.as_ref().and_then(|v| v.parse().ok()),
            last: data.price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.size,
            quote_volume: None,
            index_price: None,
            mark_price: data.mark_price.as_ref().and_then(|v| v.parse().ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(
        data: &KucoinFuturesOrderBookData,
        symbol: &str,
        is_snapshot: bool,
    ) -> WsOrderBookEvent {
        let mut bids: Vec<OrderBookEntry> = Vec::new();
        let mut asks: Vec<OrderBookEntry> = Vec::new();

        // Parse change data for incremental updates
        if let Some(change) = &data.change {
            // Format: "price,size,side" (buy or sell)
            let parts: Vec<&str> = change.split(',').collect();
            if parts.len() >= 3 {
                if let (Ok(price), Ok(size)) = (parts[0].parse(), parts[1].parse()) {
                    let entry = OrderBookEntry {
                        price,
                        amount: size,
                    };
                    if parts[2] == "buy" {
                        bids.push(entry);
                    } else {
                        asks.push(entry);
                    }
                }
            }
        }

        // Parse bids/asks arrays for snapshot
        if let Some(bid_data) = &data.bids {
            for b in bid_data {
                if b.len() >= 2 {
                    if let (Ok(price), Ok(amount)) = (b[0].parse(), b[1].parse()) {
                        bids.push(OrderBookEntry { price, amount });
                    }
                }
            }
        }

        if let Some(ask_data) = &data.asks {
            for a in ask_data {
                if a.len() >= 2 {
                    if let (Ok(price), Ok(amount)) = (a[0].parse(), a[1].parse()) {
                        asks.push(OrderBookEntry { price, amount });
                    }
                }
            }
        }

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
            nonce: data.sequence,
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

    /// 체결 메시지 파싱
    fn parse_trade(data: &KucoinFuturesTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        // KuCoin Futures time is in nanoseconds
        let timestamp = data
            .ts
            .map(|t| t / 1_000_000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data
            .size
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        let trades = vec![Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: data.side.clone(),
            taker_or_maker: Some(TakerOrMaker::Taker),
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
    fn parse_candle(data: &KucoinFuturesCandleData, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let candles = &data.candles;
        if candles.len() < 7 {
            return None;
        }

        // [timestamp, open, close, high, low, volume, turnover]
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

        if let Ok(response) = serde_json::from_str::<KucoinFuturesWsResponse>(msg) {
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
                    // Ticker V2
                    if topic.starts_with("/contractMarket/tickerV2:") {
                        if let Ok(ticker_data) =
                            serde_json::from_value::<KucoinFuturesTickerData>(data.clone())
                        {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }

                    // Ticker (legacy)
                    if topic.starts_with("/contractMarket/ticker:") {
                        if let Ok(ticker_data) =
                            serde_json::from_value::<KucoinFuturesTickerData>(data.clone())
                        {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }

                    // Level2 OrderBook
                    if topic.starts_with("/contractMarket/level2:") {
                        if let Ok(ob_data) =
                            serde_json::from_value::<KucoinFuturesOrderBookData>(data.clone())
                        {
                            let symbol_part = topic.trim_start_matches("/contractMarket/level2:");
                            let symbol = Self::to_unified_symbol(symbol_part);
                            return Some(WsMessage::OrderBook(Self::parse_order_book(
                                &ob_data, &symbol, false,
                            )));
                        }
                    }

                    // Level2 Depth (snapshot)
                    if topic.starts_with("/contractMarket/level2Depth") {
                        if let Ok(ob_data) =
                            serde_json::from_value::<KucoinFuturesOrderBookData>(data.clone())
                        {
                            let symbol_part = topic.split(':').nth(1).unwrap_or("");
                            let symbol = Self::to_unified_symbol(symbol_part);
                            return Some(WsMessage::OrderBook(Self::parse_order_book(
                                &ob_data, &symbol, true,
                            )));
                        }
                    }

                    // Execution (trades)
                    if topic.starts_with("/contractMarket/execution:") {
                        if let Ok(trade_data) =
                            serde_json::from_value::<KucoinFuturesTradeData>(data.clone())
                        {
                            return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                        }
                    }

                    // Candles
                    if topic.starts_with("/contractMarket/candle:") {
                        if let Ok(candle_data) =
                            serde_json::from_value::<KucoinFuturesCandleData>(data.clone())
                        {
                            // Extract timeframe from topic (e.g., "/contractMarket/candle:XBTUSDTM_1")
                            let parts: Vec<&str> = topic.split('_').collect();
                            let timeframe = if parts.len() >= 2 {
                                match parts[1] {
                                    "1" => Timeframe::Minute1,
                                    "5" => Timeframe::Minute5,
                                    "15" => Timeframe::Minute15,
                                    "30" => Timeframe::Minute30,
                                    "60" => Timeframe::Hour1,
                                    "120" => Timeframe::Hour2,
                                    "240" => Timeframe::Hour4,
                                    "480" => Timeframe::Hour8,
                                    "720" => Timeframe::Hour12,
                                    "1440" => Timeframe::Day1,
                                    "10080" => Timeframe::Week1,
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

                    // Order updates (/contractMarket/tradeOrders)
                    if topic.starts_with("/contractMarket/tradeOrders") {
                        if let Ok(order_data) =
                            serde_json::from_value::<KucoinFuturesOrderData>(data.clone())
                        {
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

                    // Balance updates (/contractAccount/wallet)
                    if topic.starts_with("/contractAccount/wallet") {
                        if let Ok(balance_data) =
                            serde_json::from_value::<KucoinFuturesBalanceData>(data.clone())
                        {
                            if let Some(balance_event) = Self::parse_balance_update(&balance_data) {
                                return Some(WsMessage::Balance(balance_event));
                            }
                        }
                    }

                    // Position updates (/contract/position:XBTUSDTM)
                    if topic.starts_with("/contract/position:") {
                        // Position updates can be logged or handled separately
                        // For now, we don't have a WsPositionEvent type
                        return None;
                    }
                }
            }
        }

        None
    }

    /// 주문 업데이트 파싱
    fn parse_order_update(data: &KucoinFuturesOrderData) -> Option<WsOrderEvent> {
        let symbol = Self::to_unified_symbol(&data.symbol);
        // KuCoin Futures timestamp is in nanoseconds, convert to milliseconds
        let timestamp_ms = data
            .ts
            .map(|t| t / 1_000_000)
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
            "stop_market" => OrderType::StopMarket,
            _ => OrderType::Limit,
        };

        let status = match data.status.as_deref() {
            Some("open") => OrderStatus::Open,
            Some("match") => OrderStatus::Open, // Partial fill
            Some("done") => {
                // Check if canceled or filled
                if data
                    .remain_size
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or_default()
                    > Decimal::ZERO
                {
                    OrderStatus::Canceled
                } else {
                    OrderStatus::Closed
                }
            },
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
        let filled = data
            .filled_size
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let remaining = data
            .remain_size
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let average = if let (Some(_amt), Some(fill)) = (amount, filled) {
            if fill > Decimal::ZERO {
                data.fill_price
                    .as_ref()
                    .and_then(|p| Decimal::from_str(p).ok())
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

        let fee = data
            .fee
            .as_ref()
            .and_then(|f| Decimal::from_str(f).ok())
            .map(|fee_amount| Fee {
                currency: data.fee_currency.clone(),
                cost: Some(fee_amount),
                rate: None,
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
            stop_price: data
                .stop_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            take_profit_price: None,
            stop_loss_price: None,
            average,
            amount: amount.unwrap_or_default(),
            filled: filled.unwrap_or_default(),
            remaining,
            cost,
            trades: Vec::new(),
            reduce_only: data.reduce_only,
            post_only: Some(is_post_only),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsOrderEvent { order })
    }

    /// 내 체결 파싱
    fn parse_my_trade(data: &KucoinFuturesOrderData) -> Option<WsMyTradeEvent> {
        // Only process if there's a match
        if data.match_size.is_none() || data.match_price.is_none() {
            return None;
        }

        let symbol = Self::to_unified_symbol(&data.symbol);
        // KuCoin Futures timestamp is in nanoseconds, convert to milliseconds
        let timestamp_ms = data
            .ts
            .map(|t| t / 1_000_000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let match_size = data
            .match_size
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())?;
        let match_price = data
            .match_price
            .as_ref()
            .and_then(|p| Decimal::from_str(p).ok())?;

        if match_size == Decimal::ZERO {
            return None;
        }

        let side = Some(data.side.to_lowercase());
        let taker_or_maker = if data.liquidity.as_deref() == Some("taker") {
            Some(TakerOrMaker::Taker)
        } else {
            Some(TakerOrMaker::Maker)
        };

        let fee = data
            .fee
            .as_ref()
            .and_then(|f| Decimal::from_str(f).ok())
            .map(|fee_amount| Fee {
                currency: data.fee_currency.clone(),
                cost: Some(fee_amount),
                rate: None,
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
    fn parse_balance_update(data: &KucoinFuturesBalanceData) -> Option<WsBalanceEvent> {
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());
        let mut currencies: HashMap<String, Balance> = HashMap::new();

        let total = data
            .account_equity
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let available = data
            .available_balance
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let used = match (total, available) {
            (Some(t), Some(a)) => Some(t - a),
            _ => None,
        };

        let balance = Balance {
            free: available,
            used,
            total,
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
    async fn subscribe_stream(
        &mut self,
        topics: Vec<String>,
        channel: &str,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // Get WebSocket token
        let token_data = Self::get_public_token().await?;
        let server =
            token_data
                .instance_servers
                .first()
                .ok_or_else(|| CcxtError::ExchangeError {
                    message: "No WebSocket server available".into(),
                })?;

        let ws_url = format!(
            "{}?token={}&connectId={}",
            server.endpoint, token_data.token, self.connect_id
        );

        let ping_interval = (server.ping_interval.unwrap_or(18000) / 1000) as u64;

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: ping_interval,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        for topic in &topics {
            let id = uuid::Uuid::new_v4().to_string();
            let subscribe_msg = serde_json::json!({
                "id": id,
                "type": "subscribe",
                "topic": topic,
                "privateChannel": false,
                "response": true
            });
            ws_client.send(&subscribe_msg.to_string())?;
        }

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions
                .write()
                .await
                .insert(key, channel.to_string());
        }

        // 이벤트 처리 태스크
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

    /// Private 채널 구독 (인증 필요)
    async fn subscribe_private_stream(
        &mut self,
        topics: Vec<String>,
        channel: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // Get private WebSocket token (requires authentication)
        let token_data = self.get_private_token().await?;
        let server =
            token_data
                .instance_servers
                .first()
                .ok_or_else(|| CcxtError::ExchangeError {
                    message: "No WebSocket server available".into(),
                })?;

        let ws_url = format!(
            "{}?token={}&connectId={}",
            server.endpoint, token_data.token, self.connect_id
        );

        let ping_interval = (server.ping_interval.unwrap_or(18000) / 1000) as u64;

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: ping_interval,
            connect_timeout_secs: 30,
            ..Default::default()
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
            let key = format!("private:{channel}");
            self.subscriptions
                .write()
                .await
                .insert(key, channel.to_string());
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

impl Default for KucoinFuturesWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for KucoinFuturesWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let topics = vec![format!("/contractMarket/tickerV2:{}", market_id)];
        client
            .subscribe_stream(topics, "ticker", Some(symbol))
            .await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let topics: Vec<String> = symbols
            .iter()
            .map(|s| format!("/contractMarket/tickerV2:{}", Self::format_symbol(s)))
            .collect();
        client.subscribe_stream(topics, "tickers", None).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        // Use level2Depth5 or level2Depth50 for snapshots
        let topic = match limit {
            Some(l) if l <= 5 => format!("/contractMarket/level2Depth5:{market_id}"),
            Some(l) if l <= 50 => format!("/contractMarket/level2Depth50:{market_id}"),
            _ => format!("/contractMarket/level2:{market_id}"),
        };
        let topics = vec![topic];
        client
            .subscribe_stream(topics, "orderBook", Some(symbol))
            .await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let topics = vec![format!("/contractMarket/execution:{}", market_id)];
        client
            .subscribe_stream(topics, "trades", Some(symbol))
            .await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let topics = vec![format!("/contractMarket/candle:{}_{}", market_id, interval)];
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

    async fn watch_orders(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let (Some(key), Some(secret), Some(pass)) = (
            self.api_key.as_ref(),
            self.api_secret.as_ref(),
            self.passphrase.as_ref(),
        ) {
            Self::with_credentials(key, secret, pass)
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for watch_orders".into(),
            });
        };

        let topic = if let Some(sym) = symbol {
            format!("/contractMarket/tradeOrders:{}", Self::format_symbol(sym))
        } else {
            "/contractMarket/tradeOrders".to_string()
        };

        client.subscribe_private_stream(vec![topic], "orders").await
    }

    async fn watch_my_trades(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let (Some(key), Some(secret), Some(pass)) = (
            self.api_key.as_ref(),
            self.api_secret.as_ref(),
            self.passphrase.as_ref(),
        ) {
            Self::with_credentials(key, secret, pass)
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for watch_my_trades".into(),
            });
        };

        // Use the same topic as orders - trades come from order updates
        let topic = if let Some(sym) = symbol {
            format!("/contractMarket/tradeOrders:{}", Self::format_symbol(sym))
        } else {
            "/contractMarket/tradeOrders".to_string()
        };

        client
            .subscribe_private_stream(vec![topic], "myTrades")
            .await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let (Some(key), Some(secret), Some(pass)) = (
            self.api_key.as_ref(),
            self.api_secret.as_ref(),
            self.passphrase.as_ref(),
        ) {
            Self::with_credentials(key, secret, pass)
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for watch_balance".into(),
            });
        };

        let topic = "/contractAccount/wallet".to_string();
        client
            .subscribe_private_stream(vec![topic], "balance")
            .await
    }
}

// === KuCoin Futures WebSocket Types ===

#[derive(Debug, Deserialize)]
struct KucoinFuturesTokenResponse {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    data: Option<KucoinFuturesWsToken>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesWsToken {
    token: String,
    instance_servers: Vec<KucoinFuturesWsServer>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesWsServer {
    endpoint: String,
    #[serde(default)]
    ping_interval: Option<i64>,
    #[serde(default)]
    ping_timeout: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesWsResponse {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesTickerData {
    symbol: String,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<Decimal>,
    #[serde(default)]
    best_bid_price: Option<String>,
    #[serde(default)]
    best_bid_size: Option<String>,
    #[serde(default)]
    best_ask_price: Option<String>,
    #[serde(default)]
    best_ask_size: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    index_price: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesOrderBookData {
    #[serde(default)]
    asks: Option<Vec<Vec<String>>>,
    #[serde(default)]
    bids: Option<Vec<Vec<String>>>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    sequence: Option<i64>,
    #[serde(default)]
    change: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesTradeData {
    symbol: String,
    #[serde(default)]
    trade_id: Option<String>,
    price: String,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    maker_order_id: Option<String>,
    #[serde(default)]
    taker_order_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesCandleData {
    symbol: String,
    candles: Vec<String>,
    #[serde(default)]
    time: Option<i64>,
}

// === Private Channel Data Structures ===

/// 주문 업데이트 데이터
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesOrderData {
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
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    reduce_only: Option<bool>,
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
struct KucoinFuturesBalanceData {
    currency: String,
    #[serde(default)]
    account_equity: Option<String>,
    #[serde(default)]
    available_balance: Option<String>,
    #[serde(default)]
    frozen_funds: Option<String>,
    #[serde(default)]
    order_margin: Option<String>,
    #[serde(default)]
    position_margin: Option<String>,
    #[serde(default)]
    unrealised_pnl: Option<String>,
    #[serde(default)]
    time: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(KucoinFuturesWs::format_symbol("BTC/USDT:USDT"), "BTCUSDTM");
        assert_eq!(KucoinFuturesWs::format_symbol("ETH/USDT"), "ETHUSDTM");
        assert_eq!(KucoinFuturesWs::format_symbol("XBTUSDTM"), "XBTUSDTM");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(
            KucoinFuturesWs::to_unified_symbol("XBTUSDTM"),
            "BTC/USDT:USDT"
        );
        assert_eq!(
            KucoinFuturesWs::to_unified_symbol("ETHUSDTM"),
            "ETH/USDT:USDT"
        );
        assert_eq!(KucoinFuturesWs::to_unified_symbol("BTCUSDM"), "BTC/USD:USD");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(KucoinFuturesWs::format_interval(Timeframe::Minute1), "1");
        assert_eq!(KucoinFuturesWs::format_interval(Timeframe::Hour1), "60");
        assert_eq!(KucoinFuturesWs::format_interval(Timeframe::Day1), "1440");
    }

    #[test]
    fn test_with_credentials() {
        let client = KucoinFuturesWs::with_credentials("test_key", "test_secret", "test_pass");
        assert_eq!(client.get_api_key(), Some("test_key"));
        assert!(client.api_secret.is_some());
        assert!(client.passphrase.is_some());
    }

    #[test]
    fn test_set_credentials() {
        let mut client = KucoinFuturesWs::new();
        assert!(client.get_api_key().is_none());
        client.set_credentials("api_key", "api_secret", "passphrase");
        assert_eq!(client.get_api_key(), Some("api_key"));
    }

    #[test]
    fn test_sign() {
        let signature = KucoinFuturesWs::sign("secret", "message");
        assert!(!signature.is_empty());
        // Verify it's base64 encoded
        assert!(general_purpose::STANDARD.decode(&signature).is_ok());
    }

    #[test]
    fn test_parse_order_update() {
        let json = r#"{"symbol":"XBTUSDTM","orderId":"5c52423054d74a0001a8d123","side":"buy","orderType":"limit","type":"open","price":"50000","size":"1","filledSize":"0","remainSize":"1","clientOid":"test123","ts":1609459200000000000,"status":"open","timeInForce":"GTC"}"#;
        let data: KucoinFuturesOrderData = serde_json::from_str(json).unwrap();
        let event = KucoinFuturesWs::parse_order_update(&data);
        assert!(event.is_some());
        let order_event = event.unwrap();
        assert_eq!(order_event.order.symbol, "BTC/USDT:USDT");
        assert_eq!(order_event.order.side, OrderSide::Buy);
        assert_eq!(order_event.order.order_type, OrderType::Limit);
        assert_eq!(order_event.order.status, OrderStatus::Open);
    }

    #[test]
    fn test_parse_my_trade() {
        let json = r#"{"symbol":"XBTUSDTM","orderId":"5c52423054d74a0001a8d123","side":"buy","orderType":"limit","type":"match","price":"50000","size":"1","filledSize":"0.5","remainSize":"0.5","matchPrice":"50000","matchSize":"0.5","tradeId":"trade123","liquidity":"taker","fee":"0.0001","feeCurrency":"USDT","ts":1609459200000000000}"#;
        let data: KucoinFuturesOrderData = serde_json::from_str(json).unwrap();
        let event = KucoinFuturesWs::parse_my_trade(&data);
        assert!(event.is_some());
        let trade_event = event.unwrap();
        assert_eq!(trade_event.symbol, "BTC/USDT:USDT");
        assert!(!trade_event.trades.is_empty());
        assert_eq!(
            trade_event.trades[0].amount,
            Decimal::from_str("0.5").unwrap()
        );
    }

    #[test]
    fn test_parse_balance_update() {
        let json = r#"{"currency":"USDT","accountEquity":"10000.00","availableBalance":"9000.00","frozenFunds":"1000.00","time":1609459200000}"#;
        let data: KucoinFuturesBalanceData = serde_json::from_str(json).unwrap();
        let event = KucoinFuturesWs::parse_balance_update(&data);
        assert!(event.is_some());
        let balance_event = event.unwrap();
        assert!(balance_event.balances.currencies.contains_key("USDT"));
        let usdt_balance = balance_event.balances.currencies.get("USDT").unwrap();
        assert_eq!(
            usdt_balance.free,
            Some(Decimal::from_str("9000.00").unwrap())
        );
        assert_eq!(
            usdt_balance.total,
            Some(Decimal::from_str("10000.00").unwrap())
        );
    }

    #[test]
    fn test_process_message_ticker() {
        let json = r#"{"type":"message","topic":"/contractMarket/tickerV2:XBTUSDTM","data":{"symbol":"XBTUSDTM","price":"50000","bestBidPrice":"49999","bestAskPrice":"50001","ts":1609459200000000000}}"#;
        let msg = KucoinFuturesWs::process_message(json);
        assert!(msg.is_some());
        if let Some(WsMessage::Ticker(event)) = msg {
            assert_eq!(event.symbol, "BTC/USDT:USDT");
        } else {
            panic!("Expected Ticker message");
        }
    }

    #[test]
    fn test_process_message_order_update() {
        let json = r#"{"type":"message","topic":"/contractMarket/tradeOrders","data":{"symbol":"XBTUSDTM","orderId":"order123","side":"sell","orderType":"market","type":"open","status":"open","price":"50000","size":"1","ts":1609459200000000000}}"#;
        let msg = KucoinFuturesWs::process_message(json);
        assert!(msg.is_some());
        if let Some(WsMessage::Order(event)) = msg {
            assert_eq!(event.order.symbol, "BTC/USDT:USDT");
            assert_eq!(event.order.side, OrderSide::Sell);
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_message_balance_update() {
        let json = r#"{"type":"message","topic":"/contractAccount/wallet","data":{"currency":"USDT","accountEquity":"10000","availableBalance":"9000","time":1609459200000}}"#;
        let msg = KucoinFuturesWs::process_message(json);
        assert!(msg.is_some());
        if let Some(WsMessage::Balance(event)) = msg {
            assert!(event.balances.currencies.contains_key("USDT"));
        } else {
            panic!("Expected Balance message");
        }
    }
}
