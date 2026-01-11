//! Binance WebSocket Implementation
//!
//! Binance 실시간 데이터 스트리밍 (Public + Private)

#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::Hmac;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Ticker, Timeframe, Trade, OHLCV, WsBalanceEvent, WsExchange, WsMessage, WsOhlcvEvent,
    WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_BASE_URL: &str = "wss://stream.binance.com:9443/ws";
const WS_USER_DATA_URL: &str = "wss://stream.binance.com:9443/ws";
const REST_BASE_URL: &str = "https://api.binance.com";

/// Binance WebSocket 클라이언트
pub struct BinanceWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    private_ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    listen_key: Arc<RwLock<Option<String>>>,
    listen_key_timestamp: Arc<RwLock<i64>>,
}

impl BinanceWs {
    /// 새 Binance WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            config: None,
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            listen_key: Arc::new(RwLock::new(None)),
            listen_key_timestamp: Arc::new(RwLock::new(0)),
        }
    }

    /// API 키와 시크릿으로 Binance WebSocket 클라이언트 생성
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            listen_key: Arc::new(RwLock::new(None)),
            listen_key_timestamp: Arc::new(RwLock::new(0)),
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

    /// listenKey 생성 또는 갱신
    async fn create_listen_key(&self) -> CcxtResult<String> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required for private WebSocket".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let url = format!("{REST_BASE_URL}/api/v3/userDataStream");
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(CcxtError::ExchangeError {
                message: format!("Failed to create listen key: {error_text}"),
            });
        }

        let data: ListenKeyResponse = response.json().await.map_err(|e| CcxtError::ParseError {
            data_type: "ListenKeyResponse".to_string(),
            message: e.to_string(),
        })?;

        // listenKey 저장
        *self.listen_key.write().await = Some(data.listen_key.clone());
        *self.listen_key_timestamp.write().await = Utc::now().timestamp_millis();

        Ok(data.listen_key)
    }

    /// listenKey 갱신
    #[allow(dead_code)]
    async fn keep_alive_listen_key(&self) -> CcxtResult<()> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let listen_key = self.listen_key.read().await.clone().ok_or_else(|| {
            CcxtError::AuthenticationError {
                message: "No listen key available".into(),
            }
        })?;

        let url = format!("{REST_BASE_URL}/api/v3/userDataStream");
        let client = reqwest::Client::new();
        let response = client
            .put(&url)
            .header("X-MBX-APIKEY", api_key)
            .query(&[("listenKey", &listen_key)])
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?;

        if !response.status().is_success() {
            // listenKey가 만료된 경우 새로 생성
            *self.listen_key.write().await = None;
            return Err(CcxtError::AuthenticationError {
                message: "Listen key expired, needs refresh".into(),
            });
        }

        *self.listen_key_timestamp.write().await = Utc::now().timestamp_millis();
        Ok(())
    }

    /// listenKey 갱신이 필요한지 확인 (30분마다 갱신)
    async fn needs_listen_key_refresh(&self) -> bool {
        let timestamp = *self.listen_key_timestamp.read().await;
        let now = Utc::now().timestamp_millis();
        // 30분 (1800000ms) 이상 지났으면 갱신 필요
        (now - timestamp) > 1800000
    }

    /// Private 스트림에 연결하고 이벤트 수신
    async fn subscribe_private_stream(&mut self, channel: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // listenKey 확인 또는 생성
        let listen_key = {
            let current_key = self.listen_key.read().await.clone();
            if current_key.is_none() || self.needs_listen_key_refresh().await {
                self.create_listen_key().await?
            } else {
                current_key.unwrap()
            }
        };

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // User Data Stream URL 구성
        let url = format!("{WS_USER_DATA_URL}/{listen_key}");

        // WebSocket 클라이언트 생성 및 연결
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

        // 구독 저장
        {
            let key = format!("private:{channel}");
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // listenKey 갱신 스케줄링
        let listen_key_clone = Arc::clone(&self.listen_key);
        let listen_key_timestamp_clone = Arc::clone(&self.listen_key_timestamp);
        let config_clone = self.config.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(1200)).await; // 20분마다

                if let Some(ref config) = config_clone {
                    if let Some(api_key) = config.api_key() {
                        if let Some(ref key) = *listen_key_clone.read().await {
                            let client = reqwest::Client::new();
                            let _ = client
                                .put(format!("{REST_BASE_URL}/api/v3/userDataStream"))
                                .header("X-MBX-APIKEY", api_key)
                                .query(&[("listenKey", key)])
                                .send()
                                .await;
                            *listen_key_timestamp_clone.write().await = Utc::now().timestamp_millis();
                        }
                    }
                }
            }
        });

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                        let _ = tx.send(WsMessage::Authenticated);
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

    /// Private 메시지 처리 (User Data Stream)
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;
        let event_type = json.get("e").and_then(|v| v.as_str())?;

        match event_type {
            // 주문 업데이트
            "executionReport" => {
                if let Ok(data) = serde_json::from_str::<BinanceExecutionReport>(msg) {
                    return Some(WsMessage::Order(Self::parse_execution_report(&data)));
                }
            }
            // 잔고 업데이트 (outboundAccountPosition)
            "outboundAccountPosition" => {
                if let Ok(data) = serde_json::from_str::<BinanceAccountPosition>(msg) {
                    return Some(WsMessage::Balance(Self::parse_account_position(&data)));
                }
            }
            // 잔고 업데이트 (balanceUpdate) - 입출금 등
            "balanceUpdate" => {
                if let Ok(data) = serde_json::from_str::<BinanceBalanceUpdate>(msg) {
                    return Some(WsMessage::Balance(Self::parse_balance_update(&data)));
                }
            }
            _ => {}
        }

        None
    }

    /// executionReport 파싱 (주문 이벤트)
    fn parse_execution_report(data: &BinanceExecutionReport) -> WsOrderEvent {
        let symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.E.unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.X.as_str() {
            "NEW" => OrderStatus::Open,
            "PARTIALLY_FILLED" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" => OrderStatus::Canceled,
            "EXPIRED" => OrderStatus::Expired,
            "REJECTED" => OrderStatus::Rejected,
            _ => OrderStatus::Open, // Default to Open for unknown statuses
        };

        let side = match data.S.as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.o.as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            "STOP_LOSS" => OrderType::StopMarket,
            "STOP_LOSS_LIMIT" => OrderType::StopLimit,
            "TAKE_PROFIT" => OrderType::TakeProfitMarket,
            "TAKE_PROFIT_LIMIT" => OrderType::TakeProfitLimit,
            _ => OrderType::Limit,
        };

        let price: Decimal = data.p.parse().unwrap_or_default();
        let amount: Decimal = data.q.parse().unwrap_or_default();
        let filled: Decimal = data.z.parse().unwrap_or_default();
        let remaining = amount - filled;
        let cost: Decimal = data.Z.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();

        let order = Order {
            id: data.i.to_string(),
            client_order_id: Some(data.c.clone()),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: data.T,
            last_update_timestamp: None,
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force: None,
            side,
            price: Some(price),
            average: if filled > Decimal::ZERO {
                Some(cost / filled)
            } else {
                None
            },
            amount,
            filled,
            remaining: Some(remaining),
            cost: Some(cost),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
            stop_price: data.P.as_ref().and_then(|v| v.parse().ok()),
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        };

        WsOrderEvent { order }
    }

    /// outboundAccountPosition 파싱 (잔고 이벤트)
    fn parse_account_position(data: &BinanceAccountPosition) -> WsBalanceEvent {
        let mut balances = Balances::default();
        balances.timestamp = data.E;
        balances.datetime = data.E.and_then(|t| {
            chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
        });

        for b in &data.B {
            let currency = b.a.clone();
            let free: Decimal = b.f.parse().unwrap_or_default();
            let used: Decimal = b.l.parse().unwrap_or_default();
            let total = free + used;

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

        balances.info = serde_json::to_value(data).unwrap_or_default();
        WsBalanceEvent { balances }
    }

    /// balanceUpdate 파싱 (잔고 변경 이벤트)
    fn parse_balance_update(data: &BinanceBalanceUpdate) -> WsBalanceEvent {
        let mut balances = Balances::default();
        balances.timestamp = data.E;
        balances.datetime = data.E.and_then(|t| {
            chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
        });

        let currency = data.a.clone();
        let delta: Decimal = data.d.parse().unwrap_or_default();

        balances.currencies.insert(
            currency,
            Balance {
                free: Some(delta), // delta 값만 전달 (누적 계산은 클라이언트에서)
                used: None,
                total: None,
                debt: None,
            },
        );

        balances.info = serde_json::to_value(data).unwrap_or_default();
        WsBalanceEvent { balances }
    }

    /// 심볼을 Binance 형식으로 변환 (BTC/USDT -> btcusdt)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "").to_lowercase()
    }

    /// Timeframe을 Binance kline interval로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Second1 => "1s",
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour3 => "3h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour8 => "8h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Day3 => "3d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1M",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BinanceMiniTicker) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.E.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.h.as_ref().and_then(|v| v.parse().ok()),
            low: data.l.as_ref().and_then(|v| v.parse().ok()),
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: data.o.as_ref().and_then(|v| v.parse().ok()),
            close: data.c.as_ref().and_then(|v| v.parse().ok()),
            last: data.c.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.v.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.q.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BinanceDepthUpdate, symbol: &str) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.b.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: b[0].parse().ok()?,
                    amount: b[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.a.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: a[0].parse().ok()?,
                    amount: a[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let timestamp = data.E.unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.u,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot: false,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &BinanceTradeMsg) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.T.unwrap_or_else(|| Utc::now().timestamp_millis());

        let trade = Trade {
            id: data.t.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: if data.m { Some("sell".into()) } else { Some("buy".into()) },
            taker_or_maker: None,
            price: data.p.parse().unwrap_or_default(),
            amount: data.q.parse().unwrap_or_default(),
            cost: Some(
                data.p.parse::<Decimal>().unwrap_or_default()
                    * data.q.parse::<Decimal>().unwrap_or_default(),
            ),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    /// Kline 메시지 파싱
    fn parse_kline(data: &BinanceKlineMsg, timeframe: Timeframe) -> WsOhlcvEvent {
        let symbol = Self::to_unified_symbol(&data.s);
        let k = &data.k;

        let ohlcv = OHLCV {
            timestamp: k.t,
            open: k.o.parse().unwrap_or_default(),
            high: k.h.parse().unwrap_or_default(),
            low: k.l.parse().unwrap_or_default(),
            close: k.c.parse().unwrap_or_default(),
            volume: k.v.parse().unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        }
    }

    /// Binance 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(binance_symbol: &str) -> String {
        // 일반적인 quote 통화 목록
        let quotes = ["USDT", "BUSD", "USDC", "BTC", "ETH", "BNB", "TUSD", "PAX", "DAI"];

        for quote in quotes {
            if let Some(base) = binance_symbol.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }

        // 변환할 수 없으면 원본 반환
        binance_symbol.to_string()
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // 먼저 에러 체크
        if let Ok(err) = serde_json::from_str::<BinanceError>(msg) {
            if err.code.is_some() {
                return Some(WsMessage::Error(err.msg.unwrap_or_default()));
            }
        }

        // ticker (miniTicker)
        if msg.contains("\"e\":\"24hrMiniTicker\"") {
            if let Ok(data) = serde_json::from_str::<BinanceMiniTicker>(msg) {
                return Some(WsMessage::Ticker(Self::parse_ticker(&data)));
            }
        }

        // depth update
        if msg.contains("\"e\":\"depthUpdate\"") {
            if let Ok(data) = serde_json::from_str::<BinanceDepthUpdate>(msg) {
                let symbol = Self::to_unified_symbol(&data.s);
                return Some(WsMessage::OrderBook(Self::parse_order_book(&data, &symbol)));
            }
        }

        // trade
        if msg.contains("\"e\":\"trade\"") {
            if let Ok(data) = serde_json::from_str::<BinanceTradeMsg>(msg) {
                return Some(WsMessage::Trade(Self::parse_trade(&data)));
            }
        }

        // kline
        if msg.contains("\"e\":\"kline\"") {
            if let Ok(data) = serde_json::from_str::<BinanceKlineMsg>(msg) {
                // interval에서 timeframe 추론
                let timeframe = match data.k.i.as_str() {
                    "1s" => Timeframe::Second1,
                    "1m" => Timeframe::Minute1,
                    "3m" => Timeframe::Minute3,
                    "5m" => Timeframe::Minute5,
                    "15m" => Timeframe::Minute15,
                    "30m" => Timeframe::Minute30,
                    "1h" => Timeframe::Hour1,
                    "2h" => Timeframe::Hour2,
                    "4h" => Timeframe::Hour4,
                    "6h" => Timeframe::Hour6,
                    "8h" => Timeframe::Hour8,
                    "12h" => Timeframe::Hour12,
                    "1d" => Timeframe::Day1,
                    "3d" => Timeframe::Day3,
                    "1w" => Timeframe::Week1,
                    "1M" => Timeframe::Month1,
                    _ => Timeframe::Minute1,
                };
                return Some(WsMessage::Ohlcv(Self::parse_kline(&data, timeframe)));
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, stream: &str, channel: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket URL 구성
        let url = format!("{WS_BASE_URL}/{stream}");

        // WebSocket 클라이언트 생성 및 연결
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

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, stream.to_string());
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

impl Default for BinanceWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for BinanceWs {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            listen_key: Arc::clone(&self.listen_key),
            listen_key_timestamp: Arc::clone(&self.listen_key_timestamp),
        }
    }
}

#[async_trait]
impl WsExchange for BinanceWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let stream = format!("{}@miniTicker", Self::format_symbol(symbol));
        client.subscribe_stream(&stream, "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let streams: Vec<String> = symbols
            .iter()
            .map(|s| format!("{}@miniTicker", Self::format_symbol(s)))
            .collect();
        let combined = streams.join("/");
        let url = format!("wss://stream.binance.com:9443/stream?streams={combined}");

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let mut ws_client = WsClient::new(WsConfig {
            url: url.clone(),
            auto_reconnect: true,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;
        client.ws_client = Some(ws_client);

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
                        // Combined stream format: {"stream":"btcusdt@miniTicker","data":{...}}
                        if let Ok(combined) = serde_json::from_str::<BinanceCombinedStream>(&msg) {
                            if let Some(ws_msg) = Self::process_message(&serde_json::to_string(&combined.data).unwrap_or_default()) {
                                let _ = tx.send(ws_msg);
                            }
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

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let depth = limit.unwrap_or(10).min(20);
        let stream = format!("{}@depth{}@100ms", Self::format_symbol(symbol), depth);
        client.subscribe_stream(&stream, "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let stream = format!("{}@trade", Self::format_symbol(symbol));
        client.subscribe_stream(&stream, "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let interval = Self::format_interval(timeframe);
        let stream = format!("{}@kline_{}", Self::format_symbol(symbol), interval);
        client.subscribe_stream(&stream, "ohlcv", Some(symbol)).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Binance는 구독시 자동 연결
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

    // === Private Streams ===

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("balance").await
    }

    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("orders").await
    }

    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("myTrades").await
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        // listenKey 생성
        self.create_listen_key().await?;
        Ok(())
    }
}

// === Binance WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct BinanceError {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    msg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BinanceCombinedStream {
    stream: String,
    data: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceMiniTicker {
    e: String, // Event type
    #[serde(default)]
    E: Option<i64>, // Event time
    s: String, // Symbol
    #[serde(default)]
    c: Option<String>, // Close price
    #[serde(default)]
    o: Option<String>, // Open price
    #[serde(default)]
    h: Option<String>, // High price
    #[serde(default)]
    l: Option<String>, // Low price
    #[serde(default)]
    v: Option<String>, // Base volume
    #[serde(default)]
    q: Option<String>, // Quote volume
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceDepthUpdate {
    e: String, // Event type
    #[serde(default)]
    E: Option<i64>, // Event time
    s: String, // Symbol
    #[serde(default)]
    U: Option<i64>, // First update ID
    #[serde(default)]
    u: Option<i64>, // Final update ID
    #[serde(default)]
    b: Vec<Vec<String>>, // Bids
    #[serde(default)]
    a: Vec<Vec<String>>, // Asks
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceTradeMsg {
    e: String, // Event type
    #[serde(default)]
    E: Option<i64>, // Event time
    s: String, // Symbol
    t: i64, // Trade ID
    p: String, // Price
    q: String, // Quantity
    #[serde(default)]
    T: Option<i64>, // Trade time
    m: bool, // Is buyer maker
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceKlineMsg {
    e: String, // Event type
    #[serde(default)]
    E: Option<i64>, // Event time
    s: String, // Symbol
    k: BinanceKline,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceKline {
    t: i64, // Kline start time
    #[serde(default)]
    T: Option<i64>, // Kline close time
    s: String, // Symbol
    i: String, // Interval
    o: String, // Open
    c: String, // Close
    h: String, // High
    l: String, // Low
    v: String, // Volume
    #[serde(default)]
    x: Option<bool>, // Is kline closed
}

// === Private WebSocket Message Types ===

/// Listen Key 응답
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListenKeyResponse {
    listen_key: String,
}

/// executionReport - 주문 실행 보고서
#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceExecutionReport {
    e: String,          // Event type
    #[serde(default)]
    E: Option<i64>,     // Event time
    s: String,          // Symbol
    c: String,          // Client order ID
    S: String,          // Side (BUY/SELL)
    o: String,          // Order type
    #[serde(default)]
    f: Option<String>,  // Time in force
    q: String,          // Order quantity
    p: String,          // Order price
    #[serde(default)]
    P: Option<String>,  // Stop price
    #[serde(default)]
    F: Option<String>,  // Iceberg quantity
    #[serde(default)]
    g: Option<i64>,     // OrderListId
    #[serde(default)]
    C: Option<String>,  // Original client order ID (for cancel)
    x: String,          // Current execution type
    X: String,          // Current order status
    #[serde(default)]
    r: Option<String>,  // Order reject reason
    i: i64,             // Order ID
    l: String,          // Last executed quantity
    z: String,          // Cumulative filled quantity
    L: String,          // Last executed price
    #[serde(default)]
    n: Option<String>,  // Commission amount
    #[serde(default)]
    N: Option<String>,  // Commission asset
    #[serde(default)]
    T: Option<i64>,     // Transaction time
    #[serde(default)]
    t: Option<i64>,     // Trade ID
    #[serde(default)]
    I: Option<i64>,     // Ignore
    w: bool,            // Is working
    m: bool,            // Is maker
    #[serde(default)]
    M: Option<bool>,    // Ignore
    #[serde(default)]
    O: Option<i64>,     // Order creation time
    #[serde(default)]
    Z: Option<String>,  // Cumulative quote asset transacted quantity
    #[serde(default)]
    Y: Option<String>,  // Last quote asset transacted quantity
    #[serde(default)]
    Q: Option<String>,  // Quote order quantity
}

/// outboundAccountPosition - 계정 포지션 업데이트
#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceAccountPosition {
    e: String,          // Event type
    #[serde(default)]
    E: Option<i64>,     // Event time
    #[serde(default)]
    u: Option<i64>,     // Time of last account update
    B: Vec<BinanceBalanceEntry>, // Balances
}

/// 잔고 항목
#[derive(Debug, Deserialize, Serialize)]
struct BinanceBalanceEntry {
    a: String, // Asset
    f: String, // Free
    l: String, // Locked
}

/// balanceUpdate - 잔고 업데이트 (입출금)
#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceBalanceUpdate {
    e: String,          // Event type
    #[serde(default)]
    E: Option<i64>,     // Event time
    a: String,          // Asset
    d: String,          // Balance delta
    #[serde(default)]
    T: Option<i64>,     // Clear time
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BinanceWs::format_symbol("BTC/USDT"), "btcusdt");
        assert_eq!(BinanceWs::format_symbol("ETH/BTC"), "ethbtc");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BinanceWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(BinanceWs::to_unified_symbol("ETHBTC"), "ETH/BTC");
        assert_eq!(BinanceWs::to_unified_symbol("BNBBUSD"), "BNB/BUSD");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(BinanceWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(BinanceWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(BinanceWs::format_interval(Timeframe::Day1), "1d");
    }

    #[test]
    fn test_parse_ticker() {
        let json = r#"{"e":"24hrMiniTicker","E":1234567890000,"s":"BTCUSDT","c":"50000.00","o":"49000.00","h":"51000.00","l":"48000.00","v":"1000.00","q":"50000000.00"}"#;
        let data: BinanceMiniTicker = serde_json::from_str(json).unwrap();
        let event = BinanceWs::parse_ticker(&data);

        assert_eq!(event.symbol, "BTC/USDT");
        assert_eq!(event.ticker.close, Some(Decimal::new(5000000, 2)));
    }

    #[test]
    fn test_with_credentials() {
        let ws = BinanceWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = BinanceWs::new();
        assert!(ws.config.is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
    }
}
