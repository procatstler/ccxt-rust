//! CoinEx WebSocket Implementation
//!
//! CoinEx 실시간 데이터 스트리밍
//!
//! Public URL: wss://socket.coinex.com/v2/spot
//! Private URL: wss://socket.coinex.com/v2/spot (same endpoint, authenticated)
//!
//! Authentication: HMAC-SHA256 signature
//! Private channels: order, asset

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Ticker, TimeInForce, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsOhlcvEvent,
    WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent, OHLCV,
};

const WS_PUBLIC_URL: &str = "wss://socket.coinex.com/v2/spot";

type HmacSha256 = Hmac<Sha256>;

/// Order update data from CoinEx private WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoinexOrderUpdateData {
    #[serde(default)]
    order_id: Option<i64>,
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    deal_amount: Option<String>,
    #[serde(default)]
    deal_money: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    create_time: Option<i64>,
    #[serde(default)]
    update_time: Option<i64>,
    #[serde(default)]
    client_id: Option<String>,
}

/// Balance update data from CoinEx private WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoinexBalanceUpdateData {
    #[serde(default)]
    asset: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    frozen: Option<String>,
}

/// CoinEx WebSocket 클라이언트
pub struct CoinexWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
    request_id: AtomicI64,
}

impl CoinexWs {
    /// 새 CoinEx WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: None,
            api_secret: None,
            request_id: AtomicI64::new(1),
        }
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: Some(api_key),
            api_secret: Some(api_secret),
            request_id: AtomicI64::new(1),
        }
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
    }

    /// Get next request ID
    fn next_id(&self) -> i64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Generate HMAC-SHA256 signature
    fn sign(&self, message: &str) -> CcxtResult<String> {
        let secret = self.api_secret.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret not set".to_string(),
        })?;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Failed to create HMAC: {e}"),
            })?;
        mac.update(message.as_bytes());
        let result = mac.finalize();
        Ok(hex::encode(result.into_bytes()))
    }

    /// Subscribe to private stream with authentication
    async fn subscribe_private_stream(
        &mut self,
        channel: &str,
        tx: mpsc::UnboundedSender<WsMessage>,
    ) -> CcxtResult<()> {
        let api_key = self.api_key.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key not set".to_string(),
        })?;

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 15,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Generate timestamp and signature for authentication
        let timestamp = Utc::now().timestamp_millis();
        let sign_payload = format!("access_id={api_key}&timestamp={timestamp}");
        let signature = self.sign(&sign_payload)?;

        // Send authentication message
        let auth_msg = serde_json::json!({
            "method": "server.sign",
            "params": [api_key, signature, timestamp],
            "id": self.next_id()
        });

        ws_client.send(&auth_msg.to_string())?;

        // Wait a moment for auth response
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Subscribe to private channel
        let subscribe_msg = serde_json::json!({
            "method": format!("{}.subscribe", channel),
            "params": [],
            "id": self.next_id()
        });

        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // Spawn message handler
        let channel_str = channel.to_string();
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
                        if let Some(ws_msg) = Self::process_private_message(&msg, &channel_str) {
                            if tx.send(ws_msg).is_err() {
                                break;
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

        Ok(())
    }

    /// Process private WebSocket message
    fn process_private_message(msg: &str, channel: &str) -> Option<WsMessage> {
        let parsed: serde_json::Value = serde_json::from_str(msg).ok()?;

        let method = parsed.get("method").and_then(|m| m.as_str())?;

        match method {
            "order.update" => {
                if let Some(params) = parsed.get("params").and_then(|p| p.as_array()) {
                    if let Some(first) = params.get(1) {
                        if let Ok(data) = serde_json::from_value::<CoinexOrderUpdateData>(first.clone()) {
                            let order = Self::parse_order(&data);
                            return Some(WsMessage::Order(WsOrderEvent { order }));
                        }
                    }
                }
            }
            "asset.update" => {
                if let Some(params) = parsed.get("params").and_then(|p| p.as_array()) {
                    if let Some(assets) = params.first().and_then(|a| a.as_object()) {
                        let mut currencies = HashMap::new();
                        for (currency, data) in assets {
                            if let Ok(balance_data) = serde_json::from_value::<CoinexBalanceUpdateData>(data.clone()) {
                                let balance = Self::parse_balance(&balance_data);
                                currencies.insert(currency.clone(), balance);
                            }
                        }
                        let balances = Balances {
                            timestamp: Some(Utc::now().timestamp_millis()),
                            datetime: None,
                            currencies,
                            info: parsed.clone(),
                        };
                        return Some(WsMessage::Balance(WsBalanceEvent { balances }));
                    }
                }
            }
            _ => {
                // Check for channel-specific messages
                if channel == "order" && method.starts_with("order.") {
                    // Handle other order-related messages
                } else if channel == "asset" && method.starts_with("asset.") {
                    // Handle other asset-related messages
                }
            }
        }

        None
    }

    /// Parse order from update data
    fn parse_order(data: &CoinexOrderUpdateData) -> Order {
        let symbol = data.market.as_ref()
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let status = data.status.as_ref().map(|s| match s.as_str() {
            "not_deal" => OrderStatus::Open,
            "part_deal" => OrderStatus::Open,
            "done" => OrderStatus::Closed,
            "cancel" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        }).unwrap_or(OrderStatus::Open);

        let side = data.side.as_ref().map(|s| match s.as_str() {
            "buy" | "BUY" => OrderSide::Buy,
            "sell" | "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        }).unwrap_or(OrderSide::Buy);

        let order_type = data.order_type.as_ref().map(|t| match t.as_str() {
            "limit" | "LIMIT" => OrderType::Limit,
            "market" | "MARKET" => OrderType::Market,
            _ => OrderType::Limit,
        }).unwrap_or(OrderType::Limit);

        let price = data.price.as_ref()
            .and_then(|p| p.parse::<Decimal>().ok());
        let amount = data.amount.as_ref()
            .and_then(|q| q.parse::<Decimal>().ok())
            .unwrap_or_default();
        let filled = data.deal_amount.as_ref()
            .and_then(|f| f.parse::<Decimal>().ok())
            .unwrap_or_default();
        let remaining = amount - filled;
        let cost = data.deal_money.as_ref()
            .and_then(|c| c.parse::<Decimal>().ok());

        let timestamp = data.create_time.map(|t| t * 1000);

        Order {
            id: data.order_id.map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: data.client_id.clone(),
            timestamp,
            datetime: timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: data.update_time.map(|t| t * 1000),
            status,
            symbol,
            order_type,
            time_in_force: Some(TimeInForce::GTC),
            side,
            price,
            stop_price: None,
            trigger_price: None,
            average: if filled > Decimal::ZERO {
                cost.map(|c| c / filled)
            } else {
                None
            },
            amount,
            filled,
            remaining: Some(remaining),
            cost,
            trades: vec![],
            fee: None,
            fees: vec![],
            info: serde_json::to_value(data).unwrap_or_default(),
            reduce_only: None,
            post_only: None,
            take_profit_price: None,
            stop_loss_price: None,
        }
    }

    /// Parse balance from update data
    fn parse_balance(data: &CoinexBalanceUpdateData) -> Balance {
        let free = data.available.as_ref()
            .and_then(|f| f.parse::<Decimal>().ok());
        let used = data.frozen.as_ref()
            .and_then(|l| l.parse::<Decimal>().ok());
        let total = match (free, used) {
            (Some(f), Some(u)) => Some(f + u),
            (Some(f), None) => Some(f),
            (None, Some(u)) => Some(u),
            _ => None,
        };

        Balance {
            free,
            used,
            total,
            debt: None,
        }
    }

    /// 심볼을 CoinEx 형식으로 변환 (BTC/USDT -> BTCUSDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// CoinEx 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(coinex_symbol: &str) -> String {
        for quote in &["USDT", "USDC", "BTC", "ETH"] {
            if let Some(base) = coinex_symbol.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }
        coinex_symbol.to_string()
    }

    /// Timeframe을 CoinEx 형식으로 변환
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
            _ => 60,
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &CoinexTickerData, symbol: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|v| v.parse().ok()),
            low: data.low.as_ref().and_then(|v| v.parse().ok()),
            bid: data.buy.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.buy_amount.as_ref().and_then(|v| v.parse().ok()),
            ask: data.sell.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.sell_amount.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.vol.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &CoinexOrderBookData, symbol: &str) -> WsOrderBookEvent {
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

        let timestamp = Utc::now().timestamp_millis();
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
    fn parse_trade(data: &CoinexTradeData, symbol: &str) -> WsTradeEvent {
        let timestamp = data.date_ms.unwrap_or_else(|| Utc::now().timestamp_millis());
        let unified_symbol = Self::to_unified_symbol(symbol);
        let price = data.price.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();
        let amount = data.amount.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();

        let trades = vec![Trade {
            id: data.id.map(|id| id.to_string()).unwrap_or_default(),
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
            side: data.trade_type.clone(),
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
    fn parse_candle(data: &[String], symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        if data.len() < 6 {
            return None;
        }

        let unified_symbol = Self::to_unified_symbol(symbol);

        let ohlcv = OHLCV {
            timestamp: data[0].parse().ok()?,
            open: data[1].parse().ok()?,
            close: data[2].parse().ok()?,
            high: data[3].parse().ok()?,
            low: data[4].parse().ok()?,
            volume: data[5].parse().ok()?,
        };

        Some(WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe,
            ohlcv,
        })
    }

    /// 메시지 처리
    fn process_message(msg: &str, subscribed_symbol: Option<&str>, subscribed_timeframe: Option<Timeframe>) -> Option<WsMessage> {
        let response: CoinexWsResponse = serde_json::from_str(msg).ok()?;

        let method = response.method.as_deref()?;
        let symbol = subscribed_symbol.unwrap_or("");

        match method {
            "state.update" => {
                if let Some(params) = response.params {
                    if let Some(first) = params.first() {
                        if let Ok(ticker_data) = serde_json::from_value::<CoinexTickerData>(first.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data, symbol)));
                        }
                    }
                }
            }
            "depth.update" => {
                if let Some(params) = response.params {
                    if params.len() >= 2 {
                        if let Ok(book_data) = serde_json::from_value::<CoinexOrderBookData>(params[1].clone()) {
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&book_data, symbol)));
                        }
                    }
                }
            }
            "deals.update" => {
                if let Some(params) = response.params {
                    if params.len() >= 2 {
                        if let Some(trades_arr) = params[1].as_array() {
                            if let Some(first) = trades_arr.first() {
                                if let Ok(trade_data) = serde_json::from_value::<CoinexTradeData>(first.clone()) {
                                    return Some(WsMessage::Trade(Self::parse_trade(&trade_data, symbol)));
                                }
                            }
                        }
                    }
                }
            }
            "kline.update" => {
                if let Some(params) = response.params {
                    if let Some(first) = params.first() {
                        if let Ok(kline_arr) = serde_json::from_value::<Vec<String>>(first.clone()) {
                            let timeframe = subscribed_timeframe.unwrap_or(Timeframe::Minute1);
                            if let Some(event) = Self::parse_candle(&kline_arr, symbol, timeframe) {
                                return Some(WsMessage::Ohlcv(event));
                            }
                        }
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
}

impl Default for CoinexWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for CoinexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "method": "state.subscribe",
            "params": [formatted],
            "id": 1
        });
        client.subscribe_stream(subscribe_msg, "ticker", Some(&formatted), None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let params: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        let subscribe_msg = serde_json::json!({
            "method": "state.subscribe",
            "params": params,
            "id": 1
        });
        client.subscribe_stream(subscribe_msg, "tickers", None, None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let depth = limit.unwrap_or(20).to_string();
        let subscribe_msg = serde_json::json!({
            "method": "depth.subscribe",
            "params": [formatted, depth, "0"],
            "id": 2
        });
        client.subscribe_stream(subscribe_msg, "orderBook", Some(&formatted), None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "method": "deals.subscribe",
            "params": [formatted],
            "id": 3
        });
        client.subscribe_stream(subscribe_msg, "trade", Some(&formatted), None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let subscribe_msg = serde_json::json!({
            "method": "kline.subscribe",
            "params": [formatted, interval],
            "id": 4
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

    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let mut client = Self::with_credentials(
            self.api_key.clone().unwrap(),
            self.api_secret.clone().unwrap(),
        );
        client.subscribe_private_stream("order", tx).await?;
        Ok(rx)
    }

    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        // CoinEx sends trade info as part of order updates
        let (tx, rx) = mpsc::unbounded_channel();
        let mut client = Self::with_credentials(
            self.api_key.clone().unwrap(),
            self.api_secret.clone().unwrap(),
        );
        client.subscribe_private_stream("order", tx).await?;
        Ok(rx)
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let mut client = Self::with_credentials(
            self.api_key.clone().unwrap(),
            self.api_secret.clone().unwrap(),
        );
        client.subscribe_private_stream("asset", tx).await?;
        Ok(rx)
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials not set".into(),
            });
        }
        // Authentication is handled per-connection in subscribe_private_stream
        Ok(())
    }
}

