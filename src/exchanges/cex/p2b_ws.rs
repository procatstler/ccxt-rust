//! P2B WebSocket Implementation
//!
//! P2B WebSocket API for real-time market data
//! URL: wss://apiws.p2pb2b.com/

#![allow(dead_code)]

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

use rust_decimal::Decimal;

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV, WsExchange, WsMessage,
    WsTickerEvent, WsTradeEvent, WsOrderBookEvent, WsOhlcvEvent,
};

const WS_URL: &str = "wss://apiws.p2pb2b.com/";

/// P2B WebSocket client
pub struct P2bWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
}

impl P2bWs {
    /// Create a new P2B WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Convert unified symbol to P2B format
    /// BTC/USDT -> BTC_USDT
    fn format_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Convert P2B symbol to unified format
    /// BTC_USDT -> BTC/USDT
    fn parse_symbol(&self, symbol: &str) -> String {
        symbol.replace("_", "/")
    }

    /// Convert timeframe to P2B interval (seconds)
    fn timeframe_to_interval(&self, timeframe: Timeframe) -> CcxtResult<i64> {
        match timeframe {
            Timeframe::Minute15 => Ok(900),
            Timeframe::Minute30 => Ok(1800),
            Timeframe::Hour1 => Ok(3600),
            Timeframe::Day1 => Ok(86400),
            _ => Err(CcxtError::NotSupported {
                feature: format!("P2B WebSocket does not support timeframe: {timeframe:?}"),
            }),
        }
    }

