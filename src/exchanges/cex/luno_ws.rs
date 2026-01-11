//! Luno WebSocket Implementation
//!
//! Luno WebSocket API for real-time market data
//! Base URL: wss://ws.luno.com/api/1/stream/[market_id]
//!
//! Note: Luno requires authentication (API key/secret) even for public data streams

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
    OrderBook, OrderBookEntry, Timeframe, Trade, WsExchange, WsMessage, WsOrderBookEvent,
    WsTradeEvent,
};

const WS_BASE_URL: &str = "wss://ws.luno.com/api/1/stream";

/// Luno WebSocket client
pub struct LunoWs {
    ws_stream: Arc<RwLock<Option<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    api_key: Option<String>,
    api_secret: Option<String>,
    current_market: Arc<RwLock<Option<String>>>,
}

impl LunoWs {
    /// Create a new Luno WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: None,
            api_secret: None,
            current_market: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a new Luno WebSocket client with API credentials
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        Self {
            ws_stream: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: Some(api_key),
            api_secret: Some(api_secret),
            current_market: Arc::new(RwLock::new(None)),
        }
    }

    /// Convert unified symbol to Luno format
    /// BTC/ZAR -> XBTZAR
    fn format_symbol(&self, symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            let base = if parts[0] == "BTC" { "XBT" } else { parts[0] };
            format!("{}{}", base, parts[1])
        } else {
            symbol.replace("/", "")
        }
    }

    /// Convert Luno symbol to unified format
    /// XBTZAR -> BTC/ZAR
    fn parse_symbol(market_id: &str) -> String {
        // Common Luno market pairs
        let common_pairs = [
            ("XBTZAR", "BTC/ZAR"),
            ("ETHZAR", "ETH/ZAR"),
            ("XBTEUR", "BTC/EUR"),
            ("ETHEUR", "ETH/EUR"),
            ("XBTNGN", "BTC/NGN"),
            ("ETHNGN", "ETH/NGN"),
            ("XBTUGX", "BTC/UGX"),
            ("XBTZMW", "BTC/ZMW"),
            ("XBTIDR", "BTC/IDR"),
            ("XBTMYR", "BTC/MYR"),
        ];

        for (luno_symbol, unified) in &common_pairs {
            if market_id == *luno_symbol {
                return unified.to_string();
            }
        }

        // Fallback: try to parse as XBT prefix
        if let Some(quote) = market_id.strip_prefix("XBT") {
            return format!("BTC/{quote}");
        }

        // Generic parsing
        if market_id.len() >= 6 {
            let base = &market_id[0..3];
            let quote = &market_id[3..];
            format!("{base}/{quote}")
        } else {
            market_id.to_string()
        }
    }

    /// Connect to a specific market stream
    async fn connect_to_market(&self, market_id: &str) -> CcxtResult<()> {
        let url = format!("{WS_BASE_URL}/{market_id}");

        let (ws_stream, _) = connect_async(&url)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: format!("WebSocket connection failed: {e}"),
            })?;

        {
            let mut ws_guard = self.ws_stream.write().await;
            *ws_guard = Some(ws_stream);
        }

        // Send authentication if credentials are available
        if let (Some(key), Some(secret)) = (&self.api_key, &self.api_secret) {
            self.authenticate(key, secret).await?;
        }

        self.start_message_loop();

        // Store current market
        {
            let mut current_market = self.current_market.write().await;
            *current_market = Some(market_id.to_string());
        }

        Ok(())
    }

    /// Send authentication message
    async fn authenticate(&self, api_key: &str, api_secret: &str) -> CcxtResult<()> {
        let ws_guard = self.ws_stream.read().await;
        if let Some(ref _ws) = *ws_guard {
            drop(ws_guard);

            let auth_msg = json!({
                "api_key_id": api_key,
                "api_key_secret": api_secret,
            });

            let mut ws_guard = self.ws_stream.write().await;
            if let Some(ref mut ws) = *ws_guard {
                ws.send(Message::Text(auth_msg.to_string()))
                    .await
                    .map_err(|e| CcxtError::NetworkError {
                        url: WS_BASE_URL.to_string(),
                        message: format!("Failed to send authentication: {e}"),
                    })?;
            }
        }
        Ok(())
    }

    /// Internal close method for use with &self
    async fn internal_close(&self) -> CcxtResult<()> {
        let mut ws_guard = self.ws_stream.write().await;
        if let Some(ref mut ws) = *ws_guard {
            ws.close(None).await.map_err(|e| CcxtError::NetworkError {
                url: WS_BASE_URL.to_string(),
                message: format!("Failed to close WebSocket: {e}"),
            })?;
        }
        *ws_guard = None;
        Ok(())
    }

    /// Start message processing loop
    fn start_message_loop(&self) {
        let ws_stream = self.ws_stream.clone();
        let subscriptions = self.subscriptions.clone();
        let orderbook_cache = self.orderbook_cache.clone();
        let current_market = self.current_market.clone();

        tokio::spawn(async move {
            loop {
                let msg = {
                    let mut ws_guard = ws_stream.write().await;
                    if let Some(ref mut ws) = *ws_guard {
                        ws.next().await
                    } else {
                        break;
                    }
                };

                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(data) = serde_json::from_str::<Value>(&text) {
                            Self::handle_message_static(
                                &data,
                                &subscriptions,
                                &orderbook_cache,
                                &current_market,
                            )
                            .await;
                        }
                    },
                    Some(Ok(Message::Ping(data))) => {
                        let mut ws_guard = ws_stream.write().await;
                        if let Some(ref mut ws) = *ws_guard {
                            let _ = ws.send(Message::Pong(data)).await;
                        }
                    },
                    Some(Ok(Message::Close(_))) => break,
                    Some(Err(_)) => break,
                    None => break,
                    _ => {},
                }
            }
        });
    }

    /// Handle incoming WebSocket messages
    async fn handle_message_static(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
        current_market: &Arc<RwLock<Option<String>>>,
    ) {
        // Get the current market ID
        let market_id = {
            let market = current_market.read().await;
            market.clone()
        };

        let market_id = match market_id {
            Some(id) => id,
            None => return,
        };

        let symbol = Self::parse_symbol(&market_id);

        // Check if this is a snapshot (initial message) or update
        if data.get("sequence").is_some() {
            // Handle orderbook snapshot or update
            if data.get("asks").is_some() || data.get("bids").is_some() {
                Self::handle_orderbook(data, subscriptions, orderbook_cache, &symbol).await;
            }

            // Handle trade updates
            if let Some(trade_updates) = data.get("trade_updates") {
                if trade_updates
                    .as_array()
                    .map(|a| !a.is_empty())
                    .unwrap_or(false)
                {
                    Self::handle_trades(data, subscriptions, &symbol).await;
                }
            }
        }
    }

    /// Handle orderbook message
    async fn handle_orderbook(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
        symbol: &str,
    ) {
        let key = format!("orderbook:{symbol}");

        // Determine if this is a snapshot (has asks/bids arrays) or update (has create_update/delete_update)
        let is_snapshot = data.get("asks").is_some() && data.get("bids").is_some();

        let mut bids: Vec<OrderBookEntry> = Vec::new();
        let mut asks: Vec<OrderBookEntry> = Vec::new();

        if is_snapshot {
            // Parse initial snapshot
            if let Some(asks_arr) = data.get("asks").and_then(|v| v.as_array()) {
                for ask in asks_arr {
                    if let (Some(price_str), Some(volume_str)) = (
                        ask.get("price").and_then(|v| v.as_str()),
                        ask.get("volume").and_then(|v| v.as_str()),
                    ) {
                        if let (Ok(price), Ok(amount)) =
                            (price_str.parse::<Decimal>(), volume_str.parse::<Decimal>())
                        {
                            asks.push(OrderBookEntry { price, amount });
                        }
                    }
                }
            }

            if let Some(bids_arr) = data.get("bids").and_then(|v| v.as_array()) {
                for bid in bids_arr {
                    if let (Some(price_str), Some(volume_str)) = (
                        bid.get("price").and_then(|v| v.as_str()),
                        bid.get("volume").and_then(|v| v.as_str()),
                    ) {
                        if let (Ok(price), Ok(amount)) =
                            (price_str.parse::<Decimal>(), volume_str.parse::<Decimal>())
                        {
                            bids.push(OrderBookEntry { price, amount });
                        }
                    }
                }
            }

            // Sort bids descending, asks ascending
            bids.sort_by(|a, b| b.price.cmp(&a.price));
            asks.sort_by(|a, b| a.price.cmp(&b.price));

            let timestamp = data.get("timestamp").and_then(|v| v.as_i64());

            let orderbook = OrderBook {
                symbol: symbol.to_string(),
                bids,
                asks,
                checksum: None,
                timestamp,
                datetime: None,
                nonce: None,
            };

            // Update cache
            {
                let mut cache = orderbook_cache.write().await;
                cache.insert(symbol.to_string(), orderbook.clone());
            }

            let subs = subscriptions.read().await;
            if let Some(sender) = subs.get(&key) {
                let _ = sender.send(WsMessage::OrderBook(WsOrderBookEvent {
                    symbol: symbol.to_string(),
                    order_book: orderbook,
                    is_snapshot: true,
                }));
            }
        } else {
            // Handle incremental updates
            let mut cached_book = {
                let cache = orderbook_cache.read().await;
                cache.get(symbol).cloned()
            };

            if let Some(ref mut book) = cached_book {
                // Handle create_update
                if let Some(create) = data.get("create_update") {
                    if let (Some(_order_id), Some(price_str), Some(volume_str), Some(type_str)) = (
                        create.get("order_id").and_then(|v| v.as_str()),
                        create.get("price").and_then(|v| v.as_str()),
                        create.get("volume").and_then(|v| v.as_str()),
                        create.get("type").and_then(|v| v.as_str()),
                    ) {
                        if let (Ok(price), Ok(amount)) =
                            (price_str.parse::<Decimal>(), volume_str.parse::<Decimal>())
                        {
                            let entry = OrderBookEntry { price, amount };
                            if type_str == "BID" {
                                Self::update_orderbook_side(&mut book.bids, entry, true);
                            } else if type_str == "ASK" {
                                Self::update_orderbook_side(&mut book.asks, entry, false);
                            }
                        }
                    }
                }

                // Handle delete_update
                if let Some(delete) = data.get("delete_update") {
                    if let (Some(_order_id), Some(price_str)) = (
                        delete.get("order_id").and_then(|v| v.as_str()),
                        delete.get("price").and_then(|v| v.as_str()),
                    ) {
                        if let Ok(price) = price_str.parse::<Decimal>() {
                            // Remove from both sides (we don't know which side it was on)
                            book.bids.retain(|e| e.price != price);
                            book.asks.retain(|e| e.price != price);

                            // Re-sort
                            book.bids.sort_by(|a, b| b.price.cmp(&a.price));
                            book.asks.sort_by(|a, b| a.price.cmp(&b.price));
                        }
                    }
                }

                // Update cache
                {
                    let mut cache = orderbook_cache.write().await;
                    cache.insert(symbol.to_string(), book.clone());
                }

                let subs = subscriptions.read().await;
                if let Some(sender) = subs.get(&key) {
                    let _ = sender.send(WsMessage::OrderBook(WsOrderBookEvent {
                        symbol: symbol.to_string(),
                        order_book: book.clone(),
                        is_snapshot: false,
                    }));
                }
            }
        }
    }

    /// Update orderbook side with new entry
    fn update_orderbook_side(
        book_side: &mut Vec<OrderBookEntry>,
        entry: OrderBookEntry,
        is_bid: bool,
    ) {
        // Find existing entry at this price
        let pos = book_side.iter().position(|e| e.price == entry.price);

        if entry.amount.is_zero() {
            // Remove entry
            if let Some(idx) = pos {
                book_side.remove(idx);
            }
        } else if let Some(idx) = pos {
            // Update existing entry
            book_side[idx].amount = entry.amount;
        } else {
            // Insert new entry
            book_side.push(entry);
        }

        // Re-sort
        if is_bid {
            book_side.sort_by(|a, b| b.price.cmp(&a.price));
        } else {
            book_side.sort_by(|a, b| a.price.cmp(&b.price));
        }
    }

    /// Handle trade updates
    async fn handle_trades(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        symbol: &str,
    ) {
        let key = format!("trades:{symbol}");

        if let Some(trade_updates) = data.get("trade_updates").and_then(|v| v.as_array()) {
            let mut trades = Vec::new();

            for trade_data in trade_updates {
                let base = trade_data
                    .get("base")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();
                let counter = trade_data
                    .get("counter")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();

                // Calculate price from counter/base
                let price = if !base.is_zero() {
                    counter / base
                } else {
                    Decimal::ZERO
                };

                let timestamp = trade_data.get("timestamp").and_then(|v| v.as_i64());

                let trade = Trade::new(
                    trade_data
                        .get("order_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    symbol.to_string(),
                    price,
                    base,
                );

                let trade = if let Some(ts) = timestamp {
                    trade.with_timestamp(ts)
                } else {
                    trade
                };

                // Luno doesn't provide side in trade updates, infer from maker/taker if available
                let trade = if let Some(_maker_order_id) =
                    trade_data.get("maker_order_id").and_then(|v| v.as_str())
                {
                    // Could potentially infer side, but Luno doesn't provide clear indication
                    trade
                } else {
                    trade
                };

                trades.push(trade);
            }

            if !trades.is_empty() {
                let subs = subscriptions.read().await;
                if let Some(sender) = subs.get(&key) {
                    let _ = sender.send(WsMessage::Trade(WsTradeEvent {
                        symbol: symbol.to_string(),
                        trades,
                    }));
                }
            }
        }
    }
}