// === Response Types ===

#[derive(Debug, Default, Deserialize)]
struct CoinexWsResponse {
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinexTickerData {
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    vol: Option<String>,
    #[serde(default)]
    buy: Option<String>,
    #[serde(default)]
    buy_amount: Option<String>,
    #[serde(default)]
    sell: Option<String>,
    #[serde(default)]
    sell_amount: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinexOrderBookData {
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinexTradeData {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default, rename = "type")]
    trade_type: Option<String>,
    #[serde(default)]
    date_ms: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_format_symbol() {
        assert_eq!(CoinexWs::format_symbol("BTC/USDT"), "BTCUSDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(CoinexWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(CoinexWs::to_unified_symbol("ETHBTC"), "ETH/BTC");
    }

    #[test]
    fn test_with_credentials() {
        let ws = CoinexWs::with_credentials("api_key".to_string(), "api_secret".to_string());
        assert_eq!(ws.api_key, Some("api_key".to_string()));
        assert_eq!(ws.api_secret, Some("api_secret".to_string()));
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = CoinexWs::new();
        assert!(ws.api_key.is_none());
        ws.set_credentials("api_key".to_string(), "api_secret".to_string());
        assert_eq!(ws.api_key, Some("api_key".to_string()));
    }

    #[test]
    fn test_next_id() {
        let ws = CoinexWs::new();
        assert_eq!(ws.next_id(), 1);
        assert_eq!(ws.next_id(), 2);
        assert_eq!(ws.next_id(), 3);
    }

    #[test]
    fn test_sign() {
        let ws = CoinexWs::with_credentials("api_key".to_string(), "test_secret".to_string());
        let result = ws.sign("test_message").unwrap();
        // HMAC-SHA256 should produce a 64-character hex string
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn test_parse_order() {
        let data = CoinexOrderUpdateData {
            order_id: Some(12345),
            market: Some("BTCUSDT".to_string()),
            order_type: Some("limit".to_string()),
            side: Some("buy".to_string()),
            amount: Some("1.5".to_string()),
            price: Some("50000.0".to_string()),
            deal_amount: Some("0.5".to_string()),
            deal_money: Some("25000.0".to_string()),
            status: Some("part_deal".to_string()),
            create_time: Some(1234567890),
            update_time: Some(1234567900),
            client_id: Some("client123".to_string()),
        };

        let order = CoinexWs::parse_order(&data);
        assert_eq!(order.id, "12345");
        assert_eq!(order.symbol, "BTC/USDT");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.status, OrderStatus::Open);
        assert_eq!(order.price, Some(Decimal::from_str("50000.0").unwrap()));
        assert_eq!(order.amount, Decimal::from_str("1.5").unwrap());
        assert_eq!(order.filled, Decimal::from_str("0.5").unwrap());
    }

    #[test]
    fn test_parse_order_status_mapping() {
        let test_cases = [
            ("not_deal", OrderStatus::Open),
            ("part_deal", OrderStatus::Open),
            ("done", OrderStatus::Closed),
            ("cancel", OrderStatus::Canceled),
        ];

        for (status_str, expected_status) in test_cases {
            let data = CoinexOrderUpdateData {
                order_id: Some(1),
                market: Some("BTCUSDT".to_string()),
                order_type: Some("limit".to_string()),
                side: Some("buy".to_string()),
                amount: Some("1.0".to_string()),
                price: Some("50000.0".to_string()),
                deal_amount: None,
                deal_money: None,
                status: Some(status_str.to_string()),
                create_time: None,
                update_time: None,
                client_id: None,
            };

            let order = CoinexWs::parse_order(&data);
            assert_eq!(order.status, expected_status, "Status mismatch for {status_str}");
        }
    }

    #[test]
    fn test_parse_balance() {
        let data = CoinexBalanceUpdateData {
            asset: Some("BTC".to_string()),
            available: Some("1.5".to_string()),
            frozen: Some("0.5".to_string()),
        };

        let balance = CoinexWs::parse_balance(&data);
        assert_eq!(balance.free, Some(Decimal::from_str("1.5").unwrap()));
        assert_eq!(balance.used, Some(Decimal::from_str("0.5").unwrap()));
        assert_eq!(balance.total, Some(Decimal::from_str("2.0").unwrap()));
    }

    #[test]
    fn test_parse_balance_partial() {
        let data = CoinexBalanceUpdateData {
            asset: Some("ETH".to_string()),
            available: Some("10.0".to_string()),
            frozen: None,
        };

        let balance = CoinexWs::parse_balance(&data);
        assert_eq!(balance.free, Some(Decimal::from_str("10.0").unwrap()));
        assert_eq!(balance.used, None);
        assert_eq!(balance.total, Some(Decimal::from_str("10.0").unwrap()));
    }

    #[test]
    fn test_process_private_message_order() {
        let msg = r#"{
            "method": "order.update",
            "params": [1, {
                "order_id": 12345,
                "market": "BTCUSDT",
                "order_type": "limit",
                "side": "buy",
                "amount": "1.0",
                "price": "50000.0",
                "status": "not_deal"
            }],
            "id": null
        }"#;

        let result = CoinexWs::process_private_message(msg, "order");
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "12345");
            assert_eq!(event.order.symbol, "BTC/USDT");
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_private_message_balance() {
        let msg = r#"{
            "method": "asset.update",
            "params": [{"BTC": {"available": "1.5", "frozen": "0.5"}}],
            "id": null
        }"#;

        let result = CoinexWs::process_private_message(msg, "asset");
        assert!(result.is_some());
        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("BTC"));
            let btc = event.balances.currencies.get("BTC").unwrap();
            assert_eq!(btc.free, Some(Decimal::from_str("1.5").unwrap()));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[tokio::test]
    async fn test_watch_orders_requires_credentials() {
        let ws = CoinexWs::new();
        let result = ws.watch_orders(None).await;
        assert!(result.is_err());
        if let Err(CcxtError::AuthenticationError { message }) = result {
            assert!(message.contains("credentials"));
        } else {
            panic!("Expected AuthenticationError");
        }
    }

    #[tokio::test]
    async fn test_watch_balance_requires_credentials() {
        let ws = CoinexWs::new();
        let result = ws.watch_balance().await;
        assert!(result.is_err());
        if let Err(CcxtError::AuthenticationError { message }) = result {
            assert!(message.contains("credentials"));
        } else {
            panic!("Expected AuthenticationError");
        }
    }

    #[tokio::test]
    async fn test_ws_authenticate_requires_credentials() {
        let mut ws = CoinexWs::new();
        let result = ws.ws_authenticate().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ws_authenticate_with_credentials() {
        let mut ws = CoinexWs::with_credentials("key".to_string(), "secret".to_string());
        let result = ws.ws_authenticate().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_default() {
        let ws = CoinexWs::default();
        assert!(ws.api_key.is_none());
    }
}
