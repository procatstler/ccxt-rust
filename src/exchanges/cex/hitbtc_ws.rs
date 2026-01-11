//! HitBTC WebSocket Implementation
//!
//! HitBTC 실시간 데이터 스트리밍
//!
//! Private channels (authenticated):
//! - spot/reports: Order updates
//! - spot/balance: Balance updates

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Ticker,
    TimeInForce, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsOhlcvEvent,
    WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent, OHLCV,
};

const WS_PUBLIC_URL: &str = "wss://api.hitbtc.com/api/3/ws/public";
const WS_PRIVATE_URL: &str = "wss://api.hitbtc.com/api/3/ws/trading";

/// HitBTC WebSocket 클라이언트
pub struct HitbtcWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    request_id: Arc<AtomicI64>,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl HitbtcWs {
    /// 새 HitBTC WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(AtomicI64::new(1)),
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
            request_id: Arc::new(AtomicI64::new(1)),
            api_key: Some(api_key),
            api_secret: Some(api_secret),
        }
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
    }

    /// 다음 요청 ID 생성
    fn next_id(&self) -> i64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// 심볼을 HitBTC 형식으로 변환 (BTC/USDT -> BTCUSDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// HitBTC 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(hitbtc_symbol: &str) -> String {
        // 일반적인 quote 통화 목록
        let quotes = [
            "USDT", "BUSD", "USDC", "BTC", "ETH", "BNB", "TUSD", "EUR", "USD",
        ];

        for quote in quotes {
            if let Some(base) = hitbtc_symbol.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }

        // 변환할 수 없으면 원본 반환
        hitbtc_symbol.to_string()
    }

    /// Timeframe을 HitBTC 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "M1",
            Timeframe::Minute3 => "M3",
            Timeframe::Minute5 => "M5",
            Timeframe::Minute15 => "M15",
            Timeframe::Minute30 => "M30",
            Timeframe::Hour1 => "H1",
            Timeframe::Hour4 => "H4",
            Timeframe::Day1 => "D1",
            Timeframe::Week1 => "D7",
            Timeframe::Month1 => "1M",
            _ => "M1",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &HitbtcTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data
            .timestamp
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: data.timestamp.clone(),
            high: data.high.as_ref().and_then(|v| v.parse().ok()),
            low: data.low.as_ref().and_then(|v| v.parse().ok()),
            bid: data.best_bid_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.best_bid_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.best_ask_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.best_ask_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.volume_quote.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &HitbtcOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data
            .bid
            .iter()
            .filter_map(|b| {
                Some(OrderBookEntry {
                    price: b.price.parse().ok()?,
                    amount: b.size.parse().ok()?,
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .ask
            .iter()
            .filter_map(|a| {
                Some(OrderBookEntry {
                    price: a.price.parse().ok()?,
                    amount: a.size.parse().ok()?,
                })
            })
            .collect();

        let timestamp = data
            .timestamp
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: data.timestamp.clone(),
            nonce: data.sequence,
            bids,
            asks,
            checksum: None,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot: false,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &HitbtcTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data
            .timestamp
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.quantity.parse().unwrap_or_default();

        let trade = Trade {
            id: data.id.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: data.timestamp.clone(),
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
        };

        WsTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    /// OHLCV 메시지 파싱
    fn parse_candle(data: &HitbtcCandleData, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data
            .timestamp
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())?;

        let ohlcv = OHLCV::new(
            timestamp,
            data.open.parse().ok()?,
            data.max.parse().ok()?,
            data.min.parse().ok()?,
            data.close.parse().ok()?,
            data.volume
                .as_ref()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
        );

        Some(WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        })
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // JSON-RPC 응답 파싱
        if let Ok(response) = serde_json::from_str::<HitbtcWsResponse>(msg) {
            // 에러 처리
            if let Some(error) = response.error {
                return Some(WsMessage::Error(format!(
                    "{}: {}",
                    error.code, error.message
                )));
            }

            // 결과 처리
            if let Some(result) = response.result {
                // Ticker
                if let Ok(ticker_data) = serde_json::from_value::<HitbtcTickerData>(result.clone())
                {
                    return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                }

                // OrderBook
                if let Ok(ob_data) = serde_json::from_value::<HitbtcOrderBookData>(result.clone()) {
                    let symbol = Self::to_unified_symbol(&ob_data.symbol);
                    return Some(WsMessage::OrderBook(Self::parse_order_book(
                        &ob_data, &symbol,
                    )));
                }

                // Trade
                if let Ok(trade_data) = serde_json::from_value::<HitbtcTradeData>(result.clone()) {
                    return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                }

                // Candle
                if let Ok(candle_data) = serde_json::from_value::<HitbtcCandleData>(result.clone())
                {
                    // 기본 timeframe (실제로는 구독 시 저장한 값 사용)
                    let timeframe = Timeframe::Minute1;
                    if let Some(event) = Self::parse_candle(&candle_data, timeframe) {
                        return Some(WsMessage::Ohlcv(event));
                    }
                }
            }

            // 채널 업데이트 처리
            if let Some(ch) = response.ch {
                if let Some(data) = response.data {
                    // Ticker update
                    if ch.starts_with("ticker/") {
                        if let Ok(ticker_data) =
                            serde_json::from_value::<HitbtcTickerData>(data.clone())
                        {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }

                    // Trades update
                    if ch == "trades" {
                        if let Ok(trade_data) =
                            serde_json::from_value::<HitbtcTradeData>(data.clone())
                        {
                            return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                        }
                    }

                    // OrderBook update
                    if ch.starts_with("orderbook/") {
                        if let Ok(ob_data) =
                            serde_json::from_value::<HitbtcOrderBookData>(data.clone())
                        {
                            let symbol = Self::to_unified_symbol(&ob_data.symbol);
                            return Some(WsMessage::OrderBook(Self::parse_order_book(
                                &ob_data, &symbol,
                            )));
                        }
                    }

                    // Candles update
                    if ch.starts_with("candles/") {
                        if let Ok(candle_data) =
                            serde_json::from_value::<HitbtcCandleData>(data.clone())
                        {
                            // Extract timeframe from channel name
                            let timeframe = if let Some(period) = ch.strip_prefix("candles/") {
                                match period {
                                    "M1" => Timeframe::Minute1,
                                    "M3" => Timeframe::Minute3,
                                    "M5" => Timeframe::Minute5,
                                    "M15" => Timeframe::Minute15,
                                    "M30" => Timeframe::Minute30,
                                    "H1" => Timeframe::Hour1,
                                    "H4" => Timeframe::Hour4,
                                    "D1" => Timeframe::Day1,
                                    "D7" => Timeframe::Week1,
                                    "1M" => Timeframe::Month1,
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
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        symbols: Vec<String>,
        channel_name: &str,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

        // 구독 메시지 전송
        let subscribe_msg = HitbtcSubscribe {
            method: "subscribe".to_string(),
            ch: channel.to_string(),
            params: HitbtcSubscribeParams {
                symbols: Some(symbols.clone()),
            },
            id: self.next_id(),
        };

        let subscribe_json =
            serde_json::to_string(&subscribe_msg).map_err(|e| CcxtError::ParseError {
                data_type: "HitbtcSubscribe".to_string(),
                message: e.to_string(),
            })?;

        // 구독 저장
        {
            let key = format!("{}:{}", channel_name, symbol.unwrap_or(""));
            self.subscriptions
                .write()
                .await
                .insert(key, channel.to_string());
        }

        // WebSocket으로 구독 메시지 전송
        if let Some(ws_client) = &self.ws_client {
            ws_client.send(&subscribe_json)?;
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

    /// Private 채널 구독 및 이벤트 스트림 반환
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

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PRIVATE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

        // Basic authentication: Base64(api_key:api_secret)
        let auth_string = format!("{api_key}:{api_secret}");
        let auth_token = BASE64.encode(auth_string.as_bytes());

        // Login 메시지 전송
        let login_msg = serde_json::json!({
            "method": "login",
            "params": {
                "type": "BASIC",
                "token": auth_token
            },
            "id": self.next_id()
        });

        let login_json = serde_json::to_string(&login_msg).map_err(|e| CcxtError::ParseError {
            data_type: "HitbtcLogin".to_string(),
            message: e.to_string(),
        })?;

        if let Some(ws) = &self.ws_client {
            ws.send(&login_json)?;
        }

        // 구독 메시지 전송
        let subscribe_msg = if let Some(sym) = symbol {
            let market_symbol = Self::format_symbol(sym);
            serde_json::json!({
                "method": "subscribe",
                "ch": channel,
                "params": {
                    "symbols": [market_symbol]
                },
                "id": self.next_id()
            })
        } else {
            serde_json::json!({
                "method": "subscribe",
                "ch": channel,
                "params": {},
                "id": self.next_id()
            })
        };

        let subscribe_json =
            serde_json::to_string(&subscribe_msg).map_err(|e| CcxtError::ParseError {
                data_type: "HitbtcPrivateSubscribe".to_string(),
                message: e.to_string(),
            })?;

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions
                .write()
                .await
                .insert(key, channel.to_string());
        }

        // WebSocket으로 구독 메시지 전송
        if let Some(ws_client) = &self.ws_client {
            ws_client.send(&subscribe_json)?;
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
                        // Try private message processing first, then public
                        if let Some(ws_msg) = Self::process_private_message(&msg) {
                            let _ = tx.send(ws_msg);
                        } else if let Some(ws_msg) = Self::process_message(&msg) {
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

    /// Private 메시지 처리
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        let response: serde_json::Value = serde_json::from_str(msg).ok()?;

        // Error check
        if let Some(error) = response.get("error") {
            let code = error.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
            let message = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            return Some(WsMessage::Error(format!("{code}: {message}")));
        }

        let channel = response.get("ch").and_then(|v| v.as_str())?;
        let data = response.get("data")?;

        match channel {
            "spot/order" => {
                // Order update
                if let Ok(order_data) =
                    serde_json::from_value::<HitbtcOrderUpdateData>(data.clone())
                {
                    let order = Self::parse_order(&order_data);
                    return Some(WsMessage::Order(WsOrderEvent { order }));
                }
            },
            "spot/balance" => {
                // Balance update
                if let Ok(balance_data) =
                    serde_json::from_value::<Vec<HitbtcBalanceUpdateData>>(data.clone())
                {
                    let balances = Self::parse_balances(&balance_data);
                    return Some(WsMessage::Balance(WsBalanceEvent { balances }));
                } else if let Ok(balance_data) =
                    serde_json::from_value::<HitbtcBalanceUpdateData>(data.clone())
                {
                    let balances = Self::parse_balances(&[balance_data]);
                    return Some(WsMessage::Balance(WsBalanceEvent { balances }));
                }
            },
            "reports" => {
                // Trade report (my trades)
                if let Ok(order_data) =
                    serde_json::from_value::<HitbtcOrderUpdateData>(data.clone())
                {
                    let order = Self::parse_order(&order_data);
                    return Some(WsMessage::Order(WsOrderEvent { order }));
                }
            },
            _ => {},
        }

        None
    }

    /// Order 파싱
    fn parse_order(data: &HitbtcOrderUpdateData) -> Order {
        let symbol = data
            .symbol
            .as_ref()
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let timestamp = data
            .created_at
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.status.as_deref() {
            Some("new") => OrderStatus::Open,
            Some("suspended") => OrderStatus::Open,
            Some("partiallyFilled") => OrderStatus::Open,
            Some("filled") => OrderStatus::Closed,
            Some("canceled") => OrderStatus::Canceled,
            Some("expired") => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            Some("stopLimit") => OrderType::Limit,
            Some("stopMarket") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = data
            .time_in_force
            .as_ref()
            .and_then(|tif| match tif.as_str() {
                "GTC" => Some(TimeInForce::GTC),
                "IOC" => Some(TimeInForce::IOC),
                "FOK" => Some(TimeInForce::FOK),
                "GTT" => Some(TimeInForce::GTT),
                "PO" => Some(TimeInForce::PO),
                _ => None,
            });

        let price: Decimal = data
            .price
            .as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data
            .quantity
            .as_ref()
            .and_then(|q| q.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data
            .cumulative_quantity
            .as_ref()
            .and_then(|q| q.parse().ok())
            .unwrap_or_default();
        let remaining = amount - filled;
        let cost = price * filled;

        Order {
            id: data.client_order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: data.created_at.clone(),
            last_trade_timestamp: data
                .updated_at
                .as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis()),
            last_update_timestamp: data
                .updated_at
                .as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis()),
            status,
            symbol,
            order_type,
            time_in_force,
            side,
            price: Some(price),
            trigger_price: None,
            average: data.average_price.as_ref().and_then(|p| p.parse().ok()),
            amount,
            filled,
            remaining: Some(remaining),
            stop_price: None,
            cost: Some(cost),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: data.post_only,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Balances 파싱
    fn parse_balances(data: &[HitbtcBalanceUpdateData]) -> Balances {
        let mut balances = Balances::new();

        for item in data {
            if let Some(currency) = &item.currency {
                let free: Decimal = item
                    .available
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
                let used: Decimal = item
                    .reserved
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
                let total = free + used;

                let balance = Balance {
                    free: Some(free),
                    used: Some(used),
                    total: Some(total),
                    debt: Some(Decimal::ZERO),
                };
                balances.add(currency.clone(), balance);
            }
        }

        balances
    }
}

impl Default for HitbtcWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for HitbtcWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(AtomicI64::new(1)),
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
        }
    }
}

#[async_trait]
impl WsExchange for HitbtcWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_symbol = Self::format_symbol(symbol);
        client
            .subscribe_stream("ticker/1s", vec![market_symbol], "ticker", Some(symbol))
            .await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_symbols: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        client
            .subscribe_stream("ticker/1s", market_symbols, "tickers", None)
            .await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_symbol = Self::format_symbol(symbol);
        client
            .subscribe_stream(
                "orderbook/full",
                vec![market_symbol],
                "orderBook",
                Some(symbol),
            )
            .await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_symbol = Self::format_symbol(symbol);
        client
            .subscribe_stream("trades", vec![market_symbol], "trades", Some(symbol))
            .await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let channel = format!("candles/{interval}");
        client
            .subscribe_stream(&channel, vec![market_symbol], "ohlcv", Some(symbol))
            .await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // HitBTC는 구독시 자동 연결
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

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required for authentication".to_string(),
            })?;
        let api_secret =
            self.api_secret
                .as_ref()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API secret required for authentication".to_string(),
                })?;

        // Basic authentication: Base64(api_key:api_secret)
        let auth_string = format!("{api_key}:{api_secret}");
        let auth_token = BASE64.encode(auth_string.as_bytes());

        // Login 메시지 전송
        let login_msg = serde_json::json!({
            "method": "login",
            "params": {
                "type": "BASIC",
                "token": auth_token
            },
            "id": self.next_id()
        });

        let login_json = serde_json::to_string(&login_msg).map_err(|e| CcxtError::ParseError {
            data_type: "HitbtcLogin".to_string(),
            message: e.to_string(),
        })?;

        if let Some(ws) = &self.ws_client {
            ws.send(&login_json)?;
        }

        Ok(())
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(AtomicI64::new(1)),
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
        };
        client.subscribe_private_stream("spot/balance", None).await
    }

    async fn watch_orders(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(AtomicI64::new(1)),
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
        };
        client.subscribe_private_stream("spot/order", symbol).await
    }

    async fn watch_my_trades(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(AtomicI64::new(1)),
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
        };
        client.subscribe_private_stream("reports", symbol).await
    }
}

// === HitBTC WebSocket Message Types ===

// Private channel data types
#[derive(Debug, Deserialize, Serialize, Clone)]
struct HitbtcOrderUpdateData {
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default, rename = "timeInForce")]
    time_in_force: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default, rename = "cumulativeQuantity")]
    cumulative_quantity: Option<String>,
    #[serde(default, rename = "tradePrice")]
    trade_price: Option<String>,
    #[serde(default, rename = "tradeQuantity")]
    trade_quantity: Option<String>,
    #[serde(default, rename = "averagePrice")]
    average_price: Option<String>,
    #[serde(default, rename = "postOnly")]
    post_only: Option<bool>,
    #[serde(default, rename = "createdAt")]
    created_at: Option<String>,
    #[serde(default, rename = "updatedAt")]
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct HitbtcBalanceUpdateData {
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    reserved: Option<String>,
}

