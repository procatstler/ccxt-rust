//! LATOKEN WebSocket Implementation
//!
//! LATOKEN WebSocket API using STOMP protocol for real-time market data
//! URL: wss://api.latoken.com/stomp

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage,
    WsTickerEvent, WsTradeEvent, WsOrderBookEvent,
};

const WS_URL: &str = "wss://api.latoken.com/stomp";

/// STOMP frame parser and builder
struct StompFrame {
    command: String,
    headers: HashMap<String, String>,
    body: String,
}

impl StompFrame {
    /// Parse a STOMP frame from raw text
    fn parse(raw: &str) -> Option<Self> {
        let mut lines = raw.lines();
        let command = lines.next()?.to_string();

        if command.is_empty() {
            return None;
        }

        let mut headers = HashMap::new();
        for line in lines.by_ref() {
            if line.is_empty() {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                headers.insert(key.trim().to_string(), value.trim().to_string());
            }
        }

        // Rest is body (may contain null terminator)
        let body: String = lines.collect::<Vec<_>>().join("\n");
        let body = body.trim_end_matches('\0').to_string();

        Some(Self { command, headers, body })
    }

    /// Build a STOMP frame for sending
    fn build(command: &str, headers: &[(&str, &str)], body: Option<&str>) -> String {
        let mut frame = format!("{}\n", command);

        for (key, value) in headers {
            frame.push_str(&format!("{}:{}\n", key, value));
        }

        frame.push('\n');

        if let Some(b) = body {
            frame.push_str(b);
        }

        frame.push('\0');
        frame
    }
}

/// LATOKEN WebSocket client with STOMP protocol
pub struct LatokenWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    subscription_counter: Arc<RwLock<u32>>,
    currency_map: Arc<RwLock<HashMap<String, String>>>,
}

impl LatokenWs {
    /// Create a new LATOKEN WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            subscription_counter: Arc::new(RwLock::new(0)),
            currency_map: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set currency mappings (currency_id -> code)
    pub fn set_currency_map(&mut self, map: HashMap<String, String>) {
        if let Ok(mut cache) = self.currency_map.try_write() {
            *cache = map;
        }
    }

