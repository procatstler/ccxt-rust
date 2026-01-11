//! Binance Futures (USDⓈ-M) WebSocket Implementation
//!
//! Binance USDⓈ-M Futures 실시간 데이터 스트리밍

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use std::str::FromStr;

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, MarginMode, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, Position, PositionSide, TakerOrMaker, Ticker, Timeframe,
    TimeInForce, Trade, OHLCV, WsBalanceEvent, WsExchange, WsMessage, WsMyTradeEvent,
    WsOrderBookEvent, WsOrderEvent, WsOhlcvEvent, WsPositionEvent, WsTickerEvent, WsTradeEvent,
};

const WS_BASE_URL: &str = "wss://fstream.binance.com/ws";
const WS_COMBINED_URL: &str = "wss://fstream.binance.com/stream";

/// Binance Futures WebSocket 클라이언트
pub struct BinanceFuturesWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
    listen_key: Option<String>,
}

impl BinanceFuturesWs {
    /// 새 Binance Futures WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: None,
            api_secret: None,
            listen_key: None,
        }
    }

    /// API 자격 증명으로 클라이언트 생성
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: Some(api_key),
            api_secret: Some(api_secret),
            listen_key: None,
        }
    }

    /// API 자격 증명 설정
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
    }

    /// API 키 반환
    pub fn get_api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Listen Key 설정
    pub fn set_listen_key(&mut self, listen_key: String) {
        self.listen_key = Some(listen_key);
    }

    /// Listen Key 반환
    pub fn get_listen_key(&self) -> Option<&str> {
        self.listen_key.as_deref()
    }

    /// 심볼을 Binance 형식으로 변환 (BTC/USDT:USDT -> btcusdt)
    fn format_symbol(symbol: &str) -> String {
        // Remove settle currency (BTC/USDT:USDT -> BTC/USDT)
        let base_symbol = symbol.split(':').next().unwrap_or(symbol);
        base_symbol.replace("/", "").to_lowercase()
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

    /// Binance 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT:USDT)
    fn to_unified_symbol(binance_symbol: &str) -> String {
        // Quote currencies for futures
        let quotes = ["USDT", "BUSD", "USDC"];

        for quote in quotes {
            if let Some(base) = binance_symbol.strip_suffix(quote) {
                return format!("{base}/{quote}:{quote}");
            }
        }

        binance_symbol.to_string()
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BinanceFuturesTicker) -> WsTickerEvent {
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
            vwap: data.w.as_ref().and_then(|v| v.parse().ok()),
            open: data.o.as_ref().and_then(|v| v.parse().ok()),
            close: data.c.as_ref().and_then(|v| v.parse().ok()),
            last: data.c.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.p.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.P.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.v.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.q.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// Mark Price 티커 파싱
    fn parse_mark_price_ticker(data: &BinanceMarkPriceUpdate) -> WsTickerEvent {
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
            high: None,
            low: None,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: None,
            close: None,
            last: None,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: None,
            quote_volume: None,
            index_price: data.i.as_ref().and_then(|v| v.parse().ok()),
            mark_price: data.p.as_ref().and_then(|v| v.parse().ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BinanceFuturesDepthUpdate, symbol: &str) -> WsOrderBookEvent {
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
    fn parse_trade(data: &BinanceFuturesTradeMsg) -> WsTradeEvent {
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
    fn parse_kline(data: &BinanceFuturesKlineMsg, timeframe: Timeframe) -> WsOhlcvEvent {
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

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Error check
        if let Ok(err) = serde_json::from_str::<BinanceFuturesError>(msg) {
            if err.code.is_some() {
                return Some(WsMessage::Error(err.msg.unwrap_or_default()));
            }
        }

        // 24hr ticker
        if msg.contains("\"e\":\"24hrTicker\"") {
            if let Ok(data) = serde_json::from_str::<BinanceFuturesTicker>(msg) {
                return Some(WsMessage::Ticker(Self::parse_ticker(&data)));
            }
        }

        // miniTicker
        if msg.contains("\"e\":\"24hrMiniTicker\"") {
            if let Ok(data) = serde_json::from_str::<BinanceFuturesTicker>(msg) {
                return Some(WsMessage::Ticker(Self::parse_ticker(&data)));
            }
        }

        // Mark price
        if msg.contains("\"e\":\"markPriceUpdate\"") {
            if let Ok(data) = serde_json::from_str::<BinanceMarkPriceUpdate>(msg) {
                return Some(WsMessage::Ticker(Self::parse_mark_price_ticker(&data)));
            }
        }

        // Depth update
        if msg.contains("\"e\":\"depthUpdate\"") {
            if let Ok(data) = serde_json::from_str::<BinanceFuturesDepthUpdate>(msg) {
                let symbol = Self::to_unified_symbol(&data.s);
                return Some(WsMessage::OrderBook(Self::parse_order_book(&data, &symbol)));
            }
        }

        // Aggregate trade
        if msg.contains("\"e\":\"aggTrade\"") {
            if let Ok(data) = serde_json::from_str::<BinanceFuturesTradeMsg>(msg) {
                return Some(WsMessage::Trade(Self::parse_trade(&data)));
            }
        }

        // Kline
        if msg.contains("\"e\":\"kline\"") {
            if let Ok(data) = serde_json::from_str::<BinanceFuturesKlineMsg>(msg) {
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

        // === Private Stream Events ===

        // ORDER_TRADE_UPDATE - Order and trade updates
        if msg.contains("\"e\":\"ORDER_TRADE_UPDATE\"") {
            if let Ok(data) = serde_json::from_str::<BinanceFuturesOrderUpdate>(msg) {
                // Check if this is a fill event
                if data.o.x == "TRADE" {
                    // This is a trade/fill event
                    if let Some(trade_event) = Self::parse_my_trade(&data) {
                        return Some(WsMessage::MyTrade(trade_event));
                    }
                }
                // Always send order update
                if let Some(order_event) = Self::parse_order_update(&data) {
                    return Some(WsMessage::Order(order_event));
                }
            }
        }

        // ACCOUNT_UPDATE - Balance and position updates
        if msg.contains("\"e\":\"ACCOUNT_UPDATE\"") {
            if let Ok(data) = serde_json::from_str::<BinanceFuturesAccountUpdate>(msg) {
                // Send position updates if available
                if !data.a.P.is_empty() {
                    if let Some(position_event) = Self::parse_position_update(&data) {
                        return Some(WsMessage::Position(position_event));
                    }
                }
                // Send balance updates if available
                if !data.a.B.is_empty() {
                    if let Some(balance_event) = Self::parse_balance_update(&data) {
                        return Some(WsMessage::Balance(balance_event));
                    }
                }
            }
        }

        // listenKeyExpired - Need to reconnect
        if msg.contains("\"e\":\"listenKeyExpired\"") {
            return Some(WsMessage::Error("Listen key expired, need to renew".into()));
        }

        None
    }

    /// 주문 업데이트 파싱
    fn parse_order_update(data: &BinanceFuturesOrderUpdate) -> Option<WsOrderEvent> {
        let o = &data.o;
        let symbol = Self::to_unified_symbol(&o.s);
        let timestamp = data.E.unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = match o.S.as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match o.o.as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            "STOP" => OrderType::StopLimit,
            "STOP_MARKET" => OrderType::StopMarket,
            "TAKE_PROFIT" => OrderType::TakeProfitLimit,
            "TAKE_PROFIT_MARKET" => OrderType::TakeProfitMarket,
            "TRAILING_STOP_MARKET" => OrderType::TrailingStopMarket,
            _ => OrderType::Limit,
        };

        let status = match o.X.as_str() {
            "NEW" => OrderStatus::Open,
            "PARTIALLY_FILLED" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" => OrderStatus::Canceled,
            "EXPIRED" => OrderStatus::Expired,
            "REJECTED" => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let time_in_force = match o.f.as_str() {
            "GTC" => Some(TimeInForce::GTC),
            "IOC" => Some(TimeInForce::IOC),
            "FOK" => Some(TimeInForce::FOK),
            "GTX" => Some(TimeInForce::PO), // Binance GTX = Post-Only
            _ => None,
        };

        let amount = Decimal::from_str(&o.q).unwrap_or_default();
        let filled = Decimal::from_str(&o.z).unwrap_or_default();
        let price = Decimal::from_str(&o.p).unwrap_or_default();
        let average = if filled > Decimal::ZERO {
            let total_cost = Decimal::from_str(&o.Z).unwrap_or_default();
            if total_cost > Decimal::ZERO {
                Some(total_cost / filled)
            } else {
                None
            }
        } else {
            None
        };

        let remaining = amount - filled;
        let cost = average.map(|avg| filled * avg);

        let fee = if o.n.is_some() || o.N.is_some() {
            Some(Fee {
                currency: o.N.clone(),
                cost: o.n.as_ref().and_then(|n| Decimal::from_str(n).ok()),
                rate: None,
            })
        } else {
            None
        };

        let order_timestamp = o.T.unwrap_or(timestamp);
        let order = Order {
            id: o.i.to_string(),
            client_order_id: if o.c.is_empty() { None } else { Some(o.c.clone()) },
            timestamp: Some(order_timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(order_timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: Some(timestamp),
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force,
            side,
            price: Some(price),
            trigger_price: o.sp.as_ref().and_then(|sp| Decimal::from_str(sp).ok()),
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            average,
            amount,
            filled,
            remaining: Some(remaining),
            cost,
            trades: Vec::new(),
            reduce_only: Some(o.R),
            post_only: Some(o.f == "GTX"),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsOrderEvent { order })
    }

    /// 내 체결 파싱
    fn parse_my_trade(data: &BinanceFuturesOrderUpdate) -> Option<WsMyTradeEvent> {
        let o = &data.o;
        let symbol = Self::to_unified_symbol(&o.s);
        let event_timestamp = data.E.unwrap_or_else(|| Utc::now().timestamp_millis());
        let timestamp = o.T.unwrap_or(event_timestamp);

        let side = if o.S == "BUY" { Some("buy".to_string()) } else { Some("sell".to_string()) };

        let taker_or_maker = if o.m {
            Some(TakerOrMaker::Maker)
        } else {
            Some(TakerOrMaker::Taker)
        };

        let last_qty = Decimal::from_str(&o.l).unwrap_or_default();
        let last_price = Decimal::from_str(&o.L).unwrap_or_default();

        if last_qty == Decimal::ZERO {
            return None;
        }

        let fee = if o.n.is_some() || o.N.is_some() {
            Some(Fee {
                currency: o.N.clone(),
                cost: o.n.as_ref().and_then(|n| Decimal::from_str(n).ok()),
                rate: None,
            })
        } else {
            None
        };

        let trade = Trade {
            id: o.t.map(|t| t.to_string()).unwrap_or_default(),
            order: Some(o.i.to_string()),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker,
            price: last_price,
            amount: last_qty,
            cost: Some(last_price * last_qty),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsMyTradeEvent {
            symbol,
            trades: vec![trade],
        })
    }

    /// 포지션 업데이트 파싱
    fn parse_position_update(data: &BinanceFuturesAccountUpdate) -> Option<WsPositionEvent> {
        let timestamp = data.E.unwrap_or_else(|| Utc::now().timestamp_millis());
        let mut positions = Vec::new();

        for p in &data.a.P {
            let symbol = Self::to_unified_symbol(&p.s);
            let contracts = Decimal::from_str(&p.pa).unwrap_or_default();
            let entry_price = Decimal::from_str(&p.ep).unwrap_or_default();
            let unrealized_pnl = Decimal::from_str(&p.up).unwrap_or_default();

            let side = if contracts > Decimal::ZERO {
                Some(PositionSide::Long)
            } else if contracts < Decimal::ZERO {
                Some(PositionSide::Short)
            } else {
                None
            };

            let margin_mode = match p.mt.as_str() {
                "cross" => Some(MarginMode::Cross),
                "isolated" => Some(MarginMode::Isolated),
                _ => None,
            };

            let position = Position {
                id: None,
                symbol: symbol.clone(),
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                side,
                contracts: Some(contracts.abs()),
                contract_size: None,
                entry_price: Some(entry_price),
                mark_price: None,
                last_price: None,
                notional: None,
                leverage: None,
                collateral: None,
                initial_margin: None,
                maintenance_margin: None,
                initial_margin_percentage: None,
                maintenance_margin_percentage: None,
                unrealized_pnl: Some(unrealized_pnl),
                realized_pnl: None,
                liquidation_price: None,
                margin_mode,
                margin_ratio: None,
                percentage: None,
                hedged: None,
                last_update_timestamp: Some(timestamp),
                stop_loss_price: None,
                take_profit_price: None,
                info: serde_json::to_value(p).unwrap_or_default(),
            };

            positions.push(position);
        }

        if positions.is_empty() {
            None
        } else {
            Some(WsPositionEvent { positions })
        }
    }

    /// 잔고 업데이트 파싱
    fn parse_balance_update(data: &BinanceFuturesAccountUpdate) -> Option<WsBalanceEvent> {
        let timestamp = data.E.unwrap_or_else(|| Utc::now().timestamp_millis());
        let mut currencies: HashMap<String, Balance> = HashMap::new();

        for b in &data.a.B {
            let wallet_balance = Decimal::from_str(&b.wb).unwrap_or_default();
            let cross_wallet_balance = Decimal::from_str(&b.cw).unwrap_or_default();
            let _balance_change = Decimal::from_str(&b.bc).unwrap_or_default();

            // For futures, 'free' is wallet balance, 'used' is in position margin
            let free = cross_wallet_balance;
            let used = wallet_balance - cross_wallet_balance;
            let total = wallet_balance;

            let balance = Balance {
                free: Some(free),
                used: Some(used),
                total: Some(total),
                debt: None,
            };

            currencies.insert(b.a.clone(), balance);
        }

        if currencies.is_empty() {
            None
        } else {
            Some(WsBalanceEvent {
                balances: Balances {
                    currencies,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    info: serde_json::to_value(data).unwrap_or_default(),
                },
            })
        }
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, stream: &str, channel: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let url = format!("{WS_BASE_URL}/{stream}");

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

        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, stream.to_string());
        }

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

    /// 복수 스트림 구독
    async fn subscribe_combined_streams(&mut self, streams: Vec<String>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let combined = streams.join("/");
        let url = format!("{WS_COMBINED_URL}?streams={combined}");

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
                        // Combined stream format: {"stream":"btcusdt@aggTrade","data":{...}}
                        if let Ok(combined) = serde_json::from_str::<BinanceFuturesCombinedStream>(&msg) {
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

    /// Private 스트림 구독 (listenKey 사용)
    async fn subscribe_private_stream(&mut self, listen_key: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());
        self.listen_key = Some(listen_key.to_string());

        let url = format!("{WS_BASE_URL}/{listen_key}");

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

        {
            self.subscriptions.write().await.insert("private".to_string(), listen_key.to_string());
        }

        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                        // Private stream은 연결 즉시 인증됨 (listenKey 사용)
                        let _ = tx.send(WsMessage::Authenticated);
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

impl Default for BinanceFuturesWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BinanceFuturesWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let stream = format!("{}@ticker", Self::format_symbol(symbol));
        client.subscribe_stream(&stream, "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let streams: Vec<String> = symbols
            .iter()
            .map(|s| format!("{}@ticker", Self::format_symbol(s)))
            .collect();
        client.subscribe_combined_streams(streams).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let depth = limit.unwrap_or(10).min(20);
        // Futures uses different depth stream format
        let stream = format!("{}@depth{}@100ms", Self::format_symbol(symbol), depth);
        client.subscribe_stream(&stream, "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        // Futures uses aggTrade
        let stream = format!("{}@aggTrade", Self::format_symbol(symbol));
        client.subscribe_stream(&stream, "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let interval = Self::format_interval(timeframe);
        let stream = format!("{}@kline_{}", Self::format_symbol(symbol), interval);
        client.subscribe_stream(&stream, "ohlcv", Some(symbol)).await
    }

    async fn watch_mark_price(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        // @markPrice@1s provides mark price updates every second
        let stream = format!("{}@markPrice@1s", Self::format_symbol(symbol));
        client.subscribe_stream(&stream, "markPrice", Some(symbol)).await
    }

    async fn watch_mark_prices(&self, symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        if let Some(syms) = symbols {
            let streams: Vec<String> = syms
                .iter()
                .map(|s| format!("{}@markPrice@1s", Self::format_symbol(s)))
                .collect();
            client.subscribe_combined_streams(streams).await
        } else {
            // Subscribe to all mark prices using !markPrice@arr stream
            client.subscribe_stream("!markPrice@arr@1s", "markPrice", None).await
        }
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

    /// WebSocket 인증 (Private 스트림용)
    /// Binance Futures는 listenKey를 통해 인증하므로 이 메서드는 성공 반환
    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        if self.listen_key.is_some() {
            Ok(())
        } else {
            Err(CcxtError::AuthenticationError {
                message: "Listen key not set. Call set_listen_key() first.".into(),
            })
        }
    }

    /// 주문 변경 구독 (인증 필요)
    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let listen_key = self.listen_key.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Listen key required for private streams. Call set_listen_key() first.".into(),
        })?;

        let mut client = Self::with_credentials(
            self.api_key.clone().unwrap_or_default(),
            self.api_secret.clone().unwrap_or_default(),
        );

        let _ = symbol; // Orders come for all symbols in user data stream
        client.subscribe_private_stream(listen_key).await
    }

    /// 내 체결 내역 구독 (인증 필요)
    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let listen_key = self.listen_key.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Listen key required for private streams. Call set_listen_key() first.".into(),
        })?;

        let mut client = Self::with_credentials(
            self.api_key.clone().unwrap_or_default(),
            self.api_secret.clone().unwrap_or_default(),
        );

        let _ = symbol; // Trades come for all symbols in user data stream
        client.subscribe_private_stream(listen_key).await
    }

    /// 포지션 구독 (인증 필요)
    async fn watch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let listen_key = self.listen_key.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Listen key required for private streams. Call set_listen_key() first.".into(),
        })?;

        let mut client = Self::with_credentials(
            self.api_key.clone().unwrap_or_default(),
            self.api_secret.clone().unwrap_or_default(),
        );

        let _ = symbols; // Positions come for all symbols in user data stream
        client.subscribe_private_stream(listen_key).await
    }

    /// 잔고 변경 구독 (인증 필요)
    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let listen_key = self.listen_key.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Listen key required for private streams. Call set_listen_key() first.".into(),
        })?;

        let mut client = Self::with_credentials(
            self.api_key.clone().unwrap_or_default(),
            self.api_secret.clone().unwrap_or_default(),
        );

        client.subscribe_private_stream(listen_key).await
    }
}

