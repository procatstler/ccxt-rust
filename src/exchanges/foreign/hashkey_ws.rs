//! Hashkey WebSocket Implementation
//!
//! WebSocket client for Hashkey Global exchange.
//! Supports: watchTicker, watchTrades, watchOrderBook, watchOHLCV

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
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
use crate::errors::CcxtResult;
use crate::types::{
    WsOhlcvEvent, OrderBook, OrderBookEntry, WsOrderBookEvent, Ticker, WsTickerEvent, Timeframe, Trade,
    WsTradeEvent, WsExchange, WsMessage, TakerOrMaker,
};

const WS_PUBLIC_URL: &str = "wss://stream-glb.hashkey.com/quote/ws/v1";

type WsClient = WebSocketStream<MaybeTlsStream<TcpStream>>;

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
    pub t: Option<i64>,      // timestamp
    pub s: Option<String>,   // symbol
    pub c: Option<String>,   // close price
    pub h: Option<String>,   // high
    pub l: Option<String>,   // low
    pub o: Option<String>,   // open
    pub v: Option<String>,   // volume
    pub qv: Option<String>,  // quote volume
    pub m: Option<String>,   // change percent
}

/// WebSocket trade response
#[derive(Debug, Deserialize)]
struct HashkeyWsTrade {
    pub v: Option<String>,   // trade id
    pub t: Option<i64>,      // timestamp
    pub p: Option<String>,   // price
    pub q: Option<String>,   // quantity
    pub m: Option<bool>,     // is maker
}

/// WebSocket order book data
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct HashkeyWsOrderBookData {
    pub t: Option<i64>,          // timestamp
    pub s: Option<String>,       // symbol
    pub b: Option<Vec<Vec<String>>>, // bids [[price, amount], ...]
    pub a: Option<Vec<Vec<String>>>, // asks [[price, amount], ...]
}

/// WebSocket OHLCV data
#[derive(Debug, Deserialize)]
struct HashkeyWsOhlcv {
    pub t: Option<i64>,      // timestamp
    pub o: Option<String>,   // open
    pub h: Option<String>,   // high
    pub l: Option<String>,   // low
    pub c: Option<String>,   // close
    pub v: Option<String>,   // volume
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
    pub f: Option<bool>,     // is first/snapshot
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
            if market_id.ends_with(quote) {
                let base = &market_id[..market_id.len() - quote.len()];
                return format!("{}/{}", base, quote);
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
        let (ws_stream, _) = connect_async(WS_PUBLIC_URL)
            .await
            .map_err(|e| crate::errors::CcxtError::NetworkError { url: WS_PUBLIC_URL.to_string(), message: e.to_string() })?;
        Ok(ws_stream)
    }

    /// Subscribe to a topic
    async fn subscribe(&self, ws: &mut WsClient, symbol: &str, topic: &str) -> CcxtResult<()> {
        let market_id = self.convert_symbol(symbol);

        let msg = WsSubscribeMessage {
            symbol: market_id,
            topic: topic.to_string(),
            event: "sub".to_string(),
        };

        let msg_str = serde_json::to_string(&msg)
            .map_err(|e| crate::errors::CcxtError::ParseError { data_type: "json".to_string(), message: e.to_string() })?;

        ws.send(Message::Text(msg_str))
            .await
            .map_err(|e| crate::errors::CcxtError::NetworkError { url: WS_PUBLIC_URL.to_string(), message: e.to_string() })?;

        Ok(())
    }

    /// Parse ticker from WebSocket message
    fn parse_ticker(&self, data: &HashkeyWsTicker, symbol: &str) -> Ticker {
        let last = data.c.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let high = data.h.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let low = data.l.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let open = data.o.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let base_volume = data.v.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let quote_volume = data.qv.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let percentage = data.m.as_ref()
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
            Some("sell".to_string())  // maker = sell
        } else {
            Some("buy".to_string())   // taker = buy
        };

        let price = data.p.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();
        let amount = data.q.as_ref()
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
        let bids: Vec<OrderBookEntry> = data.b.as_ref()
            .map(|b| b.iter()
                .filter_map(|entry| self.parse_order_book_entry(entry))
                .collect())
            .unwrap_or_default();

