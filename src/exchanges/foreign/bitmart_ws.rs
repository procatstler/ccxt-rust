//! BitMart WebSocket Implementation
//!
//! BitMart 실시간 데이터 스트리밍

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::Deserialize;
use sha2::Sha256;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, MarginMode, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, Position, PositionSide, TakerOrMaker, Ticker, Timeframe, Trade, OHLCV, WsBalanceEvent,
    WsExchange, WsMessage, WsOhlcvEvent, WsOrderBookEvent, WsOrderEvent, WsPositionEvent,
    WsTickerEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_PUBLIC_URL: &str = "wss://ws-manager-compress.bitmart.com/api?protocol=1.1";
const WS_PRIVATE_URL: &str = "wss://ws-manager-compress.bitmart.com/user?protocol=1.1";

/// BitMart WebSocket 클라이언트
pub struct BitmartWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    private_ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    authenticated: Arc<RwLock<bool>>,
}

impl Clone for BitmartWs {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::clone(&self.subscriptions),
            event_tx: None,
            authenticated: Arc::clone(&self.authenticated),
        }
    }
}

impl BitmartWs {
    /// 새 BitMart WebSocket 클라이언트 생성
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

    /// 설정과 함께 생성
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

    /// 심볼을 BitMart 형식으로 변환 (BTC/USDT -> BTC_USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// BitMart 심볼을 통합 심볼로 변환 (BTC_USDT -> BTC/USDT)
    fn to_unified_symbol(bitmart_symbol: &str) -> String {
        bitmart_symbol.replace("_", "/")
    }