impl Default for LunoWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for LunoWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Luno requires connecting to a specific market stream
        // Connection will be established when subscribing to a market
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        let mut ws_guard = self.ws_stream.write().await;
        if let Some(ref mut ws) = *ws_guard {
            ws.close(None).await.map_err(|e| CcxtError::NetworkError {
                url: WS_BASE_URL.to_string(),
                message: format!("Failed to close WebSocket: {e}"),
            })?;
        }
        *ws_guard = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        let ws_guard = self.ws_stream.read().await;
        ws_guard.is_some()
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("orderbook:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);

        // Check if we need to connect to a new market
        let needs_connection = {
            let current_market = self.current_market.read().await;
            current_market.as_ref() != Some(&market_id)
        };

        if needs_connection {
            // Close existing connection if any
            {
                let ws_guard = self.ws_stream.read().await;
                if ws_guard.is_some() {
                    drop(ws_guard);
                    self.internal_close().await?;
                }
            }
            // Connect to the market stream
            self.connect_to_market(&market_id).await?;
        }

        Ok(rx)
    }

    async fn watch_order_book_for_symbols(
        &self,
        symbols: &[&str],
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Luno only supports one market per connection
        // We'll connect to the first symbol
        if symbols.is_empty() {
            return Err(CcxtError::BadRequest {
                message: "At least one symbol is required".to_string(),
            });
        }

        // For now, only watch the first symbol due to Luno's one-market-per-connection limitation
        self.watch_order_book(symbols[0], None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("trades:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);

        // Check if we need to connect to a new market
        let needs_connection = {
            let current_market = self.current_market.read().await;
            current_market.as_ref() != Some(&market_id)
        };

        if needs_connection {
            // Close existing connection if any
            {
                let ws_guard = self.ws_stream.read().await;
                if ws_guard.is_some() {
                    drop(ws_guard);
                    self.internal_close().await?;
                }
            }
            // Connect to the market stream
            self.connect_to_market(&market_id).await?;
        }

        Ok(rx)
    }

    async fn watch_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Luno only supports one market per connection
        if symbols.is_empty() {
            return Err(CcxtError::BadRequest {
                message: "At least one symbol is required".to_string(),
            });
        }

        // For now, only watch the first symbol
        self.watch_trades(symbols[0]).await
    }

    async fn watch_ticker(&self, _symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Luno does not support ticker via WebSocket
        Err(CcxtError::NotSupported {
            feature: "Luno WebSocket does not support ticker".to_string(),
        })
    }

    async fn watch_tickers(
        &self,
        _symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Luno does not support ticker via WebSocket
        Err(CcxtError::NotSupported {
            feature: "Luno WebSocket does not support tickers".to_string(),
        })
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Luno does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "Luno WebSocket does not support OHLCV".to_string(),
        })
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        _symbols: &[&str],
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Luno does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "Luno WebSocket does not support OHLCV".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_luno_ws_creation() {
        let _ws = LunoWs::new();
    }

    #[test]
    fn test_luno_ws_with_credentials() {
        let _ws = LunoWs::with_credentials("test_key".to_string(), "test_secret".to_string());
    }

    #[test]
    fn test_format_symbol() {
        let ws = LunoWs::new();
        assert_eq!(ws.format_symbol("BTC/ZAR"), "XBTZAR");
        assert_eq!(ws.format_symbol("ETH/ZAR"), "ETHZAR");
        assert_eq!(ws.format_symbol("BTC/EUR"), "XBTEUR");
    }

    #[test]
    fn test_parse_symbol() {
        assert_eq!(LunoWs::parse_symbol("XBTZAR"), "BTC/ZAR");
        assert_eq!(LunoWs::parse_symbol("ETHZAR"), "ETH/ZAR");
        assert_eq!(LunoWs::parse_symbol("XBTEUR"), "BTC/EUR");
    }
}
