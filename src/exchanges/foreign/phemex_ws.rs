//! Phemex WebSocket Implementation
//!
//! Phemex 실시간 데이터 스트리밍

use async_trait::async_trait;
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
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Position, PositionSide, Ticker, Timeframe, Trade, OHLCV, WsBalanceEvent,
    WsExchange, WsMessage, WsOhlcvEvent, WsOrderBookEvent, WsOrderEvent,
    WsPositionEvent, WsTickerEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_PUBLIC_URL: &str = "wss://phemex.com/ws";
const WS_PRIVATE_URL: &str = "wss://phemex.com/ws";

/// Phemex WebSocket 클라이언트
pub struct PhemexWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    private_ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    authenticated: Arc<RwLock<bool>>,
}

impl Clone for PhemexWs {
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

impl PhemexWs {
    /// 새 Phemex WebSocket 클라이언트 생성
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

    /// 심볼을 Phemex 형식으로 변환 (BTC/USDT -> BTCUSDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// Phemex 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(phemex_symbol: &str) -> String {
        for quote in &["USDT", "USD", "BTC", "ETH"] {
            if let Some(base) = phemex_symbol.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }
        phemex_symbol.to_string()
    }

    /// Timeframe을 Phemex 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> i32 {
        match timeframe {
            Timeframe::Minute1 => 60,
            Timeframe::Minute5 => 300,
            Timeframe::Minute15 => 900,
            Timeframe::Minute30 => 1800,
            Timeframe::Hour1 => 3600,
            Timeframe::Hour4 => 14400,
            Timeframe::Day1 => 86400,
            Timeframe::Week1 => 604800,
            Timeframe::Month1 => 2592000,
            _ => 60,
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &PhemexTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let scale = Decimal::new(10_i64.pow(8), 0);

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_ev.map(|v| Decimal::from(v) / scale),
            low: data.low_ev.map(|v| Decimal::from(v) / scale),
            bid: data.bid_ev.map(|v| Decimal::from(v) / scale),
            bid_volume: None,
            ask: data.ask_ev.map(|v| Decimal::from(v) / scale),
            ask_volume: None,
            vwap: None,
            open: data.open_ev.map(|v| Decimal::from(v) / scale),
            close: data.last_ev.map(|v| Decimal::from(v) / scale),
            last: data.last_ev.map(|v| Decimal::from(v) / scale),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume_ev.map(|v| Decimal::from(v) / scale),
            quote_volume: data.turnover_ev.map(|v| Decimal::from(v) / scale),
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &PhemexOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let scale = Decimal::new(10_i64.pow(8), 0);

        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: Decimal::from(b[0]) / scale,
                    amount: Decimal::from(b[1]) / scale,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: Decimal::from(a[0]) / scale,
                    amount: Decimal::from(a[1]) / scale,
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
    fn parse_trade(data: &PhemexTradeData, symbol: &str) -> WsTradeEvent {
        let scale = Decimal::new(10_i64.pow(8), 0);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = Decimal::from(data.price_ev.unwrap_or(0)) / scale;
        let amount = Decimal::from(data.qty_ev.unwrap_or(0)) / scale;

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
            side: data.side.clone(),
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
    fn parse_candle(data: &PhemexKlineData, symbol: &str, timeframe: Timeframe) -> WsOhlcvEvent {
        let scale = Decimal::new(10_i64.pow(8), 0);

        let ohlcv = OHLCV {
            timestamp: data.timestamp.unwrap_or(0),
            open: Decimal::from(data.open_ev.unwrap_or(0)) / scale,
            high: Decimal::from(data.high_ev.unwrap_or(0)) / scale,
            low: Decimal::from(data.low_ev.unwrap_or(0)) / scale,
            close: Decimal::from(data.close_ev.unwrap_or(0)) / scale,
            volume: Decimal::from(data.volume_ev.unwrap_or(0)) / scale,
        };

        WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        }
    }

    /// 인증 서명 생성 (HMAC-SHA256, hex-encoded)
    fn generate_auth_signature(api_secret: &str, expiry: i64) -> CcxtResult<String> {
        let message = format!("/ws{}", expiry);
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(message.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
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
            ping_interval_secs: 20,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.private_ws_client = Some(ws_client);

        // 인증 메시지 전송
        let expiry = Utc::now().timestamp() + 120;
        let signature = Self::generate_auth_signature(api_secret, expiry)?;

        let auth_msg = serde_json::json!({
            "id": 1,
            "method": "user.auth",
            "params": ["API", api_key, signature, expiry]
        });

        if let Some(ws_client) = &mut self.private_ws_client {
            ws_client.send(&auth_msg.to_string())?;
        }

        // 구독 메시지 전송 (인증 후)
        let subscribe_msg = serde_json::json!({
            "id": 2,
            "method": channel,
            "params": []
        });

        if let Some(ws_client) = &mut self.private_ws_client {
            ws_client.send(&subscribe_msg.to_string())?;
        }

        *self.authenticated.write().await = true;

        // 구독 저장
        {
            self.subscriptions.write().await.insert(channel.to_string(), channel.to_string());
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
                    }
                    WsEvent::Connected => {
                        let _ = event_tx_clone.send(WsMessage::Connected);
                    }
                    WsEvent::Disconnected => {
                        let _ = event_tx_clone.send(WsMessage::Disconnected);
                    }
                    WsEvent::Error(e) => {
                        let _ = event_tx_clone.send(WsMessage::Error(e));
                    }
                    _ => {}
                }
            }
        });

        Ok(event_rx)
    }

    /// Private 메시지 처리
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        // Pong 메시지 처리
        if msg.contains("\"pong\"") {
            return None;
        }

        if let Ok(response) = serde_json::from_str::<PhemexPrivateResponse>(msg) {
            // 인증/구독 확인
            if response.result.is_some() && response.error.is_none() {
                if let Some(id) = response.id {
                    if id == 1 {
                        return Some(WsMessage::Authenticated);
                    }
                }
            }

            // 에러 처리
            if response.error.is_some() {
                return Some(WsMessage::Error(format!("{:?}", response.error)));
            }

            // Balance updates (accounts)
            if let Some(accounts) = response.accounts {
                return Some(WsMessage::Balance(Self::parse_balance_update(&accounts)));
            }

            // Order updates
            if let Some(orders) = response.orders {
                if let Some(order) = orders.first() {
                    return Some(WsMessage::Order(Self::parse_order_update(order)));
                }
            }

            // Position updates
            if let Some(positions) = response.positions {
                return Some(WsMessage::Position(Self::parse_position_update(&positions)));
            }
        }

        None
    }

    /// 잔고 업데이트 파싱
    fn parse_balance_update(data: &[PhemexAccountData]) -> WsBalanceEvent {
        let scale = Decimal::new(10_i64.pow(8), 0);
        let mut balances = Balances::new();

        for item in data {
            let currency = item.currency.clone().unwrap_or_default();
            let free = item.account_balance_ev.map(|v| Decimal::from(v) / scale).unwrap_or_default()
                - item.position_margin_ev.map(|v| Decimal::from(v) / scale).unwrap_or_default()
                - item.order_margin_ev.map(|v| Decimal::from(v) / scale).unwrap_or_default();
            let used = item.position_margin_ev.map(|v| Decimal::from(v) / scale).unwrap_or_default()
                + item.order_margin_ev.map(|v| Decimal::from(v) / scale).unwrap_or_default();
            let balance = Balance::new(free, used);
            balances.add(currency, balance);
        }

        WsBalanceEvent { balances }
    }

    /// 주문 업데이트 파싱
    fn parse_order_update(data: &PhemexOrderData) -> WsOrderEvent {
        let scale = Decimal::new(10_i64.pow(8), 0);
        let symbol = data.symbol.as_ref()
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let side = match data.side.as_deref() {
            Some("Buy") => OrderSide::Buy,
            Some("Sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.ord_type.as_deref() {
            Some("Limit") => OrderType::Limit,
            Some("Market") => OrderType::Market,
            Some("Stop") => OrderType::StopLimit,
            Some("StopLimit") => OrderType::StopLimit,
            Some("MarketIfTouched") => OrderType::StopMarket,
            _ => OrderType::Limit,
        };

        let status = match data.ord_status.as_deref() {
            Some("New") | Some("Created") => OrderStatus::Open,
            Some("PartiallyFilled") => OrderStatus::Open,
            Some("Filled") => OrderStatus::Closed,
            Some("Canceled") | Some("Cancelled") => OrderStatus::Canceled,
            Some("Rejected") => OrderStatus::Rejected,
            Some("Expired") => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let timestamp = data.action_time_ns.map(|t| t / 1_000_000).unwrap_or_else(|| Utc::now().timestamp_millis());

        let order = Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.cl_ord_id.clone(),
            symbol: symbol.clone(),
            order_type,
            side,
            status,
            price: data.price_ep.map(|v| Decimal::from(v) / scale),
            average: data.avg_price_ep.map(|v| Decimal::from(v) / scale),
            amount: data.order_qty_ev.map(|v| Decimal::from(v) / scale).unwrap_or_default(),
            filled: data.cum_qty_ev.map(|v| Decimal::from(v) / scale).unwrap_or_default(),
            remaining: data.leaves_qty_ev.map(|v| Decimal::from(v) / scale),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.transact_time_ns.map(|t| t / 1_000_000),
            time_in_force: None,
            post_only: None,
            reduce_only: data.reduce_only,
            stop_price: data.stop_px_ep.map(|v| Decimal::from(v) / scale),
            trigger_price: None,
            take_profit_price: data.take_profit_ep.map(|v| Decimal::from(v) / scale),
            stop_loss_price: data.stop_loss_ep.map(|v| Decimal::from(v) / scale),
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// 포지션 업데이트 파싱
    fn parse_position_update(data: &[PhemexPositionData]) -> WsPositionEvent {
        let scale = Decimal::new(10_i64.pow(8), 0);
        let positions: Vec<Position> = data.iter().map(|item| {
            let symbol = item.symbol.as_ref()
                .map(|s| Self::to_unified_symbol(s))
                .unwrap_or_default();

            let side = match item.side.as_deref() {
                Some("Buy") => Some(PositionSide::Long),
                Some("Sell") => Some(PositionSide::Short),
                _ => None,
            };

            let timestamp = item.transact_time_ns.map(|t| t / 1_000_000).unwrap_or_else(|| Utc::now().timestamp_millis());

            Position {
                id: None,
                symbol: symbol.clone(),
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                hedged: None,
                side,
                contracts: item.size.map(|v| Decimal::from(v)),
                contract_size: None,
                entry_price: item.avg_entry_price_ep.map(|v| Decimal::from(v) / scale),
                mark_price: item.mark_price_ep.map(|v| Decimal::from(v) / scale),
                notional: item.position_margin_ev.map(|v| Decimal::from(v) / scale),
                leverage: item.leverage.map(|v| Decimal::from(v)),
                collateral: item.position_margin_ev.map(|v| Decimal::from(v) / scale),
                initial_margin: item.position_margin_ev.map(|v| Decimal::from(v) / scale),
                maintenance_margin: None,
                initial_margin_percentage: None,
                maintenance_margin_percentage: None,
                unrealized_pnl: item.unrealized_pnl_ev.map(|v| Decimal::from(v) / scale),
                liquidation_price: item.liquidation_price_ep.map(|v| Decimal::from(v) / scale),
                margin_mode: None,
                margin_ratio: None,
                percentage: None,
                stop_loss_price: None,
                take_profit_price: None,
                last_price: item.mark_price_ep.map(|v| Decimal::from(v) / scale),
                last_update_timestamp: Some(timestamp),
                realized_pnl: None,
                info: serde_json::to_value(item).unwrap_or_default(),
            }
        }).collect();

        WsPositionEvent { positions }
    }

    /// 메시지 처리
    fn process_message(msg: &str, subscribed_symbol: Option<&str>, subscribed_timeframe: Option<Timeframe>) -> Option<WsMessage> {
        // Pong 메시지 처리
        if msg.contains("\"pong\"") {
            return None;
        }

        let response: PhemexWsResponse = serde_json::from_str(msg).ok()?;

        // 에러 처리
        if response.error.is_some() {
            return Some(WsMessage::Error(format!("{:?}", response.error)));
        }

        // 티커
        if let Some(ticker_data) = response.tick {
            if let Some(symbol) = subscribed_symbol {
                let _unified = Self::to_unified_symbol(symbol);
                let mut ticker_data = ticker_data;
                ticker_data.symbol = symbol.to_string();
                return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
            }
        }

        // 호가창
        if let Some(book_data) = response.book {
            if let Some(symbol) = subscribed_symbol {
                let unified = Self::to_unified_symbol(symbol);
                return Some(WsMessage::OrderBook(Self::parse_order_book(&book_data, &unified)));
            }
        }

        // 체결
        if let Some(trades) = response.trades {
            if let Some(first) = trades.first() {
                if let Some(symbol) = subscribed_symbol {
                    let unified = Self::to_unified_symbol(symbol);
                    return Some(WsMessage::Trade(Self::parse_trade(first, &unified)));
                }
            }
        }

        // 캔들
        if let Some(kline_data) = response.kline {
            if let Some(symbol) = subscribed_symbol {
                let unified = Self::to_unified_symbol(symbol);
                let timeframe = subscribed_timeframe.unwrap_or(Timeframe::Minute1);
                return Some(WsMessage::Ohlcv(Self::parse_candle(&kline_data, &unified, timeframe)));
            }
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

impl Default for PhemexWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for PhemexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "id": 1,
            "method": "tick.subscribe",
            "params": [formatted]
        });
        client.subscribe_stream(subscribe_msg, "ticker", Some(&formatted), None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let params: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        let subscribe_msg = serde_json::json!({
            "id": 1,
            "method": "tick.subscribe",
            "params": params
        });
        client.subscribe_stream(subscribe_msg, "tickers", None, None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "id": 2,
            "method": "orderbook.subscribe",
            "params": [formatted]
        });
        client.subscribe_stream(subscribe_msg, "orderBook", Some(&formatted), None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "id": 3,
            "method": "trade.subscribe",
            "params": [formatted]
        });
        client.subscribe_stream(subscribe_msg, "trade", Some(&formatted), None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let subscribe_msg = serde_json::json!({
            "id": 4,
            "method": "kline.subscribe",
            "params": [formatted, interval]
        });
        client.subscribe_stream(subscribe_msg, "kline", Some(&formatted), Some(timeframe)).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("aop.subscribe").await
    }

    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("aop.subscribe").await
    }

    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("aop.subscribe").await
    }

    async fn watch_positions(&self, _symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        client.subscribe_private_stream("aop.subscribe").await
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
}

