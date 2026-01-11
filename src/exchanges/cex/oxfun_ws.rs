//! OX.FUN WebSocket Implementation
//!
//! OX.FUN real-time data streaming
//! API Documentation: <https://docs.ox.fun/>
//!
//! # Public Channels
//! - ticker - Market ticker updates
//! - depth - Order book updates
//! - trade - Trade updates
//! - candle - OHLCV candle updates
//!
//! # Private Channels (requires authentication)
//! - order - Order updates
//! - balance - Account balance updates
//! - position - Position updates

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
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Ticker,
    Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsOrderBookEvent, WsOrderEvent,
    WsTickerEvent, WsTradeEvent, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

const WS_PUBLIC_URL: &str = "wss://api.ox.fun/v2/websocket";
const WS_PRIVATE_URL: &str = "wss://api.ox.fun/v2/websocket";

/// OX.FUN WebSocket client
pub struct OxfunWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl OxfunWs {
    /// Create new OX.FUN WebSocket client
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: None,
            api_secret: None,
        }
    }

    /// Set API credentials for private channels
    pub fn with_credentials(mut self, api_key: &str, api_secret: &str) -> Self {
        self.api_key = Some(api_key.to_string());
        self.api_secret = Some(api_secret.to_string());
        self
    }

    /// Convert unified symbol to exchange format (BTC/USD:USD -> BTC-USD-SWAP-LIN)
    fn format_symbol(symbol: &str) -> String {
        // Handle perpetual futures
        if symbol.contains(":") {
            let parts: Vec<&str> = symbol.split(':').collect();
            let base_quote: Vec<&str> = parts[0].split('/').collect();
            if base_quote.len() >= 2 {
                return format!("{}-{}-SWAP-LIN", base_quote[0], base_quote[1]);
            }
        }
        // Handle spot
        symbol.replace("/", "-")
    }

    /// Convert exchange format to unified symbol
    fn to_unified_symbol(market_id: &str) -> String {
        let parts: Vec<&str> = market_id.split('-').collect();
        if parts.len() >= 2 {
            if parts.len() >= 3 && parts[2] == "SWAP" {
                format!("{}/{}:{}", parts[0], parts[1], parts[1])
            } else {
                format!("{}/{}", parts[0], parts[1])
            }
        } else {
            market_id.to_string()
        }
    }

    /// Format timeframe to exchange interval (seconds)
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "60s",
            Timeframe::Minute5 => "300s",
            Timeframe::Minute15 => "900s",
            Timeframe::Minute30 => "1800s",
            Timeframe::Hour1 => "3600s",
            Timeframe::Hour2 => "7200s",
            Timeframe::Hour4 => "14400s",
            Timeframe::Hour6 => "21600s",
            Timeframe::Hour12 => "43200s",
            Timeframe::Day1 => "86400s",
            _ => "3600s",
        }
    }

    /// Generate authentication signature
    fn generate_auth_signature(secret: &str, timestamp: &str, nonce: &str) -> String {
        // OxFun WebSocket auth: timestamp\nnonce\nGET\napi.ox.fun\n/v2/websocket\n
        let host = "api.ox.fun";
        let path = "/v2/websocket";
        let msg = format!("{timestamp}\n{nonce}\nGET\n{host}\n{path}\n");

        if let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) {
            mac.update(msg.as_bytes());
            let result = mac.finalize();
            return BASE64.encode(result.into_bytes());
        }
        String::new()
    }

    /// Parse ticker data
    fn parse_ticker(data: &OxfunWsTicker, symbol: &str) -> Ticker {
        Ticker {
            symbol: symbol.to_string(),
            timestamp: data.timestamp,
            datetime: data.timestamp.map(|ts| {
                chrono::DateTime::<Utc>::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            high: data
                .high_24h
                .as_ref()
                .and_then(|h| Decimal::from_str(h).ok()),
            low: data
                .low_24h
                .as_ref()
                .and_then(|l| Decimal::from_str(l).ok()),
            bid: data
                .best_bid
                .as_ref()
                .and_then(|b| Decimal::from_str(b).ok()),
            bid_volume: data
                .best_bid_size
                .as_ref()
                .and_then(|b| Decimal::from_str(b).ok()),
            ask: data
                .best_ask
                .as_ref()
                .and_then(|a| Decimal::from_str(a).ok()),
            ask_volume: data
                .best_ask_size
                .as_ref()
                .and_then(|a| Decimal::from_str(a).ok()),
            vwap: None,
            open: data
                .open_24h
                .as_ref()
                .and_then(|o| Decimal::from_str(o).ok()),
            close: data
                .last_price
                .as_ref()
                .and_then(|c| Decimal::from_str(c).ok()),
            last: data
                .last_price
                .as_ref()
                .and_then(|l| Decimal::from_str(l).ok()),
            previous_close: None,
            change: None,
            percentage: data
                .price_change_24h
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            average: None,
            base_volume: data
                .base_volume_24h
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: data
                .volume_24h
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            index_price: data
                .index_price
                .as_ref()
                .and_then(|i| Decimal::from_str(i).ok()),
            mark_price: data
                .mark_price
                .as_ref()
                .and_then(|m| Decimal::from_str(m).ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order book data
    fn parse_order_book(data: &OxfunWsOrderBook, symbol: &str) -> OrderBook {
        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|b| {
                if b.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&b[0]).ok()?,
                        amount: Decimal::from_str(&b[1]).ok()?,
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
                        price: Decimal::from_str(&a[0]).ok()?,
                        amount: Decimal::from_str(&a[1]).ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        OrderBook {
            symbol: symbol.to_string(),
            timestamp: data.timestamp,
            datetime: data.timestamp.map(|ts| {
                chrono::DateTime::<Utc>::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            nonce: None,
            bids,
            asks,
            checksum: None,
        }
    }

    /// Parse trade data
    fn parse_trade(data: &OxfunWsTrade, symbol: &str) -> Trade {
        let price = data
            .price
            .as_ref()
            .and_then(|p| Decimal::from_str(p).ok())
            .unwrap_or_default();
        let amount = data
            .quantity
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok())
            .unwrap_or_default();

        Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: None,
            timestamp: data.timestamp,
            datetime: data.timestamp.map(|ts| {
                chrono::DateTime::<Utc>::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.side.as_ref().map(|s| s.to_lowercase()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse OHLCV data
    fn parse_ohlcv(data: &OxfunWsCandle) -> Option<OHLCV> {
        Some(OHLCV {
            timestamp: data.timestamp?,
            open: Decimal::from_str(data.open.as_ref()?).ok()?,
            high: Decimal::from_str(data.high.as_ref()?).ok()?,
            low: Decimal::from_str(data.low.as_ref()?).ok()?,
            close: Decimal::from_str(data.close.as_ref()?).ok()?,
            volume: Decimal::from_str(data.volume.as_ref()?).ok()?,
        })
    }

    /// Parse order data
    fn parse_order(data: &OxfunWsOrder, symbol: &str) -> Order {
        let status = match data.status.as_deref() {
            Some("OPEN") | Some("PARTIAL_FILL") => OrderStatus::Open,
            Some("FILLED") | Some("COMPLETED") => OrderStatus::Closed,
            Some("CANCELED") | Some("CANCELLED") => OrderStatus::Canceled,
            Some("REJECTED") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            Some("STOP") => OrderType::StopLoss,
            Some("STOP_LIMIT") => OrderType::StopLossLimit,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let amount = data
            .quantity
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok())
            .unwrap_or_default();
        let filled = data
            .filled_quantity
            .as_ref()
            .and_then(|f| Decimal::from_str(f).ok())
            .unwrap_or_default();

        Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: data.timestamp,
            datetime: data.timestamp.map(|ts| {
                chrono::DateTime::<Utc>::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: data
                .avg_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            amount,
            filled,
            remaining: Some(amount - filled),
            stop_price: data
                .stop_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance data
    fn parse_balance(data: &OxfunWsBalance) -> Option<(String, Balance)> {
        let currency = data.currency.clone()?;
        let total = data.total.as_ref().and_then(|t| Decimal::from_str(t).ok());
        let free = data
            .available
            .as_ref()
            .and_then(|a| Decimal::from_str(a).ok());
        let used = data
            .reserved
            .as_ref()
            .and_then(|r| Decimal::from_str(r).ok());

        Some((
            currency,
            Balance {
                free,
                used,
                total,
                debt: None,
            },
        ))
    }

    /// Process WebSocket message
    fn process_message(msg: &str, event_tx: &mpsc::UnboundedSender<WsMessage>) -> CcxtResult<()> {
        let json: serde_json::Value = serde_json::from_str(msg)?;

        // Check for table/channel type
        let table = json.get("table").and_then(|t| t.as_str());
        let data = json.get("data");

        match table {
            Some("ticker") => {
                if let Some(data_arr) = data.and_then(|d| d.as_array()) {
                    for item in data_arr {
                        if let Ok(ticker_data) =
                            serde_json::from_value::<OxfunWsTicker>(item.clone())
                        {
                            let symbol = ticker_data
                                .market_code
                                .as_ref()
                                .map(|m| Self::to_unified_symbol(m))
                                .unwrap_or_default();
                            let ticker = Self::parse_ticker(&ticker_data, &symbol);
                            let _ =
                                event_tx.send(WsMessage::Ticker(WsTickerEvent { symbol, ticker }));
                        }
                    }
                }
            },
            Some("depth") | Some("depthL2") => {
                if let Some(data_arr) = data.and_then(|d| d.as_array()) {
                    for item in data_arr {
                        if let Ok(depth_data) =
                            serde_json::from_value::<OxfunWsOrderBook>(item.clone())
                        {
                            let symbol = depth_data
                                .market_code
                                .as_ref()
                                .map(|m| Self::to_unified_symbol(m))
                                .unwrap_or_default();
                            let order_book = Self::parse_order_book(&depth_data, &symbol);
                            let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                                symbol,
                                order_book,
                                is_snapshot: true,
                            }));
                        }
                    }
                }
            },
            Some("trade") => {
                if let Some(data_arr) = data.and_then(|d| d.as_array()) {
                    let mut trades_by_symbol: HashMap<String, Vec<Trade>> = HashMap::new();
                    for item in data_arr {
                        if let Ok(trade_data) = serde_json::from_value::<OxfunWsTrade>(item.clone())
                        {
                            let symbol = trade_data
                                .market_code
                                .as_ref()
                                .map(|m| Self::to_unified_symbol(m))
                                .unwrap_or_default();
                            let trade = Self::parse_trade(&trade_data, &symbol);
                            trades_by_symbol.entry(symbol).or_default().push(trade);
                        }
                    }
                    for (symbol, trades) in trades_by_symbol {
                        if !trades.is_empty() {
                            let _ =
                                event_tx.send(WsMessage::Trade(WsTradeEvent { symbol, trades }));
                        }
                    }
                }
            },
            Some("candle") | Some("candles60s") | Some("candles300s") | Some("candles3600s") => {
                if let Some(data_arr) = data.and_then(|d| d.as_array()) {
                    for item in data_arr {
                        if let Ok(candle_data) =
                            serde_json::from_value::<OxfunWsCandle>(item.clone())
                        {
                            if let Some(ohlcv) = Self::parse_ohlcv(&candle_data) {
                                let symbol = candle_data
                                    .market_code
                                    .as_ref()
                                    .map(|m| Self::to_unified_symbol(m))
                                    .unwrap_or_default();
                                let _ =
                                    event_tx.send(WsMessage::Ohlcv(crate::types::WsOhlcvEvent {
                                        symbol,
                                        timeframe: Timeframe::Hour1,
                                        ohlcv,
                                    }));
                            }
                        }
                    }
                }
            },
            Some("order") => {
                if let Some(data_arr) = data.and_then(|d| d.as_array()) {
                    for item in data_arr {
                        if let Ok(order_data) = serde_json::from_value::<OxfunWsOrder>(item.clone())
                        {
                            let symbol = order_data
                                .market_code
                                .as_ref()
                                .map(|m| Self::to_unified_symbol(m))
                                .unwrap_or_default();
                            let order = Self::parse_order(&order_data, &symbol);
                            let _ = event_tx.send(WsMessage::Order(WsOrderEvent { order }));
                        }
                    }
                }
            },
            Some("balance") => {
                if let Some(data_arr) = data.and_then(|d| d.as_array()) {
                    let mut balances = Balances::new();
                    for item in data_arr {
                        if let Ok(balance_data) =
                            serde_json::from_value::<OxfunWsBalance>(item.clone())
                        {
                            if let Some((currency, balance)) = Self::parse_balance(&balance_data) {
                                balances.add(&currency, balance);
                            }
                        }
                    }
                    let _ = event_tx.send(WsMessage::Balance(WsBalanceEvent { balances }));
                }
            },
            _ => {},
        }

        Ok(())
    }

    /// Subscribe to a stream
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        market_id: &str,
        requires_auth: bool,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let ws_url = if requires_auth {
            WS_PRIVATE_URL
        } else {
            WS_PUBLIC_URL
        };

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // Authenticate if required
        if requires_auth {
            if let (Some(api_key), Some(api_secret)) = (&self.api_key, &self.api_secret) {
                let timestamp = Utc::now().timestamp_millis().to_string();
                let nonce = format!("{}", Utc::now().timestamp_nanos_opt().unwrap_or(0));
                let signature = Self::generate_auth_signature(api_secret, &timestamp, &nonce);

                let auth_msg = serde_json::json!({
                    "op": "login",
                    "tag": "auth",
                    "data": {
                        "apiKey": api_key,
                        "timestamp": timestamp,
                        "nonce": nonce,
                        "signature": signature
                    }
                });

                ws_client.send(&auth_msg.to_string())?;
            }
        }

        // Build subscription message
        let args = if market_id.is_empty() {
            vec![channel.to_string()]
        } else {
            vec![format!("{}:{}", channel, market_id)]
        };

        let sub_msg = serde_json::json!({
            "op": "subscribe",
            "tag": format!("sub_{}", Utc::now().timestamp_millis()),
            "args": args
        });

        ws_client.send(&sub_msg.to_string())?;

        // Spawn message handler
        let subscriptions = self.subscriptions.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Message(msg) => {
                        let _ = Self::process_message(&msg, &event_tx);
                    },
                    WsEvent::Connected => {
                        // Connection established
                    },
                    WsEvent::Disconnected => {
                        break;
                    },
                    WsEvent::Error(e) => {
                        eprintln!("OxFun WebSocket error: {e}");
                    },
                    WsEvent::Ping | WsEvent::Pong => {
                        // Heartbeat handled
                    },
                    _ => {
                        // Handle new events (Reconnecting, Reconnected, HealthOk, HealthWarning)

                        // Heartbeat
                    },
                }
            }

            let mut subs = subscriptions.write().await;
            subs.clear();
        });

        self.ws_client = Some(ws_client);
        Ok(event_rx)
    }

    /// Subscribe to order updates (private channel)
    pub async fn watch_orders(
        &mut self,
        symbol: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let market_id = Self::format_symbol(symbol);
        self.subscribe_stream("order", &market_id, true).await
    }

    /// Subscribe to balance updates (private channel)
    pub async fn watch_balance(&mut self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_stream("balance", "", true).await
    }
}

impl Default for OxfunWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for OxfunWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
        }
    }
}

#[async_trait]
impl WsExchange for OxfunWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        ws.subscribe_stream("ticker", &market_id, false).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        ws.subscribe_stream("depth", &market_id, false).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        ws.subscribe_stream("trade", &market_id, false).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        let channel = format!("candles{}", Self::format_interval(timeframe));
        ws.subscribe_stream(&channel, &market_id, false).await
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
                ..Default::default()
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

#[derive(Debug, Deserialize, Serialize)]
struct OxfunWsTicker {
    #[serde(rename = "marketCode")]
    #[serde(default)]
    market_code: Option<String>,
    #[serde(rename = "lastTradedPrice")]
    #[serde(default)]
    last_price: Option<String>,
    #[serde(rename = "markPrice")]
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(rename = "indexPrice")]
    #[serde(default)]
    index_price: Option<String>,
    #[serde(rename = "high24h")]
    #[serde(default)]
    high_24h: Option<String>,
    #[serde(rename = "low24h")]
    #[serde(default)]
    low_24h: Option<String>,
    #[serde(rename = "open24h")]
    #[serde(default)]
    open_24h: Option<String>,
    #[serde(rename = "volume24h")]
    #[serde(default)]
    volume_24h: Option<String>,
    #[serde(rename = "currencyVolume24h")]
    #[serde(default)]
    base_volume_24h: Option<String>,
    #[serde(rename = "priceChange24h")]
    #[serde(default)]
    price_change_24h: Option<String>,
    #[serde(rename = "bestBid")]
    #[serde(default)]
    best_bid: Option<String>,
    #[serde(rename = "bestBidSize")]
    #[serde(default)]
    best_bid_size: Option<String>,
    #[serde(rename = "bestAsk")]
    #[serde(default)]
    best_ask: Option<String>,
    #[serde(rename = "bestAskSize")]
    #[serde(default)]
    best_ask_size: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunWsOrderBook {
    #[serde(rename = "marketCode")]
    #[serde(default)]
    market_code: Option<String>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunWsTrade {
    #[serde(rename = "marketCode")]
    #[serde(default)]
    market_code: Option<String>,
    #[serde(rename = "tradeId")]
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunWsCandle {
    #[serde(rename = "marketCode")]
    #[serde(default)]
    market_code: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    volume: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunWsOrder {
    #[serde(rename = "orderId")]
    #[serde(default)]
    order_id: Option<String>,
    #[serde(rename = "clientOrderId")]
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(rename = "marketCode")]
    #[serde(default)]
    market_code: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(rename = "orderType")]
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(rename = "stopPrice")]
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(rename = "filledQuantity")]
    #[serde(default)]
    filled_quantity: Option<String>,
    #[serde(rename = "avgPrice")]
    #[serde(default)]
    avg_price: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunWsBalance {
    #[serde(rename = "instrumentId")]
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    total: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    reserved: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        let ws = OxfunWs::default();
        assert!(ws.api_key.is_none());
        assert!(ws.api_secret.is_none());
    }

    #[test]
    fn test_clone() {
        let ws = OxfunWs::new().with_credentials("api_key", "secret");
        let cloned = ws.clone();
        assert_eq!(cloned.api_key, ws.api_key);
        assert_eq!(cloned.api_secret, ws.api_secret);
    }

    #[test]
    fn test_create_with_credentials() {
        let ws = OxfunWs::new().with_credentials("test_key", "test_secret");
        assert_eq!(ws.api_key, Some("test_key".to_string()));
        assert_eq!(ws.api_secret, Some("test_secret".to_string()));
    }

    #[test]
    fn test_format_symbol() {
        assert_eq!(OxfunWs::format_symbol("BTC/USD"), "BTC-USD");
        assert_eq!(OxfunWs::format_symbol("BTC/USD:USD"), "BTC-USD-SWAP-LIN");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(OxfunWs::to_unified_symbol("BTC-USD"), "BTC/USD");
        assert_eq!(
            OxfunWs::to_unified_symbol("BTC-USD-SWAP-LIN"),
            "BTC/USD:USD"
        );
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(OxfunWs::format_interval(Timeframe::Minute1), "60s");
        assert_eq!(OxfunWs::format_interval(Timeframe::Hour1), "3600s");
        assert_eq!(OxfunWs::format_interval(Timeframe::Day1), "86400s");
    }

    #[test]
    fn test_generate_auth_signature() {
        let signature = OxfunWs::generate_auth_signature("test_secret", "1234567890", "nonce123");
        assert!(!signature.is_empty());
    }

    #[test]
    fn test_parse_ticker() {
        let data = OxfunWsTicker {
            market_code: Some("BTC-USD-SWAP-LIN".to_string()),
            last_price: Some("50000.0".to_string()),
            mark_price: Some("50001.0".to_string()),
            index_price: Some("49999.0".to_string()),
            high_24h: Some("51000.0".to_string()),
            low_24h: Some("49000.0".to_string()),
            open_24h: Some("49500.0".to_string()),
            volume_24h: Some("1000000.0".to_string()),
            base_volume_24h: Some("20.0".to_string()),
            price_change_24h: Some("1.0".to_string()),
            best_bid: Some("49999.0".to_string()),
            best_bid_size: Some("10.0".to_string()),
            best_ask: Some("50001.0".to_string()),
            best_ask_size: Some("10.0".to_string()),
            timestamp: Some(1234567890000),
        };

        let ticker = OxfunWs::parse_ticker(&data, "BTC/USD:USD");
        assert_eq!(ticker.symbol, "BTC/USD:USD");
        assert_eq!(ticker.last, Some(Decimal::from_str("50000.0").unwrap()));
    }

    #[test]
    fn test_parse_order_book() {
        let data = OxfunWsOrderBook {
            market_code: Some("BTC-USD-SWAP-LIN".to_string()),
            bids: vec![vec!["49999.0".to_string(), "10.0".to_string()]],
            asks: vec![vec!["50001.0".to_string(), "5.0".to_string()]],
            timestamp: Some(1234567890000),
        };

        let order_book = OxfunWs::parse_order_book(&data, "BTC/USD:USD");
        assert_eq!(order_book.symbol, "BTC/USD:USD");
        assert_eq!(order_book.bids.len(), 1);
        assert_eq!(order_book.asks.len(), 1);
    }

    #[test]
    fn test_parse_trade() {
        let data = OxfunWsTrade {
            market_code: Some("BTC-USD-SWAP-LIN".to_string()),
            trade_id: Some("12345".to_string()),
            price: Some("50000.0".to_string()),
            quantity: Some("1.0".to_string()),
            side: Some("BUY".to_string()),
            timestamp: Some(1234567890000),
        };

        let trade = OxfunWs::parse_trade(&data, "BTC/USD:USD");
        assert_eq!(trade.symbol, "BTC/USD:USD");
        assert_eq!(trade.id, "12345");
    }

    #[test]
    fn test_parse_ohlcv() {
        let data = OxfunWsCandle {
            market_code: Some("BTC-USD-SWAP-LIN".to_string()),
            timestamp: Some(1234567890000),
            open: Some("49500.0".to_string()),
            high: Some("51000.0".to_string()),
            low: Some("49000.0".to_string()),
            close: Some("50000.0".to_string()),
            volume: Some("1000.0".to_string()),
        };

        let ohlcv = OxfunWs::parse_ohlcv(&data);
        assert!(ohlcv.is_some());
        let ohlcv = ohlcv.unwrap();
        assert_eq!(ohlcv.timestamp, 1234567890000);
    }

    #[test]
    fn test_parse_balance() {
        let data = OxfunWsBalance {
            currency: Some("USD".to_string()),
            total: Some("10000.0".to_string()),
            available: Some("9000.0".to_string()),
            reserved: Some("1000.0".to_string()),
        };

        let result = OxfunWs::parse_balance(&data);
        assert!(result.is_some());
        let (currency, balance) = result.unwrap();
        assert_eq!(currency, "USD");
        assert_eq!(balance.total, Some(Decimal::from_str("10000.0").unwrap()));
    }
}
