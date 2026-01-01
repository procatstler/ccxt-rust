//! Upbit WebSocket Implementation
//!
//! 업비트 실시간 데이터 스트리밍 (Public + Private)

#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Ticker, Timeframe, Trade, OHLCV, WsBalanceEvent, WsExchange, WsMessage, WsOhlcvEvent,
    WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent, Fee,
};

const WS_BASE_URL: &str = "wss://api.upbit.com/websocket/v1";

/// Upbit WebSocket 클라이언트
pub struct UpbitWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    private_ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, SubscriptionRequest>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

/// 구독 요청 구조체
#[derive(Debug, Clone, Serialize)]
struct SubscriptionRequest {
    #[serde(rename = "type")]
    channel_type: String,
    codes: Vec<String>,
}

impl UpbitWs {
    /// 새 Upbit WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            config: None,
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// API 키와 시크릿으로 Upbit WebSocket 클라이언트 생성
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// JWT 토큰 생성 (Private 채널용)
    fn create_jwt(&self) -> CcxtResult<String> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required for private WebSocket".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let secret = config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let nonce = Uuid::new_v4().to_string();

        // Build JWT payload
        let payload = json!({
            "access_key": api_key,
            "nonce": nonce,
        });

        // Create JWT header
        let header = json!({
            "alg": "HS256",
            "typ": "JWT"
        });

        // Encode header and payload
        let header_b64 = BASE64.encode(header.to_string().as_bytes());
        let payload_b64 = BASE64.encode(payload.to_string().as_bytes());

        // Create signature
        let message = format!("{}.{}", header_b64, payload_b64);

