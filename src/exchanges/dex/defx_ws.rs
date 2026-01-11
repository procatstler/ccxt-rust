//! Defx WebSocket Implementation
//!
//! Defx DEX exchange WebSocket implementation for derivatives trading
//! API Documentation: <https://api-docs.defx.com/>

#![allow(dead_code)]

use async_trait::async_trait;
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
    Ticker, Timeframe, Trade, OHLCV, WsBalanceEvent, WsExchange, WsMessage,
    WsOrderBookEvent, WsOrderEvent, WsOhlcvEvent, WsTickerEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_URL: &str = "wss://api.defx.com/ws/v1";

// WebSocket message structures
#[derive(Debug, Clone, Deserialize, Serialize)]
struct DefxWsMessage {
    #[serde(rename = "type")]
    #[serde(default)]
    msg_type: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(rename = "reqId")]
    #[serde(default)]
    req_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DefxWsTicker {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    price_change: Option<String>,
    #[serde(default)]
    price_change_percent: Option<String>,
    #[serde(default)]
    last_price: Option<String>,
    #[serde(default)]
    last_qty: Option<String>,
    #[serde(default)]
    best_bid_price: Option<String>,
    #[serde(default)]
    best_bid_qty: Option<String>,
    #[serde(default)]
    best_ask_price: Option<String>,
    #[serde(default)]
    best_ask_qty: Option<String>,
    #[serde(default)]
    open_price: Option<String>,
    #[serde(default)]
    high_price: Option<String>,
    #[serde(default)]
    low_price: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    quote_volume: Option<String>,
    #[serde(default)]
    close_time: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DefxWsOrderBook {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    last_update_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DefxWsTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    qty: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    is_buyer_maker: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DefxWsKline {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    interval: Option<String>,
    #[serde(default)]
    open_time: Option<i64>,
    #[serde(default)]
    close_time: Option<i64>,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DefxWsOrder {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    filled_quantity: Option<String>,
    #[serde(default)]
    remaining_quantity: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    created_at: Option<i64>,
    #[serde(default)]
    updated_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DefxWsBalance {
    #[serde(default)]
    asset: Option<String>,
    #[serde(default)]
    free: Option<String>,
    #[serde(default)]
    locked: Option<String>,
    #[serde(default)]
    total: Option<String>,
}

/// Defx WebSocket client
pub struct DefxWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl DefxWs {
    /// Create a new Defx WebSocket client
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

    /// Convert unified symbol to exchange format (BTC/USDT -> BTC_USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Convert exchange symbol to unified format (BTC_USDT -> BTC/USDT)
    fn to_unified_symbol(market_id: &str) -> String {
        market_id.replace("_", "/")
    }

    /// Format timeframe to exchange interval
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour8 => "8h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Day3 => "3d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1M",
            _ => "1h",
        }
    }

    /// Generate authentication signature
    fn generate_auth_signature(secret: &str, timestamp: i64, nonce: &str) -> String {
        let payload = format!("{timestamp}{nonce}");
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(payload.as_bytes());
        let result = mac.finalize();
        hex::encode(result.into_bytes())
    }

    /// Parse ticker data
    fn parse_ticker(data: &DefxWsTicker, symbol: &str) -> Ticker {
        let last = data.last_price.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let open = data.open_price.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let high = data.high_price.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let low = data.low_price.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let bid = data.best_bid_price.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let bid_volume = data.best_bid_qty.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let ask = data.best_ask_price.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let ask_volume = data.best_ask_qty.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let base_volume = data.volume.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let quote_volume = data.quote_volume.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let change = data.price_change.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let percentage = data.price_change_percent.as_ref().and_then(|s| Decimal::from_str(s).ok());

        let timestamp = data.close_time.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()),
            high,
            low,
            bid,
            bid_volume,
            ask,
            ask_volume,
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
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order book data
    fn parse_order_book(data: &DefxWsOrderBook, symbol: &str) -> OrderBook {
        let bids: Vec<OrderBookEntry> = data.bids.iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    let price = Decimal::from_str(&entry[0]).ok()?;
                    let amount = Decimal::from_str(&entry[1]).ok()?;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    let price = Decimal::from_str(&entry[0]).ok()?;
                    let amount = Decimal::from_str(&entry[1]).ok()?;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
            })
            .collect();

        let timestamp = Utc::now().timestamp_millis();

        OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()),
            nonce: data.last_update_id,
            bids,
            asks,
        }
    }

    /// Parse trade data
    fn parse_trade(data: &DefxWsTrade, symbol: &str) -> Trade {
        let side = match data.side.as_deref() {
            Some("BUY") | Some("buy") => Some("buy".to_string()),
            Some("SELL") | Some("sell") => Some("sell".to_string()),
            _ => {
                if data.is_buyer_maker.unwrap_or(false) {
                    Some("sell".to_string())
                } else {
                    Some("buy".to_string())
                }
            }
        };

        let price = data.price.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let amount = data.qty.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let cost = Some(price * amount);

        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());

        Trade {
            id: data.id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()),
            symbol: symbol.to_string(),
            trade_type: None,
            side,
            taker_or_maker: None,
            price,
            amount,
            cost,
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse OHLCV data
    fn parse_ohlcv(data: &DefxWsKline) -> Option<OHLCV> {
        Some(OHLCV {
            timestamp: data.open_time?,
            open: Decimal::from_str(data.open.as_ref()?).ok()?,
            high: Decimal::from_str(data.high.as_ref()?).ok()?,
            low: Decimal::from_str(data.low.as_ref()?).ok()?,
            close: Decimal::from_str(data.close.as_ref()?).ok()?,
            volume: Decimal::from_str(data.volume.as_ref()?).ok()?,
        })
    }

    /// Parse order data
    fn parse_order(data: &DefxWsOrder, symbol: &str) -> Order {
        let side = match data.side.as_deref() {
            Some("BUY") | Some("buy") => OrderSide::Buy,
            Some("SELL") | Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") | Some("limit") => OrderType::Limit,
            Some("MARKET") | Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let status = match data.status.as_deref() {
            Some("NEW") | Some("OPEN") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("CANCELED") | Some("CANCELLED") => OrderStatus::Canceled,
            Some("EXPIRED") => OrderStatus::Expired,
            Some("REJECTED") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let amount = data.quantity.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let price = data.price.as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let filled = data.filled_quantity.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let remaining = data.remaining_quantity.as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: data.created_at,
            datetime: data.created_at.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: data.updated_at,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled,
            remaining,
            stop_price: None,
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
    fn parse_balance(data: &DefxWsBalance) -> Option<(String, Balance)> {
        let currency = data.asset.clone()?;
        let free = data.free.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let used = data.locked.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let total = match (free, used) {
            (Some(f), Some(u)) => Some(f + u),
            _ => data.total.as_ref().and_then(|s| Decimal::from_str(s).ok()),
        };

        Some((currency, Balance {
            free,
            used,
            total,
            debt: None,
        }))
    }

    /// Process WebSocket message
    fn process_message(msg: &str, event_tx: &mpsc::UnboundedSender<WsMessage>) -> CcxtResult<()> {
        let json: serde_json::Value = serde_json::from_str(msg)?;

        // Try to get type or channel
        let msg_type = json.get("type").and_then(|v| v.as_str());
        let channel = json.get("channel").and_then(|v| v.as_str());
        let data = json.get("data");

        // Determine message type
        let message_type = msg_type.or(channel);

        if let Some(data_val) = data {
            match message_type {
                Some("ticker") => {
                    if let Ok(ticker_data) = serde_json::from_value::<DefxWsTicker>(data_val.clone()) {
                        let symbol = ticker_data.symbol.as_ref()
                            .map(|s| Self::to_unified_symbol(s))
                            .unwrap_or_default();
                        let ticker = Self::parse_ticker(&ticker_data, &symbol);
                        let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                            symbol,
                            ticker,
                        }));
                    }
                }
                Some("depth") | Some("orderbook") => {
                    if let Ok(ob_data) = serde_json::from_value::<DefxWsOrderBook>(data_val.clone()) {
                        let symbol = ob_data.symbol.as_ref()
                            .map(|s| Self::to_unified_symbol(s))
                            .unwrap_or_default();
                        let order_book = Self::parse_order_book(&ob_data, &symbol);
                        let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                            symbol,
                            order_book,
                            is_snapshot: true,
                        }));
                    }
                }
                Some("trade") | Some("trades") => {
                    // Try single trade first
                    if let Ok(trade_data) = serde_json::from_value::<DefxWsTrade>(data_val.clone()) {
                        let symbol = trade_data.symbol.as_ref()
                            .map(|s| Self::to_unified_symbol(s))
                            .unwrap_or_default();
                        let trade = Self::parse_trade(&trade_data, &symbol);
                        let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                            symbol,
                            trades: vec![trade],
                        }));
                    } else if let Ok(trades) = serde_json::from_value::<Vec<DefxWsTrade>>(data_val.clone()) {
                        if let Some(first) = trades.first() {
                            let symbol = first.symbol.as_ref()
                                .map(|s| Self::to_unified_symbol(s))
                                .unwrap_or_default();
                            let parsed_trades: Vec<Trade> = trades.iter()
                                .map(|t| Self::parse_trade(t, &symbol))
                                .collect();
                            let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                                symbol,
                                trades: parsed_trades,
                            }));
                        }
                    }
                }
                Some("kline") | Some("candle") => {
                    if let Ok(kline_data) = serde_json::from_value::<DefxWsKline>(data_val.clone()) {
                        let symbol = kline_data.symbol.as_ref()
                            .map(|s| Self::to_unified_symbol(s))
                            .unwrap_or_default();
                        if let Some(ohlcv) = Self::parse_ohlcv(&kline_data) {
                            let _ = event_tx.send(WsMessage::Ohlcv(WsOhlcvEvent {
                                symbol,
                                timeframe: Timeframe::Hour1, // Default
                                ohlcv,
                            }));
                        }
                    }
                }
                Some("order") | Some("orderUpdate") => {
                    if let Ok(order_data) = serde_json::from_value::<DefxWsOrder>(data_val.clone()) {
                        let symbol = order_data.symbol.as_ref()
                            .map(|s| Self::to_unified_symbol(s))
                            .unwrap_or_default();
                        let order = Self::parse_order(&order_data, &symbol);
                        let _ = event_tx.send(WsMessage::Order(WsOrderEvent {
                            order,
                        }));
                    }
                }
                Some("balance") | Some("balanceUpdate") => {
                    if let Ok(balance_data) = serde_json::from_value::<Vec<DefxWsBalance>>(data_val.clone()) {
                        let mut balances = Balances::new();
                        for b in &balance_data {
                            if let Some((currency, balance)) = Self::parse_balance(b) {
                                balances.add(&currency, balance);
                            }
                        }
                        let _ = event_tx.send(WsMessage::Balance(WsBalanceEvent {
                            balances,
                        }));
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Subscribe to a channel and return message receiver
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        symbol: &str,
        requires_auth: bool,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 20,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Authenticate if required
        if requires_auth {
            if let (Some(api_key), Some(api_secret)) = (&self.api_key, &self.api_secret) {
                let timestamp = Utc::now().timestamp_millis();
                let nonce = uuid::Uuid::new_v4().to_string();
                let sign = Self::generate_auth_signature(api_secret, timestamp, &nonce);

                let auth_msg = serde_json::json!({
                    "type": "auth",
                    "apiKey": api_key,
                    "timestamp": timestamp,
                    "nonce": nonce,
                    "signature": sign
                });

                ws_client.send(&auth_msg.to_string())?;
            }
        }

        // Subscribe to channel
        let subscribe_msg = serde_json::json!({
            "type": "subscribe",
            "channel": channel,
            "symbol": symbol
        });

        ws_client.send(&subscribe_msg.to_string())?;

        // Record subscription
        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(format!("{channel}:{symbol}"), symbol.to_string());
        }

        // Spawn message handler
        let subscriptions = self.subscriptions.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Message(msg) => {
                        let _ = Self::process_message(&msg, &event_tx);
                    }
                    WsEvent::Connected => {}
                    WsEvent::Disconnected => {
                        break;
                    }
                    WsEvent::Error(e) => {
                        eprintln!("Defx WebSocket error: {e}");
                    }
                    WsEvent::Ping | WsEvent::Pong => {}
                }
            }
            let mut subs = subscriptions.write().await;
            subs.clear();
        });

        self.ws_client = Some(ws_client);
        Ok(event_rx)
    }

    /// Subscribe to orders (private)
    pub async fn watch_orders(&mut self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let market_id = Self::format_symbol(symbol);
        self.subscribe_stream("orders", &market_id, true).await
    }

    /// Subscribe to balance (private)
    pub async fn watch_balance(&mut self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_stream("balance", "", true).await
    }
}

