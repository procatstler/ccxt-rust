//! HTX (Huobi) WebSocket Implementation
//!
//! HTX 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::prelude::FromStr;
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
    TakerOrMaker, Ticker, Timeframe, Trade,
    WsExchange, WsMessage, WsBalanceEvent, WsMyTradeEvent, WsOrderBookEvent, WsOrderEvent,
    WsTickerEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://api.huobi.pro/ws";
const WS_MARKET_URL: &str = "wss://api.huobi.pro/ws";
const WS_PRIVATE_URL: &str = "wss://api.huobi.pro/ws/v2";

type HmacSha256 = Hmac<Sha256>;

/// HTX WebSocket 클라이언트
pub struct HtxWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    // Private channel fields
    api_key: Option<String>,
    api_secret: Option<String>,
    is_authenticated: bool,
}

impl HtxWs {
    /// 새 HTX WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: None,
            api_secret: None,
            is_authenticated: false,
        }
    }

    /// API credentials로 새 클라이언트 생성
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: Some(api_key),
            api_secret: Some(api_secret),
            is_authenticated: false,
        }
    }

    /// API credentials 설정
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
    }

    /// API key 반환
    pub fn get_api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// 인증 서명 생성
    fn sign_auth_params(secret: &str, timestamp: &str) -> CcxtResult<String> {
        // Create the signature string
        // format: GET\napi.huobi.pro\n/ws/v2\naccessKey=xxx&signatureMethod=HmacSHA256&signatureVersion=2.1&timestamp=xxx
        let params_str = format!(
            "accessKey={}&signatureMethod=HmacSHA256&signatureVersion=2.1&timestamp={}",
            "", // AccessKey is added separately
            urlencoding::encode(timestamp)
        );

        // Note: The actual accessKey needs to be included, but we exclude it from signing
        // HTX uses a specific format for WebSocket auth
        let payload = format!("GET\napi.huobi.pro\n/ws/v2\n{params_str}");

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Failed to create HMAC: {e}"),
            })?;
        mac.update(payload.as_bytes());

        Ok(BASE64.encode(mac.finalize().into_bytes()))
    }

    /// 심볼을 HTX 형식으로 변환 (BTC/USDT -> btcusdt)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "").to_lowercase()
    }

    /// HTX 심볼을 통합 심볼로 변환 (btcusdt -> BTC/USDT)
    fn to_unified_symbol(htx_symbol: &str) -> String {
        // Most common quote currencies
        let quote_currencies = ["usdt", "btc", "eth", "husd", "usdc", "trx"];
        let lower = htx_symbol.to_lowercase();

        for quote in &quote_currencies {
            if lower.ends_with(quote) {
                let base = &lower[..lower.len() - quote.len()];
                return format!("{}/{}", base.to_uppercase(), quote.to_uppercase());
            }
        }

        htx_symbol.to_uppercase()
    }

    /// Timeframe을 HTX 포맷으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1min",
            Timeframe::Minute5 => "5min",
            Timeframe::Minute15 => "15min",
            Timeframe::Minute30 => "30min",
            Timeframe::Hour1 => "60min",
            Timeframe::Hour4 => "4hour",
            Timeframe::Day1 => "1day",
            Timeframe::Week1 => "1week",
            Timeframe::Month1 => "1mon",
            _ => "1min",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &HtxTickerData, symbol: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high,
            low: data.low,
            bid: data.bid.first().copied(),
            bid_volume: data.bid.get(1).copied(),
            ask: data.ask.first().copied(),
            ask_volume: data.ask.get(1).copied(),
            vwap: None,
            open: data.open,
            close: data.close,
            last: data.close,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.amount,
            quote_volume: data.vol,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &HtxOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: b[0],
                    amount: b[1],
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: a[0],
                    amount: a[1],
                })
            } else {
                None
            }
        }).collect();

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.version,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &HtxTradeData, symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let trades = vec![Trade {
            id: data.trade_id.map(|id| id.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
            trade_type: None,
            side: data.direction.clone(),
            taker_or_maker: None,
            price: data.price.unwrap_or_default(),
            amount: data.amount.unwrap_or_default(),
            cost: data.price.and_then(|p| data.amount.map(|a| p * a)),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent { symbol: unified_symbol, trades }
    }

    /// 주문 업데이트 파싱
    fn parse_order_update(data: &HtxOrderUpdateData) -> WsOrderEvent {
        let timestamp = data.order_create_time
            .or(data.last_act_time)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let symbol = Self::to_unified_symbol(&data.symbol);

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") | Some("buy-limit") | Some("sell-limit") => OrderType::Limit,
            Some("market") | Some("buy-market") | Some("sell-market") => OrderType::Market,
            Some("stop-limit") => OrderType::StopLossLimit,
            Some("ioc") | Some("buy-ioc") | Some("sell-ioc") => OrderType::Limit,
            Some("limit-maker") | Some("buy-limit-maker") | Some("sell-limit-maker") => OrderType::Limit,
            _ => OrderType::Limit,
        };

        let status = match data.order_status.as_deref() {
            Some("submitted") | Some("pre-submitted") => OrderStatus::Open,
            Some("partial-filled") => OrderStatus::Open,
            Some("filled") => OrderStatus::Closed,
            Some("partial-canceled") | Some("canceled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let price = data.order_price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());

        let amount = data.order_size.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();

        let filled = data.trade_volume.as_ref()
            .or(data.filled_amount.as_ref())
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let remaining = amount - filled;

        let cost = data.trade_turnover.as_ref()
            .or(data.filled_cash_amount.as_ref())
            .and_then(|c| Decimal::from_str(c).ok());

        let average = if filled > Decimal::ZERO {
            cost.map(|c| c / filled)
        } else {
            None
        };

        let fee_cost = data.filled_fees.as_ref()
            .and_then(|f| Decimal::from_str(f).ok());

        let fee = fee_cost.map(|c| Fee {
            currency: data.fee_currency.clone(),
            cost: Some(c),
            rate: None,
        });

        let order = Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()),
            last_trade_timestamp: data.last_act_time,
            last_update_timestamp: data.last_act_time,
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force: None,
            side,
            price,
            average,
            amount,
            filled,
            remaining: Some(remaining),
            stop_price: data.stop_price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
            cost,
            trades: Vec::new(),
            fee,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// 내 체결 파싱
    fn parse_my_trade(data: &HtxTradeDetailData) -> (String, Trade) {
        let timestamp = data.trade_time.unwrap_or_else(|| Utc::now().timestamp_millis());
        let symbol = Self::to_unified_symbol(&data.symbol);

        let side = match data.trade_type.as_deref() {
            Some("buy") => Some("buy".to_string()),
            Some("sell") => Some("sell".to_string()),
            _ => None,
        };

        let taker_or_maker = match data.role.as_deref() {
            Some("maker") => Some(TakerOrMaker::Maker),
            Some("taker") => Some(TakerOrMaker::Taker),
            _ => None,
        };

        let price = data.trade_price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok())
            .unwrap_or_default();

        let amount = data.trade_volume.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let cost = data.trade_turnover.as_ref()
            .and_then(|c| Decimal::from_str(c).ok())
            .or_else(|| Some(price * amount));

        let fee_cost = data.trade_fee.as_ref()
            .and_then(|f| Decimal::from_str(f).ok());

        let fee = fee_cost.map(|c| Fee {
            currency: data.fee_currency.clone(),
            cost: Some(c),
            rate: None,
        });

        let trade = Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: data.order_id.clone(),
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()),
            symbol: symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker,
            price,
            amount,
            cost,
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        (symbol, trade)
    }

    /// 잔고 업데이트 파싱
    fn parse_balance_update(data: &HtxAccountUpdateData) -> WsBalanceEvent {
        let timestamp = data.change_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let mut currencies = HashMap::new();

        let currency = data.currency.to_uppercase();
        let available: Decimal = data.available.as_ref()
            .and_then(|a| Decimal::from_str(a).ok())
            .unwrap_or_default();
        let balance: Decimal = data.balance.as_ref()
            .and_then(|b| Decimal::from_str(b).ok())
            .unwrap_or_default();

        // In HTX, balance = total, available = free, frozen = used
        let used = balance - available;

        currencies.insert(currency, Balance {
            free: Some(available),
            used: Some(used),
            total: Some(balance),
            debt: None,
        });

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

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // HTX sends gzip compressed data, but our WsClient should handle decompression
        // Parse the JSON message
        if let Ok(response) = serde_json::from_str::<HtxWsResponse>(msg) {
            // Ping response - we handle pong internally
            if response.ping.is_some() {
                return None; // Ping/pong handled elsewhere
            }

            // Subscribe confirmation
            if response.subbed.is_some() {
                return Some(WsMessage::Subscribed {
                    channel: response.subbed.unwrap_or_default(),
                    symbol: None,
                });
            }

            // Data message
            if let Some(ch) = &response.ch {
                if let Some(tick) = response.tick {
                    // Parse channel to determine message type
                    if ch.contains(".ticker") {
                        // Extract symbol from channel (market.btcusdt.ticker)
                        let parts: Vec<&str> = ch.split('.').collect();
                        let symbol = parts.get(1).unwrap_or(&"");
                        if let Ok(ticker_data) = serde_json::from_value::<HtxTickerData>(tick) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data, symbol)));
                        }
                    } else if ch.contains(".depth") {
                        let parts: Vec<&str> = ch.split('.').collect();
                        let symbol = parts.get(1).unwrap_or(&"");
                        if let Ok(ob_data) = serde_json::from_value::<HtxOrderBookData>(tick) {
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, symbol, true)));
                        }
                    } else if ch.contains(".trade") {
                        let parts: Vec<&str> = ch.split('.').collect();
                        let symbol = parts.get(1).unwrap_or(&"");
                        if let Ok(trade_wrapper) = serde_json::from_value::<HtxTradeWrapper>(tick) {
                            if let Some(trade_data) = trade_wrapper.data.first() {
                                return Some(WsMessage::Trade(Self::parse_trade(trade_data, symbol)));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Private 메시지 처리 (WebSocket v2 API)
    fn process_private_message(msg: &str) -> Option<WsMessage> {
        // Parse private channel messages (different format from public)
        if let Ok(response) = serde_json::from_str::<HtxWsPrivateResponse>(msg) {
            // Ping/Pong
            if response.action.as_deref() == Some("ping") {
                return None; // Handled by ping/pong mechanism
            }

            // Authentication response
            if response.action.as_deref() == Some("req") && response.ch.as_deref() == Some("auth") {
                if response.code == Some(200) {
                    return Some(WsMessage::Authenticated);
                } else {
                    let err_msg = response.message.unwrap_or_else(|| "Authentication failed".to_string());
                    return Some(WsMessage::Error(err_msg));
                }
            }

            // Subscribe confirmation
            if response.action.as_deref() == Some("sub")
                && response.code == Some(200) {
                    return Some(WsMessage::Subscribed {
                        channel: response.ch.unwrap_or_default(),
                        symbol: None,
                    });
                }

            // Push data
            if response.action.as_deref() == Some("push") {
                if let Some(ch) = &response.ch {
                    if let Some(data) = response.data {
                        // Order updates
                        if ch.contains("orders#") {
                            if let Ok(order_data) = serde_json::from_value::<HtxOrderUpdateData>(data) {
                                return Some(WsMessage::Order(Self::parse_order_update(&order_data)));
                            }
                        }
                        // Trade updates (own trades)
                        else if ch.contains("trade.clearing#") {
                            if let Ok(trade_data) = serde_json::from_value::<HtxTradeDetailData>(data) {
                                let (symbol, trade) = Self::parse_my_trade(&trade_data);
                                return Some(WsMessage::MyTrade(WsMyTradeEvent {
                                    symbol,
                                    trades: vec![trade],
                                }));
                            }
                        }
                        // Account/balance updates
                        else if ch.contains("accounts.update#") {
                            if let Ok(account_data) = serde_json::from_value::<HtxAccountUpdateData>(data) {
                                return Some(WsMessage::Balance(Self::parse_balance_update(&account_data)));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Private 스트림 구독
    async fn subscribe_private_stream(
        &mut self,
        topic: &str,
        channel: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let api_key = self.api_key.clone().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required for private channel".into(),
        })?;
        let api_secret = self.api_secret.clone().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required for private channel".into(),
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

        // Authenticate
        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();

        // Create the signature for WebSocket v2 auth
        let params_str = format!(
            "accessKey={}&signatureMethod=HmacSHA256&signatureVersion=2.1&timestamp={}",
            urlencoding::encode(&api_key),
            urlencoding::encode(&timestamp)
        );
        let payload = format!("GET\napi.huobi.pro\n/ws/v2\n{params_str}");

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Failed to create HMAC: {e}"),
            })?;
        mac.update(payload.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        let auth_msg = serde_json::json!({
            "action": "req",
            "ch": "auth",
            "params": {
                "authType": "api",
                "accessKey": api_key,
                "signatureMethod": "HmacSHA256",
                "signatureVersion": "2.1",
                "timestamp": timestamp,
                "signature": signature
            }
        });
        ws_client.send(&auth_msg.to_string())?;

        // Subscribe to the topic
        let subscribe_msg = serde_json::json!({
            "action": "sub",
            "ch": topic
        });

        // We need to wait for auth before subscribing, but for simplicity
        // we send both and let the server handle the order
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // Store subscription
        {
            let key = topic.to_string();
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // Event processing task
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

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        topic: &str,
        channel: &str,
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
        let subscribe_msg = serde_json::json!({
            "sub": topic,
            "id": format!("sub_{}", Utc::now().timestamp_millis())
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = topic.to_string();
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
                        // Handle ping from HTX - ping/pong is handled by WsClient
                        // Just process regular messages
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

impl Default for HtxWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for HtxWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let htx_symbol = Self::format_symbol(symbol);
        let topic = format!("market.{htx_symbol}.ticker");
        client.subscribe_stream(&topic, "ticker").await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // HTX doesn't support batch ticker subscription, subscribe to first symbol
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
        let htx_symbol = Self::format_symbol(symbol);
        let depth = match limit.unwrap_or(20) {
            1..=5 => "step0",
            6..=20 => "step1",
            _ => "step2",
        };
        let topic = format!("market.{htx_symbol}.depth.{depth}");
        client.subscribe_stream(&topic, "orderBook").await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let htx_symbol = Self::format_symbol(symbol);
        let topic = format!("market.{htx_symbol}.trade.detail");
        client.subscribe_stream(&topic, "trades").await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let htx_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let topic = format!("market.{htx_symbol}.kline.{interval}");
        client.subscribe_stream(&topic, "ohlcv").await
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

    // === Private WebSocket Methods ===

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        let api_key = self.api_key.clone().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = self.api_secret.clone().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;

        // Create WebSocket connection if not exists
        if self.ws_client.is_none() {
            let mut ws_client = WsClient::new(WsConfig {
                url: WS_PRIVATE_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 20,
                connect_timeout_secs: 30,
            });

            let _ = ws_client.connect().await?;
            self.ws_client = Some(ws_client);
        }

        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();

        // Create signature
        let params_str = format!(
            "accessKey={}&signatureMethod=HmacSHA256&signatureVersion=2.1&timestamp={}",
            urlencoding::encode(&api_key),
            urlencoding::encode(&timestamp)
        );
        let payload = format!("GET\napi.huobi.pro\n/ws/v2\n{params_str}");

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Failed to create HMAC: {e}"),
            })?;
        mac.update(payload.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        let auth_msg = serde_json::json!({
            "action": "req",
            "ch": "auth",
            "params": {
                "authType": "api",
                "accessKey": api_key,
                "signatureMethod": "HmacSHA256",
                "signatureVersion": "2.1",
                "timestamp": timestamp,
                "signature": signature
            }
        });

        if let Some(client) = &self.ws_client {
            client.send(&auth_msg.to_string())?;
        }

        self.is_authenticated = true;
        Ok(())
    }

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.clone().ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?,
            self.api_secret.clone().ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?,
        );

        // HTX format: orders#btcusdt or orders#* for all
        let topic = if let Some(sym) = symbol {
            format!("orders#{}", Self::format_symbol(sym))
        } else {
            "orders#*".to_string()
        };

        client.subscribe_private_stream(&topic, "orders").await
    }

    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.clone().ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?,
            self.api_secret.clone().ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?,
        );

        // HTX format: trade.clearing#btcusdt or trade.clearing#* for all
        let topic = if let Some(sym) = symbol {
            format!("trade.clearing#{}", Self::format_symbol(sym))
        } else {
            "trade.clearing#*".to_string()
        };

        client.subscribe_private_stream(&topic, "myTrades").await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.clone().ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?,
            self.api_secret.clone().ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?,
        );

        // HTX format: accounts.update#<mode>
        // mode 0: only balance changes, mode 1: available balance changes, mode 2: all events
        let topic = "accounts.update#1".to_string();

        client.subscribe_private_stream(&topic, "balance").await
    }
}

// === HTX WebSocket Types ===

#[derive(Debug, Deserialize)]
struct HtxPing {
    #[serde(default)]
    ping: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct HtxWsResponse {
    #[serde(default)]
    ping: Option<i64>,
    #[serde(default)]
    subbed: Option<String>,
    #[serde(default)]
    ch: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    tick: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct HtxTickerData {
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    close: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    vol: Option<Decimal>,
    #[serde(default)]
    bid: Vec<Decimal>,
    #[serde(default)]
    ask: Vec<Decimal>,
    #[serde(default)]
    ts: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct HtxOrderBookData {
    #[serde(default)]
    bids: Vec<Vec<Decimal>>,
    #[serde(default)]
    asks: Vec<Vec<Decimal>>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    version: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct HtxTradeWrapper {
    #[serde(default)]
    data: Vec<HtxTradeData>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct HtxTradeData {
    #[serde(default)]
    trade_id: Option<i64>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
}

// === Private Channel Types ===

#[derive(Debug, Deserialize)]
struct HtxWsPrivateResponse {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    ch: Option<String>,
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// Order update data (orders#symbol)
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HtxOrderUpdateData {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    order_price: Option<String>,
    #[serde(default)]
    order_size: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    order_status: Option<String>,
    #[serde(default)]
    order_create_time: Option<i64>,
    #[serde(default)]
    last_act_time: Option<i64>,
    #[serde(default)]
    trade_price: Option<String>,
    #[serde(default)]
    trade_volume: Option<String>,
    #[serde(default)]
    trade_turnover: Option<String>,
    #[serde(default)]
    filled_amount: Option<String>,
    #[serde(default)]
    filled_cash_amount: Option<String>,
    #[serde(default)]
    filled_fees: Option<String>,
    #[serde(default)]
    fee_currency: Option<String>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    event_type: Option<String>,
}

/// Trade clearing data (trade.clearing#symbol)
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HtxTradeDetailData {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    trade_price: Option<String>,
    #[serde(default)]
    trade_volume: Option<String>,
    #[serde(default)]
    trade_turnover: Option<String>,
    #[serde(default)]
    trade_time: Option<i64>,
    #[serde(default, rename = "tradeType")]
    trade_type: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    trade_fee: Option<String>,
    #[serde(default)]
    fee_currency: Option<String>,
}

/// Account update data (accounts.update#mode)
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HtxAccountUpdateData {
    #[serde(default)]
    currency: String,
    #[serde(default)]
    account_id: Option<i64>,
    #[serde(default)]
    balance: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    change_type: Option<String>,
    #[serde(default)]
    account_type: Option<String>,
    #[serde(default)]
    change_time: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(HtxWs::format_symbol("BTC/USDT"), "btcusdt");
        assert_eq!(HtxWs::format_symbol("ETH/BTC"), "ethbtc");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(HtxWs::to_unified_symbol("btcusdt"), "BTC/USDT");
        assert_eq!(HtxWs::to_unified_symbol("ethbtc"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(HtxWs::format_interval(Timeframe::Minute1), "1min");
        assert_eq!(HtxWs::format_interval(Timeframe::Hour1), "60min");
        assert_eq!(HtxWs::format_interval(Timeframe::Day1), "1day");
    }

    #[test]
    fn test_with_credentials() {
        let client = HtxWs::with_credentials(
            "test_key".to_string(),
            "test_secret".to_string(),
        );
        assert_eq!(client.get_api_key(), Some("test_key"));
    }

    #[test]
    fn test_set_credentials() {
        let mut client = HtxWs::new();
        assert_eq!(client.get_api_key(), None);
        client.set_credentials("key1".to_string(), "secret1".to_string());
        assert_eq!(client.get_api_key(), Some("key1"));
    }

    #[test]
    fn test_parse_order_update() {
        let data = HtxOrderUpdateData {
            symbol: "btcusdt".to_string(),
            order_id: Some("12345".to_string()),
            client_order_id: Some("client123".to_string()),
            order_price: Some("50000.00".to_string()),
            order_size: Some("0.001".to_string()),
            order_type: Some("buy-limit".to_string()),
            side: Some("buy".to_string()),
            order_status: Some("submitted".to_string()),
            order_create_time: Some(1616663113987),
            last_act_time: None,
            trade_price: None,
            trade_volume: Some("0.00000000".to_string()),
            trade_turnover: None,
            filled_amount: Some("0.00000000".to_string()),
            filled_cash_amount: Some("0.00000".to_string()),
            filled_fees: Some("0.00000".to_string()),
            fee_currency: Some("usdt".to_string()),
            stop_price: None,
            event_type: Some("creation".to_string()),
        };

        let event = HtxWs::parse_order_update(&data);
        assert_eq!(event.order.id, "12345");
        assert_eq!(event.order.client_order_id, Some("client123".to_string()));
        assert_eq!(event.order.symbol, "BTC/USDT");
        assert_eq!(event.order.status, OrderStatus::Open);
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
    }

    #[test]
    fn test_parse_my_trade() {
        let data = HtxTradeDetailData {
            symbol: "btcusdt".to_string(),
            order_id: Some("12345".to_string()),
            trade_id: Some("67890".to_string()),
            trade_price: Some("50500.00".to_string()),
            trade_volume: Some("0.001".to_string()),
            trade_turnover: Some("50.50".to_string()),
            trade_time: Some(1616663115123),
            trade_type: Some("buy".to_string()),
            role: Some("taker".to_string()),
            trade_fee: Some("0.10100".to_string()),
            fee_currency: Some("usdt".to_string()),
        };

        let (symbol, trade) = HtxWs::parse_my_trade(&data);
        assert_eq!(symbol, "BTC/USDT");
        assert_eq!(trade.id, "67890");
        assert_eq!(trade.order, Some("12345".to_string()));
        assert_eq!(trade.side, Some("buy".to_string()));
        assert_eq!(trade.taker_or_maker, Some(TakerOrMaker::Taker));
    }

    #[test]
    fn test_order_status_mapping() {
        let test_cases = vec![
            ("submitted", OrderStatus::Open),
            ("pre-submitted", OrderStatus::Open),
            ("partial-filled", OrderStatus::Open),
            ("filled", OrderStatus::Closed),
            ("partial-canceled", OrderStatus::Canceled),
            ("canceled", OrderStatus::Canceled),
            ("unknown", OrderStatus::Open),
        ];

        for (status_str, expected) in test_cases {
            let data = HtxOrderUpdateData {
                order_status: Some(status_str.to_string()),
                ..Default::default()
            };
            let event = HtxWs::parse_order_update(&data);
            assert_eq!(event.order.status, expected, "Failed for status: {status_str}");
        }
    }

    #[test]
    fn test_process_private_message_auth_success() {
        let msg = r#"{"action":"req","ch":"auth","code":200,"message":"success"}"#;
        let result = HtxWs::process_private_message(msg);
        assert!(matches!(result, Some(WsMessage::Authenticated)));
    }

    #[test]
    fn test_process_private_message_auth_fail() {
        let msg = r#"{"action":"req","ch":"auth","code":401,"message":"Invalid signature"}"#;
        let result = HtxWs::process_private_message(msg);
        if let Some(WsMessage::Error(e)) = result {
            assert!(e.contains("Invalid signature"));
        } else {
            panic!("Expected Error message");
        }
    }

    #[test]
    fn test_process_private_message_subscribed() {
        let msg = r#"{"action":"sub","ch":"orders#btcusdt","code":200}"#;
        let result = HtxWs::process_private_message(msg);
        if let Some(WsMessage::Subscribed { channel, symbol: _ }) = result {
            assert_eq!(channel, "orders#btcusdt");
        } else {
            panic!("Expected Subscribed message");
        }
    }

    #[test]
    fn test_process_private_message_order_push() {
        let msg = r#"{"action":"push","ch":"orders#btcusdt","data":{"orderId":"12345","symbol":"btcusdt","orderStatus":"submitted","side":"buy","type":"buy-limit","orderPrice":"50000.00","orderSize":"0.001"}}"#;
        let result = HtxWs::process_private_message(msg);
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "12345");
            assert_eq!(event.order.symbol, "BTC/USDT");
            assert_eq!(event.order.status, OrderStatus::Open);
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_private_message_trade_push() {
        let msg = r#"{"action":"push","ch":"trade.clearing#btcusdt","data":{"symbol":"btcusdt","orderId":"12345","tradeId":"67890","tradePrice":"50500.00","tradeVolume":"0.001","tradeType":"buy","role":"taker"}}"#;
        let result = HtxWs::process_private_message(msg);
        if let Some(WsMessage::MyTrade(event)) = result {
            assert_eq!(event.symbol, "BTC/USDT");
            assert_eq!(event.trades.len(), 1);
            assert_eq!(event.trades[0].id, "67890");
        } else {
            panic!("Expected MyTrade message");
        }
    }

    // === Balance Tests ===

    #[test]
    fn test_parse_balance_update() {
        let data = HtxAccountUpdateData {
            currency: "usdt".to_string(),
            account_id: Some(12345),
            balance: Some("10500.00".to_string()),
            available: Some("10000.00".to_string()),
            change_type: Some("transfer".to_string()),
            account_type: Some("spot".to_string()),
            change_time: Some(1699123456789),
        };

        let event = HtxWs::parse_balance_update(&data);
        assert!(event.balances.currencies.contains_key("USDT"));

        let usdt = event.balances.currencies.get("USDT").unwrap();
        assert_eq!(usdt.free, Some(Decimal::new(1000000, 2)));
        assert_eq!(usdt.used, Some(Decimal::new(50000, 2)));
        assert_eq!(usdt.total, Some(Decimal::new(1050000, 2)));
    }

    #[test]
    fn test_parse_balance_update_btc() {
        let data = HtxAccountUpdateData {
            currency: "btc".to_string(),
            account_id: Some(12345),
            balance: Some("2.5".to_string()),
            available: Some("2.0".to_string()),
            change_type: Some("trade".to_string()),
            account_type: Some("spot".to_string()),
            change_time: Some(1699123456789),
        };

        let event = HtxWs::parse_balance_update(&data);
        assert!(event.balances.currencies.contains_key("BTC"));

        let btc = event.balances.currencies.get("BTC").unwrap();
        assert_eq!(btc.free, Some(Decimal::new(20, 1)));
        assert_eq!(btc.used, Some(Decimal::new(5, 1)));
        assert_eq!(btc.total, Some(Decimal::new(25, 1)));
    }

    #[test]
    fn test_process_private_message_balance_push() {
        let msg = r#"{"action":"push","ch":"accounts.update#1","data":{"currency":"usdt","accountId":12345,"balance":"10500.00","available":"10000.00","changeType":"transfer","accountType":"spot","changeTime":1699123456789}}"#;
        let result = HtxWs::process_private_message(msg);
        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("USDT"));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_account_data_types() {
        let json = r#"{"currency":"eth","accountId":67890,"balance":"5.0","available":"4.5","changeType":"order-match","accountType":"spot","changeTime":1699123456789}"#;
        let data: HtxAccountUpdateData = serde_json::from_str(json).unwrap();
        assert_eq!(data.currency, "eth");
        assert_eq!(data.account_id, Some(67890));
        assert_eq!(data.change_type, Some("order-match".to_string()));
    }
}
