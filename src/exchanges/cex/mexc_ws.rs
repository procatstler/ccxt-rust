//! MEXC WebSocket Implementation
//!
//! MEXC 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    TakerOrMaker, Ticker, Timeframe, Trade, OHLCV, WsExchange, WsMessage, WsBalanceEvent,
    WsMyTradeEvent, WsOrderBookEvent, WsOrderEvent, WsOhlcvEvent, WsTickerEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_PUBLIC_URL: &str = "wss://wbs.mexc.com/ws";
const REST_API_URL: &str = "https://api.mexc.com";

/// MEXC WebSocket 클라이언트
pub struct MexcWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    // Private channel fields
    api_key: Option<String>,
    api_secret: Option<String>,
    listen_key: Option<String>,
    is_authenticated: bool,
}

impl MexcWs {
    /// 새 MEXC WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: None,
            api_secret: None,
            listen_key: None,
            is_authenticated: false,
        }
    }

    /// API 자격증명으로 클라이언트 생성
    pub fn with_credentials(api_key: &str, api_secret: &str) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: Some(api_key.to_string()),
            api_secret: Some(api_secret.to_string()),
            listen_key: None,
            is_authenticated: false,
        }
    }

    /// API 자격증명 설정
    pub fn set_credentials(&mut self, api_key: &str, api_secret: &str) {
        self.api_key = Some(api_key.to_string());
        self.api_secret = Some(api_secret.to_string());
    }

    /// API 키 반환
    pub fn get_api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// listenKey 발급을 위한 서명 생성
    fn create_signature(&self, query: &str) -> CcxtResult<String> {
        let api_secret = self.api_secret.as_ref().ok_or(CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|e| CcxtError::ExchangeError { message: e.to_string() })?;
        mac.update(query.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// listenKey 발급 (REST API)
    async fn get_listen_key(&self) -> CcxtResult<String> {
        let api_key = self.api_key.as_ref().ok_or(CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();
        let query = format!("timestamp={timestamp}&recvWindow=5000");
        let signature = self.create_signature(&query)?;

        let url = format!(
            "{REST_API_URL}/api/v3/userDataStream?{query}&signature={signature}"
        );

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("X-MEXC-APIKEY", api_key)
            .header("Content-Type", "application/json")
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string()
            })?;

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| CcxtError::BadResponse { message: e.to_string() })?;

        json["listenKey"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Failed to get listenKey".into(),
            })
    }

    /// 주문 업데이트 파싱
    fn parse_order_update(data: &MexcOrderUpdateData) -> WsOrderEvent {
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());

        // Use short field names (o) if alias (order_type) is not present
        let order_type_str = data.order_type.as_deref().or(data.o.as_deref()).unwrap_or("LIMIT");
        let order_type = match order_type_str {
            "MARKET" => OrderType::Market,
            "LIMIT_MAKER" => OrderType::Limit,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref().unwrap_or("BUY") {
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        // Use short field name (X) if alias (order_status) is not present
        let status_str = data.order_status.as_deref().unwrap_or("NEW");
        let status = match status_str {
            "FILLED" => OrderStatus::Closed,
            "CANCELED" | "REJECTED" | "EXPIRED" => OrderStatus::Canceled,
            "PARTIALLY_FILLED" => OrderStatus::Open,
            _ => OrderStatus::Open,
        };

        // Use short field names (p, q, z, Z) if aliases are not present
        let price: Decimal = data.price.as_ref().or(data.p.as_ref())
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data.quantity.as_ref().or(data.q.as_ref())
            .and_then(|q| q.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data.executed_qty.as_ref().or(data.z.as_ref())
            .and_then(|q| q.parse().ok())
            .unwrap_or_default();
        let cost: Decimal = data.cummulative_quote_qty.as_ref()
            .and_then(|c| c.parse().ok())
            .unwrap_or_default();

        // Use short field name (s) if alias (symbol) is not present
        let symbol_str = data.symbol.as_deref().or(data.s.as_deref()).unwrap_or("");
        let unified_symbol = Self::to_unified_symbol(symbol_str);

        // Use short field name (i) if alias (order_id) is not present
        let order_id = data.order_id.or(data.i).map(|id| id.to_string()).unwrap_or_default();

        // Use short field name (c) if alias (client_order_id) is not present
        let client_order_id = data.client_order_id.clone().or(data.c.clone());

        let order = Order {
            id: order_id,
            client_order_id,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.time,
            status,
            symbol: unified_symbol,
            order_type,
            time_in_force: None,
            side,
            price: Some(price),
            average: if filled > Decimal::ZERO { Some(cost / filled) } else { None },
            amount,
            filled,
            remaining: Some(amount - filled),
            cost: Some(cost),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// 체결 업데이트 파싱
    fn parse_my_trade(data: &MexcTradeUpdateData) -> WsMyTradeEvent {
        // Use short field name (T) if alias (trade_time) is not present
        let timestamp = data.trade_time.unwrap_or_else(|| Utc::now().timestamp_millis());

        // Use short field names (p, q, n) if aliases are not present
        let price: Decimal = data.price.as_ref().or(data.p.as_ref())
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data.quantity.as_ref().or(data.q.as_ref())
            .and_then(|q| q.parse().ok())
            .unwrap_or_default();
        let commission: Decimal = data.commission.as_ref().or(data.n.as_ref())
            .and_then(|c| c.parse().ok())
            .unwrap_or_default();

        let side = data.side.as_deref()
            .map(|s| if s == "SELL" { "sell" } else { "buy" }.to_string());

        // Use short field name (m) if alias (is_maker) is not present
        let taker_or_maker = data.is_maker.or(data.m)
            .map(|m| if m { TakerOrMaker::Maker } else { TakerOrMaker::Taker });

        // Use short field name (s) if alias (symbol) is not present
        let symbol_str = data.symbol.as_deref().or(data.s.as_deref()).unwrap_or("");
        let unified_symbol = Self::to_unified_symbol(symbol_str);

        // Use short field names (t, i) if aliases are not present
        let trade_id = data.trade_id.or(data.t).map(|id| id.to_string()).unwrap_or_default();
        let order_id = data.order_id.or(data.i).map(|id| id.to_string());

        let trade = Trade {
            id: trade_id,
            order: order_id,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker,
            price,
            amount,
            cost: Some(price * amount),
            fee: Some(Fee {
                cost: Some(commission),
                currency: data.commission_asset.clone(),
                rate: None,
            }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsMyTradeEvent {
            symbol: unified_symbol,
            trades: vec![trade],
        }
    }

    /// 잔고 업데이트 파싱
    fn parse_balance_update(data: &MexcBalanceUpdateData) -> WsBalanceEvent {
        let timestamp = data.event_time.unwrap_or_else(|| Utc::now().timestamp_millis());

        let mut currencies = std::collections::HashMap::new();

        for item in &data.balances {
            if let Some(asset) = &item.a {
                let free: Decimal = item.f.as_ref()
                    .and_then(|f| f.parse().ok())
                    .unwrap_or_default();
                let locked: Decimal = item.l.as_ref()
                    .and_then(|l| l.parse().ok())
                    .unwrap_or_default();
                let total = free + locked;

                currencies.insert(asset.clone(), Balance {
                    free: Some(free),
                    used: Some(locked),
                    total: Some(total),
                    debt: None,
                });
            }
        }

        let balances = Balances {
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            currencies,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsBalanceEvent { balances }
    }

    /// Private 메시지 처리
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        // MEXC private channel message format
        if let Ok(response) = serde_json::from_str::<MexcPrivateWsResponse>(msg) {
            // Check for trade execution first (x == "TRADE" within executionReport)
            // Trade events have both e == "executionReport" and x == "TRADE"
            if response.x.as_deref() == Some("TRADE") {
                if let Ok(trade_data) = serde_json::from_str::<MexcTradeUpdateData>(msg) {
                    return Some(WsMessage::MyTrade(Self::parse_my_trade(&trade_data)));
                }
            }

            if let Some(event_type) = &response.e {
                match event_type.as_str() {
                    "executionReport" => {
                        // Order update (non-trade execution types like NEW, CANCELED, etc.)
                        if let Ok(order_data) = serde_json::from_str::<MexcOrderUpdateData>(msg) {
                            return Some(WsMessage::Order(Self::parse_order_update(&order_data)));
                        }
                    }
                    "outboundAccountPosition" | "balanceUpdate" => {
                        // Balance update
                        if let Ok(balance_data) = serde_json::from_str::<MexcBalanceUpdateData>(msg) {
                            return Some(WsMessage::Balance(Self::parse_balance_update(&balance_data)));
                        }
                    }
                    _ => {}
                }
            }
        }

        None
    }

    /// Private 스트림 구독
    async fn subscribe_private_stream(
        &mut self,
        channel_type: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Get listenKey
        let listen_key = self.get_listen_key().await?;
        self.listen_key = Some(listen_key.clone());

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // Connect with listenKey
        let ws_url = format!("{WS_PUBLIC_URL}?listenKey={listen_key}");

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);
        self.is_authenticated = true;

        // 구독 저장
        {
            let key = format!("private:{channel_type}");
            self.subscriptions.write().await.insert(key, channel_type.to_string());
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Authenticated);
                        let _ = tx.send(WsMessage::Subscribed {
                            channel: "private".to_string(),
                            symbol: None,
                        });
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

    /// 심볼을 MEXC 형식으로 변환 (BTC/USDT -> BTCUSDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "").to_uppercase()
    }

    /// MEXC 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(mexc_symbol: &str) -> String {
        let quote_currencies = ["USDT", "USDC", "BTC", "ETH", "BUSD", "TUSD"];
        let upper = mexc_symbol.to_uppercase();

        for quote in &quote_currencies {
            if upper.ends_with(quote) {
                let base = &upper[..upper.len() - quote.len()];
                return format!("{base}/{quote}");
            }
        }

        mexc_symbol.to_uppercase()
    }

    /// Timeframe을 MEXC 포맷으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "Min1",
            Timeframe::Minute5 => "Min5",
            Timeframe::Minute15 => "Min15",
            Timeframe::Minute30 => "Min30",
            Timeframe::Hour1 => "Hour1",
            Timeframe::Hour4 => "Hour4",
            Timeframe::Hour8 => "Hour8",
            Timeframe::Day1 => "Day1",
            Timeframe::Week1 => "Week1",
            Timeframe::Month1 => "Month1",
            _ => "Min1",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &MexcTickerData, symbol: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.h.as_ref().and_then(|v| v.parse().ok()),
            low: data.l.as_ref().and_then(|v| v.parse().ok()),
            bid: data.b.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: None,
            ask: data.a.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: data.o.as_ref().and_then(|v| v.parse().ok()),
            close: data.c.as_ref().and_then(|v| v.parse().ok()),
            last: data.c.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: data.r.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.v.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.q.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &MexcDepthData, symbol: &str) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = Utc::now().timestamp_millis();

        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            Some(OrderBookEntry {
                price: b.p.as_ref()?.parse().ok()?,
                amount: b.v.as_ref()?.parse().ok()?,
            })
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            Some(OrderBookEntry {
                price: a.p.as_ref()?.parse().ok()?,
                amount: a.v.as_ref()?.parse().ok()?,
            })
        }).collect();

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.r.map(|r| r as i64),
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
    fn parse_trade(data: &MexcTradeData, symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = data.side.as_ref().map(|s| {
            match s.as_str() {
                "1" => "buy".to_string(),
                "2" => "sell".to_string(),
                _ => s.to_lowercase(),
            }
        });

        let trades = vec![Trade {
            id: data.t.map(|t| t.to_string()).unwrap_or_default(),
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
            price: data.p.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            amount: data.v.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            cost: data.p.as_ref().and_then(|p| {
                data.v.as_ref().and_then(|v| {
                    let price: Decimal = p.parse().ok()?;
                    let amount: Decimal = v.parse().ok()?;
                    Some(price * amount)
                })
            }),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent { symbol: unified_symbol, trades }
    }

    /// K선 메시지 파싱
    fn parse_kline(data: &MexcKlineData, symbol: &str, timeframe: Timeframe) -> WsOhlcvEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ohlcv = OHLCV {
            timestamp,
            open: data.o.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            high: data.h.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            low: data.l.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            close: data.c.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            volume: data.v.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe,
            ohlcv,
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str, timeframe: Option<Timeframe>) -> Option<WsMessage> {
        // 구독 확인
        if let Ok(response) = serde_json::from_str::<MexcWsResponse>(msg) {
            if response.code.is_some() {
                // Subscription response
                if let Some(channel) = response.c {
                    return Some(WsMessage::Subscribed {
                        channel,
                        symbol: None,
                    });
                }
                return None;
            }

            // 데이터 메시지
            if let Some(channel) = &response.c {
                // Parse channel to extract symbol
                // Format: spot@public.ticker.v3.api@BTCUSDT
                let parts: Vec<&str> = channel.split('@').collect();
                let symbol = parts.last().unwrap_or(&"");

                if channel.contains("ticker") {
                    if let Some(d) = response.d {
                        if let Ok(ticker_data) = serde_json::from_value::<MexcTickerData>(d) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data, symbol)));
                        }
                    }
                } else if channel.contains("limit.depth") || channel.contains("depth") {
                    if let Some(d) = response.d {
                        if let Ok(depth_data) = serde_json::from_value::<MexcDepthData>(d) {
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&depth_data, symbol)));
                        }
                    }
                } else if channel.contains("deals") || channel.contains("trade") {
                    if let Some(d) = response.d {
                        if let Ok(trade_data) = serde_json::from_value::<MexcDealsWrapper>(d) {
                            if let Some(first_trade) = trade_data.deals.first() {
                                return Some(WsMessage::Trade(Self::parse_trade(first_trade, symbol)));
                            }
                        }
                    }
                } else if channel.contains("kline") {
                    if let Some(d) = response.d {
                        if let Ok(kline_data) = serde_json::from_value::<MexcKlineWrapper>(d) {
                            let tf = timeframe.unwrap_or(Timeframe::Minute1);
                            return Some(WsMessage::Ohlcv(Self::parse_kline(&kline_data.k, symbol, tf)));
                        }
                    }
                }
            }
        }

        // Ping response
        if msg.contains("\"msg\":\"PONG\"") {
            return None;
        }

        None
    }

    /// 구독 메시지 생성
    fn create_subscription_msg(channel: &str) -> String {
        serde_json::json!({
            "method": "SUBSCRIPTION",
            "params": [channel]
        }).to_string()
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        channel_type: &str,
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
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        let subscribe_msg = Self::create_subscription_msg(channel);
        ws_client.send(&subscribe_msg)?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = channel.to_string();
            self.subscriptions.write().await.insert(key, channel_type.to_string());
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let tf = timeframe;
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
                        if let Some(ws_msg) = Self::process_message(&msg, tf) {
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

impl Default for MexcWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for MexcWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let mexc_symbol = Self::format_symbol(symbol);
        let channel = format!("spot@public.ticker.v3.api@{mexc_symbol}");
        client.subscribe_stream(&channel, "ticker", None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // MEXC doesn't support batch ticker subscription, subscribe to first symbol
        if let Some(symbol) = symbols.first() {
            self.watch_ticker(symbol).await
        } else {
            Err(crate::errors::CcxtError::BadRequest {
                message: "No symbols provided".into(),
            })
        }
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let mexc_symbol = Self::format_symbol(symbol);
        let depth = limit.unwrap_or(5);
        let channel = format!("spot@public.limit.depth.v3.api@{mexc_symbol}@{depth}");
        client.subscribe_stream(&channel, "orderBook", None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let mexc_symbol = Self::format_symbol(symbol);
        let channel = format!("spot@public.deals.v3.api@{mexc_symbol}");
        client.subscribe_stream(&channel, "trades", None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let mexc_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let channel = format!("spot@public.kline.v3.api@{mexc_symbol}@{interval}");
        client.subscribe_stream(&channel, "ohlcv", Some(timeframe)).await
    }

    // === Private Streams ===

    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let api_key = self.api_key.as_ref().ok_or(CcxtError::AuthenticationError {
            message: "API credentials required for watch_orders".into(),
        })?;
        let api_secret = self.api_secret.as_ref().ok_or(CcxtError::AuthenticationError {
            message: "API credentials required for watch_orders".into(),
        })?;

        let mut client = Self::with_credentials(api_key, api_secret);
        client.subscribe_private_stream("orders").await
    }

    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let api_key = self.api_key.as_ref().ok_or(CcxtError::AuthenticationError {
            message: "API credentials required for watch_my_trades".into(),
        })?;
        let api_secret = self.api_secret.as_ref().ok_or(CcxtError::AuthenticationError {
            message: "API credentials required for watch_my_trades".into(),
        })?;

        let mut client = Self::with_credentials(api_key, api_secret);
        client.subscribe_private_stream("myTrades").await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let api_key = self.api_key.as_ref().ok_or(CcxtError::AuthenticationError {
            message: "API credentials required for watch_balance".into(),
        })?;
        let api_secret = self.api_secret.as_ref().ok_or(CcxtError::AuthenticationError {
            message: "API credentials required for watch_balance".into(),
        })?;

        let mut client = Self::with_credentials(api_key, api_secret);
        client.subscribe_private_stream("balance").await
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required".into(),
            });
        }

        // Get listenKey for authentication
        let listen_key = self.get_listen_key().await?;
        self.listen_key = Some(listen_key);
        self.is_authenticated = true;

        Ok(())
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