    /// Timeframe을 BitMart 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1H",
            Timeframe::Hour4 => "4H",
            Timeframe::Day1 => "1D",
            Timeframe::Week1 => "1W",
            Timeframe::Month1 => "1M",
            _ => "1m",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BitmartTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.ms_t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low_24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.best_bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.best_bid_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.best_ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.best_ask_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open_24h.as_ref().and_then(|v| v.parse().ok()),
            close: data.last_price.as_ref().and_then(|v| v.parse().ok()),
            last: data.last_price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: data.price_change_24h.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.base_volume_24h.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.quote_volume_24h.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BitmartOrderBookData, symbol: &str) -> WsOrderBookEvent {
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

        let timestamp = data.ms_t.unwrap_or_else(|| Utc::now().timestamp_millis());
        let unified_symbol = Self::to_unified_symbol(symbol);

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
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
            symbol: unified_symbol,
            order_book,
            is_snapshot: true,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &BitmartTradeData, symbol: &str) -> WsTradeEvent {
        let timestamp = data.s_t.map(|t| t * 1000).unwrap_or_else(|| Utc::now().timestamp_millis());
        let unified_symbol = Self::to_unified_symbol(symbol);
        let price = data.price.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();
        let amount = data.size.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();

        let trades = vec![Trade {
            id: String::new(),
            info: serde_json::Value::Null,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
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
            symbol: unified_symbol,
            trades,
        }
    }

    /// 캔들 메시지 파싱
    fn parse_candle(data: &BitmartKlineData, symbol: &str, timeframe: Timeframe) -> WsOhlcvEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);

        let ohlcv = OHLCV {
            timestamp: data.candle.first()
                .and_then(|t| t.parse::<i64>().ok())
                .unwrap_or(0) * 1000,
            open: data.candle.get(1)
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            high: data.candle.get(2)
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            low: data.candle.get(3)
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            close: data.candle.get(4)
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            volume: data.candle.get(5)
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe,
            ohlcv,
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str, subscribed_symbol: Option<&str>, subscribed_timeframe: Option<Timeframe>) -> Option<WsMessage> {
        // Pong 메시지 처리
        if msg.contains("\"pong\"") || msg.contains("pong") {
            return None;
        }

        let response: BitmartWsResponse = serde_json::from_str(msg).ok()?;
        let table = response.table.as_deref()?;
        let data = response.data?;
        let symbol = subscribed_symbol.unwrap_or("");

        match table {
            s if s.contains("ticker") => {
                if let Some(first) = data.first() {
                    if let Ok(ticker_data) = serde_json::from_value::<BitmartTickerData>(first.clone()) {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                    }
                }
            }
            s if s.contains("depth") => {
                if let Some(first) = data.first() {
                    if let Ok(book_data) = serde_json::from_value::<BitmartOrderBookData>(first.clone()) {
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&book_data, symbol)));
                    }
                }
            }
            s if s.contains("trade") => {
                if let Some(first) = data.first() {
                    if let Ok(trade_data) = serde_json::from_value::<BitmartTradeData>(first.clone()) {
                        return Some(WsMessage::Trade(Self::parse_trade(&trade_data, symbol)));
                    }
                }
            }
            s if s.contains("kline") => {
                if let Some(first) = data.first() {
                    if let Ok(kline_data) = serde_json::from_value::<BitmartKlineData>(first.clone()) {
                        let timeframe = subscribed_timeframe.unwrap_or(Timeframe::Minute1);
                        return Some(WsMessage::Ohlcv(Self::parse_candle(&kline_data, symbol, timeframe)));
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
            ping_interval_secs: 15,
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

    /// 인증 서명 생성 (HMAC-SHA256)
    /// Sign: timestamp + "#" + memo + "#" + body
    fn generate_auth_signature(api_secret: &str, timestamp: &str, memo: &str) -> CcxtResult<String> {
        let sign_str = format!("{}#{}#bitmart.WebSocket", timestamp, memo);
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            })?;
        mac.update(sign_str.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Private 스트림에 연결하고 이벤트 수신
    async fn subscribe_private_stream(&mut self, channel: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Config required for private streams".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;
        let memo = config.uid().unwrap_or("");

        let timestamp = Utc::now().timestamp_millis().to_string();
        let signature = Self::generate_auth_signature(api_secret, &timestamp, memo)?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PRIVATE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 15,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // 인증 메시지 전송
        let auth_msg = serde_json::json!({
            "op": "login",
            "args": [api_key, timestamp, signature]
        });
        ws_client.send(&auth_msg.to_string())?;

        // 채널 구독
        let subscribe_msg = match channel {
            "balance" => serde_json::json!({
                "op": "subscribe",
                "args": ["spot/user/balance:BALANCE_UPDATE"]
            }),
            "orders" => serde_json::json!({
                "op": "subscribe",
                "args": ["spot/user/order:ORDER_PROGRESS"]
            }),
            "myTrades" => serde_json::json!({
                "op": "subscribe",
                "args": ["spot/user/order:ORDER_PROGRESS"]
            }),
            "positions" => serde_json::json!({
                "op": "subscribe",
                "args": ["futures/user/position"]
            }),
            _ => serde_json::json!({
                "op": "subscribe",
                "args": [format!("spot/user/{}", channel)]
            }),
        };
        ws_client.send(&subscribe_msg.to_string())?;

        self.private_ws_client = Some(ws_client);
        *self.authenticated.write().await = true;

        // 구독 저장
        {
            let key = format!("private:{}", channel);
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

    /// Private 메시지 처리
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        // Pong이나 인증 응답 무시
        if msg.contains("\"pong\"") || msg.contains("\"login\"") || msg.is_empty() {
            return None;
        }

        let response: BitmartPrivateResponse = serde_json::from_str(msg).ok()?;
        let table = response.table.as_deref()?;
        let data = response.data?;

        match table {
            s if s.contains("balance") || s.contains("BALANCE") => {
                if let Some(first) = data.first() {
                    if let Ok(balance_data) = serde_json::from_value::<BitmartBalanceData>(first.clone()) {
                        return Some(WsMessage::Balance(Self::parse_balance_update(&balance_data)));
                    }
                }
            }
            s if s.contains("order") || s.contains("ORDER") => {
                if let Some(first) = data.first() {
                    if let Ok(order_data) = serde_json::from_value::<BitmartOrderData>(first.clone()) {
                        // 체결 내역인지 주문 업데이트인지 확인
                        if order_data.deal_price.is_some() && order_data.deal_vol.is_some() {
                            return Some(WsMessage::Trade(Self::parse_my_trade(&order_data)));
                        }
                        return Some(WsMessage::Order(Self::parse_order_update(&order_data)));
                    }
                }
            }
            s if s.contains("position") || s.contains("POSITION") => {
                if let Some(first) = data.first() {
                    if let Ok(position_data) = serde_json::from_value::<BitmartPositionData>(first.clone()) {
                        return Some(WsMessage::Position(Self::parse_position_update(&position_data)));
                    }
                }
            }
            _ => {}
        }

        None
    }

    /// 잔고 업데이트 파싱
    fn parse_balance_update(data: &BitmartBalanceData) -> WsBalanceEvent {
        let mut balances = Balances {
            info: serde_json::Value::Null,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies: HashMap::new(),
        };

        if let Some(currency) = &data.currency {
            let free = data.available.as_ref()
                .and_then(|v| Decimal::from_str(v).ok());
            let used = data.frozen.as_ref()
                .and_then(|v| Decimal::from_str(v).ok());
            let total = match (free, used) {
                (Some(f), Some(u)) => Some(f + u),
                (Some(f), None) => Some(f),
                (None, Some(u)) => Some(u),
                _ => None,
            };

            balances.currencies.insert(currency.clone(), Balance {
                free,
                used,
                total,
                debt: None,
            });
        }

        WsBalanceEvent { balances }
    }

    /// 주문 업데이트 파싱
    fn parse_order_update(data: &BitmartOrderData) -> WsOrderEvent {
        let symbol = data.symbol.as_ref()
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let side = match data.side.as_deref() {
            Some("buy") | Some("BUY") => OrderSide::Buy,
            Some("sell") | Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") | Some("LIMIT") => OrderType::Limit,
            Some("market") | Some("MARKET") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let status = match data.state.as_deref() {
            Some("1") | Some("new") | Some("NEW") => OrderStatus::Open,
            Some("2") | Some("filled") | Some("FILLED") => OrderStatus::Closed,
            Some("3") | Some("partially_filled") | Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("4") | Some("canceled") | Some("CANCELED") => OrderStatus::Canceled,
            Some("5") | Some("rejected") | Some("REJECTED") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let price = data.price.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let amount = data.size.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or(Decimal::ZERO);
        let filled = data.filled_size.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or(Decimal::ZERO);

        let timestamp = data.create_time
            .or(data.update_time)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order = Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            symbol: symbol.clone(),
            order_type,
            side,
            status,
            price,
            average: data.price_avg.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            amount,
            filled,
            remaining: Some(amount - filled),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.update_time,
            cost: price.map(|p| p * filled),
            fee: None,
            trades: Vec::new(),
            info: serde_json::Value::Null,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            fees: Vec::new(),
            time_in_force: None,
            post_only: None,
            reduce_only: None,
        };

        WsOrderEvent { order }
    }

    /// 내 체결 파싱
    fn parse_my_trade(data: &BitmartOrderData) -> WsTradeEvent {
        let symbol = data.symbol.as_ref()
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let side = match data.side.as_deref() {
            Some("buy") | Some("BUY") => Some("buy".to_string()),
            Some("sell") | Some("SELL") => Some("sell".to_string()),
            _ => None,
        };

        let price = data.deal_price.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or(Decimal::ZERO);
        let amount = data.deal_vol.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or(Decimal::ZERO);

        let timestamp = data.update_time.unwrap_or_else(|| Utc::now().timestamp_millis());

        let fee = data.deal_fee.as_ref().and_then(|fee_str| {
            let fee_amount = Decimal::from_str(fee_str).ok()?;
            Some(Fee {
                currency: data.fee_currency.clone(),
                cost: Some(fee_amount),
                rate: None,
            })
        });

        let trades = vec![Trade {
            id: data.order_id.clone().unwrap_or_default(),
            info: serde_json::Value::Null,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            order: data.order_id.clone(),
            trade_type: None,
            side,
            taker_or_maker: Some(TakerOrMaker::Taker),
            price,
            amount,
            cost: Some(price * amount),
            fee,
            fees: Vec::new(),
        }];

        WsTradeEvent { symbol, trades }
    }

    /// 포지션 업데이트 파싱
    fn parse_position_update(data: &BitmartPositionData) -> WsPositionEvent {
        let symbol = data.symbol.as_ref()
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let side = match data.position_side.as_deref() {
            Some("long") | Some("LONG") => PositionSide::Long,
            Some("short") | Some("SHORT") => PositionSide::Short,
            _ => PositionSide::Long,
        };

        let contracts = data.hold_vol.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or(Decimal::ZERO);

        let position = Position {
            id: None,
            symbol: symbol.clone(),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            side: Some(side),
            contracts: Some(contracts),
            contract_size: None,
            notional: None,
            leverage: data.leverage.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            unrealized_pnl: data.unrealized_pnl.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            realized_pnl: data.realized_pnl.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            collateral: None,
            entry_price: data.entry_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            mark_price: data.mark_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            liquidation_price: data.liquidation_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            margin_mode: Some(MarginMode::Cross),
            hedged: None,
            maintenance_margin: None,
            maintenance_margin_percentage: None,
            initial_margin: None,
            initial_margin_percentage: None,
            margin_ratio: None,
            last_update_timestamp: None,
            last_price: None,
            percentage: None,
            stop_loss_price: None,
            take_profit_price: None,
            info: serde_json::Value::Null,
        };

        WsPositionEvent { positions: vec![position] }
    }
}

impl Default for BitmartWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BitmartWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": [format!("spot/ticker:{}", formatted)]
        });
        client.subscribe_stream(subscribe_msg, "ticker", Some(&formatted), None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args: Vec<String> = symbols.iter()
            .map(|s| format!("spot/ticker:{}", Self::format_symbol(s)))
            .collect();
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": args
        });
        client.subscribe_stream(subscribe_msg, "tickers", None, None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let depth = limit.unwrap_or(20);
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": [format!("spot/depth{}:{}", depth, formatted)]
        });
        client.subscribe_stream(subscribe_msg, "orderBook", Some(&formatted), None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": [format!("spot/trade:{}", formatted)]
        });
        client.subscribe_stream(subscribe_msg, "trade", Some(&formatted), None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": [format!("spot/kline{}:{}", interval, formatted)]
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
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Config required for authentication".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;
        let memo = config.uid().unwrap_or("");

        let timestamp = Utc::now().timestamp_millis().to_string();
        let signature = Self::generate_auth_signature(api_secret, &timestamp, memo)?;

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PRIVATE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 15,
            connect_timeout_secs: 30,
        });

        let mut _ws_rx = ws_client.connect().await?;

        // 인증 메시지 전송
        let auth_msg = serde_json::json!({
            "op": "login",
            "args": [api_key, timestamp, signature]
        });
        ws_client.send(&auth_msg.to_string())?;

        self.private_ws_client = Some(ws_client);
        *self.authenticated.write().await = true;

        Ok(())
    }
}