        use hmac::{Hmac, Mac};
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Invalid secret key".to_string(),
            })?;
        mac.update(message.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        Ok(format!("{}.{}.{}", header_b64, payload_b64, signature))
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

    /// Upbit 마켓 ID를 통합 심볼로 변환 (KRW-BTC -> BTC/KRW)
    fn to_unified_symbol(upbit_symbol: &str) -> String {
        let parts: Vec<&str> = upbit_symbol.split('-').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[1], parts[0])
        } else {
            upbit_symbol.to_string()
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &UpbitTickerMsg) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.code);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        // Calculate change percentage
        let percentage = data.signed_change_rate.map(|r| r * Decimal::from(100));

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
            close: Some(data.trade_price),
            last: Some(data.trade_price),
            previous_close: data.prev_closing_price,
            change: data.signed_change_price,
            percentage,
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
    fn parse_order_book(data: &UpbitOrderBookMsg) -> WsOrderBookEvent {
        let symbol = Self::to_unified_symbol(&data.code);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        for unit in &data.orderbook_units {
            bids.push(OrderBookEntry {
                price: unit.bid_price,
                amount: unit.bid_size,
            });
            asks.push(OrderBookEntry {
                price: unit.ask_price,
                amount: unit.ask_size,
            });
        }

        // Sort: bids descending, asks ascending
        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

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
        };

        let is_snapshot = data.stream_type.as_deref() == Some("SNAPSHOT");

        WsOrderBookEvent {
            symbol,
            order_book,
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &UpbitTradeMsg) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.code);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = if data.ask_bid == "ASK" {
            "sell"
        } else {
            "buy"
        };

        let trade = Trade {
            id: data.sequential_id.to_string(),
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
            price: data.trade_price,
            amount: data.trade_volume,
            cost: Some(data.trade_price * data.trade_volume),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    /// OHLCV 메시지 파싱
    fn parse_ohlcv(data: &UpbitCandleMsg) -> WsOhlcvEvent {
        let symbol = Self::to_unified_symbol(&data.code);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        // Upbit only supports 1s candles via WebSocket
        let timeframe = Timeframe::Second1;

        let ohlcv = OHLCV {
            timestamp,
            open: data.opening_price,
            high: data.high_price,
            low: data.low_price,
            close: data.trade_price,
            volume: data.candle_acc_trade_volume,
        };

        WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        }
    }

    /// 주문 상태 파싱
    fn parse_order_status(status: &str) -> OrderStatus {
        match status {
            "wait" => OrderStatus::Open,
            "watch" => OrderStatus::Open,
            "trade" => OrderStatus::Open,
            "done" => OrderStatus::Closed,
            "cancel" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        }
    }

    /// 주문 메시지 파싱
    fn parse_order(data: &UpbitOrderMsg) -> WsOrderEvent {
        let symbol = Self::to_unified_symbol(&data.code);
        let timestamp = data.order_timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = match data.ask_bid.as_str() {
            "BID" => OrderSide::Buy,
            "ASK" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_str() {
            "limit" => OrderType::Limit,
            "market" | "price" | "best" => OrderType::Market,
            _ => OrderType::Limit,
        };

        let filled = data.executed_volume.unwrap_or(Decimal::ZERO);
        let remaining = data.remaining_volume.unwrap_or(Decimal::ZERO);
        let amount = data.volume.unwrap_or(Decimal::ZERO);

        let average = if filled > Decimal::ZERO {
            data.avg_price
        } else {
            None
        };

        let cost = data.executed_funds;

        let fee = data.paid_fee.map(|cost| Fee {
            cost: Some(cost),
            currency: None,
            rate: None,
        });

        let order = Order {
            id: data.uuid.clone(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.timestamp,
            status: Self::parse_order_status(&data.state),
            symbol: symbol.clone(),
            order_type,
            time_in_force: None,
            side,
            price: data.price,
            average,
            amount,
            filled,
            remaining: Some(remaining),
            cost,
            trades: Vec::new(),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        };

        WsOrderEvent { order }
    }

    /// 잔고 메시지 파싱
    fn parse_balance(data: &UpbitAssetMsg) -> WsBalanceEvent {
        let mut balances = Balances::default();
        balances.timestamp = data.timestamp;
        balances.datetime = data.timestamp.and_then(|t| {
            chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
        });

        for asset in &data.assets {
            let free = Decimal::from_str(&asset.balance).unwrap_or(Decimal::ZERO);
            let used = Decimal::from_str(&asset.locked).unwrap_or(Decimal::ZERO);
            let total = free + used;

            balances.currencies.insert(
                asset.currency.clone(),
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

    /// Public 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;
        let msg_type = json.get("type").and_then(|v| v.as_str())?;

        match msg_type {
            "ticker" => {
                if let Ok(data) = serde_json::from_str::<UpbitTickerMsg>(msg) {
                    return Some(WsMessage::Ticker(Self::parse_ticker(&data)));
                }
            }
            "orderbook" => {
                if let Ok(data) = serde_json::from_str::<UpbitOrderBookMsg>(msg) {
                    return Some(WsMessage::OrderBook(Self::parse_order_book(&data)));
                }
            }
            "trade" => {
                if let Ok(data) = serde_json::from_str::<UpbitTradeMsg>(msg) {
                    return Some(WsMessage::Trade(Self::parse_trade(&data)));
                }
            }
            "candle.1s" => {
                if let Ok(data) = serde_json::from_str::<UpbitCandleMsg>(msg) {
                    return Some(WsMessage::Ohlcv(Self::parse_ohlcv(&data)));
                }
            }
            _ => {}
        }

        None
    }

    /// Private 메시지 처리
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;
        let msg_type = json.get("type").and_then(|v| v.as_str())?;

        match msg_type {
            "myOrder" => {
                if let Ok(data) = serde_json::from_str::<UpbitOrderMsg>(msg) {
                    return Some(WsMessage::Order(Self::parse_order(&data)));
                }
            }
            "myAsset" => {
                if let Ok(data) = serde_json::from_str::<UpbitAssetMsg>(msg) {
                    return Some(WsMessage::Balance(Self::parse_balance(&data)));
                }
            }
            _ => {}
        }

        None
    }

    /// Public 스트림 구독
    async fn subscribe_public_stream(
        &mut self,
        channel: &str,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket 클라이언트 생성 및 연결
        let mut ws_client = WsClient::new(WsConfig {
            url: WS_BASE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

        // Build subscription message
        let codes: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();

        let subscription = json!([
            {
                "ticket": Uuid::new_v4().to_string()
            },
            {
                "type": channel,
                "codes": codes
            }
        ]);

        // Send subscription
        if let Some(client) = &self.ws_client {
            client.send(&subscription.to_string())?;
        }

        // Store subscription
        {
            let key = format!("{}:{}", channel, symbols.join(","));
            self.subscriptions.write().await.insert(
                key,
                SubscriptionRequest {
                    channel_type: channel.to_string(),
                    codes,
                },
            );
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

    /// Private 스트림 구독
    async fn subscribe_private_stream(
        &mut self,
        channel: &str,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let jwt = self.create_jwt()?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket 클라이언트 생성 및 연결
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), format!("Bearer {}", jwt));

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_BASE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.private_ws_client = Some(ws_client);

        // Build subscription message
        let mut subscription_parts = vec![json!({
            "ticket": Uuid::new_v4().to_string()
        })];

        let mut request = json!({
            "type": channel
        });

        if let Some(syms) = symbols {
            let codes: Vec<String> = syms.iter().map(|s| Self::format_symbol(s)).collect();
            request["codes"] = json!(codes);
        }

        subscription_parts.push(request);

        let subscription = serde_json::Value::Array(subscription_parts);

        // Send subscription
        if let Some(client) = &self.private_ws_client {
            client.send(&subscription.to_string())?;
        }

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
}

impl Default for UpbitWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for UpbitWs {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for UpbitWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_public_stream("ticker", &[symbol]).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_public_stream("ticker", symbols).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_public_stream("orderbook", &[symbol]).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_public_stream("trade", &[symbol]).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Upbit only supports 1s candles
        if timeframe != Timeframe::Second1 {
            return Err(CcxtError::NotSupported {
                feature: format!("Timeframe {:?} not supported, only 1s candles available", timeframe),
            });
        }

        let mut client = Self::new();
        client.subscribe_public_stream("candle.1s", &[symbol]).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Upbit connects automatically on subscription
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
            client.is_connected().await
        } else {
            false
        }
    }

    // === Private Streams ===

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("myAsset", None).await
    }

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        let symbols = symbol.map(|s| vec![s]);
        client.subscribe_private_stream("myOrder", symbols.as_deref()).await
    }

    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // myOrder channel also provides trade information
        let mut client = self.clone();
        let symbols = symbol.map(|s| vec![s]);
        client.subscribe_private_stream("myOrder", symbols.as_deref()).await
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        // JWT created on demand in subscribe_private_stream
        Ok(())
    }
}

