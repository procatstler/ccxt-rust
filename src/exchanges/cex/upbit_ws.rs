//! Upbit WebSocket Implementation
//!
//! Upbit 실시간 데이터 스트리밍
//!
//! Private channels:
//! - myOrder: Order status updates
//! - myAsset: Balance/asset updates

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Ticker,
    Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsOrderBookEvent, WsOrderEvent,
    WsTickerEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://api.upbit.com/websocket/v1";
const WS_PRIVATE_URL: &str = "wss://api.upbit.com/websocket/v1/private";

/// Upbit WebSocket 클라이언트
pub struct UpbitWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl UpbitWs {
    /// 새 Upbit WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
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
            api_key: Some(api_key),
            api_secret: Some(api_secret),
        }
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
    }

    /// 심볼을 Upbit 형식으로 변환 (BTC/KRW -> KRW-BTC)
    fn format_symbol(symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            format!("{}-{}", parts[1], parts[0])
        } else {
            symbol.to_string()
        }
    }

    /// Upbit 심볼을 통합 심볼로 변환 (KRW-BTC -> BTC/KRW)
    fn to_unified_symbol(upbit_symbol: &str) -> String {
        let parts: Vec<&str> = upbit_symbol.split('-').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[1], parts[0])
        } else {
            upbit_symbol.to_string()
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &UpbitTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.code);
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price,
            low: data.low_price,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: data.opening_price,
            close: data.trade_price,
            last: data.trade_price,
            previous_close: data.prev_closing_price,
            change: data.signed_change_price,
            percentage: data.signed_change_rate.map(|r| r * Decimal::from(100)),
            average: None,
            base_volume: data.acc_trade_volume_24h,
            quote_volume: data.acc_trade_price_24h,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &UpbitOrderBookData) -> WsOrderBookEvent {
        let symbol = Self::to_unified_symbol(&data.code);

        let bids: Vec<OrderBookEntry> = data
            .orderbook_units
            .iter()
            .filter_map(|u| {
                Some(OrderBookEntry {
                    price: u.bid_price?,
                    amount: u.bid_size?,
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .orderbook_units
            .iter()
            .filter_map(|u| {
                Some(OrderBookEntry {
                    price: u.ask_price?,
                    amount: u.ask_size?,
                })
            })
            .collect();

        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.clone(),
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
            symbol,
            order_book,
            is_snapshot: true, // Upbit always sends full orderbook
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &UpbitTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.code);
        let timestamp = data
            .trade_timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.trade_price.unwrap_or_default();
        let amount: Decimal = data.trade_volume.unwrap_or_default();

        let side = match data.ask_bid.as_deref() {
            Some("ASK") => "sell",
            Some("BID") => "buy",
            _ => "buy",
        };

        let trades = vec![Trade {
            id: data
                .sequential_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(side.to_string()),
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

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Try to parse as different message types
        if let Ok(ticker_data) = serde_json::from_str::<UpbitTickerData>(msg) {
            if ticker_data.msg_type.as_deref() == Some("ticker") {
                return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
            }
        }

        if let Ok(ob_data) = serde_json::from_str::<UpbitOrderBookData>(msg) {
            if ob_data.msg_type.as_deref() == Some("orderbook") {
                return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data)));
            }
        }

        if let Ok(trade_data) = serde_json::from_str::<UpbitTradeData>(msg) {
            if trade_data.msg_type.as_deref() == Some("trade") {
                return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        msg_type: &str,
        codes: Vec<String>,
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
            ping_interval_secs: 120, // Upbit requires ping every 2 minutes
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // Generate unique ticket
        let ticket = Uuid::new_v4().to_string();

        // 구독 메시지 전송
        let subscribe_msg = serde_json::json!([
            {"ticket": ticket},
            {"type": msg_type, "codes": codes, "isOnlyRealtime": true}
        ]);
        ws_client.send(&subscribe_msg.to_string())?;

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

    /// JWT 토큰 생성 (Private WebSocket 인증용)
    fn create_jwt(api_key: &str, api_secret: &str) -> CcxtResult<String> {
        let nonce = Uuid::new_v4().to_string();

        // Build JWT payload
        let payload = serde_json::json!({
            "access_key": api_key,
            "nonce": nonce,
        });

        // Create JWT header
        let header = serde_json::json!({
            "alg": "HS256",
            "typ": "JWT"
        });

        // Encode header and payload
        let header_b64 = BASE64.encode(header.to_string().as_bytes());
        let payload_b64 = BASE64.encode(payload.to_string().as_bytes());

        // Create signature
        let message = format!("{header_b64}.{payload_b64}");

        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".to_string(),
            }
        })?;
        mac.update(message.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        Ok(format!("{header_b64}.{payload_b64}.{signature}"))
    }

    /// Private WebSocket 구독 (myOrder, myAsset)
    async fn subscribe_private_stream(
        &mut self,
        channel: &str,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required for private channels".to_string(),
            })?;
        let api_secret =
            self.api_secret
                .as_ref()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API secret required for private channels".to_string(),
                })?;

        let jwt = Self::create_jwt(api_key, api_secret)?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // Note: Upbit private WebSocket requires JWT in the connection header
        // We construct the URL with the auth parameter
        let private_url = format!("{WS_PRIVATE_URL}?token={jwt}");

        let mut ws_client = WsClient::new(WsConfig {
            url: private_url,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 120,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // Generate unique ticket
        let ticket = Uuid::new_v4().to_string();

        // Build subscription based on channel type
        let subscribe_msg = match channel {
            "myOrder" => {
                // myOrder supports optional symbol filter
                if let Some(sym) = symbol {
                    let code = Self::format_symbol(sym);
                    serde_json::json!([
                        {"ticket": ticket},
                        {"type": "myOrder", "codes": [code], "isOnlyRealtime": true}
                    ])
                } else {
                    serde_json::json!([
                        {"ticket": ticket},
                        {"type": "myOrder", "isOnlyRealtime": true}
                    ])
                }
            },
            "myAsset" => {
                serde_json::json!([
                    {"ticket": ticket},
                    {"type": "myAsset", "isOnlyRealtime": true}
                ])
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Unknown private channel: {channel}"),
                });
            },
        };

        ws_client.send(&subscribe_msg.to_string())?;
        self.ws_client = Some(ws_client);

        // Save subscription
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or("all"));
            self.subscriptions
                .write()
                .await
                .insert(key, channel.to_string());
        }

        // Event processing task
        let tx = event_tx.clone();
        let channel_clone = channel.to_string();
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
                        if let Some(ws_msg) = Self::process_private_message(&msg, &channel_clone) {
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

    /// Private 메시지 파싱
    fn process_private_message(msg: &str, channel: &str) -> Option<WsMessage> {
        match channel {
            "myOrder" => {
                if let Ok(order_data) = serde_json::from_str::<UpbitMyOrderData>(msg) {
                    if order_data.msg_type.as_deref() == Some("myOrder") {
                        return Some(WsMessage::Order(Self::parse_order(&order_data)));
                    }
                }
            },
            "myAsset" => {
                if let Ok(asset_data) = serde_json::from_str::<UpbitMyAssetData>(msg) {
                    if asset_data.msg_type.as_deref() == Some("myAsset") {
                        return Some(WsMessage::Balance(Self::parse_balance(&asset_data)));
                    }
                }
            },
            _ => {},
        }
        None
    }

    /// 주문 데이터 파싱
    fn parse_order(data: &UpbitMyOrderData) -> WsOrderEvent {
        let symbol = data
            .code
            .as_ref()
            .map(|c| Self::to_unified_symbol(c))
            .unwrap_or_default();

        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.state.as_deref() {
            Some("wait") | Some("watch") => OrderStatus::Open,
            Some("done") => OrderStatus::Closed,
            Some("cancel") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let side = match data.side.as_deref() {
            Some("bid") => OrderSide::Buy,
            Some("ask") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.ord_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("price") | Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let order = Order {
            id: data.uuid.clone().unwrap_or_default(),
            client_order_id: None,
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
            price: data.price,
            average: data.avg_price,
            amount: data.volume.unwrap_or_default(),
            filled: data.executed_volume.unwrap_or_default(),
            remaining: data.remaining_volume,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            reduce_only: None,
            post_only: None,
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// 잔고 데이터 파싱
    fn parse_balance(data: &UpbitMyAssetData) -> WsBalanceEvent {
        let currency = data.currency.clone().unwrap_or_default();
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let balance = Balance {
            free: data.balance,
            used: data.locked,
            total: match (data.balance, data.locked) {
                (Some(b), Some(l)) => Some(b + l),
                (Some(b), None) => Some(b),
                (None, Some(l)) => Some(l),
                _ => None,
            },
            debt: None,
        };

        let mut balances = Balances::new();
        balances.add(currency, balance);
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(
            chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        );
        balances.info = serde_json::to_value(data).unwrap_or_default();

        WsBalanceEvent { balances }
    }
}

impl Default for UpbitWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for UpbitWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let codes = vec![Self::format_symbol(symbol)];
        client
            .subscribe_stream("ticker", codes, "ticker", Some(symbol))
            .await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let codes: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        client
            .subscribe_stream("ticker", codes, "tickers", None)
            .await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let codes = vec![Self::format_symbol(symbol)];
        client
            .subscribe_stream("orderbook", codes, "orderBook", Some(symbol))
            .await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let codes = vec![Self::format_symbol(symbol)];
        client
            .subscribe_stream("trade", codes, "trades", Some(symbol))
            .await
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Upbit does not support OHLCV streaming via WebSocket
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watch_ohlcv".to_string(),
        })
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

    async fn watch_orders(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key
                .clone()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API key required".into(),
                })?,
            self.api_secret
                .clone()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API secret required".into(),
                })?,
        );
        client.subscribe_private_stream("myOrder", symbol).await
    }

    async fn watch_my_trades(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Upbit's myOrder channel includes trade execution info
        // Trades are part of order updates when executed_volume changes
        let mut client = Self::with_credentials(
            self.api_key
                .clone()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API key required".into(),
                })?,
            self.api_secret
                .clone()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API secret required".into(),
                })?,
        );
        client.subscribe_private_stream("myOrder", symbol).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key
                .clone()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API key required".into(),
                })?,
            self.api_secret
                .clone()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API secret required".into(),
                })?,
        );
        client.subscribe_private_stream("myAsset", None).await
    }
}

