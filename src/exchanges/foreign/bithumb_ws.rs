//! Bithumb WebSocket Implementation
//!
//! Bithumb 실시간 데이터 스트리밍 (Public + Private)
//!
//! Reference: https://apidocs.bithumb.com/
//! - Public WebSocket: wss://pubwss.bithumb.com/pub/ws (v1.2.0)
//! - Private WebSocket: wss://ws-api.bithumb.com/websocket/v1/private (v2.1.5)
//!
//! Supported features:
//! - watchTicker, watchTickers ✓
//! - watchTrades ✓
//! - watchOrderBook ✓
//! - watchBalance ✓ (private)
//! - watchOrders ✓ (private)
//! - watchOHLCV ✗ (NOT supported by Bithumb)

#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Ticker, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsOrderBookEvent,
    WsOrderEvent, WsTickerEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://pubwss.bithumb.com/pub/ws";
const WS_PRIVATE_URL: &str = "wss://ws-api.bithumb.com/websocket/v1/private";

/// Bithumb WebSocket 클라이언트
pub struct BithumbWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    private_ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BithumbWs {
    /// 새 Bithumb WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            config: None,
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// API 키와 시크릿으로 Bithumb WebSocket 클라이언트 생성
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 Bithumb 형식으로 변환 (BTC/KRW -> BTC_KRW)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace('/', "_")
    }

    /// Bithumb 심볼을 통합 심볼로 변환 (BTC_KRW -> BTC/KRW)
    fn to_unified_symbol(bithumb_symbol: &str) -> String {
        bithumb_symbol.replace('_', "/")
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BithumbTickerContent) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);

        // date: "20200129", time: "121844" -> "2020-01-29T12:18:44"
        let datetime = if let (Some(ref date), Some(ref time)) = (&data.date, &data.time) {
            format!(
                "{}-{}-{}T{}:{}:{}",
                &date[0..4],
                &date[4..6],
                &date[6..8],
                &time[0..2],
                &time[2..4],
                &time[4..6]
            )
        } else {
            chrono::Utc::now().to_rfc3339()
        };

        let timestamp = chrono::DateTime::parse_from_rfc3339(&datetime)
            .ok()
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(datetime),
            high: data.high_price.as_ref().and_then(|v| v.parse().ok()),
            low: data.low_price.as_ref().and_then(|v| v.parse().ok()),
            bid: None,
            bid_volume: data.buy_volume.as_ref().and_then(|v| v.parse().ok()),
            ask: None,
            ask_volume: data.sell_volume.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open_price.as_ref().and_then(|v| v.parse().ok()),
            close: data.close_price.as_ref().and_then(|v| v.parse().ok()),
            last: data.close_price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: data.prev_close_price.as_ref().and_then(|v| v.parse().ok()),
            change: data.chg_amt.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.chg_rate.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.value.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(list: &[BithumbOrderBookEntry]) -> Option<WsOrderBookEvent> {
        if list.is_empty() {
            return None;
        }

        let first = &list[0];
        let symbol = Self::to_unified_symbol(&first.symbol);

        let timestamp = first
            .datetime
            .as_ref()
            .and_then(|dt| {
                // datetime: 1580268255864325 (microseconds)
                let micros: i64 = dt.parse().ok()?;
                Some(micros / 1000) // convert to milliseconds
            })
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        for entry in list {
            if let (Ok(price), Ok(quantity)) = (entry.price.parse::<Decimal>(), entry.quantity.parse::<Decimal>()) {
                // quantity가 0이면 해당 가격의 주문을 제거
                if quantity == Decimal::ZERO {
                    continue;
                }

                let order_book_entry = OrderBookEntry { price, amount: quantity };

                match entry.order_type.as_str() {
                    "bid" => bids.push(order_book_entry),
                    "ask" => asks.push(order_book_entry),
                    _ => {}
                }
            }
        }

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

        Some(WsOrderBookEvent {
            symbol,
            order_book,
            is_snapshot: false,
        })
    }

    /// 체결 메시지 파싱
    fn parse_trades(list: &[BithumbTradeEntry]) -> Option<WsTradeEvent> {
        if list.is_empty() {
            return None;
        }

        let first = &list[0];
        let symbol = Self::to_unified_symbol(&first.symbol);

        let mut trades = Vec::new();

        for entry in list {
            // contDtm: "2020-01-29 12:24:18.830039" (local time, -9 hours from UTC)
            let timestamp = if let Some(ref dt) = entry.cont_dtm {
                chrono::NaiveDateTime::parse_from_str(dt, "%Y-%m-%d %H:%M:%S%.f")
                    .ok()
                    .and_then(|naive| {
                        // Convert from KST to UTC (subtract 9 hours = 32400000 ms)
                        chrono::DateTime::from_timestamp_millis(
                            naive.and_utc().timestamp_millis() - 32400000
                        )
                        .map(|dt| dt.timestamp_millis())
                    })
            } else {
                None
            }
            .unwrap_or_else(|| Utc::now().timestamp_millis());

            let price: Decimal = entry.cont_price.parse().unwrap_or_default();
            let amount: Decimal = entry.cont_qty.parse().unwrap_or_default();
            let cost: Decimal = entry.cont_amt.parse().unwrap_or_default();

            // buySellGb: "1" = buy, "2" = sell
            let side = match entry.buy_sell_gb.as_str() {
                "1" => Some("buy".to_string()),
                "2" => Some("sell".to_string()),
                _ => None,
            };

            let trade = Trade {
                id: format!("{}_{}", entry.symbol, timestamp),
                order: None,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol.clone(),
                trade_type: None,
                side,
                taker_or_maker: None,
                price,
                amount,
                cost: Some(cost),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(entry).unwrap_or_default(),
            };

            trades.push(trade);
        }

        Some(WsTradeEvent { symbol, trades })
    }

    /// Private 메시지 처리 (myAsset, myOrder)
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;
        let msg_type = json.get("type").and_then(|v| v.as_str())?;

        match msg_type {
            "myAsset" => {
                if let Ok(data) = serde_json::from_str::<BithumbMyAsset>(msg) {
                    return Some(WsMessage::Balance(Self::parse_balance(&data)));
                }
            }
            "myOrder" => {
                if let Ok(data) = serde_json::from_str::<BithumbMyOrder>(msg) {
                    return Some(WsMessage::Order(Self::parse_order(&data)));
                }
            }
            _ => {}
        }

        None
    }

    /// myAsset 파싱
    fn parse_balance(data: &BithumbMyAsset) -> WsBalanceEvent {
        let mut balances = Balances::default();
        balances.timestamp = data.timestamp;
        balances.datetime = data.timestamp.and_then(|t| {
            chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
        });

        for asset in &data.assets {
            let currency = asset.currency.clone();
            let free: Decimal = asset.balance.parse().unwrap_or_default();
            let used: Decimal = asset.locked.parse().unwrap_or_default();
            let total = free + used;

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

        balances.info = serde_json::to_value(data).unwrap_or_default();
        WsBalanceEvent { balances }
    }

    /// myOrder 파싱
    fn parse_order(data: &BithumbMyOrder) -> WsOrderEvent {
        let symbol = Self::to_unified_symbol(&data.code);

        let timestamp = data.order_timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        // ask_bid: "BID" = buy, "ASK" = sell
        let side = match data.ask_bid.as_str() {
            "BID" => OrderSide::Buy,
            "ASK" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        // order_type: "limit", "price", "market"
        let order_type = match data.order_type.as_str() {
            "limit" => OrderType::Limit,
            "price" | "market" => OrderType::Market,
            _ => OrderType::Limit,
        };

        // state: "wait", "trade", "done", "cancel"
        let status = match data.state.as_str() {
            "wait" => OrderStatus::Open,
            "trade" => OrderStatus::Open,
            "done" => OrderStatus::Closed,
            "cancel" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let price: Decimal = Decimal::from_f64_retain(data.price.unwrap_or(0) as f64).unwrap_or_default();
        let amount: Decimal = Decimal::from_f64_retain(data.volume.unwrap_or_default()).unwrap_or_default();
        let filled: Decimal = Decimal::from_f64_retain(data.executed_volume.unwrap_or_default()).unwrap_or_default();
        let remaining: Decimal = Decimal::from_f64_retain(data.remaining_volume.unwrap_or_default()).unwrap_or_default();
        let cost: Decimal = Decimal::from_f64_retain(data.executed_funds.unwrap_or_default()).unwrap_or_default();
        let fee_cost: Decimal = Decimal::from_f64_retain(data.paid_fee.unwrap_or_default()).unwrap_or_default();

        let order = Order {
            id: data.uuid.clone(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: data.trade_timestamp,
            last_update_timestamp: data.timestamp,
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
            fee: if fee_cost > Decimal::ZERO {
                Some(crate::types::Fee {
                    cost: Some(fee_cost),
                    currency: None,
                    rate: None,
                })
            } else {
                None
            },
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

    /// Public 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // 에러 체크
        if let Ok(err) = serde_json::from_str::<BithumbError>(msg) {
            if let Some(status) = err.status {
                if status != "0000" {
                    return Some(WsMessage::Error(
                        err.resmsg.unwrap_or_else(|| "Unknown error".to_string()),
                    ));
                }
            }
        }

        let json: serde_json::Value = serde_json::from_str(msg).ok()?;
        let msg_type = json.get("type").and_then(|v| v.as_str())?;

        match msg_type {
            "ticker" => {
                if let Ok(data) = serde_json::from_str::<BithumbTickerMessage>(msg) {
                    return Some(WsMessage::Ticker(Self::parse_ticker(&data.content)));
                }
            }
            "orderbookdepth" => {
                if let Ok(data) = serde_json::from_str::<BithumbOrderBookMessage>(msg) {
                    return Self::parse_order_book(&data.content.list).map(WsMessage::OrderBook);
                }
            }
            "transaction" => {
                if let Ok(data) = serde_json::from_str::<BithumbTradeMessage>(msg) {
                    return Self::parse_trades(&data.content.list).map(WsMessage::Trade);
                }
            }
            _ => {}
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환 (Public)
    async fn subscribe_public(
        &mut self,
        msg_type: &str,
        symbols: &[&str],
        params: Option<serde_json::Value>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // 심볼 변환
        let symbol_list: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();

        // 구독 메시지 생성
        let mut subscribe_msg = serde_json::json!({
            "type": msg_type,
            "symbols": symbol_list,
        });

        // 추가 파라미터 병합
        if let Some(extra) = params {
            if let Some(obj) = subscribe_msg.as_object_mut() {
                if let Some(extra_obj) = extra.as_object() {
                    for (k, v) in extra_obj {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
        }

        // WebSocket 클라이언트 생성 및 연결
        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", msg_type, symbols.join(","));
            self.subscriptions
                .write()
                .await
                .insert(key, msg_type.to_string());
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
    async fn subscribe_private(
        &mut self,
        msg_type: &str,
        codes: Option<Vec<String>>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // 인증 정보 확인
        self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API credentials required for private WebSocket".into(),
        })?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // 구독 메시지 생성 (Bithumb private v2.1.5 format)
        let mut subscribe_msgs = vec![serde_json::json!({ "ticket": "ccxt" })];

        let mut msg_obj = serde_json::json!({ "type": msg_type });
        if let Some(c) = codes {
            msg_obj["codes"] = serde_json::json!(c);
        }
        subscribe_msgs.push(msg_obj);

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

        // 구독 메시지 전송 (JSON array format)
        let subscribe_str = serde_json::to_string(&subscribe_msgs).unwrap();
        ws_client.send(&subscribe_str)?;

        self.private_ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("private:{}", msg_type);
            self.subscriptions
                .write()
                .await
                .insert(key, msg_type.to_string());
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

impl Default for BithumbWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for BithumbWs {
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
impl WsExchange for BithumbWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let params = serde_json::json!({ "tickTypes": ["24H"] });
        client.subscribe_public("ticker", &[symbol], Some(params)).await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let params = serde_json::json!({ "tickTypes": ["24H"] });
        client.subscribe_public("ticker", symbols, Some(params)).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_public("orderbookdepth", &[symbol], None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_public("transaction", &[symbol], None).await
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watchOHLCV - Bithumb does not support OHLCV WebSocket".into(),
        })
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Bithumb는 구독시 자동 연결
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
        } else if let Some(client) = &self.private_ws_client {
            client.is_connected().await
        } else {
            false
        }
    }

    // === Private Streams ===

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        if client.config.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for watch_balance".into(),
            });
        }
        client.subscribe_private("myAsset", None).await
    }

    async fn watch_orders(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        if client.config.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for watch_orders".into(),
            });
        }

        let codes = symbol.map(|s| vec![Self::format_symbol(s)]);
        client.subscribe_private("myOrder", codes).await
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        // Bithumb private WebSocket는 JWT 토큰 인증 필요
        // 실제 구현에서는 JWT 생성이 필요하지만, 여기서는 단순화
        self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API credentials required".into(),
        })?;
        Ok(())
    }
}

