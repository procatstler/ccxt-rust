//! CoinCatch WebSocket Implementation
//!
//! CoinCatch real-time data streaming
//! API Documentation: <https://coincatch.github.io/github.io/en/spot/>
//!
//! # Public Channels
//! - ticker - Market ticker updates
//! - books - Order book updates
//! - trade - Trade updates
//! - candle - OHLCV candle updates
//!
//! # Private Channels (requires authentication)
//! - orders - Order updates
//! - account - Account balance updates

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
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
use crate::errors::CcxtResult;
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Ticker, Timeframe, Trade, OHLCV, WsBalanceEvent, WsExchange, WsMessage, WsOhlcvEvent,
    WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_PUBLIC_URL: &str = "wss://ws.coincatch.com/spot/v1/stream";
const WS_PRIVATE_URL: &str = "wss://ws.coincatch.com/spot/v1/stream";

/// CoinCatch WebSocket client
pub struct CoinCatchWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
}

impl CoinCatchWs {
    /// Create new CoinCatch WebSocket client
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: None,
            api_secret: None,
            passphrase: None,
        }
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: &str, api_secret: &str, passphrase: &str) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: Some(api_key.to_string()),
            api_secret: Some(api_secret.to_string()),
            passphrase: Some(passphrase.to_string()),
        }
    }

    /// Set API credentials
    pub fn set_credentials(&mut self, api_key: &str, api_secret: &str, passphrase: &str) {
        self.api_key = Some(api_key.to_string());
        self.api_secret = Some(api_secret.to_string());
        self.passphrase = Some(passphrase.to_string());
    }

    /// Get API key
    pub fn get_api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Generate authentication signature
    /// sign = Base64(HMAC_SHA256(timestamp + "GET" + "/user/verify", secretKey))
    fn generate_auth_signature(api_secret: &str, timestamp: &str) -> String {
        let sign_str = format!("{timestamp}GET/user/verify");
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(sign_str.as_bytes());
        BASE64.encode(mac.finalize().into_bytes())
    }

    /// Convert symbol to CoinCatch format (BTC/USDT -> BTCUSDT_SPBL)
    fn format_symbol(symbol: &str) -> String {
        let base = symbol.replace("/", "");
        format!("{base}_SPBL")
    }

    /// Convert CoinCatch symbol to unified format (BTCUSDT_SPBL -> BTC/USDT)
    fn to_unified_symbol(coincatch_symbol: &str) -> String {
        let id = coincatch_symbol.replace("_SPBL", "");
        for quote in &["USDT", "USDC", "BTC", "ETH"] {
            if let Some(base) = id.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }
        id
    }

    /// Convert timeframe to CoinCatch format
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1min",
            Timeframe::Minute5 => "5min",
            Timeframe::Minute15 => "15min",
            Timeframe::Minute30 => "30min",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1day",
            Timeframe::Day3 => "3day",
            Timeframe::Week1 => "1week",
            Timeframe::Month1 => "1M",
            _ => "1min",
        }
    }

    /// Parse ticker message
    fn parse_ticker(data: &CoinCatchTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: data.low24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: data.bid_pr.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: data.bid_sz.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask: data.ask_pr.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: data.ask_sz.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            close: data.last_pr.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            last: data.last_pr.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: data.change.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            percentage: data.change_pct.as_ref().and_then(|v| {
                Decimal::from_str(v.trim_end_matches('%')).ok()
            }),
            average: None,
            base_volume: data.base_vol.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: data.quote_vol.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// Parse order book message
    fn parse_order_book(data: &CoinCatchOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: Decimal::from_str(&b[0]).ok()?,
                    amount: Decimal::from_str(&b[1]).ok()?,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: Decimal::from_str(&a[0]).ok()?,
                    amount: Decimal::from_str(&a[1]).ok()?,
                })
            } else {
                None
            }
        }).collect();

        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

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
            is_snapshot,
        }
    }

    /// Parse trade message
    fn parse_trades(data: &[CoinCatchTradeData], symbol: &str) -> WsTradeEvent {
        let trades: Vec<Trade> = data.iter().map(|t| {
            let timestamp = t.ts.as_ref()
                .and_then(|ts| ts.parse::<i64>().ok())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            let price = t.price.as_ref()
                .and_then(|p| Decimal::from_str(p).ok())
                .unwrap_or(Decimal::ZERO);
            let amount = t.size.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);

            Trade {
                id: t.trade_id.clone().unwrap_or_default(),
                order: None,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol.to_string(),
                trade_type: None,
                side: t.side.clone(),
                taker_or_maker: None,
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: vec![],
                info: serde_json::to_value(t).unwrap_or_default(),
            }
        }).collect();

        WsTradeEvent {
            symbol: symbol.to_string(),
            trades,
        }
    }

    /// Parse OHLCV message
    fn parse_ohlcv(data: &CoinCatchCandleData, symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let timestamp = data.ts.as_ref()?.parse::<i64>().ok()?;

        let ohlcv = OHLCV {
            timestamp,
            open: Decimal::from_str(data.open.as_ref()?).ok()?,
            high: Decimal::from_str(data.high.as_ref()?).ok()?,
            low: Decimal::from_str(data.low.as_ref()?).ok()?,
            close: Decimal::from_str(data.close.as_ref()?).ok()?,
            volume: Decimal::from_str(data.vol.as_ref()?).ok()?,
        };

        Some(WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        })
    }

    /// Parse order update message
    fn parse_order(data: &CoinCatchOrderData) -> WsOrderEvent {
        let timestamp = data.c_time.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let symbol = Self::to_unified_symbol(data.inst_id.as_ref().unwrap_or(&String::new()));

        let status = match data.status.as_deref() {
            Some("new") => OrderStatus::Open,
            Some("partial_fill") | Some("partial-fill") => OrderStatus::Open,
            Some("full_fill") | Some("full-fill") => OrderStatus::Closed,
            Some("cancelled") | Some("canceled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let amount = data.size.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let filled = data.fill_sz.as_ref()
            .and_then(|f| Decimal::from_str(f).ok())
            .unwrap_or(Decimal::ZERO);
        let remaining = amount - filled;

        let order = Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.u_time.as_ref().and_then(|t| t.parse().ok()),
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price,
            average: data.fill_price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
            amount,
            filled,
            remaining: Some(remaining),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: data.fill_notional.as_ref().and_then(|n| Decimal::from_str(n).ok()),
            trades: vec![],
            fee: None,
            fees: vec![],
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// Parse balance update message
    fn parse_balance(data: &[CoinCatchBalanceData]) -> WsBalanceEvent {
        let mut currencies = HashMap::new();

        for asset in data {
            let currency = asset.coin.clone().unwrap_or_default();
            let free = asset.available.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);
            let frozen = asset.frozen.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);
            let locked = asset.lock.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);
            let used = frozen + locked;
            let total = free + used;

            currencies.insert(currency, Balance {
                free: Some(free),
                used: Some(used),
                total: Some(total),
                debt: None,
            });
        }

        let timestamp = Utc::now().timestamp_millis();
        let balances = Balances {
            currencies,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            info: serde_json::Value::Null,
        };

        WsBalanceEvent { balances }
    }

    /// Process incoming WebSocket message
    fn process_message(msg: &str, timeframe_hint: Option<Timeframe>) -> Option<WsMessage> {
        let json: CoinCatchWsMessage = serde_json::from_str(msg).ok()?;

        // Handle subscription response
        if json.event.as_deref() == Some("subscribe") {
            return Some(WsMessage::Subscribed {
                channel: json.arg.as_ref()?.channel.clone()?,
                symbol: json.arg.as_ref()?.inst_id.clone(),
            });
        }

        // Handle login response
        if json.event.as_deref() == Some("login") {
            if json.code.as_deref() == Some("0") {
                return Some(WsMessage::Authenticated);
            } else {
                return Some(WsMessage::Error(format!(
                    "Authentication failed: {}",
                    json.msg.unwrap_or_default()
                )));
            }
        }

        // Handle error
        if json.event.as_deref() == Some("error") {
            return Some(WsMessage::Error(json.msg.unwrap_or_default()));
        }

        // Handle data messages
        let data = json.data?;
        let arg = json.arg?;
        let channel = arg.channel.as_deref()?;
        let inst_id = arg.inst_id.as_deref().unwrap_or("");
        let symbol = Self::to_unified_symbol(inst_id);

        match channel {
            "ticker" => {
                if let Some(ticker_data) = data.first() {
                    if let Ok(ticker) = serde_json::from_value::<CoinCatchTickerData>(ticker_data.clone()) {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&ticker)));
                    }
                }
            }
            "books" | "books5" | "books15" => {
                if let Some(book_data) = data.first() {
                    if let Ok(book) = serde_json::from_value::<CoinCatchOrderBookData>(book_data.clone()) {
                        let is_snapshot = json.action.as_deref() == Some("snapshot");
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&book, &symbol, is_snapshot)));
                    }
                }
            }
            "trade" => {
                let trades: Vec<CoinCatchTradeData> = data.iter()
                    .filter_map(|d| serde_json::from_value(d.clone()).ok())
                    .collect();
                if !trades.is_empty() {
                    return Some(WsMessage::Trade(Self::parse_trades(&trades, &symbol)));
                }
            }
            c if c.starts_with("candle") => {
                if let Some(candle_data) = data.first() {
                    if let Ok(candle) = serde_json::from_value::<CoinCatchCandleData>(candle_data.clone()) {
                        let tf = timeframe_hint.unwrap_or(Timeframe::Minute1);
                        if let Some(ohlcv) = Self::parse_ohlcv(&candle, &symbol, tf) {
                            return Some(WsMessage::Ohlcv(ohlcv));
                        }
                    }
                }
            }
            "orders" => {
                if let Some(order_data) = data.first() {
                    if let Ok(order) = serde_json::from_value::<CoinCatchOrderData>(order_data.clone()) {
                        return Some(WsMessage::Order(Self::parse_order(&order)));
                    }
                }
            }
            "account" => {
                let balances: Vec<CoinCatchBalanceData> = data.iter()
                    .filter_map(|d| serde_json::from_value(d.clone()).ok())
                    .collect();
                if !balances.is_empty() {
                    return Some(WsMessage::Balance(Self::parse_balance(&balances)));
                }
            }
            _ => {}
        }

        None
    }

    /// Subscribe to a channel and return event stream
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        inst_id: &str,
        requires_auth: bool,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let ws_url = if requires_auth { WS_PRIVATE_URL } else { WS_PUBLIC_URL };

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Authenticate if required
        if requires_auth {
            if let (Some(api_key), Some(api_secret), Some(passphrase)) =
                (&self.api_key, &self.api_secret, &self.passphrase)
            {
                let timestamp = Utc::now().timestamp().to_string();
                let sign = Self::generate_auth_signature(api_secret, &timestamp);

                let login_msg = serde_json::json!({
                    "op": "login",
                    "args": [{
                        "apiKey": api_key,
                        "passphrase": passphrase,
                        "timestamp": timestamp,
                        "sign": sign
                    }]
                });

                ws_client.send(&login_msg.to_string())?;
            } else {
                return Err(crate::errors::CcxtError::AuthenticationError {
                    message: "API credentials required for private channels".to_string(),
                });
            }
        }

        // Build subscription message
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": [{
                "instType": "sp",
                "channel": channel,
                "instId": inst_id
            }]
        });

        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // Save subscription
        {
            let key = format!("{channel}:{inst_id}");
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
                        if let Some(ws_msg) = Self::process_message(&msg, None) {
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

    /// Subscribe to order updates (private channel)
    pub async fn watch_orders(&mut self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let inst_id = symbol.map(Self::format_symbol).unwrap_or_default();
        self.subscribe_stream("orders", &inst_id, true).await
    }

    /// Subscribe to balance updates (private channel)
    pub async fn watch_balance(&mut self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_stream("account", "", true).await
    }
}

impl Default for CoinCatchWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for CoinCatchWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
            passphrase: self.passphrase.clone(),
        }
    }
}

