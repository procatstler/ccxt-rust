//! BingX WebSocket Implementation
//!
//! BingX 실시간 데이터 스트리밍 (Public + Private)

#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, MarginMode, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Position, PositionSide, Ticker, Timeframe, Trade, OHLCV, WsBalanceEvent, WsExchange, WsMessage,
    WsOhlcvEvent, WsOrderBookEvent, WsOrderEvent, WsPositionEvent, WsTickerEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_PUBLIC_URL: &str = "wss://open-api-ws.bingx.com/market";
const WS_PRIVATE_URL: &str = "wss://open-api-ws.bingx.com/market";
const REST_BASE_URL: &str = "https://open-api.bingx.com";

/// BingX WebSocket 클라이언트
pub struct BingxWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    private_ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    listen_key: Arc<RwLock<Option<String>>>,
    listen_key_timestamp: Arc<RwLock<i64>>,
}

impl BingxWs {
    /// 새 BingX WebSocket 클라이언트 생성
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

    /// API 키와 시크릿으로 BingX WebSocket 클라이언트 생성
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

    /// 심볼을 BingX 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// BingX 심볼을 통합 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(bingx_symbol: &str) -> String {
        bingx_symbol.replace("-", "/")
    }

    /// Timeframe을 BingX 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1M",
            _ => "1m",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BingxTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.event_time.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            bid: data.bid_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_qty.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_qty.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.close.as_ref().and_then(|v| v.parse().ok()),
            last: data.close.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.price_change.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.price_change_percent.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.quote_volume.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BingxOrderBookData, symbol: &str) -> WsOrderBookEvent {
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

        let timestamp = Utc::now().timestamp_millis();

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
            is_snapshot: true,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &BingxTradeData, symbol: &str) -> WsTradeEvent {
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();
        let amount = data.qty.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();

        let trades = vec![Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            info: serde_json::Value::Null,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            order: None,
            trade_type: None,
            side: if data.buyer_maker.unwrap_or(false) { Some("sell".to_string()) } else { Some("buy".to_string()) },
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
        }];

        WsTradeEvent {
            symbol: symbol.to_string(),
            trades,
        }
    }

    /// 캔들 메시지 파싱
    fn parse_candle(data: &BingxKlineData, symbol: &str, timeframe: Timeframe) -> WsOhlcvEvent {
        let ohlcv = OHLCV {
            timestamp: data.start_time.unwrap_or(0),
            open: data.open.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            high: data.high.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            low: data.low.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            close: data.close.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            volume: data.volume.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        }
    }

    // === Private Channel Methods ===

    /// listenKey 생성
    async fn create_listen_key(&self) -> CcxtResult<String> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required for private WebSocket".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let query = format!("timestamp={timestamp}");

        // Sign with HMAC-SHA256
        let signature = {
            let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(query.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        };

        let url = format!("{REST_BASE_URL}/openApi/user/auth/userDataStream?{query}&signature={signature}");
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("X-BX-APIKEY", api_key)
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

        let data: BingxListenKeyResponse = response.json().await.map_err(|e| CcxtError::ParseError {
            data_type: "ListenKeyResponse".to_string(),
            message: e.to_string(),
        })?;

        let listen_key = data.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No listen key in response".into(),
        })?.listen_key;

        // listenKey 저장
        *self.listen_key.write().await = Some(listen_key.clone());
        *self.listen_key_timestamp.write().await = Utc::now().timestamp_millis();

        Ok(listen_key)
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
        let api_secret = config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let listen_key = self.listen_key.read().await.clone().ok_or_else(|| {
            CcxtError::AuthenticationError {
                message: "No listen key available".into(),
            }
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let query = format!("listenKey={listen_key}&timestamp={timestamp}");

        // Sign with HMAC-SHA256
        let signature = {
            let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(query.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        };

        let url = format!("{REST_BASE_URL}/openApi/user/auth/userDataStream?{query}&signature={signature}");
        let client = reqwest::Client::new();
        let response = client
            .put(&url)
            .header("X-BX-APIKEY", api_key)
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?;

        if !response.status().is_success() {
            // listenKey가 만료된 경우 새로 생성 필요
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

        // User Data Stream URL 구성 (listenKey를 쿼리 파라미터로)
        let url = format!("{WS_PRIVATE_URL}?listenKey={listen_key}");

        // WebSocket 클라이언트 생성 및 연결
        let mut ws_client = WsClient::new(WsConfig {
            url: url.clone(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 20,
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
                    if let (Some(api_key), Some(api_secret)) = (config.api_key(), config.secret()) {
                        if let Some(ref key) = *listen_key_clone.read().await {
                            let timestamp = Utc::now().timestamp_millis().to_string();
                            let query = format!("listenKey={key}&timestamp={timestamp}");

                            let signature = {
                                let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
                                    .expect("HMAC can take key of any size");
                                mac.update(query.as_bytes());
                                hex::encode(mac.finalize().into_bytes())
                            };

                            let client = reqwest::Client::new();
                            let _ = client
                                .put(format!("{REST_BASE_URL}/openApi/user/auth/userDataStream?{query}&signature={signature}"))
                                .header("X-BX-APIKEY", api_key)
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
                        // GZIP 압축 해제 시도 (BingX는 바이너리 데이터를 GZIP으로 전송할 수 있음)
                        let decompressed = Self::decompress_message(&msg);
                        if let Some(ws_msg) = Self::process_private_message(&decompressed) {
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

    /// 메시지 전처리 (BingX는 텍스트 JSON 형식 사용)
    fn decompress_message(data: &str) -> String {
        // BingX WebSocket은 텍스트 JSON 형식을 사용
        // Pong이나 유효한 JSON은 그대로 반환
        data.to_string()
    }

    /// Private 메시지 처리 (User Data Stream)
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        if msg == "Pong" || msg.is_empty() {
            return None;
        }

        let json: serde_json::Value = serde_json::from_str(msg).ok()?;

        // BingX 이벤트 타입 확인 (dataType 또는 e 필드)
        let event_type = json.get("dataType")
            .and_then(|v| v.as_str())
            .or_else(|| json.get("e").and_then(|v| v.as_str()))?;

        match event_type {
            // 주문 업데이트
            s if s.contains("ORDER") || s.contains("order") || s == "executionReport" => {
                if let Ok(data) = serde_json::from_str::<BingxOrderUpdate>(msg) {
                    return Some(WsMessage::Order(Self::parse_order_update(&data)));
                }
            }
            // 잔고 업데이트
            s if s.contains("ACCOUNT") || s.contains("account") || s.contains("balance") => {
                if let Ok(data) = serde_json::from_str::<BingxAccountUpdate>(msg) {
                    return Some(WsMessage::Balance(Self::parse_balance_update(&data)));
                }
            }
            // 포지션 업데이트
            s if s.contains("POSITION") || s.contains("position") => {
                if let Ok(data) = serde_json::from_str::<BingxPositionUpdate>(msg) {
                    return Some(WsMessage::Position(Self::parse_position_update(&data)));
                }
            }
            _ => {}
        }

        None
    }

    /// 주문 업데이트 파싱
    fn parse_order_update(data: &BingxOrderUpdate) -> WsOrderEvent {
        let symbol = data.data.as_ref()
            .and_then(|d| d.symbol.as_ref())
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let timestamp = data.data.as_ref()
            .and_then(|d| d.event_time)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_data = data.data.as_ref();

        let status = order_data
            .and_then(|d| d.status.as_ref())
            .map(|s| match s.as_str() {
                "NEW" | "new" => OrderStatus::Open,
                "PARTIALLY_FILLED" | "partially_filled" => OrderStatus::Open,
                "FILLED" | "filled" => OrderStatus::Closed,
                "CANCELED" | "canceled" | "CANCELLED" => OrderStatus::Canceled,
                "EXPIRED" | "expired" => OrderStatus::Expired,
                "REJECTED" | "rejected" => OrderStatus::Rejected,
                _ => OrderStatus::Open,
            })
            .unwrap_or(OrderStatus::Open);

        let side = order_data
            .and_then(|d| d.side.as_ref())
            .map(|s| match s.to_uppercase().as_str() {
                "BUY" => OrderSide::Buy,
                "SELL" => OrderSide::Sell,
                _ => OrderSide::Buy,
            })
            .unwrap_or(OrderSide::Buy);

        let order_type = order_data
            .and_then(|d| d.order_type.as_ref())
            .map(|s| match s.to_uppercase().as_str() {
                "LIMIT" => OrderType::Limit,
                "MARKET" => OrderType::Market,
                "STOP_LIMIT" | "STOP-LIMIT" => OrderType::StopLimit,
                "STOP_MARKET" | "STOP-MARKET" => OrderType::StopMarket,
                "TAKE_PROFIT" => OrderType::TakeProfitMarket,
                "TAKE_PROFIT_LIMIT" => OrderType::TakeProfitLimit,
                _ => OrderType::Limit,
            })
            .unwrap_or(OrderType::Limit);

        let price: Decimal = order_data
            .and_then(|d| d.price.as_ref())
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();

        let amount: Decimal = order_data
            .and_then(|d| d.orig_qty.as_ref())
            .and_then(|q| q.parse().ok())
            .unwrap_or_default();

        let filled: Decimal = order_data
            .and_then(|d| d.executed_qty.as_ref())
            .and_then(|q| q.parse().ok())
            .unwrap_or_default();

        let remaining = amount - filled;

        let cost: Decimal = order_data
            .and_then(|d| d.cumulative_quote_qty.as_ref())
            .and_then(|q| q.parse().ok())
            .unwrap_or_default();

        let order = Order {
            id: order_data.and_then(|d| d.order_id.as_ref()).cloned().unwrap_or_default(),
            client_order_id: order_data.and_then(|d| d.client_order_id.clone()),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: Some(timestamp),
            status,
            symbol,
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
            stop_price: order_data.and_then(|d| d.stop_price.as_ref()).and_then(|v| v.parse().ok()),
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        };

        WsOrderEvent { order }
    }

    /// 잔고 업데이트 파싱
    fn parse_balance_update(data: &BingxAccountUpdate) -> WsBalanceEvent {
        let mut balances = Balances::default();
        balances.timestamp = Some(Utc::now().timestamp_millis());
        balances.datetime = Some(Utc::now().to_rfc3339());

        if let Some(ref account_data) = data.data {
            if let Some(ref balance_list) = account_data.balances {
                for b in balance_list {
                    let currency = b.asset.clone().unwrap_or_default();
                    let free: Decimal = b.free.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();
                    let locked: Decimal = b.locked.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();
                    let total = free + locked;

                    balances.currencies.insert(
                        currency,
                        Balance {
                            free: Some(free),
                            used: Some(locked),
                            total: Some(total),
                            debt: None,
                        },
                    );
                }
            }
        }

        balances.info = serde_json::to_value(data).unwrap_or_default();
        WsBalanceEvent { balances }
    }

    /// 포지션 업데이트 파싱
    fn parse_position_update(data: &BingxPositionUpdate) -> WsPositionEvent {
        let position_data = data.data.as_ref();
        let symbol = position_data
            .and_then(|d| d.symbol.as_ref())
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let timestamp = Utc::now().timestamp_millis();

        let side = position_data.and_then(|d| d.position_side.as_ref()).map(|s| {
            if s.to_uppercase() == "LONG" { PositionSide::Long } else { PositionSide::Short }
        });

        let margin_mode = position_data.and_then(|d| d.margin_type.as_ref()).map(|m| {
            if m.to_uppercase() == "ISOLATED" { MarginMode::Isolated } else { MarginMode::Cross }
        });

        let position = Position {
            id: None,
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            hedged: None,
            side,
            contracts: position_data.and_then(|d| d.position_amt.as_ref()).and_then(|v| v.parse().ok()),
            contract_size: None,
            entry_price: position_data.and_then(|d| d.entry_price.as_ref()).and_then(|v| v.parse().ok()),
            mark_price: position_data.and_then(|d| d.mark_price.as_ref()).and_then(|v| v.parse().ok()),
            notional: None,
            leverage: position_data.and_then(|d| d.leverage.as_ref()).and_then(|v| v.parse().ok()),
            collateral: None,
            initial_margin: None,
            maintenance_margin: None,
            initial_margin_percentage: None,
            maintenance_margin_percentage: None,
            unrealized_pnl: position_data.and_then(|d| d.unrealized_profit.as_ref()).and_then(|v| v.parse().ok()),
            realized_pnl: None,
            liquidation_price: position_data.and_then(|d| d.liquidation_price.as_ref()).and_then(|v| v.parse().ok()),
            margin_mode,
            margin_ratio: None,
            percentage: None,
            info: serde_json::to_value(data).unwrap_or_default(),
            stop_loss_price: None,
            take_profit_price: None,
            last_update_timestamp: Some(timestamp),
            last_price: None,
        };

        WsPositionEvent { positions: vec![position] }
    }

    /// 메시지 처리
    fn process_message(msg: &str, subscribed_symbol: Option<&str>, subscribed_timeframe: Option<Timeframe>) -> Option<WsMessage> {
        // Pong 처리
        if msg == "Pong" {
            return None;
        }

        let response: BingxWsResponse = serde_json::from_str(msg).ok()?;

        let data_type = response.data_type.as_deref()?;
        let symbol = subscribed_symbol.map(Self::to_unified_symbol).unwrap_or_default();

        match data_type {
            s if s.contains("ticker") => {
                if let Some(data) = response.data {
                    if let Ok(ticker_data) = serde_json::from_value::<BingxTickerData>(data) {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                    }
                }
            }
            s if s.contains("depth") => {
                if let Some(data) = response.data {
                    if let Ok(book_data) = serde_json::from_value::<BingxOrderBookData>(data) {
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&book_data, &symbol)));
                    }
                }
            }
            s if s.contains("trade") => {
                if let Some(data) = response.data {
                    if let Ok(trade_data) = serde_json::from_value::<BingxTradeData>(data) {
                        return Some(WsMessage::Trade(Self::parse_trade(&trade_data, &symbol)));
                    }
                }
            }
            s if s.contains("kline") => {
                if let Some(data) = response.data {
                    if let Ok(kline_data) = serde_json::from_value::<BingxKlineData>(data) {
                        let timeframe = subscribed_timeframe.unwrap_or(Timeframe::Minute1);
                        return Some(WsMessage::Ohlcv(Self::parse_candle(&kline_data, &symbol, timeframe)));
                    }
                }
            }
            _ => {}
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        subscribe_msg: serde_json::Value,
        channel: &str,
        symbol: Option<&str>,
        timeframe: Option<Timeframe>
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 20,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let subscribed_symbol = symbol.map(|s| s.to_string());
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
                        if let Some(ws_msg) = Self::process_message(&msg, subscribed_symbol.as_deref(), timeframe) {
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

impl Default for BingxWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for BingxWs {
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
impl WsExchange for BingxWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "id": "ticker",
            "reqType": "sub",
            "dataType": format!("{}@ticker", formatted)
        });
        client.subscribe_stream(subscribe_msg, "ticker", Some(&formatted), None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let data_types: Vec<String> = symbols.iter()
            .map(|s| format!("{}@ticker", Self::format_symbol(s)))
            .collect();
        let subscribe_msg = serde_json::json!({
            "id": "tickers",
            "reqType": "sub",
            "dataType": data_types.join(",")
        });
        client.subscribe_stream(subscribe_msg, "tickers", None, None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let depth = limit.unwrap_or(20);
        let subscribe_msg = serde_json::json!({
            "id": "depth",
            "reqType": "sub",
            "dataType": format!("{}@depth{}@100ms", formatted, depth)
        });
        client.subscribe_stream(subscribe_msg, "orderBook", Some(&formatted), None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "id": "trade",
            "reqType": "sub",
            "dataType": format!("{}@trade", formatted)
        });
        client.subscribe_stream(subscribe_msg, "trade", Some(&formatted), None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let subscribe_msg = serde_json::json!({
            "id": "kline",
            "reqType": "sub",
            "dataType": format!("{}@kline_{}", formatted, interval)
        });
        client.subscribe_stream(subscribe_msg, "kline", Some(&formatted), Some(timeframe)).await
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

    async fn watch_positions(&self, _symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("positions").await
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        // listenKey 생성
        self.create_listen_key().await?;
        Ok(())
    }
}

// === Response Types ===

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BingxWsResponse {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    data_type: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BingxTickerData {
    #[serde(default, rename = "s")]
    symbol: String,
    #[serde(default, rename = "E")]
    event_time: Option<i64>,
    #[serde(default, rename = "c")]
    close: Option<String>,
    #[serde(default, rename = "o")]
    open: Option<String>,
    #[serde(default, rename = "h")]
    high: Option<String>,
    #[serde(default, rename = "l")]
    low: Option<String>,
    #[serde(default, rename = "v")]
    volume: Option<String>,
    #[serde(default, rename = "q")]
    quote_volume: Option<String>,
    #[serde(default, rename = "p")]
    price_change: Option<String>,
    #[serde(default, rename = "P")]
    price_change_percent: Option<String>,
    #[serde(default, rename = "b")]
    bid_price: Option<String>,
    #[serde(default, rename = "B")]
    bid_qty: Option<String>,
    #[serde(default, rename = "a")]
    ask_price: Option<String>,
    #[serde(default, rename = "A")]
    ask_qty: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BingxOrderBookData {
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BingxTradeData {
    #[serde(default, rename = "t")]
    trade_id: Option<String>,
    #[serde(default, rename = "T")]
    time: Option<i64>,
    #[serde(default, rename = "p")]
    price: Option<String>,
    #[serde(default, rename = "q")]
    qty: Option<String>,
    #[serde(default, rename = "m")]
    buyer_maker: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BingxKlineData {
    #[serde(default, rename = "t")]
    start_time: Option<i64>,
    #[serde(default, rename = "o")]
    open: Option<String>,
    #[serde(default, rename = "h")]
    high: Option<String>,
    #[serde(default, rename = "l")]
    low: Option<String>,
    #[serde(default, rename = "c")]
    close: Option<String>,
    #[serde(default, rename = "v")]
    volume: Option<String>,
}

// === Private WebSocket Message Types ===

/// Listen Key 응답
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BingxListenKeyResponse {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    data: Option<BingxListenKeyData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BingxListenKeyData {
    listen_key: String,
}

/// 주문 업데이트 이벤트
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxOrderUpdate {
    #[serde(default)]
    data_type: Option<String>,
    #[serde(default)]
    data: Option<BingxOrderData>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxOrderData {
    #[serde(default, alias = "s")]
    symbol: Option<String>,
    #[serde(default, alias = "E")]
    event_time: Option<i64>,
    #[serde(default, alias = "i")]
    order_id: Option<String>,
    #[serde(default, alias = "c")]
    client_order_id: Option<String>,
    #[serde(default, alias = "S")]
    side: Option<String>,
    #[serde(default, alias = "o")]
    order_type: Option<String>,
    #[serde(default, alias = "X")]
    status: Option<String>,
    #[serde(default, alias = "p")]
    price: Option<String>,
    #[serde(default, alias = "q")]
    orig_qty: Option<String>,
    #[serde(default, alias = "z")]
    executed_qty: Option<String>,
    #[serde(default, alias = "Z")]
    cumulative_quote_qty: Option<String>,
    #[serde(default, alias = "P")]
    stop_price: Option<String>,
    #[serde(default, alias = "n")]
    commission: Option<String>,
    #[serde(default, alias = "N")]
    commission_asset: Option<String>,
}

/// 계정/잔고 업데이트 이벤트
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxAccountUpdate {
    #[serde(default)]
    data_type: Option<String>,
    #[serde(default)]
    data: Option<BingxAccountData>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxAccountData {
    #[serde(default, alias = "E")]
    event_time: Option<i64>,
    #[serde(default, alias = "B")]
    balances: Option<Vec<BingxBalanceEntry>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxBalanceEntry {
    #[serde(default, alias = "a")]
    asset: Option<String>,
    #[serde(default, alias = "f")]
    free: Option<String>,
    #[serde(default, alias = "l")]
    locked: Option<String>,
}

/// 포지션 업데이트 이벤트
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxPositionUpdate {
    #[serde(default)]
    data_type: Option<String>,
    #[serde(default)]
    data: Option<BingxPositionData>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxPositionData {
    #[serde(default, alias = "s")]
    symbol: Option<String>,
    #[serde(default, alias = "E")]
    event_time: Option<i64>,
    #[serde(default)]
    position_side: Option<String>,
    #[serde(default)]
    position_amt: Option<String>,
    #[serde(default)]
    entry_price: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    unrealized_profit: Option<String>,
    #[serde(default)]
    liquidation_price: Option<String>,
    #[serde(default)]
    leverage: Option<String>,
    #[serde(default)]
    margin_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BingxWs::format_symbol("BTC/USDT"), "BTC-USDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BingxWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
    }

    #[test]
    fn test_with_config() {
        let config = ExchangeConfig::new().with_api_key("test_key").with_api_secret("test_secret");
        let client = BingxWs::with_config(config);
        assert!(client.config.is_some());
    }

    #[test]
    fn test_parse_order_update() {
        let json = r#"{
            "dataType": "ORDER",
            "data": {
                "symbol": "BTC-USDT",
                "orderId": "12345",
                "clientOrderId": "client123",
                "side": "BUY",
                "orderType": "LIMIT",
                "status": "FILLED",
                "price": "50000.00",
                "origQty": "0.1",
                "executedQty": "0.1",
                "cumulativeQuoteQty": "5000.00"
            }
        }"#;

        let data: BingxOrderUpdate = serde_json::from_str(json).unwrap();
        let event = BingxWs::parse_order_update(&data);

        assert_eq!(event.order.symbol, "BTC/USDT");
        assert_eq!(event.order.id, "12345");
        assert_eq!(event.order.status, OrderStatus::Closed);
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
    }

    #[test]
    fn test_parse_balance_update() {
        let json = r#"{
            "dataType": "ACCOUNT",
            "data": {
                "eventTime": 1234567890000,
                "balances": [
                    {"asset": "USDT", "free": "1000.00", "locked": "50.00"},
                    {"asset": "BTC", "free": "0.5", "locked": "0.1"}
                ]
            }
        }"#;

        let data: BingxAccountUpdate = serde_json::from_str(json).unwrap();
        let event = BingxWs::parse_balance_update(&data);

        assert!(event.balances.currencies.contains_key("USDT"));
        assert!(event.balances.currencies.contains_key("BTC"));

        let usdt = event.balances.currencies.get("USDT").unwrap();
        assert_eq!(usdt.free, Some(Decimal::new(100000, 2)));
        assert_eq!(usdt.used, Some(Decimal::new(5000, 2)));
    }

    #[test]
    fn test_parse_position_update() {
        let json = r#"{
            "dataType": "POSITION",
            "data": {
                "symbol": "BTC-USDT",
                "positionSide": "LONG",
                "positionAmt": "0.5",
                "entryPrice": "50000.00",
                "markPrice": "51000.00",
                "unrealizedProfit": "500.00",
                "leverage": "10",
                "marginType": "ISOLATED"
            }
        }"#;

        let data: BingxPositionUpdate = serde_json::from_str(json).unwrap();
        let event = BingxWs::parse_position_update(&data);

        assert_eq!(event.positions.len(), 1);
        let position = &event.positions[0];
        assert_eq!(position.symbol, "BTC/USDT");
        assert_eq!(position.side, Some(PositionSide::Long));
        assert_eq!(position.contracts, Some(Decimal::new(5, 1)));
        assert_eq!(position.leverage, Some(Decimal::new(10, 0)));
        assert_eq!(position.margin_mode, Some(MarginMode::Isolated));
    }

    #[test]
    fn test_process_private_message_order() {
        let json = r#"{
            "dataType": "ORDER",
            "data": {
                "symbol": "ETH-USDT",
                "orderId": "67890",
                "side": "SELL",
                "orderType": "MARKET",
                "status": "NEW",
                "origQty": "1.0"
            }
        }"#;

        let result = BingxWs::process_private_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.symbol, "ETH/USDT");
            assert_eq!(event.order.side, OrderSide::Sell);
            assert_eq!(event.order.status, OrderStatus::Open);
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_private_message_balance() {
        let json = r#"{
            "dataType": "ACCOUNT_UPDATE",
            "data": {
                "balances": [
                    {"asset": "ETH", "free": "10.0", "locked": "0.0"}
                ]
            }
        }"#;

        let result = BingxWs::process_private_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("ETH"));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_listen_key_response_parsing() {
        let json = r#"{
            "code": 0,
            "data": {
                "listenKey": "abc123def456"
            }
        }"#;

        let response: BingxListenKeyResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.code, Some(0));
        assert!(response.data.is_some());
        assert_eq!(response.data.unwrap().listen_key, "abc123def456");
    }

    #[test]
    fn test_decompress_message() {
        let json = r#"{"test": "value"}"#;
        assert_eq!(BingxWs::decompress_message(json), json);
        assert_eq!(BingxWs::decompress_message("Pong"), "Pong");
    }

    #[test]
    fn test_clone() {
        let config = ExchangeConfig::new().with_api_key("key").with_api_secret("secret");
        let original = BingxWs::with_config(config);
        let cloned = original.clone();

        assert!(cloned.config.is_some());
        assert!(cloned.ws_client.is_none());
        assert!(cloned.private_ws_client.is_none());
    }

    #[test]
    fn test_with_credentials() {
        let ws = BingxWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
        let config = ws.config.unwrap();
        assert_eq!(config.api_key(), Some("test_key"));
        assert_eq!(config.secret(), Some("test_secret"));
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = BingxWs::new();
        assert!(ws.config.is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
        let config = ws.config.unwrap();
        assert_eq!(config.api_key(), Some("test_key"));
        assert_eq!(config.secret(), Some("test_secret"));
    }
}