// === Response Types ===

#[derive(Debug, Default, Deserialize)]
struct PhemexWsResponse {
    #[serde(default)]
    error: Option<serde_json::Value>,
    #[serde(default)]
    tick: Option<PhemexTickerData>,
    #[serde(default)]
    book: Option<PhemexOrderBookData>,
    #[serde(default)]
    trades: Option<Vec<PhemexTradeData>>,
    #[serde(default)]
    kline: Option<PhemexKlineData>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexTickerData {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, rename = "lastEp")]
    last_ev: Option<i64>,
    #[serde(default, rename = "openEp")]
    open_ev: Option<i64>,
    #[serde(default, rename = "highEp")]
    high_ev: Option<i64>,
    #[serde(default, rename = "lowEp")]
    low_ev: Option<i64>,
    #[serde(default, rename = "bidEp")]
    bid_ev: Option<i64>,
    #[serde(default, rename = "askEp")]
    ask_ev: Option<i64>,
    #[serde(default, rename = "volumeEv")]
    volume_ev: Option<i64>,
    #[serde(default, rename = "turnoverEv")]
    turnover_ev: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexOrderBookData {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<i64>>,
    #[serde(default)]
    asks: Vec<Vec<i64>>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexTradeData {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, rename = "tradeId")]
    trade_id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "priceEp")]
    price_ev: Option<i64>,
    #[serde(default, rename = "qtyEv")]
    qty_ev: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexKlineData {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, rename = "openEp")]
    open_ev: Option<i64>,
    #[serde(default, rename = "highEp")]
    high_ev: Option<i64>,
    #[serde(default, rename = "lowEp")]
    low_ev: Option<i64>,
    #[serde(default, rename = "closeEp")]
    close_ev: Option<i64>,
    #[serde(default, rename = "volumeEv")]
    volume_ev: Option<i64>,
}