        let asks: Vec<OrderBookEntry> = data.a.as_ref()
            .map(|a| a.iter()
                .filter_map(|entry| self.parse_order_book_entry(entry))
                .collect())
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
                    if let Ok(data_arr) = serde_json::from_value::<Vec<HashkeyWsTicker>>(data_val.clone()) {
                        if let Some(ticker_data) = data_arr.first() {
                            let ticker = self.parse_ticker(ticker_data, &symbol);
                            let _ = tx.send(WsMessage::Ticker(WsTickerEvent {
                                symbol: symbol.clone(),
                                ticker,
                            }));
                        }
                    }
                }
            }
            "trade" => {
                // Trade data
                if let Some(data_val) = &message.data {
                    if let Ok(data_arr) = serde_json::from_value::<Vec<HashkeyWsTrade>>(data_val.clone()) {
                        let trades: Vec<Trade> = data_arr.iter()
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
            }
            "depth" => {
                // Order book data
                if let Some(data_val) = &message.data {
                    if let Ok(data_arr) = serde_json::from_value::<Vec<HashkeyWsOrderBookData>>(data_val.clone()) {
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
            }
            t if t == "kline" || t.starts_with("kline_") => {
                // OHLCV data
                if let Some(data_val) = &message.data {
                    if let Ok(data_arr) = serde_json::from_value::<Vec<HashkeyWsOhlcv>>(data_val.clone()) {
                        if let Some(ohlcv_data) = data_arr.first() {
                            // Get timeframe from params
                            let timeframe_str = message.params.as_ref()
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
                                open: ohlcv_data.o.as_ref()
                                    .and_then(|v| Decimal::from_str(v).ok())
                                    .unwrap_or_default(),
                                high: ohlcv_data.h.as_ref()
                                    .and_then(|v| Decimal::from_str(v).ok())
                                    .unwrap_or_default(),
                                low: ohlcv_data.l.as_ref()
                                    .and_then(|v| Decimal::from_str(v).ok())
                                    .unwrap_or_default(),
                                close: ohlcv_data.c.as_ref()
                                    .and_then(|v| Decimal::from_str(v).ok())
                                    .unwrap_or_default(),
                                volume: ohlcv_data.v.as_ref()
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
            }
            _ => {}
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
                if market_id.ends_with(quote) {
                    let base = &market_id[..market_id.len() - quote.len()];
                    return format!("{}/{}", base, quote);
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
                                    if let Ok(data_arr) = serde_json::from_value::<Vec<HashkeyWsTicker>>(data_val.clone()) {
                                        if let Some(ticker_data) = data_arr.first() {
                                            let market_id = message.symbol.as_deref().unwrap_or("");
                                            let symbol = if market_id.is_empty() {
                                                symbol_clone.clone()
                                            } else {
                                                this_parse_symbol(market_id)
                                            };

                                            let last = ticker_data.c.as_ref().and_then(|v| Decimal::from_str(v).ok());
                                            let high = ticker_data.h.as_ref().and_then(|v| Decimal::from_str(v).ok());
                                            let low = ticker_data.l.as_ref().and_then(|v| Decimal::from_str(v).ok());
                                            let open = ticker_data.o.as_ref().and_then(|v| Decimal::from_str(v).ok());
                                            let base_volume = ticker_data.v.as_ref().and_then(|v| Decimal::from_str(v).ok());
                                            let quote_volume = ticker_data.qv.as_ref().and_then(|v| Decimal::from_str(v).ok());
                                            let percentage = ticker_data.m.as_ref()
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
                                                    chrono::DateTime::from_timestamp(t / 1000, ((t % 1000) * 1_000_000) as u32)
                                                        .map(|dt| dt.to_rfc3339()).unwrap_or_default()
                                                }),
                                                high, low, bid: None, bid_volume: None, ask: None, ask_volume: None,
                                                vwap: None, open, close: last, last, previous_close: None,
                                                change, percentage, average: None, base_volume, quote_volume,
                                                index_price: None, mark_price: None,
                                                info: serde_json::Value::Null,
                                            };
                                            let _ = tx.send(WsMessage::Ticker(WsTickerEvent { symbol, ticker }));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    }
                    Err(_) => break,
                    _ => {}
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
                    if market_id.ends_with(quote) {
                        let base = &market_id[..market_id.len() - quote.len()];
                        return format!("{}/{}", base, quote);
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
                                    if let Ok(data_arr) = serde_json::from_value::<Vec<HashkeyWsTicker>>(data_val.clone()) {
                                        if let Some(ticker_data) = data_arr.first() {
                                            let market_id = message.symbol.as_deref().unwrap_or("");
                                            let symbol = parse_symbol(market_id);

                                            let last = ticker_data.c.as_ref().and_then(|v| Decimal::from_str(v).ok());
                                            let open = ticker_data.o.as_ref().and_then(|v| Decimal::from_str(v).ok());

                                            let ticker = Ticker {
                                                symbol: symbol.clone(),
                                                timestamp: ticker_data.t,
                                                datetime: ticker_data.t.map(|t| {
                                                    chrono::DateTime::from_timestamp(t / 1000, ((t % 1000) * 1_000_000) as u32)
                                                        .map(|dt| dt.to_rfc3339()).unwrap_or_default()
                                                }),
                                                high: ticker_data.h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                                                low: ticker_data.l.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                                                bid: None, bid_volume: None, ask: None, ask_volume: None, vwap: None,
                                                open, close: last, last, previous_close: None,
                                                change: if let (Some(l), Some(o)) = (last, open) { Some(l - o) } else { None },
                                                percentage: ticker_data.m.as_ref().and_then(|v| Decimal::from_str(v).ok()).map(|v| v * Decimal::from(100)),
                                                average: None,
                                                base_volume: ticker_data.v.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                                                quote_volume: ticker_data.qv.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                                                index_price: None, mark_price: None,
                                                info: serde_json::Value::Null,
                                            };
                                            let _ = tx.send(WsMessage::Ticker(WsTickerEvent { symbol, ticker }));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    }
                    Err(_) => break,
                    _ => {}
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
                    if market_id.ends_with(quote) {
                        let base = &market_id[..market_id.len() - quote.len()];
                        return format!("{}/{}", base, quote);
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
                                    if let Ok(data_arr) = serde_json::from_value::<Vec<HashkeyWsOrderBookData>>(data_val.clone()) {
                                        if let Some(book_data) = data_arr.first() {
                                            let market_id = message.symbol.as_deref().unwrap_or("");
                                            let symbol = if market_id.is_empty() {
                                                symbol_clone.clone()
                                            } else {
                                                parse_symbol(market_id)
                                            };

                                            let bids: Vec<OrderBookEntry> = book_data.b.as_ref()
                                                .map(|b| b.iter()
                                                    .filter_map(|entry| {
                                                        if entry.len() >= 2 {
                                                            Some(OrderBookEntry {
                                                                price: Decimal::from_str(&entry[0]).ok()?,
                                                                amount: Decimal::from_str(&entry[1]).ok()?,
                                                            })
                                                        } else { None }
                                                    }).collect())
                                                .unwrap_or_default();

                                            let asks: Vec<OrderBookEntry> = book_data.a.as_ref()
                                                .map(|a| a.iter()
                                                    .filter_map(|entry| {
                                                        if entry.len() >= 2 {
                                                            Some(OrderBookEntry {
                                                                price: Decimal::from_str(&entry[0]).ok()?,
                                                                amount: Decimal::from_str(&entry[1]).ok()?,
                                                            })
                                                        } else { None }
                                                    }).collect())
                                                .unwrap_or_default();

                                            let order_book = OrderBook {
                                                symbol: symbol.clone(),
                                                timestamp: book_data.t,
                                                datetime: book_data.t.map(|t| {
                                                    chrono::DateTime::from_timestamp(t / 1000, ((t % 1000) * 1_000_000) as u32)
                                                        .map(|dt| dt.to_rfc3339()).unwrap_or_default()
                                                }),
                                                nonce: None,
                                                bids, asks,
                                            };

                                            let _ = tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                                                symbol,
                                                order_book,
                                                is_snapshot: message.f.unwrap_or(true),
                                            }));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    }
                    Err(_) => break,
                    _ => {}
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
                    if market_id.ends_with(quote) {
                        let base = &market_id[..market_id.len() - quote.len()];
                        return format!("{}/{}", base, quote);
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
                                    if let Ok(data_arr) = serde_json::from_value::<Vec<HashkeyWsTrade>>(data_val.clone()) {
                                        let market_id = message.symbol.as_deref().unwrap_or("");
                                        let symbol = if market_id.is_empty() {
                                            symbol_clone.clone()
                                        } else {
                                            parse_symbol(market_id)
                                        };

                                        let trades: Vec<Trade> = data_arr.iter().map(|t| {
                                            let side = if t.m.unwrap_or(false) {
                                                Some("sell".to_string())
                                            } else {
                                                Some("buy".to_string())
                                            };
                                            let price = t.p.as_ref().and_then(|v| Decimal::from_str(v).ok()).unwrap_or_default();
                                            let amount = t.q.as_ref().and_then(|v| Decimal::from_str(v).ok()).unwrap_or_default();

                                            Trade {
                                                id: t.v.clone().unwrap_or_default(),
                                                order: None,
                                                timestamp: t.t,
                                                datetime: t.t.map(|ts| {
                                                    chrono::DateTime::from_timestamp(ts / 1000, ((ts % 1000) * 1_000_000) as u32)
                                                        .map(|dt| dt.to_rfc3339()).unwrap_or_default()
                                                }),
                                                symbol: symbol.clone(),
                                                trade_type: None,
                                                side,
                                                taker_or_maker: if t.m.unwrap_or(false) { Some(TakerOrMaker::Maker) } else { Some(TakerOrMaker::Taker) },
                                                price,
                                                amount,
                                                cost: Some(price * amount),
                                                fee: None,
                                                fees: vec![],
                                                info: serde_json::Value::Null,
                                            }
                                        }).collect();

                                        if !trades.is_empty() {
                                            let _ = tx.send(WsMessage::Trade(WsTradeEvent { symbol, trades }));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    }
                    Err(_) => break,
                    _ => {}
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
        let topic = format!("kline_{}", interval);
        self.subscribe(&mut ws, symbol, &topic).await?;

        let _ = tx.send(WsMessage::Connected);

        let symbol_clone = symbol.to_string();
        let timeframe_clone = timeframe;

        tokio::spawn(async move {
            let parse_symbol = |market_id: &str| -> String {
                let quote_currencies = ["USDT", "USD", "BTC", "ETH", "EUR"];
                for quote in quote_currencies.iter() {
                    if market_id.ends_with(quote) {
                        let base = &market_id[..market_id.len() - quote.len()];
                        return format!("{}/{}", base, quote);
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
                                    if let Ok(data_arr) = serde_json::from_value::<Vec<HashkeyWsOhlcv>>(data_val.clone()) {
                                        if let Some(ohlcv_data) = data_arr.first() {
                                            let market_id = message.symbol.as_deref().unwrap_or("");
                                            let symbol = if market_id.is_empty() {
                                                symbol_clone.clone()
                                            } else {
                                                parse_symbol(market_id)
                                            };

                                            let ohlcv = crate::types::OHLCV {
                                                timestamp: ohlcv_data.t.unwrap_or(0),
                                                open: ohlcv_data.o.as_ref().and_then(|v| Decimal::from_str(v).ok()).unwrap_or_default(),
                                                high: ohlcv_data.h.as_ref().and_then(|v| Decimal::from_str(v).ok()).unwrap_or_default(),
                                                low: ohlcv_data.l.as_ref().and_then(|v| Decimal::from_str(v).ok()).unwrap_or_default(),
                                                close: ohlcv_data.c.as_ref().and_then(|v| Decimal::from_str(v).ok()).unwrap_or_default(),
                                                volume: ohlcv_data.v.as_ref().and_then(|v| Decimal::from_str(v).ok()).unwrap_or_default(),
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
                    }
                    Ok(Message::Ping(data)) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
        });

        Ok(rx)
    }

    // === Connection Management ===

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_some() {
            return Ok(());
        }
        let (ws_stream, _) = connect_async(WS_PUBLIC_URL)
            .await
            .map_err(|e| crate::errors::CcxtError::NetworkError { url: WS_PUBLIC_URL.to_string(), message: e.to_string() })?;
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
}