    /// Send a subscription message
    async fn subscribe(&self, method: &str, params: Value) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let msg = json!({
                "method": method,
                "params": params,
                "id": chrono::Utc::now().timestamp_millis(),
            });

            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(msg.to_string()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to send subscribe: {e}"),
                })?;
        }
        Ok(())
    }

    /// Start ping loop to keep connection alive
    fn start_ping_loop(&self) {
        let ws_stream = self.ws_stream.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));

            loop {
                interval.tick().await;

                if let Some(ws) = &ws_stream {
                    let ping_msg = json!({
                        "method": "server.ping",
                        "params": [],
                        "id": chrono::Utc::now().timestamp_millis(),
                    });

                    let mut ws_guard = ws.write().await;
                    let _ = ws_guard.send(Message::Text(ping_msg.to_string())).await;
                } else {
                    break;
                }
            }
        });
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
                            if let Ok(data) = serde_json::from_str::<Value>(&text) {
                                Self::handle_message_static(&data, &subscriptions, &orderbook_cache).await;
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

    /// Handle incoming WebSocket messages
    async fn handle_message_static(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        // Check for pong response
        if let Some(result) = data.get("result").and_then(|v| v.as_str()) {
            if result == "pong" {
                return; // Ping-pong handled
            }
        }

        let method = data.get("method").and_then(|v| v.as_str()).unwrap_or("");

        match method {
            "state.update" | "price.update" => {
                Self::handle_ticker(data, subscriptions).await;
            }
            "deals.update" => {
                Self::handle_trades(data, subscriptions).await;
            }
            "depth.update" => {
                Self::handle_orderbook(data, subscriptions, orderbook_cache).await;
            }
            "kline.update" => {
                Self::handle_ohlcv(data, subscriptions).await;
            }
            _ => {}
        }
    }

    /// Handle ticker message
    async fn handle_ticker(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let params = data.get("params").and_then(|v| v.as_array());
        if params.is_none() {
            return;
        }
        let params = params.unwrap();

        let market_id = params.first().and_then(|v| v.as_str()).unwrap_or("");
        let symbol = Self::parse_symbol_static(market_id);

        let method = data.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let _method_prefix = method.split('.').next().unwrap_or("");
        let key = format!("ticker:{symbol}");

        let ticker = if method == "price.update" {
            // Price update: just last price
            let last_price = params.get(1).and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Decimal>().ok());

            Ticker {
                symbol: symbol.clone(),
                last: last_price,
                timestamp: Some(chrono::Utc::now().timestamp_millis()),
                ..Default::default()
            }
        } else {
            // State update: full ticker data
            let ticker_data = params.get(1).and_then(|v| v.as_object());
            if ticker_data.is_none() {
                return;
            }
            let ticker_data = ticker_data.unwrap();

            Ticker {
                symbol: symbol.clone(),
                high: ticker_data.get("high").and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
                low: ticker_data.get("low").and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
                last: ticker_data.get("last").and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
                open: ticker_data.get("open").and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
                close: ticker_data.get("close").and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
                base_volume: ticker_data.get("volume").and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
                quote_volume: ticker_data.get("deal").and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
                timestamp: Some(chrono::Utc::now().timestamp_millis()),
                ..Default::default()
            }
        };

        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&key) {
            let _ = sender.send(WsMessage::Ticker(WsTickerEvent {
                symbol: symbol.clone(),
                ticker,
            }));
        }
    }

    /// Handle trades message
    async fn handle_trades(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let params = data.get("params").and_then(|v| v.as_array());
        if params.is_none() {
            return;
        }
        let params = params.unwrap();

        let market_id = params.first().and_then(|v| v.as_str()).unwrap_or("");
        let symbol = Self::parse_symbol_static(market_id);
        let key = format!("trades:{symbol}");

        if let Some(trades) = params.get(1).and_then(|v| v.as_array()) {
            for trade_data in trades {
                let id = trade_data.get("id")
                    .and_then(|v| v.as_i64())
                    .map(|i| i.to_string())
                    .unwrap_or_default();

                let price = trade_data.get("price").and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();

                let amount = trade_data.get("amount").and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();

                let time = trade_data.get("time").and_then(|v| v.as_f64())
                    .map(|t| (t * 1000.0) as i64); // Convert to milliseconds

                let side = trade_data.get("type").and_then(|v| v.as_str())
                    .unwrap_or("buy");

                let mut trade = Trade::new(id, symbol.clone(), price, amount);

                if let Some(ts) = time {
                    trade = trade.with_timestamp(ts);
                }

                trade = trade.with_side(side);

                let subs = subscriptions.read().await;
                if let Some(sender) = subs.get(&key) {
                    let _ = sender.send(WsMessage::Trade(WsTradeEvent {
                        symbol: symbol.clone(),
                        trades: vec![trade],
                    }));
                }
            }
        }
    }

    /// Handle orderbook message
    async fn handle_orderbook(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        let params = data.get("params").and_then(|v| v.as_array());
        if params.is_none() {
            return;
        }
        let params = params.unwrap();

        // params[0]: true = snapshot, false = delta
        let is_snapshot = params.first().and_then(|v| v.as_bool()).unwrap_or(false);

        // params[2]: market_id
        let market_id = params.get(2).and_then(|v| v.as_str()).unwrap_or("");
        let symbol = Self::parse_symbol_static(market_id);
        let key = format!("orderbook:{symbol}");

        // params[1]: orderbook data
        let ob_data = params.get(1).and_then(|v| v.as_object());
        if ob_data.is_none() {
            return;
        }
        let ob_data = ob_data.unwrap();

        let mut bids: Vec<OrderBookEntry> = Vec::new();
        let mut asks: Vec<OrderBookEntry> = Vec::new();

        if let Some(asks_arr) = ob_data.get("asks").and_then(|v| v.as_array()) {
            for entry in asks_arr {
                if let Some(arr) = entry.as_array() {
                    let price: Decimal = arr.first().and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok()).unwrap_or_default();
                    let amount: Decimal = arr.get(1).and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok()).unwrap_or_default();

                    asks.push(OrderBookEntry { price, amount });
                }
            }
        }

        if let Some(bids_arr) = ob_data.get("bids").and_then(|v| v.as_array()) {
            for entry in bids_arr {
                if let Some(arr) = entry.as_array() {
                    let price: Decimal = arr.first().and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok()).unwrap_or_default();
                    let amount: Decimal = arr.get(1).and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok()).unwrap_or_default();

                    bids.push(OrderBookEntry { price, amount });
                }
            }
        }

        // Sort bids descending, asks ascending
        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

        let orderbook = if is_snapshot {
            OrderBook {
                symbol: symbol.clone(),
                bids,
                asks,
                timestamp: Some(chrono::Utc::now().timestamp_millis()),
                datetime: None,
                nonce: None,
            }
        } else {
            // For incremental updates, merge with cache
            let cache = orderbook_cache.read().await;
            if let Some(cached) = cache.get(&symbol) {
                let mut new_ob = cached.clone();
                Self::apply_deltas(&mut new_ob.bids, &bids, true);
                Self::apply_deltas(&mut new_ob.asks, &asks, false);
                new_ob.timestamp = Some(chrono::Utc::now().timestamp_millis());
                new_ob
            } else {
                OrderBook {
                    symbol: symbol.clone(),
                    bids,
                    asks,
                    timestamp: Some(chrono::Utc::now().timestamp_millis()),
                    datetime: None,
                    nonce: None,
                }
            }
        };

        // Update cache
        {
            let mut cache = orderbook_cache.write().await;
            cache.insert(symbol.clone(), orderbook.clone());
        }

        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&key) {
            let _ = sender.send(WsMessage::OrderBook(WsOrderBookEvent {
                symbol: symbol.clone(),
                order_book: orderbook,
                is_snapshot,
            }));
        }
    }

    /// Handle OHLCV message
    async fn handle_ohlcv(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let params = data.get("params").and_then(|v| v.as_array());
        if params.is_none() {
            return;
        }
        let params = params.unwrap();

        // params[0] is the kline data array
        if let Some(kline) = params.first().and_then(|v| v.as_array()) {
            // [timestamp, open, close, high, low, vol_stock, vol_money, market]
            let timestamp = kline.first().and_then(|v| v.as_i64())
                .map(|t| t * 1000) // Convert to milliseconds
                .unwrap_or(0);

            let open = kline.get(1).and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Decimal>().ok())
                .unwrap_or_default();

            let close = kline.get(2).and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Decimal>().ok())
                .unwrap_or_default();

            let high = kline.get(3).and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Decimal>().ok())
                .unwrap_or_default();

            let low = kline.get(4).and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Decimal>().ok())
                .unwrap_or_default();

            let volume = kline.get(5).and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Decimal>().ok())
                .unwrap_or_default();

            let market_id = kline.get(7).and_then(|v| v.as_str()).unwrap_or("");
            let symbol = Self::parse_symbol_static(market_id);

            let ohlcv = OHLCV::new(timestamp, open, high, low, close, volume);

            // Determine timeframe from subscription
            // For now, we'll need to track this in subscriptions
            let key = format!("ohlcv:{symbol}");

            let subs = subscriptions.read().await;
            if let Some(sender) = subs.get(&key) {
                let _ = sender.send(WsMessage::Ohlcv(WsOhlcvEvent {
                    symbol: symbol.clone(),
                    timeframe: Timeframe::Minute15, // Default, should be tracked
                    ohlcv,
                }));
            }
        }
    }

    /// Apply delta updates to orderbook side
    fn apply_deltas(book_side: &mut Vec<OrderBookEntry>, updates: &Vec<OrderBookEntry>, is_bid: bool) {
        for update in updates {
            // Find existing entry at this price
            let pos = book_side.iter().position(|e| e.price == update.price);

            if update.amount.is_zero() {
                // Remove entry
                if let Some(idx) = pos {
                    book_side.remove(idx);
                }
            } else if let Some(idx) = pos {
                // Update existing entry
                book_side[idx].amount = update.amount;
            } else {
                // Insert new entry
                book_side.push(update.clone());
            }
        }

        // Re-sort
        if is_bid {
            book_side.sort_by(|a, b| b.price.cmp(&a.price));
        } else {
            book_side.sort_by(|a, b| a.price.cmp(&b.price));
        }
    }

    /// Static helper for parsing symbol
    fn parse_symbol_static(symbol: &str) -> String {
        symbol.replace("_", "/")
    }
}