// Private channel data types

#[derive(Debug, Default, Deserialize)]
struct PhemexPrivateResponse {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<serde_json::Value>,
    #[serde(default)]
    accounts: Option<Vec<PhemexAccountData>>,
    #[serde(default)]
    orders: Option<Vec<PhemexOrderData>>,
    #[serde(default)]
    positions: Option<Vec<PhemexPositionData>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PhemexAccountData {
    #[serde(default)]
    currency: Option<String>,
    #[serde(default, rename = "accountBalanceEv")]
    account_balance_ev: Option<i64>,
    #[serde(default, rename = "positionMarginEv")]
    position_margin_ev: Option<i64>,
    #[serde(default, rename = "orderMarginEv")]
    order_margin_ev: Option<i64>,
    #[serde(default, rename = "bonusBalanceEv")]
    bonus_balance_ev: Option<i64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PhemexOrderData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default, rename = "orderID")]
    order_id: Option<String>,
    #[serde(default, rename = "clOrdID")]
    cl_ord_id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "ordType")]
    ord_type: Option<String>,
    #[serde(default, rename = "ordStatus")]
    ord_status: Option<String>,
    #[serde(default, rename = "priceEp")]
    price_ep: Option<i64>,
    #[serde(default, rename = "avgPriceEp")]
    avg_price_ep: Option<i64>,
    #[serde(default, rename = "orderQtyEv")]
    order_qty_ev: Option<i64>,
    #[serde(default, rename = "cumQtyEv")]
    cum_qty_ev: Option<i64>,
    #[serde(default, rename = "leavesQtyEv")]
    leaves_qty_ev: Option<i64>,
    #[serde(default, rename = "stopPxEp")]
    stop_px_ep: Option<i64>,
    #[serde(default, rename = "takeProfitEp")]
    take_profit_ep: Option<i64>,
    #[serde(default, rename = "stopLossEp")]
    stop_loss_ep: Option<i64>,
    #[serde(default, rename = "reduceOnly")]
    reduce_only: Option<bool>,
    #[serde(default, rename = "actionTimeNs")]
    action_time_ns: Option<i64>,
    #[serde(default, rename = "transactTimeNs")]
    transact_time_ns: Option<i64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PhemexPositionData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    size: Option<i64>,
    #[serde(default)]
    leverage: Option<i64>,
    #[serde(default, rename = "avgEntryPriceEp")]
    avg_entry_price_ep: Option<i64>,
    #[serde(default, rename = "markPriceEp")]
    mark_price_ep: Option<i64>,
    #[serde(default, rename = "positionMarginEv")]
    position_margin_ev: Option<i64>,
    #[serde(default, rename = "unrealisedPnlEv")]
    unrealized_pnl_ev: Option<i64>,
    #[serde(default, rename = "liquidationPriceEp")]
    liquidation_price_ep: Option<i64>,
    #[serde(default, rename = "transactTimeNs")]
    transact_time_ns: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(PhemexWs::format_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(PhemexWs::format_symbol("ETH/USD"), "ETHUSD");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(PhemexWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(PhemexWs::to_unified_symbol("ETHUSD"), "ETH/USD");
    }

    #[test]
    fn test_generate_auth_signature() {
        let signature = PhemexWs::generate_auth_signature("test_secret", 1704067200).unwrap();
        assert!(!signature.is_empty());
        // HMAC-SHA256 hex-encoded produces 64 chars
        assert_eq!(signature.len(), 64);
    }

    #[test]
    fn test_parse_balance_update() {
        let data = vec![
            PhemexAccountData {
                currency: Some("BTC".to_string()),
                account_balance_ev: Some(200000000), // 2.0 BTC (scaled by 10^8)
                position_margin_ev: Some(50000000), // 0.5 BTC
                order_margin_ev: Some(50000000), // 0.5 BTC
                bonus_balance_ev: None,
            },
        ];

        let event = PhemexWs::parse_balance_update(&data);
        let btc_balance = event.balances.get("BTC").unwrap();
        assert_eq!(btc_balance.free, Some(Decimal::from_str("1.0").unwrap())); // 2.0 - 0.5 - 0.5
        assert_eq!(btc_balance.used, Some(Decimal::from_str("1.0").unwrap())); // 0.5 + 0.5
    }

    #[test]
    fn test_parse_order_update() {
        let data = PhemexOrderData {
            symbol: Some("BTCUSDT".to_string()),
            order_id: Some("order123".to_string()),
            cl_ord_id: Some("client123".to_string()),
            side: Some("Buy".to_string()),
            ord_type: Some("Limit".to_string()),
            ord_status: Some("New".to_string()),
            price_ep: Some(5000000000000), // 50000 (scaled by 10^8)
            avg_price_ep: None,
            order_qty_ev: Some(10000000), // 0.1 BTC
            cum_qty_ev: Some(0),
            leaves_qty_ev: Some(10000000),
            stop_px_ep: None,
            take_profit_ep: None,
            stop_loss_ep: None,
            reduce_only: None,
            action_time_ns: Some(1704067200000000000),
            transact_time_ns: None,
        };

        let event = PhemexWs::parse_order_update(&data);
        assert_eq!(event.order.id, "order123");
        assert_eq!(event.order.symbol, "BTC/USDT");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
        assert_eq!(event.order.status, OrderStatus::Open);
    }

    #[test]
    fn test_parse_order_update_filled() {
        let data = PhemexOrderData {
            symbol: Some("ETHUSDT".to_string()),
            order_id: Some("order456".to_string()),
            cl_ord_id: None,
            side: Some("Sell".to_string()),
            ord_type: Some("Market".to_string()),
            ord_status: Some("Filled".to_string()),
            price_ep: None,
            avg_price_ep: Some(250000000000), // 2500
            order_qty_ev: Some(100000000), // 1.0 ETH
            cum_qty_ev: Some(100000000),
            leaves_qty_ev: Some(0),
            stop_px_ep: None,
            take_profit_ep: None,
            stop_loss_ep: None,
            reduce_only: None,
            action_time_ns: Some(1704067200000000000),
            transact_time_ns: None,
        };

        let event = PhemexWs::parse_order_update(&data);
        assert_eq!(event.order.id, "order456");
        assert_eq!(event.order.symbol, "ETH/USDT");
        assert_eq!(event.order.side, OrderSide::Sell);
        assert_eq!(event.order.order_type, OrderType::Market);
        assert_eq!(event.order.status, OrderStatus::Closed);
    }

    #[test]
    fn test_parse_position_update() {
        let data = vec![
            PhemexPositionData {
                symbol: Some("BTCUSDT".to_string()),
                side: Some("Buy".to_string()),
                size: Some(1),
                leverage: Some(10),
                avg_entry_price_ep: Some(5000000000000), // 50000
                mark_price_ep: Some(5050000000000), // 50500
                position_margin_ev: Some(500000000), // 5.0
                unrealized_pnl_ev: Some(10000000), // 0.1
                liquidation_price_ep: Some(4500000000000), // 45000
                transact_time_ns: Some(1704067200000000000),
            },
        ];

        let event = PhemexWs::parse_position_update(&data);
        assert_eq!(event.positions.len(), 1);

        let pos = &event.positions[0];
        assert_eq!(pos.symbol, "BTC/USDT");
        assert_eq!(pos.side, Some(PositionSide::Long));
        assert_eq!(pos.contracts, Some(Decimal::from(1)));
        assert_eq!(pos.leverage, Some(Decimal::from(10)));
    }

    #[test]
    fn test_parse_position_update_short() {
        let data = vec![
            PhemexPositionData {
                symbol: Some("ETHUSDT".to_string()),
                side: Some("Sell".to_string()),
                size: Some(2),
                leverage: Some(5),
                avg_entry_price_ep: Some(250000000000),
                mark_price_ep: Some(248000000000),
                position_margin_ev: Some(100000000),
                unrealized_pnl_ev: Some(4000000),
                liquidation_price_ep: Some(275000000000),
                transact_time_ns: Some(1704067200000000000),
            },
        ];

        let event = PhemexWs::parse_position_update(&data);
        let pos = &event.positions[0];
        assert_eq!(pos.side, Some(PositionSide::Short));
    }

    #[test]
    fn test_process_private_message_balance() {
        let msg = r#"{
            "accounts": [{
                "currency": "BTC",
                "accountBalanceEv": 200000000,
                "positionMarginEv": 50000000,
                "orderMarginEv": 50000000
            }]
        }"#;

        let result = PhemexWs::process_private_message(msg);
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
            "orders": [{
                "symbol": "BTCUSDT",
                "orderID": "test123",
                "side": "Buy",
                "ordType": "Limit",
                "ordStatus": "New",
                "priceEp": 5000000000000,
                "orderQtyEv": 10000000,
                "cumQtyEv": 0,
                "actionTimeNs": 1704067200000000000
            }]
        }"#;

        let result = PhemexWs::process_private_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "test123");
            assert_eq!(event.order.symbol, "BTC/USDT");
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_private_message_position() {
        let msg = r#"{
            "positions": [{
                "symbol": "BTCUSDT",
                "side": "Buy",
                "size": 1,
                "leverage": 10,
                "avgEntryPriceEp": 5000000000000,
                "markPriceEp": 5050000000000,
                "positionMarginEv": 500000000
            }]
        }"#;

        let result = PhemexWs::process_private_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Position(event)) = result {
            assert_eq!(event.positions.len(), 1);
            assert_eq!(event.positions[0].symbol, "BTC/USDT");
        } else {
            panic!("Expected Position message");
        }
    }

    #[test]
    fn test_with_config() {
        let config = ExchangeConfig::new()
            .with_api_key("test_key")
            .with_api_secret("test_secret");

        let ws = PhemexWs::with_config(config);
        assert!(ws.config.is_some());
        assert_eq!(ws.config.as_ref().unwrap().api_key(), Some("test_key"));
    }

    #[test]
    fn test_clone() {
        let config = ExchangeConfig::new()
            .with_api_key("test_key")
            .with_api_secret("test_secret");

        let ws = PhemexWs::with_config(config);
        let cloned = ws.clone();

        assert!(cloned.config.is_some());
        assert_eq!(cloned.config.as_ref().unwrap().api_key(), Some("test_key"));
        assert!(cloned.ws_client.is_none());
        assert!(cloned.private_ws_client.is_none());
    }
}