// === MEXC WebSocket Types ===

#[derive(Debug, Deserialize)]
struct MexcWsResponse {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    c: Option<String>,
    #[serde(default)]
    d: Option<serde_json::Value>,
    #[serde(default)]
    t: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct MexcTickerData {
    #[serde(default)]
    o: Option<String>,  // open
    #[serde(default)]
    c: Option<String>,  // close
    #[serde(default)]
    h: Option<String>,  // high
    #[serde(default)]
    l: Option<String>,  // low
    #[serde(default)]
    v: Option<String>,  // volume
    #[serde(default)]
    q: Option<String>,  // quote volume
    #[serde(default)]
    r: Option<String>,  // price change rate
    #[serde(default)]
    b: Option<String>,  // bid
    #[serde(default)]
    a: Option<String>,  // ask
    #[serde(default)]
    t: Option<i64>,     // timestamp
}

#[derive(Debug, Default, Deserialize)]
struct MexcDepthData {
    #[serde(default)]
    bids: Vec<MexcDepthEntry>,
    #[serde(default)]
    asks: Vec<MexcDepthEntry>,
    #[serde(default)]
    r: Option<u64>,  // version
}

#[derive(Debug, Default, Deserialize)]
struct MexcDepthEntry {
    #[serde(default)]
    p: Option<String>,  // price
    #[serde(default)]
    v: Option<String>,  // volume
}

#[derive(Debug, Default, Deserialize)]
struct MexcDealsWrapper {
    #[serde(default)]
    deals: Vec<MexcTradeData>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct MexcTradeData {
    #[serde(default)]
    p: Option<String>,  // price
    #[serde(default)]
    v: Option<String>,  // volume
    #[serde(default, rename = "S")]
    side: Option<String>,  // side: 1=buy, 2=sell
    #[serde(default)]
    t: Option<i64>,     // timestamp
}

#[derive(Debug, Default, Deserialize)]
struct MexcKlineWrapper {
    #[serde(default)]
    k: MexcKlineData,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct MexcKlineData {
    #[serde(default)]
    t: Option<i64>,     // timestamp
    #[serde(default)]
    o: Option<String>,  // open
    #[serde(default)]
    c: Option<String>,  // close
    #[serde(default)]
    h: Option<String>,  // high
    #[serde(default)]
    l: Option<String>,  // low
    #[serde(default)]
    v: Option<String>,  // volume
    #[serde(default)]
    q: Option<String>,  // quote volume
}

// === Private Channel Types ===

#[derive(Debug, Default, Deserialize)]
struct MexcPrivateWsResponse {
    #[serde(default)]
    e: Option<String>,  // event type
    #[serde(default, rename = "E")]
    event_time: Option<i64>,
    #[serde(default)]
    s: Option<String>,  // symbol
    #[serde(default)]
    x: Option<String>,  // execution type (NEW, TRADE, CANCELED, etc.)
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct MexcOrderUpdateData {
    #[serde(default)]
    e: Option<String>,      // event type
    #[serde(default, rename = "E")]
    event_time: Option<i64>,
    #[serde(default)]
    s: Option<String>,      // symbol
    #[serde(default)]
    c: Option<String>,      // client order id
    #[serde(default, rename = "S")]
    side: Option<String>,   // BUY/SELL
    #[serde(default)]
    o: Option<String>,      // order type (LIMIT/MARKET)
    #[serde(default)]
    q: Option<String>,      // quantity
    #[serde(default)]
    p: Option<String>,      // price
    #[serde(default, rename = "X")]
    order_status: Option<String>,  // NEW, PARTIALLY_FILLED, FILLED, CANCELED
    #[serde(default)]
    i: Option<i64>,         // order id
    #[serde(default)]
    z: Option<String>,      // cumulative filled quantity
    #[serde(default, rename = "Z")]
    cummulative_quote_qty: Option<String>,
    #[serde(default, rename = "T")]
    time: Option<i64>,      // order time

    // Alias fields for convenience
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    order_id: Option<i64>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    executed_qty: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct MexcTradeUpdateData {
    #[serde(default)]
    e: Option<String>,      // event type
    #[serde(default, rename = "E")]
    event_time: Option<i64>,
    #[serde(default)]
    s: Option<String>,      // symbol
    #[serde(default)]
    t: Option<i64>,         // trade id
    #[serde(default)]
    p: Option<String>,      // price
    #[serde(default)]
    q: Option<String>,      // quantity
    #[serde(default)]
    n: Option<String>,      // commission
    #[serde(default, rename = "N")]
    commission_asset: Option<String>,
    #[serde(default, rename = "T")]
    trade_time: Option<i64>,
    #[serde(default)]
    m: Option<bool>,        // is maker
    #[serde(default, rename = "S")]
    side: Option<String>,   // BUY/SELL
    #[serde(default)]
    i: Option<i64>,         // order id

    // Alias fields for convenience
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    order_id: Option<i64>,
    #[serde(default)]
    trade_id: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    commission: Option<String>,
    #[serde(default)]
    is_maker: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct MexcBalanceUpdateData {
    #[serde(default)]
    e: Option<String>,      // event type
    #[serde(default, rename = "E")]
    event_time: Option<i64>,
    #[serde(default)]
    u: Option<i64>,         // last account update time
    #[serde(default, rename = "B")]
    balances: Vec<MexcBalanceItem>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct MexcBalanceItem {
    #[serde(default)]
    a: Option<String>,      // asset (currency)
    #[serde(default)]
    f: Option<String>,      // free balance
    #[serde(default)]
    l: Option<String>,      // locked balance
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(MexcWs::format_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(MexcWs::format_symbol("ETH/BTC"), "ETHBTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(MexcWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(MexcWs::to_unified_symbol("ETHBTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(MexcWs::format_interval(Timeframe::Minute1), "Min1");
        assert_eq!(MexcWs::format_interval(Timeframe::Hour1), "Hour1");
        assert_eq!(MexcWs::format_interval(Timeframe::Day1), "Day1");
    }

    // === Private Channel Tests ===

    #[test]
    fn test_credentials() {
        let client = MexcWs::with_credentials("test_key", "test_secret");
        assert_eq!(client.get_api_key(), Some("test_key"));
        assert!(!client.is_authenticated);
    }

    #[test]
    fn test_set_credentials() {
        let mut client = MexcWs::new();
        assert!(client.get_api_key().is_none());

        client.set_credentials("my_key", "my_secret");
        assert_eq!(client.get_api_key(), Some("my_key"));
    }

    #[test]
    fn test_parse_order_update() {
        let data = MexcOrderUpdateData {
            e: Some("executionReport".to_string()),
            event_time: Some(1699123456789),
            s: Some("BTCUSDT".to_string()),
            symbol: Some("BTCUSDT".to_string()),
            c: Some("client123".to_string()),
            client_order_id: Some("client123".to_string()),
            side: Some("BUY".to_string()),
            o: Some("LIMIT".to_string()),
            order_type: Some("LIMIT".to_string()),
            q: Some("0.001".to_string()),
            quantity: Some("0.001".to_string()),
            p: Some("50000.00".to_string()),
            price: Some("50000.00".to_string()),
            order_status: Some("NEW".to_string()),
            i: Some(12345),
            order_id: Some(12345),
            z: Some("0.0".to_string()),
            executed_qty: Some("0.0".to_string()),
            cummulative_quote_qty: Some("0.0".to_string()),
            time: Some(1699123456789),
        };

        let event = MexcWs::parse_order_update(&data);
        assert_eq!(event.order.symbol, "BTC/USDT");
        assert_eq!(event.order.id, "12345");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
        assert_eq!(event.order.status, OrderStatus::Open);
        assert_eq!(event.order.price, Some(Decimal::new(5000000, 2)));
    }

    #[test]
    fn test_parse_order_update_filled() {
        let data = MexcOrderUpdateData {
            symbol: Some("ETHUSDT".to_string()),
            order_id: Some(67890),
            client_order_id: Some("order_abc".to_string()),
            side: Some("SELL".to_string()),
            order_type: Some("MARKET".to_string()),
            quantity: Some("1.0".to_string()),
            price: Some("0".to_string()),
            order_status: Some("FILLED".to_string()),
            executed_qty: Some("1.0".to_string()),
            cummulative_quote_qty: Some("2000.00".to_string()),
            time: Some(1699123456000),
            ..Default::default()
        };

        let event = MexcWs::parse_order_update(&data);
        assert_eq!(event.order.symbol, "ETH/USDT");
        assert_eq!(event.order.side, OrderSide::Sell);
        assert_eq!(event.order.order_type, OrderType::Market);
        assert_eq!(event.order.status, OrderStatus::Closed);
        assert_eq!(event.order.filled, Decimal::new(10, 1));
    }

    #[test]
    fn test_parse_my_trade() {
        let data = MexcTradeUpdateData {
            e: Some("executionReport".to_string()),
            event_time: Some(1699123456789),
            s: Some("BTCUSDT".to_string()),
            symbol: Some("BTCUSDT".to_string()),
            t: Some(1111),
            trade_id: Some(1111),
            p: Some("50000.00".to_string()),
            price: Some("50000.00".to_string()),
            q: Some("0.001".to_string()),
            quantity: Some("0.001".to_string()),
            n: Some("0.025".to_string()),
            commission: Some("0.025".to_string()),
            commission_asset: Some("USDT".to_string()),
            trade_time: Some(1699123456789),
            m: Some(true),
            is_maker: Some(true),
            side: Some("BUY".to_string()),
            i: Some(12345),
            order_id: Some(12345),
        };

        let event = MexcWs::parse_my_trade(&data);
        assert_eq!(event.symbol, "BTC/USDT");
        assert_eq!(event.trades.len(), 1);

        let trade = &event.trades[0];
        assert_eq!(trade.id, "1111");
        assert_eq!(trade.order, Some("12345".to_string()));
        assert_eq!(trade.price, Decimal::new(5000000, 2));
        assert_eq!(trade.amount, Decimal::new(1, 3));
        assert_eq!(trade.side, Some("buy".to_string()));
        assert_eq!(trade.taker_or_maker, Some(TakerOrMaker::Maker));
        assert!(trade.fee.is_some());
        let fee = trade.fee.as_ref().unwrap();
        assert_eq!(fee.cost, Some(Decimal::new(25, 3)));
        assert_eq!(fee.currency, Some("USDT".to_string()));
    }

    #[test]
    fn test_parse_order_update_canceled() {
        let data = MexcOrderUpdateData {
            symbol: Some("BTCUSDT".to_string()),
            order_id: Some(99999),
            side: Some("BUY".to_string()),
            order_type: Some("LIMIT".to_string()),
            quantity: Some("0.01".to_string()),
            price: Some("45000.00".to_string()),
            order_status: Some("CANCELED".to_string()),
            executed_qty: Some("0.0".to_string()),
            cummulative_quote_qty: Some("0.0".to_string()),
            time: Some(1699123456000),
            ..Default::default()
        };

        let event = MexcWs::parse_order_update(&data);
        assert_eq!(event.order.status, OrderStatus::Canceled);
    }

    #[test]
    fn test_process_private_message_order() {
        let msg = r#"{"e":"executionReport","E":1699123456789,"s":"BTCUSDT","S":"BUY","o":"LIMIT","q":"0.001","p":"50000.00","X":"NEW","i":12345,"z":"0.0","Z":"0.0","T":1699123456789}"#;

        let result = MexcWs::process_private_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.symbol, "BTC/USDT");
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_private_message_trade() {
        let msg = r#"{"e":"executionReport","E":1699123456789,"s":"BTCUSDT","x":"TRADE","S":"BUY","p":"50000.00","q":"0.001","n":"0.025","N":"USDT","T":1699123456789,"m":true,"t":1111,"i":12345}"#;

        let result = MexcWs::process_private_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::MyTrade(event)) = result {
            assert_eq!(event.symbol, "BTC/USDT");
            assert_eq!(event.trades.len(), 1);
        } else {
            panic!("Expected MyTrade message");
        }
    }

    #[test]
    fn test_process_private_message_unknown() {
        let msg = r#"{"e":"unknown_event","E":1699123456789}"#;

        let result = MexcWs::process_private_message(msg);
        assert!(result.is_none());
    }

    #[test]
    fn test_private_data_types() {
        // Test MexcPrivateWsResponse
        let json = r#"{"e":"executionReport","E":1699123456789,"s":"BTCUSDT","x":"NEW"}"#;
        let response: MexcPrivateWsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.e, Some("executionReport".to_string()));
        assert_eq!(response.x, Some("NEW".to_string()));
    }

    #[test]
    fn test_order_side_parsing() {
        // BUY side
        let data = MexcOrderUpdateData {
            side: Some("BUY".to_string()),
            ..Default::default()
        };
        let event = MexcWs::parse_order_update(&data);
        assert_eq!(event.order.side, OrderSide::Buy);

        // SELL side
        let data = MexcOrderUpdateData {
            side: Some("SELL".to_string()),
            ..Default::default()
        };
        let event = MexcWs::parse_order_update(&data);
        assert_eq!(event.order.side, OrderSide::Sell);
    }

    #[test]
    fn test_order_status_parsing() {
        // NEW
        let data = MexcOrderUpdateData {
            order_status: Some("NEW".to_string()),
            ..Default::default()
        };
        assert_eq!(MexcWs::parse_order_update(&data).order.status, OrderStatus::Open);

        // PARTIALLY_FILLED
        let data = MexcOrderUpdateData {
            order_status: Some("PARTIALLY_FILLED".to_string()),
            ..Default::default()
        };
        assert_eq!(MexcWs::parse_order_update(&data).order.status, OrderStatus::Open);

        // FILLED
        let data = MexcOrderUpdateData {
            order_status: Some("FILLED".to_string()),
            ..Default::default()
        };
        assert_eq!(MexcWs::parse_order_update(&data).order.status, OrderStatus::Closed);

        // CANCELED
        let data = MexcOrderUpdateData {
            order_status: Some("CANCELED".to_string()),
            ..Default::default()
        };
        assert_eq!(MexcWs::parse_order_update(&data).order.status, OrderStatus::Canceled);

        // REJECTED
        let data = MexcOrderUpdateData {
            order_status: Some("REJECTED".to_string()),
            ..Default::default()
        };
        assert_eq!(MexcWs::parse_order_update(&data).order.status, OrderStatus::Canceled);

        // EXPIRED
        let data = MexcOrderUpdateData {
            order_status: Some("EXPIRED".to_string()),
            ..Default::default()
        };
        assert_eq!(MexcWs::parse_order_update(&data).order.status, OrderStatus::Canceled);
    }

    #[test]
    fn test_trade_taker_maker() {
        // Maker
        let data = MexcTradeUpdateData {
            is_maker: Some(true),
            symbol: Some("BTCUSDT".to_string()),
            ..Default::default()
        };
        let event = MexcWs::parse_my_trade(&data);
        assert_eq!(event.trades[0].taker_or_maker, Some(TakerOrMaker::Maker));

        // Taker
        let data = MexcTradeUpdateData {
            is_maker: Some(false),
            symbol: Some("BTCUSDT".to_string()),
            ..Default::default()
        };
        let event = MexcWs::parse_my_trade(&data);
        assert_eq!(event.trades[0].taker_or_maker, Some(TakerOrMaker::Taker));
    }

    // === Balance Tests ===

    #[test]
    fn test_parse_balance_update() {
        let data = MexcBalanceUpdateData {
            e: Some("outboundAccountPosition".to_string()),
            event_time: Some(1699123456789),
            u: Some(1699123456789),
            balances: vec![
                MexcBalanceItem {
                    a: Some("USDT".to_string()),
                    f: Some("10000.00".to_string()),
                    l: Some("500.00".to_string()),
                },
                MexcBalanceItem {
                    a: Some("BTC".to_string()),
                    f: Some("1.5".to_string()),
                    l: Some("0.5".to_string()),
                },
            ],
        };

        let event = MexcWs::parse_balance_update(&data);
        assert!(event.balances.currencies.contains_key("USDT"));
        assert!(event.balances.currencies.contains_key("BTC"));

        let usdt = event.balances.currencies.get("USDT").unwrap();
        assert_eq!(usdt.free, Some(Decimal::new(1000000, 2)));
        assert_eq!(usdt.used, Some(Decimal::new(50000, 2)));
        assert_eq!(usdt.total, Some(Decimal::new(1050000, 2)));

        let btc = event.balances.currencies.get("BTC").unwrap();
        assert_eq!(btc.free, Some(Decimal::new(15, 1)));
        assert_eq!(btc.used, Some(Decimal::new(5, 1)));
        assert_eq!(btc.total, Some(Decimal::new(20, 1)));
    }

    #[test]
    fn test_parse_balance_update_empty() {
        let data = MexcBalanceUpdateData {
            e: Some("outboundAccountPosition".to_string()),
            event_time: Some(1699123456789),
            u: None,
            balances: vec![],
        };

        let event = MexcWs::parse_balance_update(&data);
        assert!(event.balances.currencies.is_empty());
        assert_eq!(event.balances.timestamp, Some(1699123456789));
    }

    #[test]
    fn test_process_private_message_balance() {
        let msg = r#"{"e":"outboundAccountPosition","E":1699123456789,"u":1699123456789,"B":[{"a":"USDT","f":"10000.00","l":"500.00"}]}"#;

        let result = MexcWs::process_private_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("USDT"));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_balance_data_types() {
        // Test MexcBalanceUpdateData deserialization
        let json = r#"{"e":"outboundAccountPosition","E":1699123456789,"u":1699123456789,"B":[{"a":"ETH","f":"5.0","l":"1.0"}]}"#;
        let data: MexcBalanceUpdateData = serde_json::from_str(json).unwrap();
        assert_eq!(data.e, Some("outboundAccountPosition".to_string()));
        assert_eq!(data.balances.len(), 1);
        assert_eq!(data.balances[0].a, Some("ETH".to_string()));
    }

    #[test]
    fn test_balance_update_event() {
        // Test balanceUpdate event type (alternative to outboundAccountPosition)
        let msg = r#"{"e":"balanceUpdate","E":1699123456789,"B":[{"a":"BTC","f":"2.0","l":"0.5"}]}"#;

        let result = MexcWs::process_private_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("BTC"));
            let btc = event.balances.currencies.get("BTC").unwrap();
            assert_eq!(btc.free, Some(Decimal::new(20, 1)));
            assert_eq!(btc.used, Some(Decimal::new(5, 1)));
        } else {
            panic!("Expected Balance message");
        }
    }
}