impl Default for DefxWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for DefxWs {
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
impl WsExchange for DefxWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        ws.subscribe_stream("ticker", &market_id, false).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        ws.subscribe_stream("depth", &market_id, false).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        ws.subscribe_stream("trades", &market_id, false).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        let channel = format!("kline_{}", Self::format_interval(timeframe));
        ws.subscribe_stream(&channel, &market_id, false).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let mut ws_client = WsClient::new(WsConfig {
                url: WS_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 20,
                connect_timeout_secs: 30,
            });

            let _ = ws_client.connect().await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        let ws = DefxWs::default();
        assert!(ws.api_key.is_none());
        assert!(ws.api_secret.is_none());
    }

    #[test]
    fn test_clone() {
        let ws = DefxWs::new().with_credentials("key", "secret");
        let cloned = ws.clone();
        assert_eq!(cloned.api_key, Some("key".to_string()));
        assert_eq!(cloned.api_secret, Some("secret".to_string()));
    }

    #[test]
    fn test_format_symbol() {
        assert_eq!(DefxWs::format_symbol("BTC/USDT"), "BTC_USDT");
        assert_eq!(DefxWs::format_symbol("ETH/USDC"), "ETH_USDC");
        assert_eq!(DefxWs::format_symbol("DOGE/USDT"), "DOGE_USDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(DefxWs::to_unified_symbol("BTC_USDT"), "BTC/USDT");
        assert_eq!(DefxWs::to_unified_symbol("ETH_USDC"), "ETH/USDC");
        assert_eq!(DefxWs::to_unified_symbol("DOGE_USDT"), "DOGE/USDT");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(DefxWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(DefxWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(DefxWs::format_interval(Timeframe::Day1), "1d");
        assert_eq!(DefxWs::format_interval(Timeframe::Week1), "1w");
        assert_eq!(DefxWs::format_interval(Timeframe::Month1), "1M");
    }

    #[test]
    fn test_generate_signature() {
        let signature = DefxWs::generate_auth_signature("secret", 1234567890, "nonce123");
        assert!(!signature.is_empty());
        assert_eq!(signature.len(), 64); // HMAC-SHA256 hex = 64 chars
    }

    #[test]
    fn test_parse_ticker() {
        let ticker_data = DefxWsTicker {
            symbol: Some("BTC_USDT".to_string()),
            price_change: Some("100.00".to_string()),
            price_change_percent: Some("0.25".to_string()),
            last_price: Some("40000.00".to_string()),
            last_qty: Some("0.5".to_string()),
            best_bid_price: Some("39999.00".to_string()),
            best_bid_qty: Some("1.0".to_string()),
            best_ask_price: Some("40001.00".to_string()),
            best_ask_qty: Some("1.5".to_string()),
            open_price: Some("39900.00".to_string()),
            high_price: Some("40500.00".to_string()),
            low_price: Some("39500.00".to_string()),
            volume: Some("1000.0".to_string()),
            quote_volume: Some("40000000.0".to_string()),
            close_time: Some(1234567890000),
        };

        let ticker = DefxWs::parse_ticker(&ticker_data, "BTC/USDT");
        assert_eq!(ticker.symbol, "BTC/USDT");
        assert_eq!(ticker.last, Some(Decimal::from(40000)));
        assert_eq!(ticker.bid, Some(Decimal::from(39999)));
        assert_eq!(ticker.ask, Some(Decimal::from(40001)));
    }

    #[test]
    fn test_parse_order_book() {
        let ob_data = DefxWsOrderBook {
            symbol: Some("ETH_USDC".to_string()),
            bids: vec![
                vec!["2500.00".to_string(), "10.0".to_string()],
                vec!["2499.00".to_string(), "5.0".to_string()],
            ],
            asks: vec![
                vec!["2501.00".to_string(), "8.0".to_string()],
                vec!["2502.00".to_string(), "12.0".to_string()],
            ],
            last_update_id: Some(12345),
        };

        let order_book = DefxWs::parse_order_book(&ob_data, "ETH/USDC");
        assert_eq!(order_book.symbol, "ETH/USDC");
        assert_eq!(order_book.bids.len(), 2);
        assert_eq!(order_book.asks.len(), 2);
        assert_eq!(order_book.bids[0].price, Decimal::from(2500));
        assert_eq!(order_book.asks[0].price, Decimal::from(2501));
        assert_eq!(order_book.nonce, Some(12345));
    }

    #[test]
    fn test_parse_trade() {
        let trade_data = DefxWsTrade {
            id: Some("123456".to_string()),
            symbol: Some("SOL_USDT".to_string()),
            price: Some("100.00".to_string()),
            qty: Some("5.0".to_string()),
            side: Some("BUY".to_string()),
            time: Some(1234567890000),
            is_buyer_maker: Some(false),
        };

        let trade = DefxWs::parse_trade(&trade_data, "SOL/USDT");
        assert_eq!(trade.symbol, "SOL/USDT");
        assert_eq!(trade.id, "123456");
        assert_eq!(trade.price, Decimal::from(100));
        assert_eq!(trade.amount, Decimal::from(5));
        assert_eq!(trade.side, Some("buy".to_string()));
    }

    #[test]
    fn test_parse_ohlcv() {
        let kline_data = DefxWsKline {
            symbol: Some("BTC_USDT".to_string()),
            interval: Some("1h".to_string()),
            open_time: Some(1234567890000),
            close_time: Some(1234571490000),
            open: Some("40000.00".to_string()),
            high: Some("40500.00".to_string()),
            low: Some("39500.00".to_string()),
            close: Some("40200.00".to_string()),
            volume: Some("1000.0".to_string()),
        };

        let ohlcv = DefxWs::parse_ohlcv(&kline_data);
        assert!(ohlcv.is_some());
        let ohlcv = ohlcv.unwrap();
        assert_eq!(ohlcv.timestamp, 1234567890000);
        assert_eq!(ohlcv.open, Decimal::from(40000));
        assert_eq!(ohlcv.high, Decimal::from(40500));
        assert_eq!(ohlcv.low, Decimal::from(39500));
        assert_eq!(ohlcv.close, Decimal::from(40200));
        assert_eq!(ohlcv.volume, Decimal::from(1000));
    }

    #[test]
    fn test_parse_balance() {
        let balance_data = DefxWsBalance {
            asset: Some("USDT".to_string()),
            free: Some("1000.00".to_string()),
            locked: Some("100.00".to_string()),
            total: Some("1100.00".to_string()),
        };

        let result = DefxWs::parse_balance(&balance_data);
        assert!(result.is_some());
        let (currency, balance) = result.unwrap();
        assert_eq!(currency, "USDT");
        assert_eq!(balance.free, Some(Decimal::from(1000)));
        assert_eq!(balance.used, Some(Decimal::from(100)));
    }

    #[test]
    fn test_parse_order() {
        let order_data = DefxWsOrder {
            order_id: Some("order123".to_string()),
            client_order_id: Some("client123".to_string()),
            symbol: Some("BTC_USDT".to_string()),
            side: Some("BUY".to_string()),
            order_type: Some("LIMIT".to_string()),
            price: Some("40000.00".to_string()),
            quantity: Some("1.0".to_string()),
            filled_quantity: Some("0.5".to_string()),
            remaining_quantity: Some("0.5".to_string()),
            status: Some("PARTIALLY_FILLED".to_string()),
            created_at: Some(1234567890000),
            updated_at: Some(1234567900000),
        };

        let order = DefxWs::parse_order(&order_data, "BTC/USDT");
        assert_eq!(order.id, "order123");
        assert_eq!(order.symbol, "BTC/USDT");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.status, OrderStatus::Open);
        assert_eq!(order.price, Some(Decimal::from(40000)));
    }
}
