//! Hashkey WebSocket Implementation
//!
//! WebSocket client for Hashkey Global exchange.
//! Supports: watchTicker, watchTrades, watchOrderBook, watchOHLCV

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

use crate::client::ExchangeConfig;
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage,
    WsOhlcvEvent, WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://stream-glb.hashkey.com/quote/ws/v1";
const WS_PRIVATE_URL: &str = "wss://stream-glb.hashkey.com/api/v1/ws";

type WsClient = WebSocketStream<MaybeTlsStream<TcpStream>>;
type HmacSha256 = Hmac<Sha256>;

/// Order update data from private WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
struct HashkeyOrderUpdateData {
    #[serde(rename = "e")]
    event_type: Option<String>,
    #[serde(rename = "E")]
    event_time: Option<i64>,
    #[serde(rename = "s")]
    symbol: Option<String>,
    #[serde(rename = "c")]
    client_order_id: Option<String>,
    #[serde(rename = "S")]
    side: Option<String>,
    #[serde(rename = "o")]
    order_type: Option<String>,
    #[serde(rename = "f")]
    time_in_force: Option<String>,
    #[serde(rename = "q")]
    quantity: Option<String>,
    #[serde(rename = "p")]
    price: Option<String>,
    #[serde(rename = "X")]
    order_status: Option<String>,
    #[serde(rename = "i")]
    order_id: Option<i64>,
    #[serde(rename = "l")]
    last_filled_qty: Option<String>,
    #[serde(rename = "z")]
    cumulative_filled_qty: Option<String>,
    #[serde(rename = "L")]
    last_filled_price: Option<String>,
    #[serde(rename = "T")]
    trade_time: Option<i64>,
    #[serde(rename = "t")]
    trade_id: Option<i64>,
    #[serde(rename = "n")]
    commission: Option<String>,
    #[serde(rename = "N")]
    commission_asset: Option<String>,
}

/// Balance update data from private WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
struct HashkeyBalanceUpdateData {
    #[serde(rename = "a")]
    asset: Option<String>,
    #[serde(rename = "f")]
    free: Option<String>,
    #[serde(rename = "l")]
    locked: Option<String>,
}

/// Hashkey WebSocket client
#[allow(dead_code)]
pub struct HashkeyWs {
    config: ExchangeConfig,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    request_id: AtomicI64,
    order_books: Arc<RwLock<HashMap<String, OrderBook>>>,
}

/// WebSocket subscribe/unsubscribe message
#[derive(Debug, Serialize)]
struct WsSubscribeMessage {
    symbol: String,
    topic: String,
    event: String,
}

/// WebSocket ticker response
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct HashkeyWsTicker {
    pub t: Option<i64>,     // timestamp
    pub s: Option<String>,  // symbol
    pub c: Option<String>,  // close price
    pub h: Option<String>,  // high
    pub l: Option<String>,  // low
    pub o: Option<String>,  // open
    pub v: Option<String>,  // volume
    pub qv: Option<String>, // quote volume
    pub m: Option<String>,  // change percent
}

/// WebSocket trade response
#[derive(Debug, Deserialize)]
struct HashkeyWsTrade {
    pub v: Option<String>, // trade id
    pub t: Option<i64>,    // timestamp
    pub p: Option<String>, // price
    pub q: Option<String>, // quantity
    pub m: Option<bool>,   // is maker
}

/// WebSocket order book data
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct HashkeyWsOrderBookData {
    pub t: Option<i64>,              // timestamp
    pub s: Option<String>,           // symbol
    pub b: Option<Vec<Vec<String>>>, // bids [[price, amount], ...]
    pub a: Option<Vec<Vec<String>>>, // asks [[price, amount], ...]
}

/// WebSocket OHLCV data
#[derive(Debug, Deserialize)]
struct HashkeyWsOhlcv {
    pub t: Option<i64>,    // timestamp
    pub o: Option<String>, // open
    pub h: Option<String>, // high
    pub l: Option<String>, // low
    pub c: Option<String>, // close
    pub v: Option<String>, // volume
}

/// WebSocket message wrapper
#[derive(Debug, Deserialize)]
struct HashkeyWsMessage {
    pub symbol: Option<String>,
    #[serde(rename = "symbolName")]
    pub symbol_name: Option<String>,
    pub topic: Option<String>,
    pub params: Option<HashMap<String, String>>,
    pub data: Option<serde_json::Value>,
    pub f: Option<bool>, // is first/snapshot
    #[serde(rename = "sendTime")]
    pub send_time: Option<i64>,
}

