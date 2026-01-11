//! Crypto.com WebSocket Implementation
//!
//! Crypto.com 실시간 데이터 스트리밍 (Public + Private)

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
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Ticker, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage,
    WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_PUBLIC_URL: &str = "wss://stream.crypto.com/exchange/v1/market";
const WS_PRIVATE_URL: &str = "wss://stream.crypto.com/exchange/v1/user";

/// Crypto.com WebSocket 클라이언트
pub struct CryptoComWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    private_ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    authenticated: Arc<RwLock<bool>>,
}

impl CryptoComWs {
    /// 새 Crypto.com WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            config: None,
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            authenticated: Arc::new(RwLock::new(false)),
        }
    }

    /// API 키와 시크릿으로 Crypto.com WebSocket 클라이언트 생성
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            authenticated: Arc::new(RwLock::new(false)),
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

    /// 인증 서명 생성
    fn sign_auth(&self, method: &str, nonce: &str) -> CcxtResult<String> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API config required for authentication".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let secret = config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;

        // Crypto.com signature: method + nonce + api_key + nonce
        let sig_payload = format!("{method}{nonce}{api_key}{nonce}");

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("HMAC error: {e}"),
            })?;
        mac.update(sig_payload.as_bytes());
        let result = mac.finalize();
        Ok(hex::encode(result.into_bytes()))
    }

    /// Private 스트림에 연결하고 이벤트 수신
    async fn subscribe_private_stream(&mut self, channel: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket 클라이언트 생성 및 연결
        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PRIVATE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.private_ws_client = Some(ws_client);

        // 인증 수행
        self.authenticate_ws().await?;

        // 구독 메시지 전송
        let nonce = Utc::now().timestamp_millis();
        let subscribe_msg = serde_json::json!({
            "id": nonce,
            "method": "subscribe",
            "params": {
                "channels": [channel]
            },
            "nonce": nonce
        });

        if let Some(ws_client) = &self.private_ws_client {
            let msg_str = serde_json::to_string(&subscribe_msg)
                .map_err(|e| CcxtError::ParseError {
                    data_type: "JSON".to_string(),
                    message: e.to_string(),
                })?;
            ws_client.send(&msg_str)?;
        }

        // 구독 저장
        {
            let key = format!("private:{channel}");
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

    /// WebSocket 인증 수행
    async fn authenticate_ws(&self) -> CcxtResult<()> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API config required for authentication".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let method = "public/auth";
        let nonce = Utc::now().timestamp_millis().to_string();
        let signature = self.sign_auth(method, &nonce)?;

        let auth_msg = serde_json::json!({
            "id": nonce.parse::<i64>().unwrap_or(0),
            "method": method,
            "api_key": api_key,
            "sig": signature,
            "nonce": nonce.parse::<i64>().unwrap_or(0)
        });

        if let Some(ws_client) = &self.private_ws_client {
            let msg_str = serde_json::to_string(&auth_msg)
                .map_err(|e| CcxtError::ParseError {
                    data_type: "JSON".to_string(),
                    message: e.to_string(),
                })?;
            ws_client.send(&msg_str)?;
        }

        *self.authenticated.write().await = true;
        Ok(())
    }

    /// Private 메시지 처리
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;

        // Heartbeat 응답은 무시
        if json.get("method").and_then(|v| v.as_str()) == Some("public/heartbeat") {
            return None;
        }

        // 인증 응답은 무시
        if json.get("method").and_then(|v| v.as_str()) == Some("public/auth") {
            return None;
        }

        let result = json.get("result")?;
        let channel = result.get("channel")?.as_str()?;

        match channel {
            "user.order" => {
                if let Ok(data) = serde_json::from_value::<CryptoComOrderUpdate>(result.clone()) {
                    return Some(WsMessage::Order(Self::parse_order_update(&data)));
                }
            }
            "user.balance" => {
                if let Ok(data) = serde_json::from_value::<CryptoComBalanceUpdate>(result.clone()) {
                    return Some(WsMessage::Balance(Self::parse_balance_update(&data)));
                }
            }
            _ => {}
        }

        None
    }

    /// 주문 업데이트 파싱
    fn parse_order_update(data: &CryptoComOrderUpdate) -> WsOrderEvent {
        let order = if let Some(o) = data.data.first() {
            let status = match o.status.to_uppercase().as_str() {
                "ACTIVE" | "NEW" | "PENDING" => OrderStatus::Open,
                "FILLED" => OrderStatus::Closed,
                "CANCELED" | "CANCELLED" | "EXPIRED" | "REJECTED" => OrderStatus::Canceled,
                _ => OrderStatus::Open,
            };

            let side = match o.side.to_uppercase().as_str() {
                "BUY" => OrderSide::Buy,
                "SELL" => OrderSide::Sell,
                _ => OrderSide::Buy,
            };

            let order_type = match o.order_type.to_uppercase().as_str() {
                "LIMIT" => OrderType::Limit,
                "MARKET" => OrderType::Market,
                _ => OrderType::Limit,
            };

            let price: Decimal = o.price.parse().unwrap_or_default();
            let amount: Decimal = o.quantity.parse().unwrap_or_default();
            let filled: Decimal = o.cumulative_quantity.parse().unwrap_or_default();
            let cost: Decimal = o.cumulative_value.parse().unwrap_or_default();

            Order {
                id: o.order_id.clone(),
                client_order_id: o.client_oid.clone(),
                timestamp: Some(o.create_time),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(o.create_time)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                last_trade_timestamp: o.update_time,
                last_update_timestamp: o.update_time,
                status,
                symbol: Self::to_unified_symbol(&o.instrument_name),
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
                remaining: Some(amount - filled),
                cost: Some(cost),
                trades: Vec::new(),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(o).unwrap_or_default(),
                stop_price: None,
                trigger_price: None,
                take_profit_price: None,
                stop_loss_price: None,
                reduce_only: None,
                post_only: None,
            }
        } else {
            // 빈 주문 데이터인 경우 기본값 생성
            Order {
                id: String::new(),
                client_order_id: None,
                timestamp: None,
                datetime: None,
                last_trade_timestamp: None,
                last_update_timestamp: None,
                status: OrderStatus::Open,
                symbol: String::new(),
                order_type: OrderType::Limit,
                time_in_force: None,
                side: OrderSide::Buy,
                price: None,
                average: None,
                amount: Decimal::ZERO,
                filled: Decimal::ZERO,
                remaining: None,
                cost: None,
                trades: Vec::new(),
                fee: None,
                fees: Vec::new(),
                info: serde_json::Value::Null,
                stop_price: None,
                trigger_price: None,
                take_profit_price: None,
                stop_loss_price: None,
                reduce_only: None,
                post_only: None,
            }
        };

        WsOrderEvent { order }
    }

    /// 잔고 업데이트 파싱
    fn parse_balance_update(data: &CryptoComBalanceUpdate) -> WsBalanceEvent {
        let mut balances = Balances::default();

        for item in &data.data {
            for balance in &item.position_balances {
                let currency = balance.instrument_name.clone();
                let total: Decimal = balance.quantity.parse().unwrap_or_default();
                let used: Decimal = balance.reserved_qty.parse().unwrap_or_default();
                let free = total - used;

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
        }

        balances.info = serde_json::to_value(data).unwrap_or_default();
        WsBalanceEvent { balances }
    }

    /// 심볼을 Crypto.com 형식으로 변환 (BTC/USDT -> BTC_USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Crypto.com 심볼을 통합 심볼로 변환 (BTC_USDT -> BTC/USDT)
    fn to_unified_symbol(cryptocom_symbol: &str) -> String {
        cryptocom_symbol.replace("_", "/")
    }

    /// Timeframe을 Crypto.com interval로 변환
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
            Timeframe::Day1 => "1D",
            Timeframe::Week1 => "7D",
            Timeframe::Month1 => "1M",
            _ => "1h",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &CryptoComTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.i);
        let timestamp = data.t;

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
            bid: data.b.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bs.as_ref().and_then(|v| v.parse().ok()),
            ask: data.k.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ks.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: None,
            close: data.a.as_ref().and_then(|v| v.parse().ok()),
            last: data.a.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: data.c.as_ref().and_then(|v| v.parse::<f64>().ok())
                .map(|p| Decimal::from_f64_retain(p * 100.0).unwrap_or_default()),
            average: None,
            base_volume: data.v.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.vv.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &CryptoComOrderBookData, symbol: &str) -> WsOrderBookEvent {
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

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(data.t),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: Some(data.s),
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
    fn parse_trade(data: &CryptoComTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.i);
        let timestamp = data.t;

        let trade = Trade {
            id: data.d.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(data.s.to_lowercase()),
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

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;

        // Heartbeat 응답 처리
        if json.get("method").and_then(|v| v.as_str()) == Some("public/heartbeat") {
            return None; // Heartbeat는 pong으로 응답해야 함 (별도 처리)
        }

        let result = json.get("result")?;
        let channel = result.get("channel")?.as_str()?;

        match channel {
            "ticker" => {
                let data = result.get("data")?.as_array()?.first()?;
                if let Ok(ticker_data) = serde_json::from_value::<CryptoComTickerData>(data.clone()) {
                    return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                }
            }
            "book" | "book.update" => {
                let data = result.get("data")?.as_array()?.first()?;
                if let Ok(book_data) = serde_json::from_value::<CryptoComOrderBookData>(data.clone()) {
                    let instrument_name = result.get("instrument_name")?.as_str()?;
                    let symbol = Self::to_unified_symbol(instrument_name);
                    return Some(WsMessage::OrderBook(Self::parse_order_book(&book_data, &symbol)));
                }
            }
            "trade" => {
                let data_array = result.get("data")?.as_array()?;
                let mut all_trades = Vec::new();
                for data in data_array {
                    if let Ok(trade_data) = serde_json::from_value::<CryptoComTradeData>(data.clone()) {
                        let event = Self::parse_trade(&trade_data);
                        all_trades.extend(event.trades);
                    }
                }
                if !all_trades.is_empty() {
                    let symbol = all_trades[0].symbol.clone();
                    return Some(WsMessage::Trade(WsTradeEvent {
                        symbol,
                        trades: all_trades,
                    }));
                }
            }
            _ => {}
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, channel: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket URL 구성
        let url = WS_PUBLIC_URL.to_string();

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

        // 구독 메시지 전송
        let nonce = Utc::now().timestamp_millis();
        let subscribe_msg = serde_json::json!({
            "id": nonce,
            "method": "subscribe",
            "params": {
                "channels": [channel]
            },
            "nonce": nonce
        });

        let msg_str = serde_json::to_string(&subscribe_msg)
            .map_err(|e| CcxtError::ParseError {
                data_type: "JSON".to_string(),
                message: e.to_string(),
            })?;
        ws_client.send(&msg_str)?;
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
}

impl Default for CryptoComWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for CryptoComWs {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            authenticated: Arc::clone(&self.authenticated),
        }
    }
}