// === Upbit WebSocket Types ===

#[derive(Debug, Deserialize, Serialize)]
struct UpbitTickerData {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    code: String,
    #[serde(default)]
    opening_price: Option<Decimal>,
    #[serde(default)]
    high_price: Option<Decimal>,
    #[serde(default)]
    low_price: Option<Decimal>,
    #[serde(default)]
    trade_price: Option<Decimal>,
    #[serde(default)]
    prev_closing_price: Option<Decimal>,
    #[serde(default)]
    signed_change_price: Option<Decimal>,
    #[serde(default)]
    signed_change_rate: Option<Decimal>,
    #[serde(default)]
    acc_trade_volume_24h: Option<Decimal>,
    #[serde(default)]
    acc_trade_price_24h: Option<Decimal>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitOrderBookData {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    code: String,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    orderbook_units: Vec<UpbitOrderBookUnit>,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitOrderBookUnit {
    #[serde(default)]
    ask_price: Option<Decimal>,
    #[serde(default)]
    bid_price: Option<Decimal>,
    #[serde(default)]
    ask_size: Option<Decimal>,
    #[serde(default)]
    bid_size: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitTradeData {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    code: String,
    #[serde(default)]
    trade_price: Option<Decimal>,
    #[serde(default)]
    trade_volume: Option<Decimal>,
    #[serde(default)]
    ask_bid: Option<String>,
    #[serde(default)]
    trade_timestamp: Option<i64>,
    #[serde(default)]
    sequential_id: Option<i64>,
}

// === Private Channel Types ===

/// 내 주문 데이터 (myOrder channel)
#[derive(Debug, Deserialize, Serialize)]
struct UpbitMyOrderData {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    /// 마켓 코드 (ex: KRW-BTC)
    #[serde(default)]
    code: Option<String>,
    /// 주문 UUID
    #[serde(default)]
    uuid: Option<String>,
    /// 주문 종류 (bid: 매수, ask: 매도)
    #[serde(default)]
    side: Option<String>,
    /// 주문 방식 (limit: 지정가, price: 시장가 매수, market: 시장가 매도)
    #[serde(default)]
    ord_type: Option<String>,
    /// 주문 상태 (wait: 대기, watch: 감시, done: 완료, cancel: 취소)
    #[serde(default)]
    state: Option<String>,
    /// 주문 가격
    #[serde(default)]
    price: Option<Decimal>,
    /// 평균 체결 가격
    #[serde(default)]
    avg_price: Option<Decimal>,
    /// 주문 수량
    #[serde(default)]
    volume: Option<Decimal>,
    /// 체결된 수량
    #[serde(default)]
    executed_volume: Option<Decimal>,
    /// 미체결 수량
    #[serde(default)]
    remaining_volume: Option<Decimal>,
    /// 체결된 금액
    #[serde(default)]
    executed_funds: Option<Decimal>,
    /// 주문 생성 시각
    #[serde(default)]
    created_at: Option<String>,
    /// 타임스탬프
    #[serde(default)]
    timestamp: Option<i64>,
}

/// 내 자산 데이터 (myAsset channel)
#[derive(Debug, Deserialize, Serialize)]
struct UpbitMyAssetData {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    /// 화폐 코드 (ex: BTC, KRW)
    #[serde(default)]
    currency: Option<String>,
    /// 주문 가능 잔고
    #[serde(default)]
    balance: Option<Decimal>,
    /// 주문 중 묶여있는 잔고
    #[serde(default)]
    locked: Option<Decimal>,
    /// 타임스탬프
    #[serde(default)]
    timestamp: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(UpbitWs::format_symbol("BTC/KRW"), "KRW-BTC");
        assert_eq!(UpbitWs::format_symbol("ETH/BTC"), "BTC-ETH");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(UpbitWs::to_unified_symbol("KRW-BTC"), "BTC/KRW");
        assert_eq!(UpbitWs::to_unified_symbol("BTC-ETH"), "ETH/BTC");
    }

    #[test]
    fn test_with_credentials() {
        let ws = UpbitWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.api_key.is_some());
        assert!(ws.api_secret.is_some());
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = UpbitWs::new();
        assert!(ws.api_key.is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.api_key.is_some());
        assert!(ws.api_secret.is_some());
    }

    #[test]
    fn test_create_jwt() {
        let jwt = UpbitWs::create_jwt("test_access_key", "test_secret_key").unwrap();
        // JWT format: header.payload.signature
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        // Verify header contains alg and typ
        let header = String::from_utf8(BASE64.decode(parts[0]).unwrap()).unwrap();
        assert!(header.contains("HS256"));
        assert!(header.contains("JWT"));
    }

    #[test]
    fn test_parse_my_order_data() {
        let json = r#"{
            "type": "myOrder",
            "code": "KRW-BTC",
            "uuid": "test-uuid-123",
            "side": "bid",
            "ord_type": "limit",
            "state": "wait",
            "price": "50000000",
            "volume": "0.001",
            "executed_volume": "0",
            "remaining_volume": "0.001",
            "timestamp": 1704067200000
        }"#;

        let data: UpbitMyOrderData = serde_json::from_str(json).unwrap();
        assert_eq!(data.msg_type.as_deref(), Some("myOrder"));
        assert_eq!(data.code.as_deref(), Some("KRW-BTC"));
        assert_eq!(data.side.as_deref(), Some("bid"));
        assert_eq!(data.state.as_deref(), Some("wait"));
    }

    #[test]
    fn test_parse_my_asset_data() {
        let json = r#"{
            "type": "myAsset",
            "currency": "BTC",
            "balance": "1.5",
            "locked": "0.5",
            "timestamp": 1704067200000
        }"#;

        let data: UpbitMyAssetData = serde_json::from_str(json).unwrap();
        assert_eq!(data.msg_type.as_deref(), Some("myAsset"));
        assert_eq!(data.currency.as_deref(), Some("BTC"));
        assert!(data.balance.is_some());
        assert!(data.locked.is_some());
    }
}
