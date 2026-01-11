//! XT WebSocket Implementation
//!
//! XT Exchange real-time data streaming (Spot & Derivatives)

#![allow(dead_code)]
#![allow(unreachable_patterns)]

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

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Ticker,
    Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsOhlcvEvent, WsOrderBookEvent,
    WsOrderEvent, WsTickerEvent, WsTradeEvent, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

const WS_SPOT_URL: &str = "wss://stream.xt.com/public";
const WS_CONTRACT_URL: &str = "wss://fstream.xt.com/ws/market";

/// XT WebSocket 클라이언트
///
/// XT Exchange는 Spot과 Derivatives 거래를 지원합니다.
/// - Spot WebSocket: wss://stream.xt.com/public
/// - Contract WebSocket: wss://fstream.xt.com/ws/market
/// - 채널: ticker, tickers, kline, trade, depth, depth_update
pub struct XtWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    order_book_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    use_contract: bool,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl XtWs {
    /// 새 XT WebSocket 클라이언트 생성 (Spot)
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
            use_contract: false,
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
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
            use_contract: false,
            api_key: Some(api_key),
            api_secret: Some(api_secret),
        }
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
    }

    // === Private Channel Methods ===

    /// Sign a payload using HMAC-SHA256
    fn sign(&self, payload: &str) -> CcxtResult<String> {
        let secret = self
            .api_secret
            .as_ref()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required for signing".into(),
            })?;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;

        mac.update(payload.as_bytes());
        let result = mac.finalize();
        Ok(hex::encode(result.into_bytes()))
    }

    /// Parse order update from private message
    fn parse_order(&self, data: &XtOrderUpdateData) -> Option<WsMessage> {
        let symbol = data.symbol.as_ref().map(|s| Self::to_unified_symbol(s))?;
        let order_id = data.order_id.clone()?;

        let status = match data.state.as_deref() {
            Some("NEW") | Some("PENDING") => OrderStatus::Open,
            Some("FILLED") | Some("FULL_FILLED") => OrderStatus::Closed,
            Some("CANCELED") | Some("CANCELLED") | Some("REJECTED") => OrderStatus::Canceled,
            Some("PARTIALLY_FILLED") => OrderStatus::Open,
            _ => OrderStatus::Open,
        };

        let side = match data.side.as_deref() {
            Some("BUY") | Some("buy") => OrderSide::Buy,
            Some("SELL") | Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") | Some("limit") => OrderType::Limit,
            Some("MARKET") | Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let amount = data
            .orig_qty
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok());
        let filled = data
            .executed_qty
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok());
        let remaining = match (amount, filled) {
            (Some(a), Some(f)) => Some(a - f),
            _ => None,
        };

        let timestamp = data.create_time;

        let order = Order {
            id: order_id,
            client_order_id: data.client_order_id.clone(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: data.update_time,
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount: amount.unwrap_or_default(),
            filled: filled.unwrap_or_default(),
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: match (price, filled) {
                (Some(p), Some(f)) => Some(p * f),
                _ => None,
            },
            reduce_only: None,
            post_only: None,
            trades: vec![],
            fee: None,
            fees: vec![],
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsMessage::Order(WsOrderEvent { order }))
    }

    /// Parse balance update from private message
    fn parse_balance(&self, balances_data: &[XtBalanceUpdateData]) -> Option<WsMessage> {
        let mut currencies: HashMap<String, Balance> = HashMap::new();

        for data in balances_data {
            if let Some(currency) = &data.currency {
                let free = data
                    .available_amount
                    .as_ref()
                    .and_then(|a| Decimal::from_str(a).ok());
                let used = data
                    .frozen_amount
                    .as_ref()
                    .and_then(|f| Decimal::from_str(f).ok());
                let total = data
                    .total_amount
                    .as_ref()
                    .and_then(|t| Decimal::from_str(t).ok());

                currencies.insert(
                    currency.clone(),
                    Balance {
                        free,
                        used,
                        total,
                        debt: None,
                    },
                );
            }
        }

        if currencies.is_empty() {
            return None;
        }

        let now = Utc::now();
        Some(WsMessage::Balance(WsBalanceEvent {
            balances: Balances {
                timestamp: Some(now.timestamp_millis()),
                datetime: Some(now.to_rfc3339()),
                currencies,
                info: serde_json::to_value(balances_data).unwrap_or_default(),
            },
        }))
    }

    /// Contract WebSocket 클라이언트 생성
    pub fn new_contract() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
            use_contract: true,
            api_key: None,
            api_secret: None,
        }
    }

    /// 심볼을 XT 형식으로 변환 (BTC/USDT -> btc_usdt)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_").to_lowercase()
    }

    /// XT 심볼을 통합 심볼로 변환 (btc_usdt -> BTC/USDT)
    fn to_unified_symbol(xt_symbol: &str) -> String {
        let parts: Vec<&str> = xt_symbol.split('_').collect();
        if parts.len() >= 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            xt_symbol.to_uppercase()
        }
    }

    /// Parse timeframe string to Timeframe enum
    fn parse_timeframe(tf: &str) -> Timeframe {
        match tf {
            "1m" => Timeframe::Minute1,
            "3m" => Timeframe::Minute3,
            "5m" => Timeframe::Minute5,
            "15m" => Timeframe::Minute15,
            "30m" => Timeframe::Minute30,
            "1h" => Timeframe::Hour1,
            "2h" => Timeframe::Hour2,
            "4h" => Timeframe::Hour4,
            "6h" => Timeframe::Hour6,
            "12h" => Timeframe::Hour12,
            "1d" => Timeframe::Day1,
            "1w" => Timeframe::Week1,
            "1M" => Timeframe::Month1,
            _ => Timeframe::Minute1, // default
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &XtTickerData) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.h,
            low: data.l,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: data.o,
            close: data.c,
            last: data.c,
            previous_close: None,
            change: data.cv,
            percentage: data.cr,
            average: None,
            base_volume: data.q,
            quote_volume: data.v,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent {
            symbol: unified_symbol,
            ticker,
        }
    }

    /// OHLCV 메시지 파싱
    fn parse_ohlcv(data: &XtKlineData, timeframe: &str) -> WsOhlcvEvent {
        let unified_symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ohlcv = OHLCV {
            timestamp,
            open: data.o.unwrap_or_default(),
            high: data.h.unwrap_or_default(),
            low: data.l.unwrap_or_default(),
            close: data.c.unwrap_or_default(),
            volume: data.q.or(data.a).unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe: Self::parse_timeframe(timeframe),
            ohlcv,
        }
    }

    /// 호가창 스냅샷 파싱
    fn parse_order_book(data: &XtDepthData, is_snapshot: bool) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.s);
        let timestamp = Utc::now().timestamp_millis();

        let bids: Vec<OrderBookEntry> = data
            .b
            .iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    let price = entry[0].parse::<Decimal>().ok()?;
                    let amount = entry[1].parse::<Decimal>().ok()?;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .a
            .iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    let price = entry[0].parse::<Decimal>().ok()?;
                    let amount = entry[1].parse::<Decimal>().ok()?;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
            })
            .collect();

        let nonce = data.i.or(data.u);

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce,
            bids,
            asks,
            checksum: None,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot,
        }
    }

    /// 호가창 업데이트 적용
    fn apply_order_book_update(cache: &mut OrderBook, data: &XtDepthData) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        // 비드 업데이트
        for entry in &data.b {
            if entry.len() >= 2 {
                if let (Ok(price), Ok(qty)) =
                    (entry[0].parse::<Decimal>(), entry[1].parse::<Decimal>())
                {
                    cache.bids.retain(|e| e.price != price);
                    if qty > Decimal::ZERO {
                        cache.bids.push(OrderBookEntry { price, amount: qty });
                    }
                }
            }
        }
        cache.bids.sort_by(|a, b| b.price.cmp(&a.price));

        // 애스크 업데이트
        for entry in &data.a {
            if entry.len() >= 2 {
                if let (Ok(price), Ok(qty)) =
                    (entry[0].parse::<Decimal>(), entry[1].parse::<Decimal>())
                {
                    cache.asks.retain(|e| e.price != price);
                    if qty > Decimal::ZERO {
                        cache.asks.push(OrderBookEntry { price, amount: qty });
                    }
                }
            }
        }
        cache.asks.sort_by(|a, b| a.price.cmp(&b.price));

        cache.timestamp = Some(timestamp);
        cache.datetime = Some(
            chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        );
        cache.nonce = data.i.or(data.u);

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book: cache.clone(),
            is_snapshot: false,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &XtTradeData) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        // 사이드 결정
        let side = if let Some(b) = data.b {
            if b {
                Some("buy".to_string())
            } else {
                Some("sell".to_string())
            }
        } else {
            data.m.as_ref().map(|m| m.to_lowercase())
        };

        let trade = Trade {
            id: data.i.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker: None,
            price: data.p.unwrap_or_default(),
            amount: data.q.or(data.a).unwrap_or_default(),
            cost: data.p.and_then(|p| data.q.or(data.a).map(|q| p * q)),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTradeEvent {
            symbol: unified_symbol,
            trades: vec![trade],
        }
    }

    /// 메시지 처리
    fn process_message(
        msg: &str,
        order_book_cache: &mut HashMap<String, OrderBook>,
    ) -> Option<WsMessage> {
        // Pong 처리
        if msg == "pong" {
            return None;
        }

        if let Ok(response) = serde_json::from_str::<XtWsResponse>(msg) {
            let topic = response.topic.as_deref();
            let event = response.event.as_deref().unwrap_or("");

            match topic {
                Some("ticker") | Some("agg_ticker") => {
                    if let Some(data) = response.data_ticker {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&data)));
                    }
                },
                Some("tickers") | Some("agg_tickers") => {
                    if let Some(data_list) = response.data_tickers {
                        if let Some(data) = data_list.first() {
                            return Some(WsMessage::Ticker(Self::parse_ticker(data)));
                        }
                    }
                },
                Some("kline") => {
                    if let Some(data) = response.data_kline {
                        // event에서 timeframe 추출 (예: kline@btc_usdt,5m)
                        let timeframe = event.split(',').next_back().unwrap_or("1m");
                        return Some(WsMessage::Ohlcv(Self::parse_ohlcv(&data, timeframe)));
                    }
                },
                Some("trade") => {
                    if let Some(data) = response.data_trade {
                        return Some(WsMessage::Trade(Self::parse_trade(&data)));
                    }
                },
                Some("depth") => {
                    if let Some(data) = response.data_depth {
                        let unified_symbol = Self::to_unified_symbol(&data.s);
                        let event = Self::parse_order_book(&data, true);
                        order_book_cache.insert(unified_symbol, event.order_book.clone());
                        return Some(WsMessage::OrderBook(event));
                    }
                },
                Some("depth_update") => {
                    if let Some(data) = response.data_depth {
                        let unified_symbol = Self::to_unified_symbol(&data.s);
                        if let Some(cache) = order_book_cache.get_mut(&unified_symbol) {
                            let event = Self::apply_order_book_update(cache, &data);
                            return Some(WsMessage::OrderBook(event));
                        } else {
                            // 캐시가 없으면 스냅샷으로 처리
                            let event = Self::parse_order_book(&data, false);
                            order_book_cache.insert(unified_symbol, event.order_book.clone());
                            return Some(WsMessage::OrderBook(event));
                        }
                    }
                },
                _ => {},
            }
        }

        // 구독 확인 메시지
        if let Ok(status) = serde_json::from_str::<XtSubscriptionStatus>(msg) {
            if status.code == Some(0) {
                return Some(WsMessage::Subscribed {
                    channel: status.id.unwrap_or_default(),
                    symbol: None,
                });
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        channels: Vec<String>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let url = if self.use_contract {
            WS_CONTRACT_URL
        } else {
            WS_SPOT_URL
        };

        let mut ws_client = WsClient::new(WsConfig {
            url: url.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 20,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        let method = if self.use_contract {
            "SUBSCRIBE"
        } else {
            "subscribe"
        };
        let id = format!("{}{}", Utc::now().timestamp_millis(), channels.join("_"));
        let subscribe_msg = serde_json::json!({
            "method": method,
            "id": id,
            "params": channels
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = channels.join(",");
            self.subscriptions.write().await.insert(key.clone(), key);
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let order_book_cache = Arc::clone(&self.order_book_cache);

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
                        let mut cache = order_book_cache.write().await;
                        if let Some(ws_msg) = Self::process_message(&msg, &mut cache) {
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

impl Default for XtWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for XtWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract {
            Self::new_contract()
        } else {
            Self::new()
        };
        let xt_symbol = Self::format_symbol(symbol);
        let channel = format!("ticker@{xt_symbol}");
        client.subscribe_stream(vec![channel]).await
    }

    async fn watch_tickers(
        &self,
        _symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract {
            Self::new_contract()
        } else {
            Self::new()
        };
        // XT는 tickers로 전체 티커를 구독
        client.subscribe_stream(vec!["tickers".to_string()]).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract {
            Self::new_contract()
        } else {
            Self::new()
        };
        let xt_symbol = Self::format_symbol(symbol);
        let channel = if let Some(l) = limit {
            format!("depth@{xt_symbol},{l}")
        } else {
            format!("depth_update@{xt_symbol}")
        };
        client.subscribe_stream(vec![channel]).await
    }

    async fn watch_order_book_for_symbols(
        &self,
        symbols: &[&str],
        limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract {
            Self::new_contract()
        } else {
            Self::new()
        };
        let channels: Vec<String> = symbols
            .iter()
            .map(|s| {
                let xt_symbol = Self::format_symbol(s);
                if let Some(l) = limit {
                    format!("depth@{xt_symbol},{l}")
                } else {
                    format!("depth_update@{xt_symbol}")
                }
            })
            .collect();
        client.subscribe_stream(channels).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract {
            Self::new_contract()
        } else {
            Self::new()
        };
        let xt_symbol = Self::format_symbol(symbol);
        let channel = format!("trade@{xt_symbol}");
        client.subscribe_stream(vec![channel]).await
    }

    async fn watch_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract {
            Self::new_contract()
        } else {
            Self::new()
        };
        let channels: Vec<String> = symbols
            .iter()
            .map(|s| format!("trade@{}", Self::format_symbol(s)))
            .collect();
        client.subscribe_stream(channels).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract {
            Self::new_contract()
        } else {
            Self::new()
        };
        let xt_symbol = Self::format_symbol(symbol);
        let tf_str = match timeframe {
            Timeframe::Second1 => "1m", // XT doesn't support 1s, use 1m
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour3 => "4h", // XT doesn't support 3h, use 4h
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour8 => "8h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Day3 => "3d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1M",
        };
        let channel = format!("kline@{xt_symbol},{tf_str}");
        client.subscribe_stream(vec![channel]).await
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        symbols: &[&str],
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract {
            Self::new_contract()
        } else {
            Self::new()
        };
        let tf_str = match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour4 => "4h",
            Timeframe::Day1 => "1d",
            _ => "1h",
        };
        let channels: Vec<String> = symbols
            .iter()
            .map(|s| format!("kline@{},{}", Self::format_symbol(s), tf_str))
            .collect();
        client.subscribe_stream(channels).await
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
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        // XT private channels require private WebSocket connection
        // Full implementation would:
        // 1. Connect to private WebSocket endpoint
        // 2. Authenticate with HMAC-SHA256 signature
        // 3. Subscribe to order update channel
        // 4. Process messages through parse_order
        Err(CcxtError::NotSupported {
            feature: "watch_orders - XT private WebSocket requires dedicated implementation"
                .to_string(),
        })
    }

    async fn watch_my_trades(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        // XT private channels require private WebSocket connection
        Err(CcxtError::NotSupported {
            feature: "watch_my_trades - XT private WebSocket requires dedicated implementation"
                .to_string(),
        })
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        // XT private channels require private WebSocket connection
        Err(CcxtError::NotSupported {
            feature: "watch_balance - XT private WebSocket requires dedicated implementation"
                .to_string(),
        })
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required".into(),
            });
        }

        // XT authentication would involve:
        // 1. Generate timestamp
        // 2. Create signature using HMAC-SHA256
        // 3. Send authentication message to private WebSocket
        Ok(())
    }
}