#[async_trait]
impl WsExchange for CoinCatchWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let inst_id = Self::format_symbol(symbol);
        ws.subscribe_stream("ticker", &inst_id, false).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let inst_id = Self::format_symbol(symbol);
        ws.subscribe_stream("books", &inst_id, false).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let inst_id = Self::format_symbol(symbol);
        ws.subscribe_stream("trade", &inst_id, false).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let inst_id = Self::format_symbol(symbol);
        let channel = format!("candle{}", Self::format_interval(timeframe));
        ws.subscribe_stream(&channel, &inst_id, false).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let mut ws_client = WsClient::new(WsConfig {
                url: WS_PUBLIC_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 30,
                connect_timeout_secs: 30,
            });

            ws_client.connect().await?;
            self.ws_client = Some(ws_client);
        }
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws_client) = &self.ws_client {
            ws_client.close()?;
            self.ws_client = None;
        }
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        match &self.ws_client {
            Some(c) => c.is_connected().await,
            None => false,
        }
    }
}

// ===== WebSocket Message Structures =====

#[derive(Debug, Deserialize)]
struct CoinCatchWsMessage {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    arg: Option<CoinCatchWsArg>,
    #[serde(default)]
    data: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
struct CoinCatchWsArg {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default, rename = "instId")]
    inst_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CoinCatchTickerData {
    #[serde(default, rename = "instId")]
    inst_id: String,
    #[serde(default)]
    ts: Option<String>,
    #[serde(default, rename = "lastPr")]
    last_pr: Option<String>,
    #[serde(default, rename = "bidPr")]
    bid_pr: Option<String>,
    #[serde(default, rename = "bidSz")]
    bid_sz: Option<String>,
    #[serde(default, rename = "askPr")]
    ask_pr: Option<String>,
    #[serde(default, rename = "askSz")]
    ask_sz: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default, rename = "high24h")]
    high24h: Option<String>,
    #[serde(default, rename = "low24h")]
    low24h: Option<String>,
    #[serde(default)]
    change: Option<String>,
    #[serde(default, rename = "changePercentage")]
    change_pct: Option<String>,
    #[serde(default, rename = "baseVol")]
    base_vol: Option<String>,
    #[serde(default, rename = "quoteVol")]
    quote_vol: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoinCatchOrderBookData {
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CoinCatchTradeData {
    #[serde(default, rename = "tradeId")]
    trade_id: Option<String>,
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoinCatchCandleData {
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    vol: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CoinCatchOrderData {
    #[serde(default, rename = "instId")]
    inst_id: Option<String>,
    #[serde(default, rename = "orderId")]
    order_id: Option<String>,
    #[serde(default, rename = "clientOrderId")]
    client_order_id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "orderType")]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "fillSz")]
    fill_sz: Option<String>,
    #[serde(default, rename = "fillPrice")]
    fill_price: Option<String>,
    #[serde(default, rename = "fillNotional")]
    fill_notional: Option<String>,
    #[serde(default, rename = "cTime")]
    c_time: Option<String>,
    #[serde(default, rename = "uTime")]
    u_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoinCatchBalanceData {
    #[serde(default)]
    coin: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    frozen: Option<String>,
    #[serde(default)]
    lock: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_client() {
        let ws = CoinCatchWs::new();
        assert!(ws.ws_client.is_none());
        assert!(ws.api_key.is_none());
    }

    #[test]
    fn test_create_with_credentials() {
        let ws = CoinCatchWs::with_credentials("key", "secret", "pass");
        assert_eq!(ws.get_api_key(), Some("key"));
    }

    #[test]
    fn test_format_symbol() {
        assert_eq!(CoinCatchWs::format_symbol("BTC/USDT"), "BTCUSDT_SPBL");
        assert_eq!(CoinCatchWs::format_symbol("ETH/BTC"), "ETHBTC_SPBL");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(CoinCatchWs::to_unified_symbol("BTCUSDT_SPBL"), "BTC/USDT");
        assert_eq!(CoinCatchWs::to_unified_symbol("ETHBTC_SPBL"), "ETH/BTC");
        assert_eq!(CoinCatchWs::to_unified_symbol("BTCUSDC_SPBL"), "BTC/USDC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(CoinCatchWs::format_interval(Timeframe::Minute1), "1min");
        assert_eq!(CoinCatchWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(CoinCatchWs::format_interval(Timeframe::Day1), "1day");
    }

    #[test]
    fn test_parse_ticker() {
        let data = CoinCatchTickerData {
            inst_id: "BTCUSDT_SPBL".to_string(),
            ts: Some("1704067200000".to_string()),
            last_pr: Some("42000.00".to_string()),
            bid_pr: Some("41999.00".to_string()),
            bid_sz: Some("1.5".to_string()),
            ask_pr: Some("42001.00".to_string()),
            ask_sz: Some("2.0".to_string()),
            open: Some("41800.00".to_string()),
            high24h: Some("42500.00".to_string()),
            low24h: Some("41500.00".to_string()),
            change: Some("200.00".to_string()),
            change_pct: Some("0.48".to_string()),
            base_vol: Some("1000.00".to_string()),
            quote_vol: Some("42000000.00".to_string()),
        };

        let event = CoinCatchWs::parse_ticker(&data);
        assert_eq!(event.symbol, "BTC/USDT");
        assert_eq!(event.ticker.last, Some(Decimal::from_str("42000.00").unwrap()));
        assert_eq!(event.ticker.bid, Some(Decimal::from_str("41999.00").unwrap()));
        assert_eq!(event.ticker.ask, Some(Decimal::from_str("42001.00").unwrap()));
    }

    #[test]
    fn test_parse_order_book() {
        let data = CoinCatchOrderBookData {
            ts: Some("1704067200000".to_string()),
            bids: vec![
                vec!["42000.00".to_string(), "1.5".to_string()],
                vec!["41999.00".to_string(), "2.0".to_string()],
            ],
            asks: vec![
                vec!["42001.00".to_string(), "1.0".to_string()],
                vec!["42002.00".to_string(), "3.0".to_string()],
            ],
        };

        let event = CoinCatchWs::parse_order_book(&data, "BTC/USDT", true);
        assert_eq!(event.symbol, "BTC/USDT");
        assert!(event.is_snapshot);
        assert_eq!(event.order_book.bids.len(), 2);
        assert_eq!(event.order_book.asks.len(), 2);
        assert_eq!(event.order_book.bids[0].price, Decimal::from_str("42000.00").unwrap());
    }

    #[test]
    fn test_parse_trades() {
        let data = vec![
            CoinCatchTradeData {
                trade_id: Some("123".to_string()),
                ts: Some("1704067200000".to_string()),
                side: Some("buy".to_string()),
                price: Some("42000.00".to_string()),
                size: Some("0.1".to_string()),
            },
        ];

        let event = CoinCatchWs::parse_trades(&data, "BTC/USDT");
        assert_eq!(event.symbol, "BTC/USDT");
        assert_eq!(event.trades.len(), 1);
        assert_eq!(event.trades[0].id, "123");
        assert_eq!(event.trades[0].price, Decimal::from_str("42000.00").unwrap());
    }

    #[test]
    fn test_clone() {
        let ws = CoinCatchWs::with_credentials("key", "secret", "pass");
        let cloned = ws.clone();
        assert_eq!(cloned.api_key, Some("key".to_string()));
        assert!(cloned.ws_client.is_none());
    }

    #[test]
    fn test_default() {
        let ws = CoinCatchWs::default();
        assert!(ws.api_key.is_none());
    }

    #[test]
    fn test_generate_auth_signature() {
        let sig = CoinCatchWs::generate_auth_signature("secret123", "1704067200");
        assert!(!sig.is_empty());
    }
}