    /// Convert unified symbol to LATOKEN format
    /// BTC/USDT -> btc/usdt (lowercase)
    fn format_symbol(&self, symbol: &str) -> (String, String) {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            (parts[0].to_lowercase(), parts[1].to_lowercase())
        } else {
            (symbol.to_lowercase(), "usdt".to_string())
        }
    }

    /// Convert LATOKEN symbol to unified format
    fn parse_symbol(base: &str, quote: &str) -> String {
        format!("{}/{}", base.to_uppercase(), quote.to_uppercase())
    }

    /// Send STOMP CONNECT frame
    async fn stomp_connect(&self) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let connect_frame = StompFrame::build(
                "CONNECT",
                &[
                    ("accept-version", "1.1,1.2"),
                    ("heart-beat", "10000,10000"),
                ],
                None,
            );

            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(connect_frame))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to send STOMP CONNECT: {}", e),
                })?;
        }
        Ok(())
    }

    /// Send STOMP SUBSCRIBE frame
    async fn stomp_subscribe(&self, destination: &str) -> CcxtResult<u32> {
        let sub_id = {
            let mut counter = self.subscription_counter.write().await;
            let id = *counter;
            *counter += 1;
            id
        };

        if let Some(ws) = &self.ws_stream {
            let subscribe_frame = StompFrame::build(
                "SUBSCRIBE",
                &[
                    ("id", &sub_id.to_string()),
                    ("destination", destination),
                    ("ack", "auto"),
                ],
                None,
            );

            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(subscribe_frame))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to send STOMP SUBSCRIBE: {}", e),
                })?;
        }

        Ok(sub_id)
    }

    /// Start message processing loop
    fn start_message_loop(&self) {
        let ws_stream = self.ws_stream.clone();
        let subscriptions = self.subscriptions.clone();
        let orderbook_cache = self.orderbook_cache.clone();

        tokio::spawn(async move {
            if let Some(ws) = ws_stream {
                loop {
                    let msg = {
                        let mut ws_guard = ws.write().await;
                        ws_guard.next().await
                    };

                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Some(frame) = StompFrame::parse(&text) {
                                Self::handle_stomp_frame(&frame, &subscriptions, &orderbook_cache).await;
                            }
                        }
                        Some(Ok(Message::Ping(data))) => {
                            let mut ws_guard = ws.write().await;
                            let _ = ws_guard.send(Message::Pong(data)).await;
                        }
                        Some(Ok(Message::Close(_))) => break,
                        Some(Err(_)) => break,
                        None => break,
                        _ => {}
                    }
                }
            }
        });
    }

    /// Handle incoming STOMP frames
    async fn handle_stomp_frame(
        frame: &StompFrame,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        match frame.command.as_str() {
            "MESSAGE" => {
                let destination = frame.headers.get("destination").map(|s| s.as_str()).unwrap_or("");

                if let Ok(body) = serde_json::from_str::<Value>(&frame.body) {
                    if destination.starts_with("/v1/book/") {
                        Self::handle_orderbook(&body, destination, subscriptions, orderbook_cache).await;
                    } else if destination.starts_with("/v1/trade/") {
                        Self::handle_trades(&body, destination, subscriptions).await;
                    } else if destination.starts_with("/v1/ticker") {
                        Self::handle_ticker(&body, destination, subscriptions).await;
                    }
                }
            }
            "CONNECTED" => {
                // Connection established
            }
            "ERROR" => {
                // Handle error frame
            }
            _ => {}
        }
    }

    /// Handle orderbook messages
    async fn handle_orderbook(
        data: &Value,
        destination: &str,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        // Parse symbol from destination: /v1/book/{base}/{quote}
        let parts: Vec<&str> = destination.split('/').collect();
        if parts.len() < 4 {
            return;
        }
        let base = parts[parts.len() - 2];
        let quote = parts[parts.len() - 1];
        let symbol = Self::parse_symbol(base, quote);
        let key = format!("orderbook:{}", symbol);

        let payload = data.get("payload").unwrap_or(data);

        let mut bids: Vec<OrderBookEntry> = Vec::new();
        let mut asks: Vec<OrderBookEntry> = Vec::new();

        // Parse bids
        if let Some(bid_data) = payload.get("bid").and_then(|v| v.as_array()) {
            for entry in bid_data {
                let price = Self::parse_decimal(entry.get("price").unwrap_or(&Value::Null));
                let amount = Self::parse_decimal(entry.get("quantity").unwrap_or(&Value::Null));
                if !price.is_zero() {
                    bids.push(OrderBookEntry { price, amount });
                }
            }
        }

        // Parse asks
        if let Some(ask_data) = payload.get("ask").and_then(|v| v.as_array()) {
            for entry in ask_data {
                let price = Self::parse_decimal(entry.get("price").unwrap_or(&Value::Null));
                let amount = Self::parse_decimal(entry.get("quantity").unwrap_or(&Value::Null));
                if !price.is_zero() {
                    asks.push(OrderBookEntry { price, amount });
                }
            }
        }

        // Sort bids descending, asks ascending
        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

        let timestamp = data.get("timestamp")
            .and_then(|v| v.as_i64())
            .or_else(|| Some(Utc::now().timestamp_millis()));

        let orderbook = OrderBook {
            symbol: symbol.clone(),
            bids,
            asks,
            timestamp,
            datetime: None,
            nonce: data.get("nonce").and_then(|v| v.as_i64()),
        };

        // Update cache
        {
            let mut cache = orderbook_cache.write().await;
            cache.insert(symbol.clone(), orderbook.clone());
        }

        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&key) {
            let _ = sender.send(WsMessage::OrderBook(WsOrderBookEvent {
                symbol,
                order_book: orderbook,
                is_snapshot: true,
            }));
        }
    }

    /// Handle trades messages
    async fn handle_trades(
        data: &Value,
        destination: &str,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        // Parse symbol from destination: /v1/trade/{base}/{quote}
        let parts: Vec<&str> = destination.split('/').collect();
        if parts.len() < 4 {
            return;
        }
        let base = parts[parts.len() - 2];
        let quote = parts[parts.len() - 1];
        let symbol = Self::parse_symbol(base, quote);
        let key = format!("trades:{}", symbol);

        let payload = data.get("payload").unwrap_or(data);

        let trades_array = if let Some(arr) = payload.as_array() {
            arr.clone()
        } else {
            vec![payload.clone()]
        };

        let mut trades = Vec::new();
        for trade_data in &trades_array {
            let price = Self::parse_decimal(trade_data.get("price").unwrap_or(&Value::Null));
            let amount = Self::parse_decimal(trade_data.get("quantity").unwrap_or(&Value::Null));

            let timestamp = trade_data.get("timestamp")
                .and_then(|v| v.as_i64())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            let trade_id = trade_data.get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| timestamp.to_string());

            let mut trade = Trade::new(
                trade_id,
                symbol.clone(),
                price,
                amount,
            );

            trade = trade.with_timestamp(timestamp);

            // Direction: "buy" or "sell"
            if let Some(direction) = trade_data.get("direction").and_then(|v| v.as_str()) {
                trade = trade.with_side(direction);
            } else if let Some(maker_buyer) = trade_data.get("makerBuyer").and_then(|v| v.as_bool()) {
                let side = if maker_buyer { "sell" } else { "buy" };
                trade = trade.with_side(side);
            }

            trades.push(trade);
        }

        if !trades.is_empty() {
            let subs = subscriptions.read().await;
            if let Some(sender) = subs.get(&key) {
                let _ = sender.send(WsMessage::Trade(WsTradeEvent {
                    symbol,
                    trades,
                }));
            }
        }
    }

    /// Handle ticker messages
    async fn handle_ticker(
        data: &Value,
        destination: &str,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        // Parse symbol from destination: /v1/ticker/{base}/{quote} or /v1/ticker
        let parts: Vec<&str> = destination.split('/').collect();

        let (base, quote) = if parts.len() >= 4 {
            (parts[parts.len() - 2], parts[parts.len() - 1])
        } else {
            // For /v1/ticker (all tickers), extract from payload
            let payload = data.get("payload").unwrap_or(data);
            let b = payload.get("baseCurrency").and_then(|v| v.as_str()).unwrap_or("");
            let q = payload.get("quoteCurrency").and_then(|v| v.as_str()).unwrap_or("");
            (b, q)
        };

        let symbol = Self::parse_symbol(base, quote);
        let key = format!("ticker:{}", symbol);

        let payload = data.get("payload").unwrap_or(data);
        let timestamp = data.get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: payload.get("high").and_then(|v| Self::parse_decimal_opt(v)),
            low: payload.get("low").and_then(|v| Self::parse_decimal_opt(v)),
            bid: payload.get("bestBid").and_then(|v| Self::parse_decimal_opt(v)),
            bid_volume: None,
            ask: payload.get("bestAsk").and_then(|v| Self::parse_decimal_opt(v)),
            ask_volume: None,
            vwap: None,
            open: payload.get("open").and_then(|v| Self::parse_decimal_opt(v)),
            close: payload.get("lastPrice").and_then(|v| Self::parse_decimal_opt(v)),
            last: payload.get("lastPrice").and_then(|v| Self::parse_decimal_opt(v)),
            previous_close: None,
            change: payload.get("change24h").and_then(|v| Self::parse_decimal_opt(v)),
            percentage: None,
            average: None,
            base_volume: payload.get("volume24h").and_then(|v| Self::parse_decimal_opt(v)),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: data.clone(),
        };

        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&key) {
            let _ = sender.send(WsMessage::Ticker(WsTickerEvent {
                symbol,
                ticker,
            }));
        }
    }

    /// Parse Value to Decimal
    fn parse_decimal(value: &Value) -> Decimal {
        if let Some(s) = value.as_str() {
            s.parse::<Decimal>().unwrap_or_default()
        } else if let Some(f) = value.as_f64() {
            Decimal::from_f64(f).unwrap_or_default()
        } else if let Some(i) = value.as_i64() {
            Decimal::from_i64(i).unwrap_or_default()
        } else {
            Decimal::ZERO
        }
    }

    /// Parse Value to Option<Decimal>
    fn parse_decimal_opt(value: &Value) -> Option<Decimal> {
        if let Some(s) = value.as_str() {
            s.parse::<Decimal>().ok()
        } else if let Some(f) = value.as_f64() {
            Decimal::from_f64(f)
        } else if let Some(i) = value.as_i64() {
            Decimal::from_i64(i)
        } else {
            None
        }
    }
}