// === XT WebSocket Types ===

#[derive(Debug, Deserialize)]
struct XtWsResponse {
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    event: Option<String>,
    #[serde(default, rename = "data")]
    data_ticker: Option<XtTickerData>,
    #[serde(default, rename = "data")]
    data_tickers: Option<Vec<XtTickerData>>,
    #[serde(default, rename = "data")]
    data_kline: Option<XtKlineData>,
    #[serde(default, rename = "data")]
    data_trade: Option<XtTradeData>,
    #[serde(default, rename = "data")]
    data_depth: Option<XtDepthData>,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
struct XtTickerData {
    #[serde(default)]
    s: String, // symbol
    #[serde(default)]
    t: Option<i64>, // timestamp
    #[serde(default)]
    cv: Option<Decimal>, // price change value (spot)
    #[serde(default)]
    cr: Option<Decimal>, // price change rate (spot)
    #[serde(default)]
    ch: Option<Decimal>, // price change (contract)
    #[serde(default)]
    o: Option<Decimal>, // open
    #[serde(default)]
    c: Option<Decimal>, // close
    #[serde(default)]
    h: Option<Decimal>, // high
    #[serde(default)]
    l: Option<Decimal>, // low
    #[serde(default)]
    q: Option<Decimal>, // quantity (base volume)
    #[serde(default)]
    v: Option<Decimal>, // volume (quote volume)
    #[serde(default)]
    a: Option<Decimal>, // amount (contract)
    #[serde(default)]
    i: Option<Decimal>, // index price
    #[serde(default)]
    m: Option<Decimal>, // mark price
    #[serde(default)]
    bp: Option<Decimal>, // best bid
    #[serde(default)]
    ap: Option<Decimal>, // best ask
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct XtKlineData {
    #[serde(default)]
    s: String, // symbol
    #[serde(default)]
    t: Option<i64>, // timestamp
    #[serde(default)]
    i: Option<String>, // interval
    #[serde(default)]
    o: Option<Decimal>,
    #[serde(default)]
    h: Option<Decimal>,
    #[serde(default)]
    l: Option<Decimal>,
    #[serde(default)]
    c: Option<Decimal>,
    #[serde(default)]
    q: Option<Decimal>, // quantity (spot)
    #[serde(default)]
    v: Option<Decimal>, // volume
    #[serde(default)]
    a: Option<Decimal>, // amount (contract)
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct XtTradeData {
    #[serde(default)]
    s: String, // symbol
    #[serde(default)]
    i: Option<String>, // trade id
    #[serde(default)]
    t: Option<i64>, // timestamp
    #[serde(default)]
    p: Option<Decimal>, // price
    #[serde(default)]
    q: Option<Decimal>, // quantity (spot)
    #[serde(default)]
    a: Option<Decimal>, // amount (contract)
    #[serde(default)]
    b: Option<bool>, // is buy (spot)
    #[serde(default)]
    m: Option<String>, // side: BID/ASK (contract)
}

#[derive(Debug, Default, Deserialize)]
struct XtDepthData {
    #[serde(default)]
    s: String, // symbol
    #[serde(default)]
    fi: Option<i64>, // first update id (spot)
    #[serde(default)]
    i: Option<i64>, // update id (spot)
    #[serde(default)]
    pu: Option<i64>, // previous update id (contract)
    #[serde(default)]
    fu: Option<i64>, // first update id (contract)
    #[serde(default)]
    u: Option<i64>, // update id (contract)
    #[serde(default)]
    t: Option<i64>, // timestamp
    #[serde(default)]
    a: Vec<Vec<String>>, // asks [[price, qty], ...]
    #[serde(default)]
    b: Vec<Vec<String>>, // bids [[price, qty], ...]
}

#[derive(Debug, Default, Deserialize)]
struct XtSubscriptionStatus {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    method: Option<String>,
}

// === Private Channel Types ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct XtOrderUpdateData {
    #[serde(default, rename = "orderId")]
    order_id: Option<String>,
    #[serde(default, rename = "clientOrderId")]
    client_order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default, rename = "origQty")]
    orig_qty: Option<String>,
    #[serde(default, rename = "executedQty")]
    executed_qty: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default, rename = "createTime")]
    create_time: Option<i64>,
    #[serde(default, rename = "updateTime")]
    update_time: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct XtBalanceUpdateData {
    #[serde(default)]
    currency: Option<String>,
    #[serde(default, rename = "availableAmount")]
    available_amount: Option<String>,
    #[serde(default, rename = "frozenAmount")]
    frozen_amount: Option<String>,
    #[serde(default, rename = "totalAmount")]
    total_amount: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xt_ws_creation() {
        let _ws = XtWs::new();
        let _ws_contract = XtWs::new_contract();
    }

    #[test]
    fn test_format_symbol() {
        assert_eq!(XtWs::format_symbol("BTC/USDT"), "btc_usdt");
        assert_eq!(XtWs::format_symbol("ETH/BTC"), "eth_btc");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(XtWs::to_unified_symbol("btc_usdt"), "BTC/USDT");
        assert_eq!(XtWs::to_unified_symbol("eth_btc"), "ETH/BTC");
    }

    #[test]
    fn test_parse_ticker() {
        let ticker_data = XtTickerData {
            s: "btc_usdt".to_string(),
            t: Some(1683501935877),
            o: Some(Decimal::from(28823)),
            c: Some(Decimal::from(28741)),
            h: Some(Decimal::from(29137)),
            l: Some(Decimal::from(28660)),
            q: Some(Decimal::from(6372)),
            v: Some(Decimal::from(184086075)),
            ..Default::default()
        };

        let result = XtWs::parse_ticker(&ticker_data);
        assert_eq!(result.symbol, "BTC/USDT");
        assert_eq!(result.ticker.last, Some(Decimal::from(28741)));
    }

    #[test]
    fn test_with_credentials() {
        let _ws = XtWs::with_credentials("api_key".to_string(), "api_secret".to_string());
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = XtWs::new();
        ws.set_credentials("api_key".to_string(), "api_secret".to_string());
    }

    #[test]
    fn test_sign() {
        let ws = XtWs::with_credentials("test_api_key".to_string(), "test_api_secret".to_string());

        let signature = ws.sign("test_payload").unwrap();
        assert!(!signature.is_empty());
        assert_eq!(signature.len(), 64); // SHA256 produces 32 bytes = 64 hex chars
    }

    #[test]
    fn test_parse_order() {
        let ws = XtWs::new();
        let order_data = XtOrderUpdateData {
            order_id: Some("123456789".to_string()),
            client_order_id: Some("client_123".to_string()),
            symbol: Some("btc_usdt".to_string()),
            side: Some("BUY".to_string()),
            order_type: Some("LIMIT".to_string()),
            price: Some("50000.00".to_string()),
            orig_qty: Some("0.1".to_string()),
            executed_qty: Some("0.05".to_string()),
            state: Some("PARTIALLY_FILLED".to_string()),
            create_time: Some(1700000000000),
            update_time: Some(1700000001000),
        };

        let result = ws.parse_order(&order_data);
        assert!(result.is_some());

        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.symbol, "BTC/USDT");
            assert_eq!(event.order.id, "123456789");
            assert_eq!(event.order.side, OrderSide::Buy);
            assert_eq!(event.order.order_type, OrderType::Limit);
            assert_eq!(event.order.status, OrderStatus::Open);
            assert_eq!(event.order.filled, Decimal::from_str("0.05").unwrap());
        }
    }

    #[test]
    fn test_parse_order_status() {
        let ws = XtWs::new();

        // Test FILLED status
        let filled_order = XtOrderUpdateData {
            order_id: Some("1".to_string()),
            symbol: Some("eth_usdt".to_string()),
            state: Some("FILLED".to_string()),
            ..Default::default()
        };
        if let Some(WsMessage::Order(event)) = ws.parse_order(&filled_order) {
            assert_eq!(event.order.status, OrderStatus::Closed);
        }

        // Test CANCELED status
        let canceled_order = XtOrderUpdateData {
            order_id: Some("2".to_string()),
            symbol: Some("eth_usdt".to_string()),
            state: Some("CANCELED".to_string()),
            ..Default::default()
        };
        if let Some(WsMessage::Order(event)) = ws.parse_order(&canceled_order) {
            assert_eq!(event.order.status, OrderStatus::Canceled);
        }
    }

    #[test]
    fn test_parse_balance() {
        let ws = XtWs::new();
        let balances_data = vec![
            XtBalanceUpdateData {
                currency: Some("USDT".to_string()),
                available_amount: Some("1000.50".to_string()),
                frozen_amount: Some("100.25".to_string()),
                total_amount: Some("1100.75".to_string()),
            },
            XtBalanceUpdateData {
                currency: Some("BTC".to_string()),
                available_amount: Some("0.5".to_string()),
                frozen_amount: Some("0.1".to_string()),
                total_amount: Some("0.6".to_string()),
            },
        ];

        let result = ws.parse_balance(&balances_data);
        assert!(result.is_some());

        if let Some(WsMessage::Balance(event)) = result {
            assert_eq!(event.balances.currencies.len(), 2);

            let usdt = event.balances.currencies.get("USDT").unwrap();
            assert_eq!(usdt.free, Some(Decimal::from_str("1000.50").unwrap()));
            assert_eq!(usdt.used, Some(Decimal::from_str("100.25").unwrap()));

            let btc = event.balances.currencies.get("BTC").unwrap();
            assert_eq!(btc.free, Some(Decimal::from_str("0.5").unwrap()));
        }
    }
}