impl HashkeyWs {
    /// Create a new Hashkey WebSocket client
    pub fn new(config: ExchangeConfig) -> Self {
        Self {
            config,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: AtomicI64::new(1),
            order_books: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        let config = ExchangeConfig::default()
            .with_api_key(api_key)
            .with_api_secret(api_secret);
        Self::new(config)
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.config = ExchangeConfig::default()
            .with_api_key(api_key)
            .with_api_secret(api_secret);
    }

    /// Get next request ID
    fn next_request_id(&self) -> i64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Convert CCXT symbol to exchange symbol (BTC/USDT -> BTCUSDT)
    fn convert_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// Convert exchange symbol to CCXT symbol (BTCUSDT -> BTC/USDT)
    fn parse_symbol(&self, market_id: &str) -> String {
        // Common quote currencies
        let quote_currencies = ["USDT", "USD", "BTC", "ETH", "EUR"];

        for quote in quote_currencies.iter() {
            if let Some(base) = market_id.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }
        market_id.to_string()
    }

    /// Convert timeframe to exchange interval
    fn convert_timeframe(&self, timeframe: Timeframe) -> String {
        match timeframe {
            Timeframe::Minute1 => "1m".to_string(),
            Timeframe::Minute3 => "3m".to_string(),
            Timeframe::Minute5 => "5m".to_string(),
            Timeframe::Minute15 => "15m".to_string(),
            Timeframe::Minute30 => "30m".to_string(),
            Timeframe::Hour1 => "1h".to_string(),
            Timeframe::Hour2 => "2h".to_string(),
            Timeframe::Hour4 => "4h".to_string(),
            Timeframe::Hour6 => "6h".to_string(),
            Timeframe::Hour8 => "8h".to_string(),
            Timeframe::Hour12 => "12h".to_string(),
            Timeframe::Day1 => "1d".to_string(),
            Timeframe::Week1 => "1w".to_string(),
            Timeframe::Month1 => "1M".to_string(),
            _ => "1m".to_string(),
        }
    }

    /// Connect to WebSocket
    async fn connect(&self) -> CcxtResult<WsClient> {
        let (ws_stream, _) = connect_async(WS_PUBLIC_URL).await.map_err(|e| {
            crate::errors::CcxtError::NetworkError {
                url: WS_PUBLIC_URL.to_string(),
                message: e.to_string(),
            }
        })?;
        Ok(ws_stream)
    }

    /// Connect to private WebSocket
    async fn connect_private(&self) -> CcxtResult<WsClient> {
        let (ws_stream, _) =
            connect_async(WS_PRIVATE_URL)
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_PRIVATE_URL.to_string(),
                    message: e.to_string(),
                })?;
        Ok(ws_stream)
    }

    /// Sign a message using HMAC-SHA256
    fn sign(&self, message: &str) -> CcxtResult<String> {
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required for signing".into(),
            })?;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| {
            CcxtError::AuthenticationError {
                message: format!("HMAC error: {e}"),
            }
        })?;
        mac.update(message.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Subscribe to private streams
    async fn subscribe_private_stream(&self, ws: &mut WsClient, channel: &str) -> CcxtResult<()> {
        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required for private channels".into(),
            })?;

        let timestamp = Utc::now().timestamp_millis();
        let sign_payload = format!("{api_key}timestamp={timestamp}");
        let signature = self.sign(&sign_payload)?;

        // Authentication message
        let auth_msg = serde_json::json!({
            "id": self.next_request_id(),
            "method": "LOGIN",
            "params": {
                "apiKey": api_key,
                "timestamp": timestamp,
                "signature": signature
            }
        });

        let auth_str = serde_json::to_string(&auth_msg).map_err(|e| CcxtError::ParseError {
            data_type: "json".to_string(),
            message: e.to_string(),
        })?;

        ws.send(Message::Text(auth_str))
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_PRIVATE_URL.to_string(),
                message: e.to_string(),
            })?;

        // Subscribe to channel
        let sub_msg = serde_json::json!({
            "id": self.next_request_id(),
            "method": "SUBSCRIBE",
            "params": [channel]
        });

        let sub_str = serde_json::to_string(&sub_msg).map_err(|e| CcxtError::ParseError {
            data_type: "json".to_string(),
            message: e.to_string(),
        })?;

        ws.send(Message::Text(sub_str))
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_PRIVATE_URL.to_string(),
                message: e.to_string(),
            })?;

        Ok(())
    }

    /// Process private WebSocket message
    fn process_private_message(&self, msg: &str) -> Option<WsMessage> {
        let parsed: serde_json::Value = serde_json::from_str(msg).ok()?;

        // Check event type
        let event_type = parsed.get("e").and_then(|v| v.as_str())?;

        match event_type {
            "executionReport" => {
                // Order update
                if let Ok(order_data) =
                    serde_json::from_value::<HashkeyOrderUpdateData>(parsed.clone())
                {
                    return self.parse_order(&order_data);
                }
            },
            "outboundAccountPosition" | "balanceUpdate" => {
                // Balance update
                if let Some(balances_arr) = parsed.get("B").and_then(|v| v.as_array()) {
                    return self.parse_balance(balances_arr, &parsed);
                }
            },
            _ => {},
        }

        None
    }

    /// Parse order from private message
    fn parse_order(&self, data: &HashkeyOrderUpdateData) -> Option<WsMessage> {
        let symbol = data.symbol.as_ref().map(|s| self.parse_symbol(s))?;
        let order_id = data.order_id.map(|id| id.to_string())?;

        let status = match data.order_status.as_deref() {
            Some("NEW") => OrderStatus::Open,
            Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("CANCELED") => OrderStatus::Canceled,
            Some("REJECTED") => OrderStatus::Rejected,
            Some("EXPIRED") => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let side = match data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            Some("STOP_LOSS") => OrderType::StopLoss,
            Some("STOP_LOSS_LIMIT") => OrderType::StopLossLimit,
            Some("TAKE_PROFIT") => OrderType::TakeProfit,
            Some("TAKE_PROFIT_LIMIT") => OrderType::TakeProfitLimit,
            _ => OrderType::Limit,
        };

        let time_in_force = match data.time_in_force.as_deref() {
            Some("GTC") => Some(TimeInForce::GTC),
            Some("IOC") => Some(TimeInForce::IOC),
            Some("FOK") => Some(TimeInForce::FOK),
            Some("GTD") => Some(TimeInForce::GTC),
            _ => None,
        };

        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let amount = data
            .quantity
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok());
        let filled = data
            .cumulative_filled_qty
            .as_ref()
            .and_then(|f| Decimal::from_str(f).ok());
        let remaining = match (amount, filled) {
            (Some(a), Some(f)) => Some(a - f),
            _ => None,
        };
        let cost = match (price, filled) {
            (Some(p), Some(f)) => Some(p * f),
            _ => None,
        };

        let order = Order {
            id: order_id.clone(),
            client_order_id: data.client_order_id.clone(),
            timestamp: data.event_time,
            datetime: data.event_time.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: data.trade_time,
            last_update_timestamp: data.event_time,
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force,
            side,
            price,
            average: data
                .last_filled_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            amount: amount.unwrap_or_default(),
            filled: filled.unwrap_or_default(),
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost,
            reduce_only: None,
            post_only: None,
            trades: vec![],
            fee: None,
            fees: vec![],
            info: serde_json::to_value(data).unwrap_or(serde_json::Value::Null),
        };

        Some(WsMessage::Order(WsOrderEvent { order }))
    }

    /// Parse balance from private message
    fn parse_balance(
        &self,
        balances_arr: &[serde_json::Value],
        parsed: &serde_json::Value,
    ) -> Option<WsMessage> {
        let mut currencies: HashMap<String, Balance> = HashMap::new();

        for bal in balances_arr {
            if let Ok(balance_data) =
                serde_json::from_value::<HashkeyBalanceUpdateData>(bal.clone())
            {
                if let Some(asset) = &balance_data.asset {
                    let free = balance_data
                        .free
                        .as_ref()
                        .and_then(|f| Decimal::from_str(f).ok())
                        .unwrap_or_default();
                    let locked = balance_data
                        .locked
                        .as_ref()
                        .and_then(|l| Decimal::from_str(l).ok())
                        .unwrap_or_default();
                    let total = free + locked;

                    currencies.insert(
                        asset.clone(),
                        Balance {
                            free: Some(free),
                            used: Some(locked),
                            total: Some(total),
                            debt: None,
                        },
                    );
                }
            }
        }

        let timestamp = parsed
            .get("E")
            .and_then(|v| v.as_i64())
            .or_else(|| Some(Utc::now().timestamp_millis()));

        let balances = Balances {
            timestamp,
            datetime: None,
            currencies,
            info: parsed.clone(),
        };

        Some(WsMessage::Balance(WsBalanceEvent { balances }))
    }

    /// Subscribe to a topic
    async fn subscribe(&self, ws: &mut WsClient, symbol: &str, topic: &str) -> CcxtResult<()> {
        let market_id = self.convert_symbol(symbol);

        let msg = WsSubscribeMessage {
            symbol: market_id,
            topic: topic.to_string(),
            event: "sub".to_string(),
        };

        let msg_str =
            serde_json::to_string(&msg).map_err(|e| crate::errors::CcxtError::ParseError {
                data_type: "json".to_string(),
                message: e.to_string(),
            })?;

        ws.send(Message::Text(msg_str)).await.map_err(|e| {
            crate::errors::CcxtError::NetworkError {
                url: WS_PUBLIC_URL.to_string(),
                message: e.to_string(),
            }
        })?;

        Ok(())
    }

    /// Parse ticker from WebSocket message
    fn parse_ticker(&self, data: &HashkeyWsTicker, symbol: &str) -> Ticker {
        let last = data.c.as_ref().and_then(|v| Decimal::from_str(v).ok());
        let high = data.h.as_ref().and_then(|v| Decimal::from_str(v).ok());
        let low = data.l.as_ref().and_then(|v| Decimal::from_str(v).ok());
        let open = data.o.as_ref().and_then(|v| Decimal::from_str(v).ok());
        let base_volume = data.v.as_ref().and_then(|v| Decimal::from_str(v).ok());
        let quote_volume = data.qv.as_ref().and_then(|v| Decimal::from_str(v).ok());
        let percentage = data
            .m
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .map(|v| v * Decimal::from(100));

        let change = if let (Some(l), Some(o)) = (last, open) {
            Some(l - o)
        } else {
            None
        };

        Ticker {
            symbol: symbol.to_string(),
            timestamp: data.t,
            datetime: data.t.map(|t| {
                chrono::DateTime::from_timestamp(t / 1000, ((t % 1000) * 1_000_000) as u32)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            high,
            low,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open,
            close: last,
            last,
            previous_close: None,
            change,
            percentage,
            average: None,
            base_volume,
            quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        }
    }

    /// Parse trade from WebSocket message
    fn parse_trade(&self, data: &HashkeyWsTrade, symbol: &str) -> Trade {
        let side = if data.m.unwrap_or(false) {
            Some("sell".to_string()) // maker = sell
        } else {
            Some("buy".to_string()) // taker = buy
        };

        let price = data
            .p
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();
        let amount = data
            .q
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();
        let cost = price * amount;

        Trade {
            id: data.v.clone().unwrap_or_default(),
            order: None,
            timestamp: data.t,
            datetime: data.t.map(|t| {
                chrono::DateTime::from_timestamp(t / 1000, ((t % 1000) * 1_000_000) as u32)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            symbol: symbol.to_string(),
            trade_type: None,
            side,
            taker_or_maker: if data.m.unwrap_or(false) {
                Some(TakerOrMaker::Maker)
            } else {
                Some(TakerOrMaker::Taker)
            },
            price,
            amount,
            cost: Some(cost),
            fee: None,
            fees: vec![],
            info: serde_json::Value::Null,
        }
    }

    /// Parse order book entry
    fn parse_order_book_entry(&self, entry: &[String]) -> Option<OrderBookEntry> {
        if entry.len() >= 2 {
            let price = Decimal::from_str(&entry[0]).ok()?;
            let amount = Decimal::from_str(&entry[1]).ok()?;
            Some(OrderBookEntry { price, amount })
        } else {
            None
        }
    }

    /// Parse order book from WebSocket message
    fn parse_order_book(&self, data: &HashkeyWsOrderBookData, symbol: &str) -> OrderBook {
        let bids: Vec<OrderBookEntry> = data
            .b
            .as_ref()
            .map(|b| {
                b.iter()
                    .filter_map(|entry| self.parse_order_book_entry(entry))
                    .collect()
            })
            .unwrap_or_default();

        let asks: Vec<OrderBookEntry> = data
            .a
            .as_ref()
            .map(|a| {
                a.iter()
                    .filter_map(|entry| self.parse_order_book_entry(entry))
                    .collect()
            })
            .unwrap_or_default();

        OrderBook {
            symbol: symbol.to_string(),
            timestamp: data.t,
            datetime: data.t.map(|t| {
                chrono::DateTime::from_timestamp(t / 1000, ((t % 1000) * 1_000_000) as u32)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            nonce: None,
            bids,
            asks,
            checksum: None,
        }
    }

    /// Handle incoming WebSocket message
    async fn handle_message(
        &self,
        msg: &str,
        tx: &mpsc::UnboundedSender<WsMessage>,
        subscribed_symbol: &str,
    ) -> CcxtResult<()> {
        // Try to parse as array first (some responses come as arrays)
        if let Ok(arr) = serde_json::from_str::<Vec<HashkeyWsMessage>>(msg) {
            if let Some(first) = arr.first() {
                return self.process_message(first, tx, subscribed_symbol).await;
            }
        }

        // Parse as single message
        if let Ok(message) = serde_json::from_str::<HashkeyWsMessage>(msg) {
            return self.process_message(&message, tx, subscribed_symbol).await;
        }

        Ok(())
    }

    /// Process parsed WebSocket message
    async fn process_message(
        &self,
        message: &HashkeyWsMessage,
        tx: &mpsc::UnboundedSender<WsMessage>,
        subscribed_symbol: &str,
    ) -> CcxtResult<()> {
        let topic = message.topic.as_deref().unwrap_or("");
        let market_id = message.symbol.as_deref().unwrap_or("");
        let symbol = if market_id.is_empty() {
            subscribed_symbol.to_string()
        } else {
            self.parse_symbol(market_id)
        };

        match topic {
            "realtimes" => {
                // Ticker data
                if let Some(data_val) = &message.data {
                    if let Ok(data_arr) =
                        serde_json::from_value::<Vec<HashkeyWsTicker>>(data_val.clone())
                    {
                        if let Some(ticker_data) = data_arr.first() {
                            let ticker = self.parse_ticker(ticker_data, &symbol);
                            let _ = tx.send(WsMessage::Ticker(WsTickerEvent {
                                symbol: symbol.clone(),
                                ticker,
                            }));
                        }
                    }
                }
            },
            "trade" => {
                // Trade data
                if let Some(data_val) = &message.data {
                    if let Ok(data_arr) =
                        serde_json::from_value::<Vec<HashkeyWsTrade>>(data_val.clone())
                    {
                        let trades: Vec<Trade> = data_arr
                            .iter()
                            .map(|t| self.parse_trade(t, &symbol))
                            .collect();
                        if !trades.is_empty() {
                            let _ = tx.send(WsMessage::Trade(WsTradeEvent {
                                symbol: symbol.clone(),
                                trades,
                            }));
                        }
                    }
                }
            },
            "depth" => {
                // Order book data
                if let Some(data_val) = &message.data {
                    if let Ok(data_arr) =
                        serde_json::from_value::<Vec<HashkeyWsOrderBookData>>(data_val.clone())
                    {
                        if let Some(book_data) = data_arr.first() {
                            let order_book = self.parse_order_book(book_data, &symbol);

                            // Update stored order book
                            {
                                let mut books = self.order_books.write().await;
                                books.insert(symbol.clone(), order_book.clone());
                            }

                            let _ = tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                                symbol: symbol.clone(),
                                order_book,
                                is_snapshot: message.f.unwrap_or(true),
                            }));
                        }
                    }
                }
            },
            t if t == "kline" || t.starts_with("kline_") => {
                // OHLCV data
                if let Some(data_val) = &message.data {
                    if let Ok(data_arr) =
                        serde_json::from_value::<Vec<HashkeyWsOhlcv>>(data_val.clone())
                    {
                        if let Some(ohlcv_data) = data_arr.first() {
                            // Get timeframe from params
                            let timeframe_str = message
                                .params
                                .as_ref()
                                .and_then(|p| p.get("klineType"))
                                .map(|s| s.as_str())
                                .unwrap_or("1m");

                            let timeframe = match timeframe_str {
                                "1m" => Timeframe::Minute1,
                                "3m" => Timeframe::Minute3,
                                "5m" => Timeframe::Minute5,
                                "15m" => Timeframe::Minute15,
                                "30m" => Timeframe::Minute30,
                                "1h" => Timeframe::Hour1,
                                "4h" => Timeframe::Hour4,
                                "1d" => Timeframe::Day1,
                                "1w" => Timeframe::Week1,
                                _ => Timeframe::Minute1,
                            };

                            let ohlcv = crate::types::OHLCV {
                                timestamp: ohlcv_data.t.unwrap_or(0),
                                open: ohlcv_data
                                    .o
                                    .as_ref()
                                    .and_then(|v| Decimal::from_str(v).ok())
                                    .unwrap_or_default(),
                                high: ohlcv_data
                                    .h
                                    .as_ref()
                                    .and_then(|v| Decimal::from_str(v).ok())
                                    .unwrap_or_default(),
                                low: ohlcv_data
                                    .l
                                    .as_ref()
                                    .and_then(|v| Decimal::from_str(v).ok())
                                    .unwrap_or_default(),
                                close: ohlcv_data
                                    .c
                                    .as_ref()
                                    .and_then(|v| Decimal::from_str(v).ok())
                                    .unwrap_or_default(),
                                volume: ohlcv_data
                                    .v
                                    .as_ref()
                                    .and_then(|v| Decimal::from_str(v).ok())
                                    .unwrap_or_default(),
                            };

                            let _ = tx.send(WsMessage::Ohlcv(WsOhlcvEvent {
                                symbol: symbol.clone(),
                                timeframe,
                                ohlcv,
                            }));
                        }
                    }
                }
            },
            _ => {},
        }

        Ok(())
    }
}

#[async_trait]
impl WsExchange for HashkeyWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut ws = self.connect().await?;

        // Subscribe to ticker
        self.subscribe(&mut ws, symbol, "realtimes").await?;

        let _ = tx.send(WsMessage::Connected);

        let _order_books = self.order_books.clone();
        let symbol_clone = symbol.to_string();
        let this_parse_symbol = |market_id: &str| -> String {
            let quote_currencies = ["USDT", "USD", "BTC", "ETH", "EUR"];
            for quote in quote_currencies.iter() {
                if let Some(base) = market_id.strip_suffix(quote) {
                    return format!("{base}/{quote}");
                }
            }
            market_id.to_string()
        };

        tokio::spawn(async move {
            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        // Parse and handle message inline
                        if let Ok(message) = serde_json::from_str::<HashkeyWsMessage>(&text) {
                            if message.topic.as_deref() == Some("realtimes") {
                                if let Some(data_val) = &message.data {
                                    if let Ok(data_arr) =
                                        serde_json::from_value::<Vec<HashkeyWsTicker>>(
                                            data_val.clone(),
                                        )
                                    {
                                        if let Some(ticker_data) = data_arr.first() {
                                            let market_id = message.symbol.as_deref().unwrap_or("");
                                            let symbol = if market_id.is_empty() {
                                                symbol_clone.clone()
                                            } else {
                                                this_parse_symbol(market_id)
                                            };

                                            let last = ticker_data
                                                .c
                                                .as_ref()
                                                .and_then(|v| Decimal::from_str(v).ok());
                                            let high = ticker_data
                                                .h
                                                .as_ref()
                                                .and_then(|v| Decimal::from_str(v).ok());
                                            let low = ticker_data
                                                .l
                                                .as_ref()
                                                .and_then(|v| Decimal::from_str(v).ok());
                                            let open = ticker_data
                                                .o
                                                .as_ref()
                                                .and_then(|v| Decimal::from_str(v).ok());
                                            let base_volume = ticker_data
                                                .v
                                                .as_ref()
                                                .and_then(|v| Decimal::from_str(v).ok());
                                            let quote_volume = ticker_data
                                                .qv
                                                .as_ref()
                                                .and_then(|v| Decimal::from_str(v).ok());
                                            let percentage = ticker_data
                                                .m
                                                .as_ref()
                                                .and_then(|v| Decimal::from_str(v).ok())
                                                .map(|v| v * Decimal::from(100));
                                            let change = if let (Some(l), Some(o)) = (last, open) {
                                                Some(l - o)
                                            } else {
                                                None
                                            };

                                            let ticker = Ticker {
                                                symbol: symbol.clone(),
                                                timestamp: ticker_data.t,
                                                datetime: ticker_data.t.map(|t| {
                                                    chrono::DateTime::from_timestamp(
                                                        t / 1000,
                                                        ((t % 1000) * 1_000_000) as u32,
                                                    )
                                                    .map(|dt| dt.to_rfc3339())
                                                    .unwrap_or_default()
                                                }),
                                                high,
                                                low,
                                                bid: None,
                                                bid_volume: None,
                                                ask: None,
                                                ask_volume: None,
                                                vwap: None,
                                                open,
                                                close: last,
                                                last,
                                                previous_close: None,
                                                change,
                                                percentage,
                                                average: None,
                                                base_volume,
                                                quote_volume,
                                                index_price: None,
                                                mark_price: None,
                                                info: serde_json::Value::Null,
                                            };
                                            let _ = tx.send(WsMessage::Ticker(WsTickerEvent {
                                                symbol,
                                                ticker,
                                            }));
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    },
                    Err(_) => break,
                    _ => {},
                }
            }
        });

        Ok(rx)
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut ws = self.connect().await?;

        // Subscribe to all symbols
        for symbol in symbols {
            self.subscribe(&mut ws, symbol, "realtimes").await?;
        }

        let _ = tx.send(WsMessage::Connected);

        let _symbols_clone: Vec<String> = symbols.iter().map(|s| s.to_string()).collect();

        tokio::spawn(async move {
            let parse_symbol = |market_id: &str| -> String {
                let quote_currencies = ["USDT", "USD", "BTC", "ETH", "EUR"];
                for quote in quote_currencies.iter() {
                    if let Some(base) = market_id.strip_suffix(quote) {
                        return format!("{base}/{quote}");
                    }
                }
                market_id.to_string()
            };

            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(message) = serde_json::from_str::<HashkeyWsMessage>(&text) {
                            if message.topic.as_deref() == Some("realtimes") {
                                if let Some(data_val) = &message.data {
                                    if let Ok(data_arr) =
                                        serde_json::from_value::<Vec<HashkeyWsTicker>>(
                                            data_val.clone(),
                                        )
                                    {
                                        if let Some(ticker_data) = data_arr.first() {
                                            let market_id = message.symbol.as_deref().unwrap_or("");
                                            let symbol = parse_symbol(market_id);

                                            let last = ticker_data
                                                .c
                                                .as_ref()
                                                .and_then(|v| Decimal::from_str(v).ok());
                                            let open = ticker_data
                                                .o
                                                .as_ref()
                                                .and_then(|v| Decimal::from_str(v).ok());

                                            let ticker = Ticker {
                                                symbol: symbol.clone(),
                                                timestamp: ticker_data.t,
                                                datetime: ticker_data.t.map(|t| {
                                                    chrono::DateTime::from_timestamp(
                                                        t / 1000,
                                                        ((t % 1000) * 1_000_000) as u32,
                                                    )
                                                    .map(|dt| dt.to_rfc3339())
                                                    .unwrap_or_default()
                                                }),
                                                high: ticker_data
                                                    .h
                                                    .as_ref()
                                                    .and_then(|v| Decimal::from_str(v).ok()),
                                                low: ticker_data
                                                    .l
                                                    .as_ref()
                                                    .and_then(|v| Decimal::from_str(v).ok()),
                                                bid: None,
                                                bid_volume: None,
                                                ask: None,
                                                ask_volume: None,
                                                vwap: None,
                                                open,
                                                close: last,
                                                last,
                                                previous_close: None,
                                                change: if let (Some(l), Some(o)) = (last, open) {
                                                    Some(l - o)
                                                } else {
                                                    None
                                                },
                                                percentage: ticker_data
                                                    .m
                                                    .as_ref()
                                                    .and_then(|v| Decimal::from_str(v).ok())
                                                    .map(|v| v * Decimal::from(100)),
                                                average: None,
                                                base_volume: ticker_data
                                                    .v
                                                    .as_ref()
                                                    .and_then(|v| Decimal::from_str(v).ok()),
                                                quote_volume: ticker_data
                                                    .qv
                                                    .as_ref()
                                                    .and_then(|v| Decimal::from_str(v).ok()),
                                                index_price: None,
                                                mark_price: None,
                                                info: serde_json::Value::Null,
                                            };
                                            let _ = tx.send(WsMessage::Ticker(WsTickerEvent {
                                                symbol,
                                                ticker,
                                            }));
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    },
                    Err(_) => break,
                    _ => {},
                }
            }
        });

        Ok(rx)
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut ws = self.connect().await?;

        // Subscribe to order book
        self.subscribe(&mut ws, symbol, "depth").await?;

        let _ = tx.send(WsMessage::Connected);

        let symbol_clone = symbol.to_string();
        let _order_books = self.order_books.clone();

        tokio::spawn(async move {
            let parse_symbol = |market_id: &str| -> String {
                let quote_currencies = ["USDT", "USD", "BTC", "ETH", "EUR"];
                for quote in quote_currencies.iter() {
                    if let Some(base) = market_id.strip_suffix(quote) {
                        return format!("{base}/{quote}");
                    }
                }
                market_id.to_string()
            };

            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(message) = serde_json::from_str::<HashkeyWsMessage>(&text) {
                            if message.topic.as_deref() == Some("depth") {
                                if let Some(data_val) = &message.data {
                                    if let Ok(data_arr) =
                                        serde_json::from_value::<Vec<HashkeyWsOrderBookData>>(
                                            data_val.clone(),
                                        )
                                    {
                                        if let Some(book_data) = data_arr.first() {
                                            let market_id = message.symbol.as_deref().unwrap_or("");
                                            let symbol = if market_id.is_empty() {
                                                symbol_clone.clone()
                                            } else {
                                                parse_symbol(market_id)
                                            };

                                            let bids: Vec<OrderBookEntry> = book_data
                                                .b
                                                .as_ref()
                                                .map(|b| {
                                                    b.iter()
                                                        .filter_map(|entry| {
                                                            if entry.len() >= 2 {
                                                                Some(OrderBookEntry {
                                                                    price: Decimal::from_str(
                                                                        &entry[0],
                                                                    )
                                                                    .ok()?,
                                                                    amount: Decimal::from_str(
                                                                        &entry[1],
                                                                    )
                                                                    .ok()?,
                                                                })
                                                            } else {
                                                                None
                                                            }
                                                        })
                                                        .collect()
                                                })
                                                .unwrap_or_default();

                                            let asks: Vec<OrderBookEntry> = book_data
                                                .a
                                                .as_ref()
                                                .map(|a| {
                                                    a.iter()
                                                        .filter_map(|entry| {
                                                            if entry.len() >= 2 {
                                                                Some(OrderBookEntry {
                                                                    price: Decimal::from_str(
                                                                        &entry[0],
                                                                    )
                                                                    .ok()?,
                                                                    amount: Decimal::from_str(
                                                                        &entry[1],
                                                                    )
                                                                    .ok()?,
                                                                })
                                                            } else {
                                                                None
                                                            }
                                                        })
                                                        .collect()
                                                })
                                                .unwrap_or_default();

                                            let order_book = OrderBook {
                                                symbol: symbol.clone(),
                                                timestamp: book_data.t,
                                                datetime: book_data.t.map(|t| {
                                                    chrono::DateTime::from_timestamp(
                                                        t / 1000,
                                                        ((t % 1000) * 1_000_000) as u32,
                                                    )
                                                    .map(|dt| dt.to_rfc3339())
                                                    .unwrap_or_default()
                                                }),
                                                nonce: None,
                                                bids,
                                                asks,
                                                checksum: None,
                                            };

                                            let _ =
                                                tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                                                    symbol,
                                                    order_book,
                                                    is_snapshot: message.f.unwrap_or(true),
                                                }));
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    },
                    Err(_) => break,
                    _ => {},
                }
            }
        });

        Ok(rx)
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut ws = self.connect().await?;

        // Subscribe to trades
        self.subscribe(&mut ws, symbol, "trade").await?;

        let _ = tx.send(WsMessage::Connected);

        let symbol_clone = symbol.to_string();

        tokio::spawn(async move {
            let parse_symbol = |market_id: &str| -> String {
                let quote_currencies = ["USDT", "USD", "BTC", "ETH", "EUR"];
                for quote in quote_currencies.iter() {
                    if let Some(base) = market_id.strip_suffix(quote) {
                        return format!("{base}/{quote}");
                    }
                }
                market_id.to_string()
            };

            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(message) = serde_json::from_str::<HashkeyWsMessage>(&text) {
                            if message.topic.as_deref() == Some("trade") {
                                if let Some(data_val) = &message.data {
                                    if let Ok(data_arr) =
                                        serde_json::from_value::<Vec<HashkeyWsTrade>>(
                                            data_val.clone(),
                                        )
                                    {
                                        let market_id = message.symbol.as_deref().unwrap_or("");
                                        let symbol = if market_id.is_empty() {
                                            symbol_clone.clone()
                                        } else {
                                            parse_symbol(market_id)
                                        };

                                        let trades: Vec<Trade> = data_arr
                                            .iter()
                                            .map(|t| {
                                                let side = if t.m.unwrap_or(false) {
                                                    Some("sell".to_string())
                                                } else {
                                                    Some("buy".to_string())
                                                };
                                                let price =
                                                    t.p.as_ref()
                                                        .and_then(|v| Decimal::from_str(v).ok())
                                                        .unwrap_or_default();
                                                let amount =
                                                    t.q.as_ref()
                                                        .and_then(|v| Decimal::from_str(v).ok())
                                                        .unwrap_or_default();

                                                Trade {
                                                    id: t.v.clone().unwrap_or_default(),
                                                    order: None,
                                                    timestamp: t.t,
                                                    datetime: t.t.map(|ts| {
                                                        chrono::DateTime::from_timestamp(
                                                            ts / 1000,
                                                            ((ts % 1000) * 1_000_000) as u32,
                                                        )
                                                        .map(|dt| dt.to_rfc3339())
                                                        .unwrap_or_default()
                                                    }),
                                                    symbol: symbol.clone(),
                                                    trade_type: None,
                                                    side,
                                                    taker_or_maker: if t.m.unwrap_or(false) {
                                                        Some(TakerOrMaker::Maker)
                                                    } else {
                                                        Some(TakerOrMaker::Taker)
                                                    },
                                                    price,
                                                    amount,
                                                    cost: Some(price * amount),
                                                    fee: None,
                                                    fees: vec![],
                                                    info: serde_json::Value::Null,
                                                }
                                            })
                                            .collect();

                                        if !trades.is_empty() {
                                            let _ = tx.send(WsMessage::Trade(WsTradeEvent {
                                                symbol,
                                                trades,
                                            }));
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    },
                    Err(_) => break,
                    _ => {},
                }
            }
        });

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut ws = self.connect().await?;

        // Subscribe to kline with timeframe
        let interval = self.convert_timeframe(timeframe);
        let topic = format!("kline_{interval}");
        self.subscribe(&mut ws, symbol, &topic).await?;

        let _ = tx.send(WsMessage::Connected);

        let symbol_clone = symbol.to_string();
        let timeframe_clone = timeframe;

        tokio::spawn(async move {
            let parse_symbol = |market_id: &str| -> String {
                let quote_currencies = ["USDT", "USD", "BTC", "ETH", "EUR"];
                for quote in quote_currencies.iter() {
                    if let Some(base) = market_id.strip_suffix(quote) {
                        return format!("{base}/{quote}");
                    }
                }
                market_id.to_string()
            };

            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(message) = serde_json::from_str::<HashkeyWsMessage>(&text) {
                            let topic = message.topic.as_deref().unwrap_or("");
                            if topic == "kline" || topic.starts_with("kline_") {
                                if let Some(data_val) = &message.data {
                                    if let Ok(data_arr) =
                                        serde_json::from_value::<Vec<HashkeyWsOhlcv>>(
                                            data_val.clone(),
                                        )
                                    {
                                        if let Some(ohlcv_data) = data_arr.first() {
                                            let market_id = message.symbol.as_deref().unwrap_or("");
                                            let symbol = if market_id.is_empty() {
                                                symbol_clone.clone()
                                            } else {
                                                parse_symbol(market_id)
                                            };

                                            let ohlcv = crate::types::OHLCV {
                                                timestamp: ohlcv_data.t.unwrap_or(0),
                                                open: ohlcv_data
                                                    .o
                                                    .as_ref()
                                                    .and_then(|v| Decimal::from_str(v).ok())
                                                    .unwrap_or_default(),
                                                high: ohlcv_data
                                                    .h
                                                    .as_ref()
                                                    .and_then(|v| Decimal::from_str(v).ok())
                                                    .unwrap_or_default(),
                                                low: ohlcv_data
                                                    .l
                                                    .as_ref()
                                                    .and_then(|v| Decimal::from_str(v).ok())
                                                    .unwrap_or_default(),
                                                close: ohlcv_data
                                                    .c
                                                    .as_ref()
                                                    .and_then(|v| Decimal::from_str(v).ok())
                                                    .unwrap_or_default(),
                                                volume: ohlcv_data
                                                    .v
                                                    .as_ref()
                                                    .and_then(|v| Decimal::from_str(v).ok())
                                                    .unwrap_or_default(),
                                            };

                                            let _ = tx.send(WsMessage::Ohlcv(WsOhlcvEvent {
                                                symbol,
                                                timeframe: timeframe_clone,
                                                ohlcv,
                                            }));
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    },
                    Err(_) => break,
                    _ => {},
                }
            }
        });

        Ok(rx)
    }

    // === Private Channel Methods ===

    async fn watch_orders(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.config.api_key().is_none() || self.config.secret().is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let mut ws = self.connect_private().await?;

        // Subscribe to order updates
        self.subscribe_private_stream(&mut ws, "executionReport")
            .await?;

        let _ = tx.send(WsMessage::Connected);

        // Clone values for the spawned task
        let config = self.config.clone();

        tokio::spawn(async move {
            let parse_symbol = |market_id: &str| -> String {
                let quote_currencies = ["USDT", "USD", "BTC", "ETH", "EUR"];
                for quote in quote_currencies.iter() {
                    if let Some(base) = market_id.strip_suffix(quote) {
                        return format!("{base}/{quote}");
                    }
                }
                market_id.to_string()
            };

            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                            if parsed.get("e").and_then(|v| v.as_str()) == Some("executionReport") {
                                if let Ok(data) =
                                    serde_json::from_value::<HashkeyOrderUpdateData>(parsed.clone())
                                {
                                    if let Some(symbol_raw) = &data.symbol {
                                        let symbol = parse_symbol(symbol_raw);
                                        let order_id = data
                                            .order_id
                                            .map(|id| id.to_string())
                                            .unwrap_or_default();

                                        let status = match data.order_status.as_deref() {
                                            Some("NEW") => OrderStatus::Open,
                                            Some("PARTIALLY_FILLED") => OrderStatus::Open,
                                            Some("FILLED") => OrderStatus::Closed,
                                            Some("CANCELED") => OrderStatus::Canceled,
                                            Some("REJECTED") => OrderStatus::Rejected,
                                            Some("EXPIRED") => OrderStatus::Expired,
                                            _ => OrderStatus::Open,
                                        };

                                        let side = match data.side.as_deref() {
                                            Some("BUY") => OrderSide::Buy,
                                            _ => OrderSide::Sell,
                                        };

                                        let order_type = match data.order_type.as_deref() {
                                            Some("LIMIT") => OrderType::Limit,
                                            Some("MARKET") => OrderType::Market,
                                            _ => OrderType::Limit,
                                        };

                                        let price = data
                                            .price
                                            .as_ref()
                                            .and_then(|p| Decimal::from_str(p).ok());
                                        let amount = data
                                            .quantity
                                            .as_ref()
                                            .and_then(|q| Decimal::from_str(q).ok());
                                        let filled = data
                                            .cumulative_filled_qty
                                            .as_ref()
                                            .and_then(|f| Decimal::from_str(f).ok());

                                        let order = Order {
                                            id: order_id,
                                            client_order_id: data.client_order_id.clone(),
                                            timestamp: data.event_time,
                                            datetime: data.event_time.map(|t| {
                                                chrono::DateTime::from_timestamp_millis(t)
                                                    .map(|dt| dt.to_rfc3339())
                                                    .unwrap_or_default()
                                            }),
                                            last_trade_timestamp: data.trade_time,
                                            last_update_timestamp: data.event_time,
                                            status,
                                            symbol,
                                            order_type,
                                            time_in_force: None,
                                            side,
                                            price,
                                            average: data
                                                .last_filled_price
                                                .as_ref()
                                                .and_then(|p| Decimal::from_str(p).ok()),
                                            amount: amount.unwrap_or_default(),
                                            filled: filled.unwrap_or_default(),
                                            remaining: match (amount, filled) {
                                                (Some(a), Some(f)) => Some(a - f),
                                                _ => None,
                                            },
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
                                            info: serde_json::to_value(&data)
                                                .unwrap_or(serde_json::Value::Null),
                                        };

                                        let _ = tx.send(WsMessage::Order(WsOrderEvent { order }));
                                    }
                                }
                            }
                        }
                    },
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    },
                    Err(_) => break,
                    _ => {},
                }
            }
            drop(config);
        });

        Ok(rx)
    }

    async fn watch_my_trades(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // HashKey sends trade info within executionReport, reuse watch_orders
        self.watch_orders(_symbol).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.config.api_key().is_none() || self.config.secret().is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let mut ws = self.connect_private().await?;

        // Subscribe to balance updates
        self.subscribe_private_stream(&mut ws, "outboundAccountPosition")
            .await?;

        let _ = tx.send(WsMessage::Connected);

        tokio::spawn(async move {
            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                            let event_type = parsed.get("e").and_then(|v| v.as_str());
                            if event_type == Some("outboundAccountPosition")
                                || event_type == Some("balanceUpdate")
                            {
                                if let Some(balances_arr) =
                                    parsed.get("B").and_then(|v| v.as_array())
                                {
                                    let mut currencies: HashMap<String, Balance> = HashMap::new();

                                    for bal in balances_arr {
                                        if let Ok(balance_data) =
                                            serde_json::from_value::<HashkeyBalanceUpdateData>(
                                                bal.clone(),
                                            )
                                        {
                                            if let Some(asset) = &balance_data.asset {
                                                let free = balance_data
                                                    .free
                                                    .as_ref()
                                                    .and_then(|f| Decimal::from_str(f).ok())
                                                    .unwrap_or_default();
                                                let locked = balance_data
                                                    .locked
                                                    .as_ref()
                                                    .and_then(|l| Decimal::from_str(l).ok())
                                                    .unwrap_or_default();

                                                currencies.insert(
                                                    asset.clone(),
                                                    Balance {
                                                        free: Some(free),
                                                        used: Some(locked),
                                                        total: Some(free + locked),
                                                        debt: None,
                                                    },
                                                );
                                            }
                                        }
                                    }

                                    let timestamp = parsed
                                        .get("E")
                                        .and_then(|v| v.as_i64())
                                        .or_else(|| Some(Utc::now().timestamp_millis()));

                                    let balances = Balances {
                                        timestamp,
                                        datetime: None,
                                        currencies,
                                        info: parsed.clone(),
                                    };

                                    let _ =
                                        tx.send(WsMessage::Balance(WsBalanceEvent { balances }));
                                }
                            }
                        }
                    },
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    },
                    Err(_) => break,
                    _ => {},
                }
            }
        });

        Ok(rx)
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        if self.config.api_key().is_none() || self.config.secret().is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API credentials required for authentication".into(),
            });
        }

        let mut ws = self.connect_private().await?;

        let api_key = self.config.api_key().unwrap();
        let timestamp = Utc::now().timestamp_millis();
        let sign_payload = format!("{api_key}timestamp={timestamp}");
        let signature = self.sign(&sign_payload)?;

        let auth_msg = serde_json::json!({
            "id": self.next_request_id(),
            "method": "LOGIN",
            "params": {
                "apiKey": api_key,
                "timestamp": timestamp,
                "signature": signature
            }
        });

        let auth_str = serde_json::to_string(&auth_msg).map_err(|e| CcxtError::ParseError {
            data_type: "json".to_string(),
            message: e.to_string(),
        })?;

        ws.send(Message::Text(auth_str))
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_PRIVATE_URL.to_string(),
                message: e.to_string(),
            })?;

        self.ws_client = Some(ws);
        Ok(())
    }

    // === Connection Management ===

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_some() {
            return Ok(());
        }
        let (ws_stream, _) = connect_async(WS_PUBLIC_URL).await.map_err(|e| {
            crate::errors::CcxtError::NetworkError {
                url: WS_PUBLIC_URL.to_string(),
                message: e.to_string(),
            }
        })?;
        self.ws_client = Some(ws_stream);
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(mut ws) = self.ws_client.take() {
            let _ = ws.close(None).await;
        }
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        self.ws_client.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_symbol() {
        let config = ExchangeConfig::default();
        let ws = HashkeyWs::new(config);

        assert_eq!(ws.convert_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(ws.convert_symbol("ETH/USD"), "ETHUSD");
    }

    #[test]
    fn test_parse_symbol() {
        let config = ExchangeConfig::default();
        let ws = HashkeyWs::new(config);

        assert_eq!(ws.parse_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(ws.parse_symbol("ETHUSD"), "ETH/USD");
        assert_eq!(ws.parse_symbol("ETHBTC"), "ETH/BTC");
    }

    #[test]
    fn test_convert_timeframe() {
        let config = ExchangeConfig::default();
        let ws = HashkeyWs::new(config);

        assert_eq!(ws.convert_timeframe(Timeframe::Minute1), "1m");
        assert_eq!(ws.convert_timeframe(Timeframe::Hour1), "1h");
        assert_eq!(ws.convert_timeframe(Timeframe::Day1), "1d");
    }

    #[test]
    fn test_with_credentials() {
        let ws = HashkeyWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert_eq!(ws.config.api_key(), Some("test_key"));
        assert_eq!(ws.config.secret(), Some("test_secret"));
    }

    #[test]
    fn test_set_credentials() {
        let config = ExchangeConfig::default();
        let mut ws = HashkeyWs::new(config);
        assert!(ws.config.api_key().is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert_eq!(ws.config.api_key(), Some("test_key"));
        assert_eq!(ws.config.secret(), Some("test_secret"));
    }

    #[test]
    fn test_next_request_id() {
        let ws = HashkeyWs::with_credentials("key".to_string(), "secret".to_string());
        let id1 = ws.next_request_id();
        let id2 = ws.next_request_id();
        let id3 = ws.next_request_id();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    #[test]
    fn test_sign() {
        let ws = HashkeyWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        let signature = ws.sign("test_message").unwrap();
        assert!(!signature.is_empty());
        assert_eq!(signature.len(), 64); // HMAC-SHA256 produces 32 bytes = 64 hex chars
    }

    #[test]
    fn test_parse_order() {
        let ws = HashkeyWs::with_credentials("key".to_string(), "secret".to_string());

        let order_data = HashkeyOrderUpdateData {
            event_type: Some("executionReport".to_string()),
            event_time: Some(1700000000000),
            symbol: Some("BTCUSDT".to_string()),
            client_order_id: Some("client123".to_string()),
            side: Some("BUY".to_string()),
            order_type: Some("LIMIT".to_string()),
            time_in_force: Some("GTC".to_string()),
            quantity: Some("1.5".to_string()),
            price: Some("50000.0".to_string()),
            order_status: Some("NEW".to_string()),
            order_id: Some(12345),
            last_filled_qty: None,
            cumulative_filled_qty: Some("0".to_string()),
            last_filled_price: None,
            trade_time: None,
            trade_id: None,
            commission: None,
            commission_asset: None,
        };

        let result = ws.parse_order(&order_data);
        assert!(result.is_some());

        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.symbol, "BTC/USDT");
            assert_eq!(event.order.id, "12345");
            assert_eq!(event.order.side, OrderSide::Buy);
            assert_eq!(event.order.order_type, OrderType::Limit);
            assert_eq!(event.order.status, OrderStatus::Open);
            assert_eq!(event.order.amount, Decimal::from_str("1.5").unwrap());
            assert_eq!(
                event.order.price,
                Some(Decimal::from_str("50000.0").unwrap())
            );
        }
    }

    #[test]
    fn test_parse_order_status_mapping() {
        let ws = HashkeyWs::with_credentials("key".to_string(), "secret".to_string());

        let test_cases = vec![
            ("NEW", OrderStatus::Open),
            ("PARTIALLY_FILLED", OrderStatus::Open),
            ("FILLED", OrderStatus::Closed),
            ("CANCELED", OrderStatus::Canceled),
            ("REJECTED", OrderStatus::Rejected),
            ("EXPIRED", OrderStatus::Expired),
        ];

        for (status_str, expected_status) in test_cases {
            let order_data = HashkeyOrderUpdateData {
                event_type: Some("executionReport".to_string()),
                event_time: Some(1700000000000),
                symbol: Some("BTCUSDT".to_string()),
                client_order_id: None,
                side: Some("BUY".to_string()),
                order_type: Some("LIMIT".to_string()),
                time_in_force: None,
                quantity: Some("1.0".to_string()),
                price: Some("50000.0".to_string()),
                order_status: Some(status_str.to_string()),
                order_id: Some(123),
                last_filled_qty: None,
                cumulative_filled_qty: None,
                last_filled_price: None,
                trade_time: None,
                trade_id: None,
                commission: None,
                commission_asset: None,
            };

            let result = ws.parse_order(&order_data);
            if let Some(WsMessage::Order(event)) = result {
                assert_eq!(
                    event.order.status, expected_status,
                    "Failed for status: {status_str}"
                );
            }
        }
    }

    #[test]
    fn test_parse_balance() {
        let ws = HashkeyWs::with_credentials("key".to_string(), "secret".to_string());

        let balances_json = serde_json::json!([
            {"a": "BTC", "f": "1.5", "l": "0.5"},
            {"a": "USDT", "f": "10000", "l": "2000"}
        ]);
        let balances_arr = balances_json.as_array().unwrap();

        let parsed_json = serde_json::json!({
            "e": "outboundAccountPosition",
            "E": 1700000000000i64
        });

        let result = ws.parse_balance(balances_arr, &parsed_json);
        assert!(result.is_some());

        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("BTC"));
            assert!(event.balances.currencies.contains_key("USDT"));

            let btc = event.balances.currencies.get("BTC").unwrap();
            assert_eq!(btc.free, Some(Decimal::from_str("1.5").unwrap()));
            assert_eq!(btc.used, Some(Decimal::from_str("0.5").unwrap()));
            assert_eq!(btc.total, Some(Decimal::from_str("2.0").unwrap()));

            let usdt = event.balances.currencies.get("USDT").unwrap();
            assert_eq!(usdt.free, Some(Decimal::from_str("10000").unwrap()));
            assert_eq!(usdt.used, Some(Decimal::from_str("2000").unwrap()));
            assert_eq!(usdt.total, Some(Decimal::from_str("12000").unwrap()));
        }
    }

    #[test]
    fn test_process_private_message_order() {
        let ws = HashkeyWs::with_credentials("key".to_string(), "secret".to_string());

        let order_msg = r#"{
            "e": "executionReport",
            "E": 1700000000000,
            "s": "BTCUSDT",
            "c": "client123",
            "S": "BUY",
            "o": "LIMIT",
            "f": "GTC",
            "q": "1.0",
            "p": "50000",
            "X": "NEW",
            "i": 12345
        }"#;

        let result = ws.process_private_message(order_msg);
        assert!(result.is_some());
        assert!(matches!(result, Some(WsMessage::Order(_))));
    }

    #[test]
    fn test_process_private_message_balance() {
        let ws = HashkeyWs::with_credentials("key".to_string(), "secret".to_string());

        let balance_msg = r#"{
            "e": "outboundAccountPosition",
            "E": 1700000000000,
            "B": [
                {"a": "BTC", "f": "1.5", "l": "0.5"},
                {"a": "ETH", "f": "10", "l": "2"}
            ]
        }"#;

        let result = ws.process_private_message(balance_msg);
        assert!(result.is_some());
        assert!(matches!(result, Some(WsMessage::Balance(_))));
    }

    #[tokio::test]
    async fn test_watch_orders_requires_credentials() {
        let config = ExchangeConfig::default();
        let ws = HashkeyWs::new(config);

        let result = ws.watch_orders(None).await;
        assert!(result.is_err());
        if let Err(CcxtError::AuthenticationError { message }) = result {
            assert!(message.contains("credentials"));
        }
    }

    #[tokio::test]
    async fn test_watch_balance_requires_credentials() {
        let config = ExchangeConfig::default();
        let ws = HashkeyWs::new(config);

        let result = ws.watch_balance().await;
        assert!(result.is_err());
        if let Err(CcxtError::AuthenticationError { message }) = result {
            assert!(message.contains("credentials"));
        }
    }

    #[tokio::test]
    async fn test_ws_authenticate_requires_credentials() {
        let config = ExchangeConfig::default();
        let mut ws = HashkeyWs::new(config);

        let result = ws.ws_authenticate().await;
        assert!(result.is_err());
        if let Err(CcxtError::AuthenticationError { message }) = result {
            assert!(message.contains("credentials"));
        }
    }

    #[test]
    fn test_default() {
        let config = ExchangeConfig::default();
        let ws = HashkeyWs::new(config);

        assert!(ws.config.api_key().is_none());
        assert!(ws.config.secret().is_none());
        assert!(ws.ws_client.is_none());
    }
}