// === Bithumb WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct BithumbError {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    resmsg: Option<String>,
}

/// Ticker 메시지
#[derive(Debug, Deserialize, Serialize)]
struct BithumbTickerMessage {
    #[serde(rename = "type")]
    msg_type: String,
    content: BithumbTickerContent,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BithumbTickerContent {
    symbol: String,
    #[serde(default)]
    tick_type: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    open_price: Option<String>,
    #[serde(default)]
    close_price: Option<String>,
    #[serde(default)]
    low_price: Option<String>,
    #[serde(default)]
    high_price: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    sell_volume: Option<String>,
    #[serde(default)]
    buy_volume: Option<String>,
    #[serde(default)]
    prev_close_price: Option<String>,
    #[serde(default)]
    chg_rate: Option<String>,
    #[serde(default)]
    chg_amt: Option<String>,
    #[serde(default)]
    volume_power: Option<String>,
}

/// OrderBook 메시지
#[derive(Debug, Deserialize, Serialize)]
struct BithumbOrderBookMessage {
    #[serde(rename = "type")]
    msg_type: String,
    content: BithumbOrderBookContent,
}

#[derive(Debug, Deserialize, Serialize)]
struct BithumbOrderBookContent {
    list: Vec<BithumbOrderBookEntry>,
    #[serde(default)]
    datetime: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BithumbOrderBookEntry {
    symbol: String,
    order_type: String, // "bid" or "ask"
    price: String,
    quantity: String,
    #[serde(default)]
    total: Option<String>,
    #[serde(default)]
    datetime: Option<String>,
}

/// Trade 메시지
#[derive(Debug, Deserialize, Serialize)]
struct BithumbTradeMessage {
    #[serde(rename = "type")]
    msg_type: String,
    content: BithumbTradeContent,
}

#[derive(Debug, Deserialize, Serialize)]
struct BithumbTradeContent {
    list: Vec<BithumbTradeEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BithumbTradeEntry {
    symbol: String,
    buy_sell_gb: String, // "1" = buy, "2" = sell
    cont_price: String,
    cont_qty: String,
    cont_amt: String,
    #[serde(default)]
    cont_dtm: Option<String>, // "2020-01-29 12:24:18.830039"
    #[serde(default)]
    updn: Option<String>,
}

// === Private WebSocket Message Types ===

/// myAsset - 내 자산
#[derive(Debug, Deserialize, Serialize)]
struct BithumbMyAsset {
    #[serde(rename = "type")]
    msg_type: String,
    assets: Vec<BithumbAsset>,
    #[serde(default)]
    asset_timestamp: Option<i64>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    stream_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BithumbAsset {
    currency: String,
    balance: String,
    locked: String,
}

/// myOrder - 내 주문
#[derive(Debug, Deserialize, Serialize)]
struct BithumbMyOrder {
    #[serde(rename = "type")]
    msg_type: String,
    code: String,         // "KRW-BTC"
    uuid: String,         // Order ID
    ask_bid: String,      // "BID" or "ASK"
    order_type: String,   // "limit", "price", "market"
    state: String,        // "wait", "trade", "done", "cancel"
    #[serde(default)]
    trade_uuid: Option<String>,
    #[serde(default)]
    price: Option<i64>,
    #[serde(default)]
    volume: Option<f64>,
    #[serde(default)]
    remaining_volume: Option<f64>,
    #[serde(default)]
    executed_volume: Option<f64>,
    #[serde(default)]
    trades_count: Option<i32>,
    #[serde(default)]
    reserved_fee: Option<f64>,
    #[serde(default)]
    remaining_fee: Option<f64>,
    #[serde(default)]
    paid_fee: Option<f64>,
    #[serde(default)]
    executed_funds: Option<f64>,
    #[serde(default)]
    trade_timestamp: Option<i64>,
    #[serde(default)]
    order_timestamp: Option<i64>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    stream_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BithumbWs::format_symbol("BTC/KRW"), "BTC_KRW");
        assert_eq!(BithumbWs::format_symbol("ETH/KRW"), "ETH_KRW");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BithumbWs::to_unified_symbol("BTC_KRW"), "BTC/KRW");
        assert_eq!(BithumbWs::to_unified_symbol("ETH_KRW"), "ETH/KRW");
    }

    #[test]
    fn test_parse_ticker() {
        let content = BithumbTickerContent {
            symbol: "BTC_KRW".to_string(),
            tick_type: Some("24H".to_string()),
            date: Some("20200129".to_string()),
            time: Some("121844".to_string()),
            open_price: Some("2302".to_string()),
            close_price: Some("2317".to_string()),
            low_price: Some("2272".to_string()),
            high_price: Some("2344".to_string()),
            value: Some("2831915078.07065789".to_string()),
            volume: Some("1222314.51355788".to_string()),
            sell_volume: Some("760129.34079004".to_string()),
            buy_volume: Some("462185.17276784".to_string()),
            prev_close_price: Some("2326".to_string()),
            chg_rate: Some("0.65".to_string()),
            chg_amt: Some("15".to_string()),
            volume_power: Some("60.80".to_string()),
        };

        let event = BithumbWs::parse_ticker(&content);
        assert_eq!(event.symbol, "BTC/KRW");
        assert_eq!(event.ticker.close, Some(Decimal::new(2317, 0)));
    }
}