// === Upbit WebSocket Message Types ===

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct UpbitTickerMsg {
    #[serde(rename = "type")]
    msg_type: String,
    code: String,
    opening_price: Option<Decimal>,
    high_price: Option<Decimal>,
    low_price: Option<Decimal>,
    trade_price: Decimal,
    prev_closing_price: Option<Decimal>,
    #[serde(default)]
    change: Option<String>,
    change_price: Option<Decimal>,
    change_rate: Option<Decimal>,
    signed_change_price: Option<Decimal>,
    signed_change_rate: Option<Decimal>,
    trade_volume: Option<Decimal>,
    acc_trade_price: Option<Decimal>,
    acc_trade_price_24h: Option<Decimal>,
    acc_trade_volume: Option<Decimal>,
    acc_trade_volume_24h: Option<Decimal>,
    highest_52_week_price: Option<Decimal>,
    highest_52_week_date: Option<String>,
    lowest_52_week_price: Option<Decimal>,
    lowest_52_week_date: Option<String>,
    timestamp: Option<i64>,
    #[serde(default)]
    stream_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitOrderBookUnit {
    ask_price: Decimal,
    bid_price: Decimal,
    ask_size: Decimal,
    bid_size: Decimal,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitOrderBookMsg {
    #[serde(rename = "type")]
    msg_type: String,
    code: String,
    timestamp: Option<i64>,
    total_ask_size: Option<Decimal>,
    total_bid_size: Option<Decimal>,
    orderbook_units: Vec<UpbitOrderBookUnit>,
    #[serde(default)]
    stream_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitTradeMsg {
    #[serde(rename = "type")]
    msg_type: String,
    code: String,
    timestamp: Option<i64>,
    trade_date: Option<String>,
    trade_time: Option<String>,
    trade_timestamp: Option<i64>,
    trade_price: Decimal,
    trade_volume: Decimal,
    ask_bid: String,
    prev_closing_price: Option<Decimal>,
    change: Option<String>,
    change_price: Option<Decimal>,
    sequential_id: i64,
    #[serde(default)]
    stream_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitCandleMsg {
    #[serde(rename = "type")]
    msg_type: String,
    code: String,
    candle_date_time_utc: Option<String>,
    candle_date_time_kst: Option<String>,
    opening_price: Decimal,
    high_price: Decimal,
    low_price: Decimal,
    trade_price: Decimal,
    timestamp: Option<i64>,
    candle_acc_trade_volume: Decimal,
    candle_acc_trade_price: Option<Decimal>,
    #[serde(default)]
    stream_type: Option<String>,
}

// === Private WebSocket Message Types ===

#[derive(Debug, Deserialize, Serialize)]
struct UpbitOrderMsg {
    #[serde(rename = "type")]
    msg_type: String,
    code: String,
    uuid: String,
    ask_bid: String,
    order_type: String,
    state: String,
    price: Option<Decimal>,
    avg_price: Option<Decimal>,
    volume: Option<Decimal>,
    remaining_volume: Option<Decimal>,
    executed_volume: Option<Decimal>,
    trades_count: Option<i32>,
    reserved_fee: Option<Decimal>,
    remaining_fee: Option<Decimal>,
    paid_fee: Option<Decimal>,
    locked: Option<Decimal>,
    executed_funds: Option<Decimal>,
    order_timestamp: Option<i64>,
    timestamp: Option<i64>,
    #[serde(default)]
    stream_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitAssetEntry {
    currency: String,
    balance: String,
    locked: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitAssetMsg {
    #[serde(rename = "type")]
    msg_type: String,
    asset_uuid: Option<String>,
    assets: Vec<UpbitAssetEntry>,
    asset_timestamp: Option<i64>,
    timestamp: Option<i64>,
    #[serde(default)]
    stream_type: Option<String>,
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
    fn test_parse_order_status() {
        assert_eq!(UpbitWs::parse_order_status("wait"), OrderStatus::Open);
        assert_eq!(UpbitWs::parse_order_status("done"), OrderStatus::Closed);
        assert_eq!(UpbitWs::parse_order_status("cancel"), OrderStatus::Canceled);
    }
}