#[derive(Debug, Serialize)]
struct HitbtcSubscribe {
    method: String,
    ch: String,
    params: HitbtcSubscribeParams,
    id: i64,
}

#[derive(Debug, Serialize)]
struct HitbtcSubscribeParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    symbols: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct HitbtcWsResponse {
    #[serde(default)]
    ch: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<HitbtcError>,
    #[serde(default)]
    id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct HitbtcError {
    code: i32,
    message: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct HitbtcTickerData {
    symbol: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    volume_quote: Option<String>,
    #[serde(default, rename = "best_bid_price")]
    best_bid_price: Option<String>,
    #[serde(default, rename = "best_bid_size")]
    best_bid_size: Option<String>,
    #[serde(default, rename = "best_ask_price")]
    best_ask_price: Option<String>,
    #[serde(default, rename = "best_ask_size")]
    best_ask_size: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HitbtcOrderBookLevel {
    price: String,
    size: String,
}

#[derive(Debug, Deserialize)]
struct HitbtcOrderBookData {
    symbol: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    sequence: Option<i64>,
    #[serde(default)]
    ask: Vec<HitbtcOrderBookLevel>,
    #[serde(default)]
    bid: Vec<HitbtcOrderBookLevel>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HitbtcTradeData {
    #[serde(default)]
    id: i64,
    symbol: String,
    price: String,
    quantity: String,
    side: String,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HitbtcCandleData {
    symbol: String,
    #[serde(default)]
    timestamp: Option<String>,
    open: String,
    close: String,
    min: String,
    max: String,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    volume_quote: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(HitbtcWs::format_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(HitbtcWs::format_symbol("ETH/BTC"), "ETHBTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(HitbtcWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(HitbtcWs::to_unified_symbol("ETHBTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(HitbtcWs::format_interval(Timeframe::Minute1), "M1");
        assert_eq!(HitbtcWs::format_interval(Timeframe::Hour1), "H1");
        assert_eq!(HitbtcWs::format_interval(Timeframe::Day1), "D1");
    }

    #[test]
    fn test_with_credentials() {
        let client = HitbtcWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert_eq!(client.api_key, Some("test_key".to_string()));
        assert_eq!(client.api_secret, Some("test_secret".to_string()));
    }

    #[test]
    fn test_set_credentials() {
        let mut client = HitbtcWs::new();
        assert!(client.api_key.is_none());
        assert!(client.api_secret.is_none());

        client.set_credentials("api_key".to_string(), "api_secret".to_string());
        assert_eq!(client.api_key, Some("api_key".to_string()));
        assert_eq!(client.api_secret, Some("api_secret".to_string()));
    }

    #[test]
    fn test_basic_auth_token() {
        let api_key = "test_key";
        let api_secret = "test_secret";
        let auth_string = format!("{api_key}:{api_secret}");
        let auth_token = BASE64.encode(auth_string.as_bytes());

        // Verify Base64 encoding
        let decoded = BASE64.decode(&auth_token).unwrap();
        let decoded_str = String::from_utf8(decoded).unwrap();
        assert_eq!(decoded_str, "test_key:test_secret");
    }

    #[test]
    fn test_parse_order() {
        let order_data = HitbtcOrderUpdateData {
            client_order_id: Some("order123".to_string()),
            symbol: Some("BTCUSDT".to_string()),
            side: Some("buy".to_string()),
            status: Some("new".to_string()),
            order_type: Some("limit".to_string()),
            time_in_force: Some("GTC".to_string()),
            price: Some("50000.0".to_string()),
            quantity: Some("1.0".to_string()),
            cumulative_quantity: Some("0.0".to_string()),
            trade_price: None,
            trade_quantity: None,
            average_price: None,
            post_only: Some(false),
            created_at: Some("2024-01-01T00:00:00Z".to_string()),
            updated_at: None,
        };

        let order = HitbtcWs::parse_order(&order_data);
        assert_eq!(order.id, "order123");
        assert_eq!(order.symbol, "BTC/USDT");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.status, OrderStatus::Open);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.price, Some(Decimal::from(50000)));
        assert_eq!(order.amount, Decimal::from(1));
    }

    #[test]
    fn test_parse_balances() {
        let balance_data = vec![
            HitbtcBalanceUpdateData {
                currency: Some("BTC".to_string()),
                available: Some("1.5".to_string()),
                reserved: Some("0.5".to_string()),
            },
            HitbtcBalanceUpdateData {
                currency: Some("ETH".to_string()),
                available: Some("10.0".to_string()),
                reserved: Some("0.0".to_string()),
            },
        ];

        let balances = HitbtcWs::parse_balances(&balance_data);

        let btc = balances.get("BTC").unwrap();
        assert_eq!(btc.free, Some(Decimal::new(15, 1))); // 1.5
        assert_eq!(btc.used, Some(Decimal::new(5, 1))); // 0.5
        assert_eq!(btc.total, Some(Decimal::from(2)));

        let eth = balances.get("ETH").unwrap();
        assert_eq!(eth.free, Some(Decimal::from(10)));
        assert_eq!(eth.used, Some(Decimal::ZERO));
        assert_eq!(eth.total, Some(Decimal::from(10)));
    }

    #[test]
    fn test_process_private_order_message() {
        let msg = r#"{
            "ch": "spot/order",
            "data": {
                "client_order_id": "test_order",
                "symbol": "ETHBTC",
                "side": "sell",
                "status": "filled",
                "type": "market",
                "quantity": "2.0",
                "cumulativeQuantity": "2.0"
            }
        }"#;

        let result = HitbtcWs::process_private_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.symbol, "ETH/BTC");
            assert_eq!(event.order.side, OrderSide::Sell);
            assert_eq!(event.order.status, OrderStatus::Closed);
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_private_balance_message() {
        let msg = r#"{
            "ch": "spot/balance",
            "data": [
                {
                    "currency": "USDT",
                    "available": "1000.0",
                    "reserved": "100.0"
                }
            ]
        }"#;

        let result = HitbtcWs::process_private_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::Balance(event)) = result {
            let usdt = event.balances.get("USDT").unwrap();
            assert_eq!(usdt.free, Some(Decimal::from(1000)));
            assert_eq!(usdt.used, Some(Decimal::from(100)));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_clone_preserves_credentials() {
        let client = HitbtcWs::with_credentials("key".to_string(), "secret".to_string());
        let cloned = client.clone();

        assert_eq!(cloned.api_key, Some("key".to_string()));
        assert_eq!(cloned.api_secret, Some("secret".to_string()));
    }
}