#[async_trait]
impl WsExchange for CryptoComWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let channel = format!("ticker.{}", Self::format_symbol(symbol));
        client.subscribe_stream(&channel, Some(symbol)).await
    }

    async fn watch_tickers(&self, _symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Crypto.com doesn't support multiple ticker subscriptions in one message
        // This would require multiple subscriptions or use watch_ticker for each symbol
        Err(CcxtError::NotSupported {
            feature: "watch_tickers not supported, use watch_ticker for each symbol".to_string(),
        })
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let depth = limit.unwrap_or(50);
        let channel = format!("book.{}.{}", Self::format_symbol(symbol), depth);
        client.subscribe_stream(&channel, Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let channel = format!("trade.{}", Self::format_symbol(symbol));
        client.subscribe_stream(&channel, Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let interval = Self::format_interval(timeframe);
        let channel = format!("candlestick.{}.{}", interval, Self::format_symbol(symbol));
        client.subscribe_stream(&channel, Some(symbol)).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Crypto.com은 구독시 자동 연결
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
        client.subscribe_private_stream("user.balance").await
    }

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        let channel = if let Some(sym) = symbol {
            format!("user.order.{}", Self::format_symbol(sym))
        } else {
            "user.order".to_string()
        };
        client.subscribe_private_stream(&channel).await
    }

    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        let channel = if let Some(sym) = symbol {
            format!("user.trade.{}", Self::format_symbol(sym))
        } else {
            "user.trade".to_string()
        };
        client.subscribe_private_stream(&channel).await
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        self.authenticate_ws().await
    }
}