impl Default for LatokenWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for LatokenWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        let (ws_stream, _) = connect_async(WS_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: format!("WebSocket connection failed: {}", e),
            })?;

        self.ws_stream = Some(Arc::new(RwLock::new(ws_stream)));

        // Send STOMP CONNECT frame
        self.stomp_connect().await?;

        self.start_message_loop();

        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            // Send STOMP DISCONNECT
            let disconnect_frame = StompFrame::build("DISCONNECT", &[], None);

            let mut ws_guard = ws.write().await;
            let _ = ws_guard.send(Message::Text(disconnect_frame)).await;
            ws_guard
                .close(None)
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to close WebSocket: {}", e),
                })?;
        }
        self.ws_stream = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        self.ws_stream.is_some()
    }

    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("ticker:{}", symbol);

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let (base, quote) = self.format_symbol(symbol);
        let destination = format!("/v1/ticker/{}/{}", base, quote);
        self.stomp_subscribe(&destination).await?;

        Ok(rx)
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("ticker:{}", symbol);
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx.clone());

            let (base, quote) = self.format_symbol(symbol);
            let destination = format!("/v1/ticker/{}/{}", base, quote);
            self.stomp_subscribe(&destination).await?;
        }

        Ok(rx)
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("orderbook:{}", symbol);

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let (base, quote) = self.format_symbol(symbol);
        let destination = format!("/v1/book/{}/{}", base, quote);
        self.stomp_subscribe(&destination).await?;

        Ok(rx)
    }

    async fn watch_order_book_for_symbols(
        &self,
        symbols: &[&str],
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("orderbook:{}", symbol);
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx.clone());

            let (base, quote) = self.format_symbol(symbol);
            let destination = format!("/v1/book/{}/{}", base, quote);
            self.stomp_subscribe(&destination).await?;
        }

        Ok(rx)
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("trades:{}", symbol);

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let (base, quote) = self.format_symbol(symbol);
        let destination = format!("/v1/trade/{}/{}", base, quote);
        self.stomp_subscribe(&destination).await?;

        Ok(rx)
    }

    async fn watch_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("trades:{}", symbol);
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx.clone());

            let (base, quote) = self.format_symbol(symbol);
            let destination = format!("/v1/trade/{}/{}", base, quote);
            self.stomp_subscribe(&destination).await?;
        }

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // LATOKEN does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "LATOKEN WebSocket does not support OHLCV".to_string(),
        })
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        _symbols: &[&str],
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // LATOKEN does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "LATOKEN WebSocket does not support OHLCV".to_string(),
        })
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Private streams require authentication
        Err(CcxtError::NotSupported {
            feature: "LATOKEN WebSocket private streams - authentication not yet implemented".to_string(),
        })
    }

    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Private streams require authentication
        Err(CcxtError::NotSupported {
            feature: "LATOKEN WebSocket private streams - authentication not yet implemented".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latoken_ws_creation() {
        let _ws = LatokenWs::new();
    }

    #[test]
    fn test_format_symbol() {
        let ws = LatokenWs::new();
        assert_eq!(ws.format_symbol("BTC/USDT"), ("btc".to_string(), "usdt".to_string()));
        assert_eq!(ws.format_symbol("ETH/BTC"), ("eth".to_string(), "btc".to_string()));
    }

    #[test]
    fn test_parse_symbol() {
        assert_eq!(LatokenWs::parse_symbol("btc", "usdt"), "BTC/USDT");
        assert_eq!(LatokenWs::parse_symbol("eth", "btc"), "ETH/BTC");
    }

    #[test]
    fn test_stomp_frame_build() {
        let frame = StompFrame::build(
            "CONNECT",
            &[("accept-version", "1.2"), ("heart-beat", "10000,10000")],
            None,
        );
        assert!(frame.starts_with("CONNECT\n"));
        assert!(frame.contains("accept-version:1.2"));
        assert!(frame.ends_with('\0'));
    }

    #[test]
    fn test_stomp_frame_parse() {
        let raw = "MESSAGE\ndestination:/v1/book/btc/usdt\ncontent-type:application/json\n\n{\"test\":true}\0";
        let frame = StompFrame::parse(raw).unwrap();
        assert_eq!(frame.command, "MESSAGE");
        assert_eq!(frame.headers.get("destination"), Some(&"/v1/book/btc/usdt".to_string()));
        assert_eq!(frame.body, "{\"test\":true}");
    }

    #[test]
    fn test_parse_decimal() {
        use serde_json::json;
        assert_eq!(LatokenWs::parse_decimal(&json!("123.45")), Decimal::new(12345, 2));
        assert_eq!(LatokenWs::parse_decimal(&json!(100)), Decimal::from(100));
    }
}