impl Default for P2bWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for P2bWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        let (ws_stream, _) = connect_async(WS_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: format!("WebSocket connection failed: {e}"),
            })?;

        self.ws_stream = Some(Arc::new(RwLock::new(ws_stream)));
        self.start_message_loop();
        self.start_ping_loop();

        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let mut ws_guard = ws.write().await;
            ws_guard
                .close(None)
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to close WebSocket: {e}"),
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
        let key = format!("ticker:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        self.subscribe("state.subscribe", json!([market_id])).await?;

        Ok(rx)
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        let mut market_ids = Vec::new();
        for symbol in symbols {
            let key = format!("ticker:{symbol}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }
            market_ids.push(self.format_symbol(symbol));
        }

        self.subscribe("state.subscribe", json!(market_ids)).await?;

        Ok(rx)
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("orderbook:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        let depth_limit = limit.unwrap_or(100);
        let interval = "0.001"; // Default price precision interval

        self.subscribe("depth.subscribe", json!([market_id, depth_limit, interval])).await?;

        Ok(rx)
    }

    async fn watch_order_book_for_symbols(
        &self,
        _symbols: &[&str],
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "P2B WebSocket does not support watching multiple orderbooks at once".to_string(),
        })
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("trades:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        self.subscribe("deals.subscribe", json!([market_id])).await?;

        Ok(rx)
    }

    async fn watch_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        let mut market_ids = Vec::new();
        for symbol in symbols {
            let key = format!("trades:{symbol}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }
            market_ids.push(self.format_symbol(symbol));
        }

        self.subscribe("deals.subscribe", json!(market_ids)).await?;

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("ohlcv:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        let interval = self.timeframe_to_interval(timeframe)?;

        self.subscribe("kline.subscribe", json!([market_id, interval])).await?;

        Ok(rx)
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        _symbols: &[&str],
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "P2B WebSocket does not support watching multiple OHLCV streams at once".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_p2b_ws_creation() {
        let _ws = P2bWs::new();
    }

    #[test]
    fn test_format_symbol() {
        let ws = P2bWs::new();
        assert_eq!(ws.format_symbol("BTC/USDT"), "BTC_USDT");
        assert_eq!(ws.format_symbol("ETH/BTC"), "ETH_BTC");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = P2bWs::new();
        assert_eq!(ws.parse_symbol("BTC_USDT"), "BTC/USDT");
        assert_eq!(ws.parse_symbol("ETH_BTC"), "ETH/BTC");
    }

    #[test]
    fn test_timeframe_conversion() {
        let ws = P2bWs::new();
        assert_eq!(ws.timeframe_to_interval(Timeframe::Minute15).unwrap(), 900);
        assert_eq!(ws.timeframe_to_interval(Timeframe::Minute30).unwrap(), 1800);
        assert_eq!(ws.timeframe_to_interval(Timeframe::Hour1).unwrap(), 3600);
        assert_eq!(ws.timeframe_to_interval(Timeframe::Day1).unwrap(), 86400);
    }
}