// === Binance Futures WebSocket Types ===

#[derive(Debug, Deserialize)]
struct BinanceFuturesError {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    msg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BinanceFuturesCombinedStream {
    #[allow(dead_code)]
    stream: String,
    data: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesTicker {
    e: String,
    #[serde(default)]
    E: Option<i64>,
    s: String,
    #[serde(default)]
    p: Option<String>,  // Price change
    #[serde(default)]
    P: Option<String>,  // Price change percent
    #[serde(default)]
    w: Option<String>,  // Weighted average price
    #[serde(default)]
    c: Option<String>,  // Last price
    #[serde(default)]
    Q: Option<String>,  // Last quantity
    #[serde(default)]
    o: Option<String>,  // Open price
    #[serde(default)]
    h: Option<String>,  // High price
    #[serde(default)]
    l: Option<String>,  // Low price
    #[serde(default)]
    v: Option<String>,  // Base volume
    #[serde(default)]
    q: Option<String>,  // Quote volume
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceMarkPriceUpdate {
    e: String,
    #[serde(default)]
    E: Option<i64>,
    s: String,
    #[serde(default)]
    p: Option<String>,  // Mark price
    #[serde(default)]
    i: Option<String>,  // Index price
    #[serde(default)]
    P: Option<String>,  // Estimated settle price
    #[serde(default)]
    r: Option<String>,  // Funding rate
    #[serde(default)]
    T: Option<i64>,     // Next funding time
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesDepthUpdate {
    e: String,
    #[serde(default)]
    E: Option<i64>,
    #[serde(default)]
    T: Option<i64>,
    s: String,
    #[serde(default)]
    U: Option<i64>,
    #[serde(default)]
    u: Option<i64>,
    #[serde(default)]
    pu: Option<i64>,  // Previous final update ID
    #[serde(default)]
    b: Vec<Vec<String>>,
    #[serde(default)]
    a: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesTradeMsg {
    e: String,
    #[serde(default)]
    E: Option<i64>,
    s: String,
    #[serde(default, rename = "a")]
    t: i64,  // Aggregate trade ID (use 'a' field)
    p: String,
    q: String,
    #[serde(default)]
    f: Option<i64>,  // First trade ID
    #[serde(default)]
    l: Option<i64>,  // Last trade ID
    #[serde(default)]
    T: Option<i64>,
    m: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesKlineMsg {
    e: String,
    #[serde(default)]
    E: Option<i64>,
    s: String,
    k: BinanceFuturesKline,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesKline {
    t: i64,
    #[serde(default)]
    T: Option<i64>,
    s: String,
    i: String,
    #[serde(default)]
    f: Option<i64>,  // First trade ID
    #[serde(default)]
    L: Option<i64>,  // Last trade ID
    o: String,
    c: String,
    h: String,
    l: String,
    v: String,
    #[serde(default)]
    n: Option<i64>,  // Number of trades
    #[serde(default)]
    x: Option<bool>,
    #[serde(default)]
    q: Option<String>,  // Quote volume
    #[serde(default)]
    V: Option<String>,  // Taker buy base volume
    #[serde(default)]
    Q: Option<String>,  // Taker buy quote volume
}

// === Private Stream Types ===

/// ORDER_TRADE_UPDATE 이벤트
#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesOrderUpdate {
    e: String,       // Event type
    #[serde(default)]
    E: Option<i64>,  // Event time
    T: i64,          // Transaction time
    o: BinanceFuturesOrderData,
}

/// 주문 데이터
#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesOrderData {
    s: String,       // Symbol
    c: String,       // Client order ID
    S: String,       // Side (BUY/SELL)
    o: String,       // Order type
    f: String,       // Time in force
    q: String,       // Original quantity
    p: String,       // Original price
    #[serde(default)]
    ap: Option<String>, // Average price
    #[serde(default)]
    sp: Option<String>, // Stop price
    x: String,       // Execution type (NEW, TRADE, CANCELED, etc.)
    X: String,       // Order status
    i: i64,          // Order ID
    l: String,       // Last filled quantity
    z: String,       // Cumulative filled quantity
    L: String,       // Last filled price
    #[serde(default)]
    N: Option<String>, // Commission asset
    #[serde(default)]
    n: Option<String>, // Commission
    #[serde(default)]
    T: Option<i64>,  // Order trade time (optional, not present for NEW orders)
    #[serde(default)]
    t: Option<i64>,  // Trade ID
    #[serde(default)]
    b: Option<String>, // Bids notional
    #[serde(default)]
    a: Option<String>, // Asks notional
    m: bool,         // Is maker
    R: bool,         // Is reduce only
    #[serde(default)]
    wt: Option<String>, // Working type
    #[serde(default)]
    ot: Option<String>, // Original order type
    #[serde(default)]
    ps: Option<String>, // Position side
    #[serde(default)]
    cp: Option<bool>, // Close all
    #[serde(default)]
    AP: Option<String>, // Activation price
    #[serde(default)]
    cr: Option<String>, // Callback rate
    #[serde(default)]
    rp: Option<String>, // Realized profit
    Z: String,       // Cumulative quote quantity
}

/// ACCOUNT_UPDATE 이벤트
#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesAccountUpdate {
    e: String,       // Event type
    #[serde(default)]
    E: Option<i64>,  // Event time
    T: i64,          // Transaction time
    a: BinanceFuturesAccountData,
}

/// 계정 데이터
#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesAccountData {
    m: String,       // Event reason type
    #[serde(default)]
    B: Vec<BinanceFuturesBalanceData>,
    #[serde(default)]
    P: Vec<BinanceFuturesPositionData>,
}

/// 잔고 데이터
#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesBalanceData {
    a: String,       // Asset
    wb: String,      // Wallet balance
    cw: String,      // Cross wallet balance
    bc: String,      // Balance change
}

/// 포지션 데이터
#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesPositionData {
    s: String,       // Symbol
    pa: String,      // Position amount
    ep: String,      // Entry price
    #[serde(default)]
    cr: Option<String>, // Accumulated realized
    up: String,      // Unrealized PnL
    mt: String,      // Margin type
    #[serde(default)]
    iw: Option<String>, // Isolated wallet
    ps: String,      // Position side
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BinanceFuturesWs::format_symbol("BTC/USDT:USDT"), "btcusdt");
        assert_eq!(BinanceFuturesWs::format_symbol("ETH/USDT:USDT"), "ethusdt");
        assert_eq!(BinanceFuturesWs::format_symbol("BTC/USDT"), "btcusdt");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BinanceFuturesWs::to_unified_symbol("BTCUSDT"), "BTC/USDT:USDT");
        assert_eq!(BinanceFuturesWs::to_unified_symbol("ETHUSDT"), "ETH/USDT:USDT");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(BinanceFuturesWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(BinanceFuturesWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(BinanceFuturesWs::format_interval(Timeframe::Day1), "1d");
    }

    #[test]
    fn test_parse_ticker() {
        let json = r#"{"e":"24hrTicker","E":1234567890000,"s":"BTCUSDT","p":"-50.00","P":"-0.1","w":"50000.00","c":"50000.00","o":"49000.00","h":"51000.00","l":"48000.00","v":"1000.00","q":"50000000.00"}"#;
        let data: BinanceFuturesTicker = serde_json::from_str(json).unwrap();
        let event = BinanceFuturesWs::parse_ticker(&data);

        assert_eq!(event.symbol, "BTC/USDT:USDT");
        assert_eq!(event.ticker.close, Some(Decimal::new(5000000, 2)));
    }

    #[test]
    fn test_with_credentials() {
        let client = BinanceFuturesWs::with_credentials(
            "test_api_key".to_string(),
            "test_api_secret".to_string(),
        );
        assert_eq!(client.get_api_key(), Some("test_api_key"));
    }

    #[test]
    fn test_set_listen_key() {
        let mut client = BinanceFuturesWs::new();
        assert!(client.get_listen_key().is_none());

        client.set_listen_key("test_listen_key".to_string());
        assert_eq!(client.get_listen_key(), Some("test_listen_key"));
    }

    #[test]
    fn test_parse_order_update() {
        let json = r#"{
            "e": "ORDER_TRADE_UPDATE",
            "E": 1568879465651,
            "T": 1568879465650,
            "o": {
                "s": "BTCUSDT",
                "c": "TEST",
                "S": "BUY",
                "o": "LIMIT",
                "f": "GTC",
                "q": "0.001",
                "p": "50000.00",
                "ap": "0",
                "sp": "0",
                "x": "NEW",
                "X": "NEW",
                "i": 8886774,
                "l": "0",
                "z": "0",
                "L": "0",
                "N": "USDT",
                "n": "0",
                "T": 1568879465651,
                "t": 0,
                "m": false,
                "R": false,
                "Z": "0"
            }
        }"#;

        let data: BinanceFuturesOrderUpdate = serde_json::from_str(json).unwrap();
        let event = BinanceFuturesWs::parse_order_update(&data).unwrap();

        assert_eq!(event.order.symbol, "BTC/USDT:USDT");
        assert_eq!(event.order.id, "8886774");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
        assert_eq!(event.order.status, OrderStatus::Open);
        assert_eq!(event.order.time_in_force, Some(TimeInForce::GTC));
        assert_eq!(event.order.price, Some(Decimal::new(5000000, 2)));
    }

    #[test]
    fn test_parse_my_trade() {
        let json = r#"{
            "e": "ORDER_TRADE_UPDATE",
            "E": 1568879465651,
            "T": 1568879465650,
            "o": {
                "s": "BTCUSDT",
                "c": "TEST",
                "S": "BUY",
                "o": "LIMIT",
                "f": "GTC",
                "q": "0.001",
                "p": "50000.00",
                "ap": "50000.00",
                "sp": "0",
                "x": "TRADE",
                "X": "FILLED",
                "i": 8886774,
                "l": "0.001",
                "z": "0.001",
                "L": "50000.00",
                "N": "USDT",
                "n": "0.025",
                "T": 1568879465651,
                "t": 123456,
                "m": false,
                "R": false,
                "Z": "50.00"
            }
        }"#;

        let data: BinanceFuturesOrderUpdate = serde_json::from_str(json).unwrap();
        let event = BinanceFuturesWs::parse_my_trade(&data).unwrap();

        assert_eq!(event.symbol, "BTC/USDT:USDT");
        assert_eq!(event.trades.len(), 1);
        let trade = &event.trades[0];
        assert_eq!(trade.id, "123456");
        assert_eq!(trade.side, Some("buy".to_string()));
        assert_eq!(trade.price, Decimal::new(5000000, 2));
        assert_eq!(trade.amount, Decimal::new(1, 3));
        assert_eq!(trade.taker_or_maker, Some(TakerOrMaker::Taker));
    }

    #[test]
    fn test_parse_position_update() {
        let json = r#"{
            "e": "ACCOUNT_UPDATE",
            "E": 1564745798939,
            "T": 1564745798938,
            "a": {
                "m": "ORDER",
                "B": [],
                "P": [
                    {
                        "s": "BTCUSDT",
                        "pa": "1.000",
                        "ep": "50000.00",
                        "cr": "100.00",
                        "up": "500.00",
                        "mt": "cross",
                        "iw": "0",
                        "ps": "BOTH"
                    }
                ]
            }
        }"#;

        let data: BinanceFuturesAccountUpdate = serde_json::from_str(json).unwrap();
        let event = BinanceFuturesWs::parse_position_update(&data).unwrap();

        assert_eq!(event.positions.len(), 1);
        let position = &event.positions[0];
        assert_eq!(position.symbol, "BTC/USDT:USDT");
        assert_eq!(position.contracts, Some(Decimal::new(1000, 3)));
        assert_eq!(position.entry_price, Some(Decimal::new(5000000, 2)));
        assert_eq!(position.unrealized_pnl, Some(Decimal::new(50000, 2)));
        assert_eq!(position.side, Some(PositionSide::Long));
        assert_eq!(position.margin_mode, Some(MarginMode::Cross));
    }

    #[test]
    fn test_parse_balance_update() {
        let json = r#"{
            "e": "ACCOUNT_UPDATE",
            "E": 1564745798939,
            "T": 1564745798938,
            "a": {
                "m": "DEPOSIT",
                "B": [
                    {
                        "a": "USDT",
                        "wb": "1000.00",
                        "cw": "900.00",
                        "bc": "100.00"
                    }
                ],
                "P": []
            }
        }"#;

        let data: BinanceFuturesAccountUpdate = serde_json::from_str(json).unwrap();
        let event = BinanceFuturesWs::parse_balance_update(&data).unwrap();

        let usdt_balance = event.balances.currencies.get("USDT").unwrap();
        assert_eq!(usdt_balance.free, Some(Decimal::new(90000, 2)));
        assert_eq!(usdt_balance.used, Some(Decimal::new(10000, 2)));
        assert_eq!(usdt_balance.total, Some(Decimal::new(100000, 2)));
    }

    #[test]
    fn test_process_message_order_update() {
        let json = r#"{"e":"ORDER_TRADE_UPDATE","E":1568879465651,"T":1568879465650,"o":{"s":"BTCUSDT","c":"TEST","S":"SELL","o":"MARKET","f":"IOC","q":"0.5","p":"0","ap":"49000.00","sp":"0","x":"NEW","X":"NEW","i":8886774,"l":"0","z":"0","L":"0","m":false,"R":true,"Z":"0"}}"#;

        let msg = BinanceFuturesWs::process_message(json);
        assert!(msg.is_some());

        if let Some(WsMessage::Order(event)) = msg {
            assert_eq!(event.order.symbol, "BTC/USDT:USDT");
            assert_eq!(event.order.side, OrderSide::Sell);
            assert_eq!(event.order.order_type, OrderType::Market);
            assert_eq!(event.order.reduce_only, Some(true));
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_message_account_update() {
        let json = r#"{"e":"ACCOUNT_UPDATE","E":1564745798939,"T":1564745798938,"a":{"m":"ORDER","B":[{"a":"BTC","wb":"10.0","cw":"9.0","bc":"1.0"}],"P":[{"s":"ETHUSDT","pa":"-2.0","ep":"3000.00","up":"-50.00","mt":"isolated","ps":"BOTH"}]}}"#;

        // Test position message
        let msg = BinanceFuturesWs::process_message(json);
        assert!(msg.is_some());

        if let Some(WsMessage::Position(event)) = msg {
            assert_eq!(event.positions.len(), 1);
            let pos = &event.positions[0];
            assert_eq!(pos.symbol, "ETH/USDT:USDT");
            assert_eq!(pos.side, Some(PositionSide::Short));
            assert_eq!(pos.contracts, Some(Decimal::new(20, 1)));
            assert_eq!(pos.margin_mode, Some(MarginMode::Isolated));
        } else {
            panic!("Expected Position message");
        }
    }
}