// === Crypto.com WebSocket Message Types ===

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComTickerData {
    i: String,  // instrument name
    #[serde(default)]
    h: Option<String>,  // high
    #[serde(default)]
    l: Option<String>,  // low
    #[serde(default)]
    a: Option<String>,  // last price
    #[serde(default)]
    v: Option<String>,  // 24h volume
    #[serde(default)]
    vv: Option<String>,  // 24h volume in quote
    #[serde(default)]
    c: Option<String>,  // 24h price change
    #[serde(default)]
    b: Option<String>,  // best bid
    #[serde(default)]
    bs: Option<String>,  // best bid size
    #[serde(default)]
    k: Option<String>,  // best ask
    #[serde(default)]
    ks: Option<String>,  // best ask size
    t: i64,  // timestamp
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComOrderBookData {
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
    t: i64,  // timestamp
    s: i64,  // sequence number
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComTradeData {
    d: String,  // trade id
    s: String,  // side
    p: String,  // price
    q: String,  // quantity
    t: i64,     // timestamp
    i: String,  // instrument name
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComOrderUpdate {
    channel: String,
    data: Vec<CryptoComOrderData>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComOrderData {
    order_id: String,
    client_oid: Option<String>,
    instrument_name: String,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    price: String,
    quantity: String,
    cumulative_quantity: String,
    cumulative_value: String,
    status: String,
    create_time: i64,
    update_time: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComBalanceUpdate {
    channel: String,
    data: Vec<CryptoComBalanceData>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComBalanceData {
    position_balances: Vec<CryptoComPositionBalance>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComPositionBalance {
    instrument_name: String,
    quantity: String,
    reserved_qty: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(CryptoComWs::format_symbol("BTC/USDT"), "BTC_USDT");
        assert_eq!(CryptoComWs::format_symbol("ETH/BTC"), "ETH_BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(CryptoComWs::to_unified_symbol("BTC_USDT"), "BTC/USDT");
        assert_eq!(CryptoComWs::to_unified_symbol("ETH_BTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(CryptoComWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(CryptoComWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(CryptoComWs::format_interval(Timeframe::Day1), "1D");
    }

    #[test]
    fn test_with_credentials() {
        let ws = CryptoComWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
        let config = ws.config.unwrap();
        assert_eq!(config.api_key(), Some("test_key"));
        assert_eq!(config.secret(), Some("test_secret"));
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = CryptoComWs::new();
        assert!(ws.config.is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
        let config = ws.config.unwrap();
        assert_eq!(config.api_key(), Some("test_key"));
        assert_eq!(config.secret(), Some("test_secret"));
    }
}