// === Response Types ===

#[derive(Debug, Default, Deserialize)]
struct BitmartWsResponse {
    #[serde(default)]
    table: Option<String>,
    #[serde(default)]
    data: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartTickerData {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    ms_t: Option<i64>,
    #[serde(default)]
    last_price: Option<String>,
    #[serde(default)]
    open_24h: Option<String>,
    #[serde(default)]
    high_24h: Option<String>,
    #[serde(default)]
    low_24h: Option<String>,
    #[serde(default)]
    base_volume_24h: Option<String>,
    #[serde(default)]
    quote_volume_24h: Option<String>,
    #[serde(default)]
    price_change_24h: Option<String>,
    #[serde(default)]
    best_bid: Option<String>,
    #[serde(default)]
    best_bid_size: Option<String>,
    #[serde(default)]
    best_ask: Option<String>,
    #[serde(default)]
    best_ask_size: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartOrderBookData {
    #[serde(default)]
    ms_t: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartTradeData {
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    s_t: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartKlineData {
    #[serde(default)]
    candle: Vec<String>,
}

// === Private Response Types ===

#[derive(Debug, Default, Deserialize)]
struct BitmartPrivateResponse {
    #[serde(default)]
    table: Option<String>,
    #[serde(default)]
    data: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartBalanceData {
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    frozen: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartOrderData {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    price_avg: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    filled_size: Option<String>,
    #[serde(default)]
    create_time: Option<i64>,
    #[serde(default)]
    update_time: Option<i64>,
    #[serde(default)]
    deal_price: Option<String>,
    #[serde(default)]
    deal_vol: Option<String>,
    #[serde(default)]
    deal_fee: Option<String>,
    #[serde(default)]
    fee_currency: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartPositionData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    position_side: Option<String>,
    #[serde(default)]
    hold_vol: Option<String>,
    #[serde(default)]
    leverage: Option<String>,
    #[serde(default)]
    entry_price: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    liquidation_price: Option<String>,
    #[serde(default)]
    unrealized_pnl: Option<String>,
    #[serde(default)]
    realized_pnl: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BitmartWs::format_symbol("BTC/USDT"), "BTC_USDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BitmartWs::to_unified_symbol("BTC_USDT"), "BTC/USDT");
    }

    #[test]
    fn test_with_config() {
        let config = ExchangeConfig::new()
            .with_api_key("test_key")
            .with_api_secret("test_secret")
            .with_uid("test_memo");
        let client = BitmartWs::with_config(config);
        assert!(client.config.is_some());
    }

    #[test]
    fn test_clone() {
        let config = ExchangeConfig::new()
            .with_api_key("test_key")
            .with_api_secret("test_secret");
        let client = BitmartWs::with_config(config);
        let cloned = client.clone();
        assert!(cloned.config.is_some());
        assert!(cloned.ws_client.is_none());
    }

    #[test]
    fn test_generate_auth_signature() {
        let result = BitmartWs::generate_auth_signature("secret", "1234567890", "memo");
        assert!(result.is_ok());
        let sig = result.unwrap();
        assert!(!sig.is_empty());
        assert_eq!(sig.len(), 64); // HMAC-SHA256 produces 64 hex chars
    }

    #[test]
    fn test_parse_balance_update() {
        let data = BitmartBalanceData {
            currency: Some("BTC".to_string()),
            available: Some("1.5".to_string()),
            frozen: Some("0.5".to_string()),
        };

        let event = BitmartWs::parse_balance_update(&data);
        assert!(event.balances.currencies.contains_key("BTC"));
        let balance = event.balances.currencies.get("BTC").unwrap();
        assert_eq!(balance.free, Some(Decimal::from_str("1.5").unwrap()));
        assert_eq!(balance.used, Some(Decimal::from_str("0.5").unwrap()));
        assert_eq!(balance.total, Some(Decimal::from_str("2.0").unwrap()));
    }

    #[test]
    fn test_parse_order_update() {
        let data = BitmartOrderData {
            order_id: Some("12345".to_string()),
            client_order_id: None,
            symbol: Some("BTC_USDT".to_string()),
            side: Some("buy".to_string()),
            order_type: Some("limit".to_string()),
            state: Some("1".to_string()),
            price: Some("50000.00".to_string()),
            price_avg: None,
            size: Some("0.1".to_string()),
            filled_size: Some("0.0".to_string()),
            create_time: Some(1640000000000),
            update_time: None,
            deal_price: None,
            deal_vol: None,
            deal_fee: None,
            fee_currency: None,
        };

        let event = BitmartWs::parse_order_update(&data);
        assert_eq!(event.order.id, "12345");
        assert_eq!(event.order.symbol, "BTC/USDT");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.status, OrderStatus::Open);
        assert_eq!(event.order.price, Some(Decimal::from_str("50000.00").unwrap()));
    }

    #[test]
    fn test_parse_my_trade() {
        let data = BitmartOrderData {
            order_id: Some("12345".to_string()),
            client_order_id: None,
            symbol: Some("ETH_USDT".to_string()),
            side: Some("sell".to_string()),
            order_type: None,
            state: None,
            price: None,
            price_avg: None,
            size: None,
            filled_size: None,
            create_time: None,
            update_time: Some(1640000000000),
            deal_price: Some("3000.00".to_string()),
            deal_vol: Some("1.5".to_string()),
            deal_fee: Some("4.5".to_string()),
            fee_currency: Some("USDT".to_string()),
        };

        let event = BitmartWs::parse_my_trade(&data);
        assert_eq!(event.symbol, "ETH/USDT");
        assert_eq!(event.trades.len(), 1);
        let trade = &event.trades[0];
        assert_eq!(trade.price, Decimal::from_str("3000.00").unwrap());
        assert_eq!(trade.amount, Decimal::from_str("1.5").unwrap());
        assert!(trade.fee.is_some());
    }

    #[test]
    fn test_parse_position_update() {
        let data = BitmartPositionData {
            symbol: Some("BTC_USDT".to_string()),
            position_side: Some("long".to_string()),
            hold_vol: Some("1.0".to_string()),
            leverage: Some("10".to_string()),
            entry_price: Some("50000.00".to_string()),
            mark_price: Some("51000.00".to_string()),
            liquidation_price: Some("45000.00".to_string()),
            unrealized_pnl: Some("1000.00".to_string()),
            realized_pnl: None,
        };

        let event = BitmartWs::parse_position_update(&data);
        assert_eq!(event.positions.len(), 1);
        let pos = &event.positions[0];
        assert_eq!(pos.symbol, "BTC/USDT");
        assert_eq!(pos.side, Some(PositionSide::Long));
        assert_eq!(pos.contracts, Some(Decimal::from_str("1.0").unwrap()));
        assert_eq!(pos.leverage, Some(Decimal::from_str("10").unwrap()));
    }

    #[test]
    fn test_process_private_message_balance() {
        let json = r#"{
            "table": "spot/user/balance:BALANCE_UPDATE",
            "data": [{
                "currency": "USDT",
                "available": "10000.00",
                "frozen": "500.00"
            }]
        }"#;

        let result = BitmartWs::process_private_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("USDT"));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_process_private_message_order() {
        let json = r#"{
            "table": "spot/user/order:ORDER_PROGRESS",
            "data": [{
                "order_id": "67890",
                "symbol": "ETH_USDT",
                "side": "sell",
                "state": "2",
                "price": "3500.00",
                "size": "2.0",
                "filled_size": "2.0"
            }]
        }"#;

        let result = BitmartWs::process_private_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "67890");
            assert_eq!(event.order.symbol, "ETH/USDT");
            assert_eq!(event.order.status, OrderStatus::Closed);
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_private_message_position() {
        let json = r#"{
            "table": "futures/user/position",
            "data": [{
                "symbol": "BTC_USDT",
                "position_side": "short",
                "hold_vol": "0.5",
                "leverage": "20",
                "entry_price": "60000.00"
            }]
        }"#;

        let result = BitmartWs::process_private_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Position(event)) = result {
            assert_eq!(event.positions.len(), 1);
            let pos = &event.positions[0];
            assert_eq!(pos.symbol, "BTC/USDT");
            assert_eq!(pos.side, Some(PositionSide::Short));
        } else {
            panic!("Expected Position message");
        }
    }
}
