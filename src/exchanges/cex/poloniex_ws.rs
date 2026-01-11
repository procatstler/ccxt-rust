//! Poloniex WebSocket Implementation
//!
//! Poloniex 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    TakerOrMaker, Ticker, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsMyTradeEvent,
    WsOhlcvEvent, WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

const WS_PUBLIC_URL: &str = "wss://ws.poloniex.com/ws/public";
const WS_PRIVATE_URL: &str = "wss://ws.poloniex.com/ws/private";

/// Poloniex WebSocket 클라이언트
pub struct PoloniexWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    private_ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    authenticated: Arc<RwLock<bool>>,
}

impl Clone for PoloniexWs {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            authenticated: Arc::new(RwLock::new(false)),
        }
    }
}

impl PoloniexWs {
    /// 새 Poloniex WebSocket 클라이언트 생성
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

    /// 설정으로 초기화
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

    /// 심볼을 Poloniex 형식으로 변환 (BTC/USDT -> BTC_USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Poloniex 심볼을 통합 심볼로 변환 (BTC_USDT -> BTC/USDT)
    fn to_unified_symbol(poloniex_symbol: &str) -> String {
        poloniex_symbol.replace("_", "/")
    }

    /// Timeframe을 Poloniex 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "MINUTE_1",
            Timeframe::Minute5 => "MINUTE_5",
            Timeframe::Minute15 => "MINUTE_15",
            Timeframe::Minute30 => "MINUTE_30",
            Timeframe::Hour1 => "HOUR_1",
            Timeframe::Hour2 => "HOUR_2",
            Timeframe::Hour4 => "HOUR_4",
            Timeframe::Hour6 => "HOUR_6",
            Timeframe::Hour12 => "HOUR_12",
            Timeframe::Day1 => "DAY_1",
            Timeframe::Day3 => "DAY_3",
            Timeframe::Week1 => "WEEK_1",
            _ => "MINUTE_1",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &PoloniexTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            bid: data.bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_quantity.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_quantity.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.close.as_ref().and_then(|v| v.parse().ok()),
            last: data.close.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.quantity.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.amount.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: data.mark_price.as_ref().and_then(|v| v.parse().ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(
        data: &PoloniexOrderBookData,
        symbol: &str,
        is_snapshot: bool,
    ) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|b| {
                if b.len() >= 2 {
                    Some(OrderBookEntry {
                        price: b[0].parse().ok()?,
                        amount: b[1].parse().ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|a| {
                if a.len() >= 2 {
                    Some(OrderBookEntry {
                        price: a[0].parse().ok()?,
                        amount: a[1].parse().ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            checksum: None,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &PoloniexTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.quantity.parse().unwrap_or_default();

        let trades = vec![Trade {
            id: data.id.clone(),
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
    fn parse_candle(data: &PoloniexCandleData, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let timestamp = data.ts?;
        let ohlcv = OHLCV::new(
            timestamp,
            data.open.parse().ok()?,
            data.high.parse().ok()?,
            data.low.parse().ok()?,
            data.close.parse().ok()?,
            data.quantity.parse().ok()?,
        );

        let symbol = Self::to_unified_symbol(&data.symbol);

        Some(WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        })
    }

    /// 인증 서명 생성 (HMAC-SHA256 + Base64)
    fn generate_auth_signature(api_secret: &str, timestamp: i64) -> CcxtResult<String> {
        let message = format!("GET\n/ws\nsignTimestamp={timestamp}");
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(message.as_bytes());
        Ok(BASE64.encode(mac.finalize().into_bytes()))
    }

    /// Private 채널 구독
    async fn subscribe_private_stream(
        &mut self,
        channel: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let config = self.config.as_ref().ok_or(CcxtError::AuthenticationError {
            message: "API credentials required for private channels".into(),
        })?;

        let api_key = config.api_key().ok_or(CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let api_secret = config.secret().ok_or(CcxtError::AuthenticationError {
            message: "API secret required".into(),
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
        self.private_ws_client = Some(ws_client);

        // 인증 메시지 전송
        let timestamp = Utc::now().timestamp_millis();
        let signature = Self::generate_auth_signature(api_secret, timestamp)?;

        let auth_msg = serde_json::json!({
            "event": "subscribe",
            "channel": [channel],
            "params": {
                "key": api_key,
                "signTimestamp": timestamp,
                "signature": signature
            }
        });

        if let Some(ws_client) = &mut self.private_ws_client {
            ws_client.send(&auth_msg.to_string())?;
        }

        *self.authenticated.write().await = true;

        // 구독 저장
        {
            self.subscriptions
                .write()
                .await
                .insert(channel.to_string(), channel.to_string());
        }

        // 메시지 핸들러
        let event_tx_clone = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_private_message(&msg) {
                            let _ = event_tx_clone.send(ws_msg);
                        }
                    },
                    WsEvent::Connected => {
                        let _ = event_tx_clone.send(WsMessage::Connected);
                    },
                    WsEvent::Disconnected => {
                        let _ = event_tx_clone.send(WsMessage::Disconnected);
                    },
                    WsEvent::Error(e) => {
                        let _ = event_tx_clone.send(WsMessage::Error(e));
                    },
                    _ => {},
                }
            }
        });

        Ok(event_rx)
    }

    /// Private 메시지 처리
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<PoloniexWsResponse>(msg) {
            // 인증/구독 확인 이벤트
            if let Some(event) = &response.event {
                if event == "subscribe" {
                    return Some(WsMessage::Subscribed {
                        channel: response
                            .channel
                            .clone()
                            .unwrap_or_default()
                            .into_iter()
                            .next()
                            .unwrap_or_default(),
                        symbol: None,
                    });
                }
            }

            // 데이터 처리
            if let (Some(channel), Some(data)) = (&response.channel, &response.data) {
                let channel_name = channel.first().map(|s| s.as_str()).unwrap_or("");

                // Balance updates
                if channel_name == "balances" {
                    if let Ok(balance_data) =
                        serde_json::from_value::<Vec<PoloniexBalanceData>>(data.clone())
                    {
                        return Some(WsMessage::Balance(Self::parse_balance_update(
                            &balance_data,
                        )));
                    }
                }

                // Order updates
                if channel_name == "orders" {
                    if let Ok(order_data) =
                        serde_json::from_value::<Vec<PoloniexOrderData>>(data.clone())
                    {
                        if let Some(order) = order_data.first() {
                            return Some(WsMessage::Order(Self::parse_order_update(order)));
                        }
                    }
                }

                // Trade updates (my trades)
                if channel_name == "trades" {
                    if let Ok(trade_data) =
                        serde_json::from_value::<Vec<PoloniexMyTradeData>>(data.clone())
                    {
                        if let Some(trade) = trade_data.first() {
                            return Some(WsMessage::MyTrade(Self::parse_my_trade(trade)));
                        }
                    }
                }
            }
        }

        None
    }

    /// 잔고 업데이트 파싱
    fn parse_balance_update(data: &[PoloniexBalanceData]) -> WsBalanceEvent {
        let mut balances = Balances::new();

        for item in data {
            let currency = item.currency.clone().unwrap_or_default();
            let free = item
                .available
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok())
                .unwrap_or_default();
            let used = item
                .hold
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok())
                .unwrap_or_default();
            let balance = Balance::new(free, used);
            balances.add(currency, balance);
        }

        WsBalanceEvent { balances }
    }

    /// 주문 업데이트 파싱
    fn parse_order_update(data: &PoloniexOrderData) -> WsOrderEvent {
        let symbol = data
            .symbol
            .as_ref()
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let side = match data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            Some("LIMIT_MAKER") => OrderType::LimitMaker,
            _ => OrderType::Limit,
        };

        let status = match data.state.as_deref() {
            Some("NEW") => OrderStatus::Open,
            Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("CANCELED") | Some("CANCELLED") => OrderStatus::Canceled,
            Some("EXPIRED") => OrderStatus::Expired,
            Some("FAILED") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let timestamp = data
            .create_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order = Order {
            id: data.id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            symbol: symbol.clone(),
            order_type,
            side,
            status,
            price: data.price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            average: data
                .avg_price
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            amount: data
                .quantity
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok())
                .unwrap_or_default(),
            filled: data
                .filled_quantity
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok())
                .unwrap_or_default(),
            remaining: data
                .quantity
                .as_ref()
                .and_then(|q| Decimal::from_str(q).ok())
                .and_then(|qty| {
                    data.filled_quantity
                        .as_ref()
                        .and_then(|f| Decimal::from_str(f).ok())
                        .map(|filled| qty - filled)
                }),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.update_time,
            time_in_force: None,
            post_only: None,
            reduce_only: None,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// 내 체결 파싱
    fn parse_my_trade(data: &PoloniexMyTradeData) -> WsMyTradeEvent {
        let symbol = data
            .symbol
            .as_ref()
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let timestamp = data
            .create_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data
            .price
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();
        let amount: Decimal = data
            .quantity
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let trades = vec![Trade {
            id: data.id.clone().unwrap_or_default(),
            order: data.order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: data.side.as_ref().map(|s| s.to_lowercase()),
            taker_or_maker: match data.role.as_deref() {
                Some("MAKER") => Some(TakerOrMaker::Maker),
                Some("TAKER") => Some(TakerOrMaker::Taker),
                _ => None,
            },
            price,
            amount,
            cost: Some(price * amount),
            fee: data
                .fee_amount
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok())
                .map(|cost| Fee {
                    currency: data.fee_currency.clone(),
                    cost: Some(cost),
                    rate: None,
                }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsMyTradeEvent { symbol, trades }
    }

    /// 메시지 처리
    fn process_message(msg: &str, timeframe: Option<Timeframe>) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<PoloniexWsResponse>(msg) {
            // Event handling
            if let Some(event) = &response.event {
                if event == "subscribe" {
                    return Some(WsMessage::Subscribed {
                        channel: response
                            .channel
                            .clone()
                            .unwrap_or_default()
                            .into_iter()
                            .next()
                            .unwrap_or_default(),
                        symbol: response.symbols.clone().and_then(|s| s.into_iter().next()),
                    });
                }
            }

            // Data handling
            if let (Some(channel), Some(data)) = (&response.channel, &response.data) {
                let channel_name = channel.first().map(|s| s.as_str()).unwrap_or("");

                // Ticker
                if channel_name == "ticker" {
                    if let Ok(ticker_data) =
                        serde_json::from_value::<PoloniexTickerData>(data.clone())
                    {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                    }
                }

                // OrderBook
                if channel_name == "book_lv2" {
                    if let Ok(ob_data) =
                        serde_json::from_value::<PoloniexOrderBookData>(data.clone())
                    {
                        let symbol = ob_data
                            .symbol
                            .as_ref()
                            .map(|s| Self::to_unified_symbol(s))
                            .unwrap_or_default();
                        let is_snapshot = response.event.as_deref() == Some("snapshot");
                        return Some(WsMessage::OrderBook(Self::parse_order_book(
                            &ob_data,
                            &symbol,
                            is_snapshot,
                        )));
                    }
                }

                // Trades
                if channel_name == "trades" {
                    if let Ok(trade_data) =
                        serde_json::from_value::<PoloniexTradeData>(data.clone())
                    {
                        return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                    }
                }

                // Candles
                if channel_name.starts_with("candles_") {
                    if let Ok(candle_data) =
                        serde_json::from_value::<PoloniexCandleData>(data.clone())
                    {
                        if let Some(tf) = timeframe {
                            if let Some(event) = Self::parse_candle(&candle_data, tf) {
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
        timeframe: Option<Timeframe>,
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
        let subscribe_msg = PoloniexSubscribeMessage {
            event: "subscribe".to_string(),
            channel: vec![channel.to_string()],
            symbols,
        };

        let subscribe_json =
            serde_json::to_string(&subscribe_msg).map_err(|e| CcxtError::ParseError {
                data_type: "PoloniexSubscribeMessage".to_string(),
                message: e.to_string(),
            })?;

        if let Some(ws_client) = &mut self.ws_client {
            ws_client.send(&subscribe_json)?;
        }

        // 구독 저장
        {
            let key = format!("{channel}:{}", subscribe_msg.symbols.join(","));
            self.subscriptions
                .write()
                .await
                .insert(key, channel.to_string());
        }

        // 메시지 핸들러
        let event_tx_clone = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_message(&msg, timeframe) {
                            let _ = event_tx_clone.send(ws_msg);
                        }
                    },
                    WsEvent::Connected => {
                        let _ = event_tx_clone.send(WsMessage::Connected);
                    },
                    WsEvent::Disconnected => {
                        let _ = event_tx_clone.send(WsMessage::Disconnected);
                    },
                    WsEvent::Error(e) => {
                        let _ = event_tx_clone.send(WsMessage::Error(e));
                    },
                    _ => {},
                }
            }
        });

        Ok(event_rx)
    }
}

impl Default for PoloniexWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for PoloniexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let poloniex_symbol = Self::format_symbol(symbol);
        let mut ws = Self::new();
        ws.subscribe_stream("ticker", vec![poloniex_symbol], None)
            .await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let poloniex_symbol = Self::format_symbol(symbol);
        let mut ws = Self::new();
        ws.subscribe_stream("book_lv2", vec![poloniex_symbol], None)
            .await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let poloniex_symbol = Self::format_symbol(symbol);
        let mut ws = Self::new();
        ws.subscribe_stream("trades", vec![poloniex_symbol], None)
            .await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let poloniex_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let channel = format!("candles_{}", interval.to_lowercase());
        let mut ws = Self::new();
        ws.subscribe_stream(&channel, vec![poloniex_symbol], Some(timeframe))
            .await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("balances").await
    }

    async fn watch_orders(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("orders").await
    }

    async fn watch_my_trades(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("trades").await
    }

    async fn watch_positions(
        &self,
        _symbols: Option<&[&str]>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watchPositions".into(),
        })
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let ws_client = WsClient::new(WsConfig {
                url: WS_PUBLIC_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 30,
                connect_timeout_secs: 30,
                ..Default::default()
            });
            self.ws_client = Some(ws_client);
        }
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws_client) = self.ws_client.take() {
            ws_client.close()?;
        }
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        match self.ws_client.as_ref() {
            Some(ws) => ws.is_connected().await,
            None => false,
        }
    }
}

// WebSocket Message Types

#[derive(Debug, Serialize, Deserialize)]
struct PoloniexSubscribeMessage {
    event: String,
    channel: Vec<String>,
    symbols: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PoloniexWsResponse {
    event: Option<String>,
    channel: Option<Vec<String>>,
    symbols: Option<Vec<String>>,
    data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PoloniexTickerData {
    symbol: String,
    #[serde(rename = "dailyChange")]
    daily_change: Option<String>,
    high: Option<String>,
    low: Option<String>,
    amount: Option<String>,
    quantity: Option<String>,
    #[serde(rename = "tradeCount")]
    trade_count: Option<u64>,
    #[serde(rename = "startTime")]
    start_time: Option<i64>,
    close: Option<String>,
    open: Option<String>,
    ts: Option<i64>,
    #[serde(rename = "markPrice")]
    mark_price: Option<String>,
    bid: Option<String>,
    #[serde(rename = "bidQuantity")]
    bid_quantity: Option<String>,
    ask: Option<String>,
    #[serde(rename = "askQuantity")]
    ask_quantity: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PoloniexOrderBookData {
    symbol: Option<String>,
    #[serde(rename = "createTime")]
    create_time: Option<i64>,
    asks: Vec<Vec<String>>,
    bids: Vec<Vec<String>>,
    #[serde(rename = "lastId")]
    last_id: Option<String>,
    id: Option<String>,
    ts: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PoloniexTradeData {
    symbol: String,
    id: String,
    price: String,
    quantity: String,
    #[serde(rename = "takerSide")]
    side: String,
    #[serde(rename = "createTime")]
    create_time: Option<i64>,
    ts: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct PoloniexCandleData {
    symbol: String,
    open: String,
    high: String,
    low: String,
    close: String,
    quantity: String,
    amount: Option<String>,
    #[serde(rename = "tradeCount")]
    trade_count: Option<u64>,
    #[serde(rename = "startTime")]
    start_time: Option<i64>,
    #[serde(rename = "closeTime")]
    close_time: Option<i64>,
    ts: Option<i64>,
}

// Private channel data types

#[derive(Debug, Serialize, Deserialize)]
struct PoloniexBalanceData {
    currency: Option<String>,
    available: Option<String>,
    hold: Option<String>,
    #[serde(rename = "accountId")]
    account_id: Option<String>,
    #[serde(rename = "accountType")]
    account_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PoloniexOrderData {
    id: Option<String>,
    #[serde(rename = "clientOrderId")]
    client_order_id: Option<String>,
    symbol: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<String>,
    side: Option<String>,
    state: Option<String>,
    price: Option<String>,
    #[serde(rename = "avgPrice")]
    avg_price: Option<String>,
    quantity: Option<String>,
    #[serde(rename = "filledQuantity")]
    filled_quantity: Option<String>,
    #[serde(rename = "filledAmount")]
    filled_amount: Option<String>,
    #[serde(rename = "createTime")]
    create_time: Option<i64>,
    #[serde(rename = "updateTime")]
    update_time: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PoloniexMyTradeData {
    id: Option<String>,
    #[serde(rename = "orderId")]
    order_id: Option<String>,
    symbol: Option<String>,
    side: Option<String>,
    price: Option<String>,
    quantity: Option<String>,
    amount: Option<String>,
    #[serde(rename = "feeAmount")]
    fee_amount: Option<String>,
    #[serde(rename = "feeCurrency")]
    fee_currency: Option<String>,
    role: Option<String>,
    #[serde(rename = "createTime")]
    create_time: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(PoloniexWs::format_symbol("BTC/USDT"), "BTC_USDT");
        assert_eq!(PoloniexWs::format_symbol("ETH/BTC"), "ETH_BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(PoloniexWs::to_unified_symbol("BTC_USDT"), "BTC/USDT");
        assert_eq!(PoloniexWs::to_unified_symbol("ETH_BTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(PoloniexWs::format_interval(Timeframe::Minute1), "MINUTE_1");
        assert_eq!(PoloniexWs::format_interval(Timeframe::Hour1), "HOUR_1");
        assert_eq!(PoloniexWs::format_interval(Timeframe::Day1), "DAY_1");
    }

    #[test]
    fn test_generate_auth_signature() {
        let signature = PoloniexWs::generate_auth_signature("test_secret", 1704067200000).unwrap();
        assert!(!signature.is_empty());
        // HMAC-SHA256 + Base64 produces consistent length output
        assert!(signature.len() > 20);
    }

    #[test]
    fn test_parse_balance_update() {
        let data = vec![
            PoloniexBalanceData {
                currency: Some("BTC".to_string()),
                available: Some("1.5".to_string()),
                hold: Some("0.5".to_string()),
                account_id: Some("12345".to_string()),
                account_type: Some("SPOT".to_string()),
            },
            PoloniexBalanceData {
                currency: Some("USDT".to_string()),
                available: Some("10000".to_string()),
                hold: Some("5000".to_string()),
                account_id: Some("12345".to_string()),
                account_type: Some("SPOT".to_string()),
            },
        ];

        let event = PoloniexWs::parse_balance_update(&data);
        assert_eq!(event.balances.currencies.len(), 2);

        let btc_balance = event.balances.get("BTC").unwrap();
        assert_eq!(btc_balance.free, Some(Decimal::from_str("1.5").unwrap()));
        assert_eq!(btc_balance.used, Some(Decimal::from_str("0.5").unwrap()));

        let usdt_balance = event.balances.get("USDT").unwrap();
        assert_eq!(usdt_balance.free, Some(Decimal::from_str("10000").unwrap()));
    }

    #[test]
    fn test_parse_order_update() {
        let data = PoloniexOrderData {
            id: Some("123456789".to_string()),
            client_order_id: Some("client_order_123".to_string()),
            symbol: Some("BTC_USDT".to_string()),
            order_type: Some("LIMIT".to_string()),
            side: Some("BUY".to_string()),
            state: Some("NEW".to_string()),
            price: Some("50000".to_string()),
            avg_price: None,
            quantity: Some("0.1".to_string()),
            filled_quantity: Some("0".to_string()),
            filled_amount: Some("0".to_string()),
            create_time: Some(1704067200000),
            update_time: None,
        };

        let event = PoloniexWs::parse_order_update(&data);
        assert_eq!(event.order.id, "123456789");
        assert_eq!(event.order.symbol, "BTC/USDT");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
        assert_eq!(event.order.status, OrderStatus::Open);
        assert_eq!(event.order.price, Some(Decimal::from_str("50000").unwrap()));
        assert_eq!(event.order.amount, Decimal::from_str("0.1").unwrap());
    }

    #[test]
    fn test_parse_order_update_filled() {
        let data = PoloniexOrderData {
            id: Some("987654321".to_string()),
            client_order_id: None,
            symbol: Some("ETH_USDT".to_string()),
            order_type: Some("MARKET".to_string()),
            side: Some("SELL".to_string()),
            state: Some("FILLED".to_string()),
            price: None,
            avg_price: Some("2500.50".to_string()),
            quantity: Some("1.0".to_string()),
            filled_quantity: Some("1.0".to_string()),
            filled_amount: Some("2500.50".to_string()),
            create_time: Some(1704067200000),
            update_time: Some(1704067201000),
        };

        let event = PoloniexWs::parse_order_update(&data);
        assert_eq!(event.order.id, "987654321");
        assert_eq!(event.order.symbol, "ETH/USDT");
        assert_eq!(event.order.side, OrderSide::Sell);
        assert_eq!(event.order.order_type, OrderType::Market);
        assert_eq!(event.order.status, OrderStatus::Closed);
        assert_eq!(event.order.filled, Decimal::from_str("1.0").unwrap());
    }

    #[test]
    fn test_parse_my_trade() {
        let data = PoloniexMyTradeData {
            id: Some("trade_123".to_string()),
            order_id: Some("order_456".to_string()),
            symbol: Some("BTC_USDT".to_string()),
            side: Some("BUY".to_string()),
            price: Some("50000".to_string()),
            quantity: Some("0.1".to_string()),
            amount: Some("5000".to_string()),
            fee_amount: Some("5".to_string()),
            fee_currency: Some("USDT".to_string()),
            role: Some("TAKER".to_string()),
            create_time: Some(1704067200000),
        };

        let event = PoloniexWs::parse_my_trade(&data);
        assert_eq!(event.symbol, "BTC/USDT");
        assert_eq!(event.trades.len(), 1);

        let trade = &event.trades[0];
        assert_eq!(trade.id, "trade_123");
        assert_eq!(trade.order, Some("order_456".to_string()));
        assert_eq!(trade.price, Decimal::from_str("50000").unwrap());
        assert_eq!(trade.amount, Decimal::from_str("0.1").unwrap());
        assert_eq!(trade.side, Some("buy".to_string()));
        assert_eq!(trade.taker_or_maker, Some(TakerOrMaker::Taker));
        assert!(trade.fee.is_some());
        let fee = trade.fee.as_ref().unwrap();
        assert_eq!(fee.cost, Some(Decimal::from_str("5").unwrap()));
        assert_eq!(fee.currency, Some("USDT".to_string()));
    }

    #[test]
    fn test_parse_my_trade_maker() {
        let data = PoloniexMyTradeData {
            id: Some("trade_789".to_string()),
            order_id: Some("order_012".to_string()),
            symbol: Some("ETH_USDT".to_string()),
            side: Some("SELL".to_string()),
            price: Some("2500".to_string()),
            quantity: Some("2.0".to_string()),
            amount: Some("5000".to_string()),
            fee_amount: Some("2.5".to_string()),
            fee_currency: Some("USDT".to_string()),
            role: Some("MAKER".to_string()),
            create_time: Some(1704067200000),
        };

        let event = PoloniexWs::parse_my_trade(&data);
        let trade = &event.trades[0];
        assert_eq!(trade.taker_or_maker, Some(TakerOrMaker::Maker));
    }

    #[test]
    fn test_process_private_message_balance() {
        let msg = r#"{
            "channel": ["balances"],
            "data": [{
                "currency": "BTC",
                "available": "1.0",
                "hold": "0.5",
                "accountId": "12345",
                "accountType": "SPOT"
            }]
        }"#;

        let result = PoloniexWs::process_private_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.get("BTC").is_some());
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_process_private_message_order() {
        let msg = r#"{
            "channel": ["orders"],
            "data": [{
                "id": "123456",
                "symbol": "BTC_USDT",
                "type": "LIMIT",
                "side": "BUY",
                "state": "NEW",
                "price": "50000",
                "quantity": "0.1",
                "filledQuantity": "0",
                "createTime": 1704067200000
            }]
        }"#;

        let result = PoloniexWs::process_private_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "123456");
            assert_eq!(event.order.symbol, "BTC/USDT");
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_private_message_trade() {
        let msg = r#"{
            "channel": ["trades"],
            "data": [{
                "id": "trade_123",
                "orderId": "order_456",
                "symbol": "BTC_USDT",
                "side": "BUY",
                "price": "50000",
                "quantity": "0.1",
                "feeAmount": "5",
                "feeCurrency": "USDT",
                "role": "TAKER",
                "createTime": 1704067200000
            }]
        }"#;

        let result = PoloniexWs::process_private_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::MyTrade(event)) = result {
            assert_eq!(event.symbol, "BTC/USDT");
            assert_eq!(event.trades.len(), 1);
        } else {
            panic!("Expected MyTrade message");
        }
    }

    #[test]
    fn test_with_config() {
        let config = ExchangeConfig::new()
            .with_api_key("test_key")
            .with_api_secret("test_secret");

        let ws = PoloniexWs::with_config(config);
        assert!(ws.config.is_some());
        assert_eq!(ws.config.as_ref().unwrap().api_key(), Some("test_key"));
    }

    #[test]
    fn test_clone() {
        let config = ExchangeConfig::new()
            .with_api_key("test_key")
            .with_api_secret("test_secret");

        let ws = PoloniexWs::with_config(config);
        let cloned = ws.clone();

        assert!(cloned.config.is_some());
        assert_eq!(cloned.config.as_ref().unwrap().api_key(), Some("test_key"));
        assert!(cloned.ws_client.is_none());
        assert!(cloned.private_ws_client.is_none());
    }
}
