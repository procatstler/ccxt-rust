//! BigONE WebSocket Implementation
//!
//! BigONE real-time data streaming
//! API Documentation: <https://open.big.one/docs/ws_api.html>
//!
//! # Public Channels
//! - ticker - Market ticker updates
//! - depth - Order book updates
//! - trade - Trade updates
//! - kline - OHLCV candle updates
//!
//! # Private Channels (requires authentication)
//! - orders - Order state updates
//! - account - Account balance updates

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
    WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_URL: &str = "wss://big.one/ws/v2";

/// BigONE WebSocket client
pub struct BigoneWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl BigoneWs {
    /// Create new BigONE WebSocket client
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

    /// Convert unified symbol to exchange format (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Convert exchange format to unified symbol (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(market_id: &str) -> String {
        market_id.replace("-", "/")
    }

    /// Format timeframe to exchange interval
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "min1",
            Timeframe::Minute5 => "min5",
            Timeframe::Minute15 => "min15",
            Timeframe::Minute30 => "min30",
            Timeframe::Hour1 => "hour1",
            Timeframe::Hour3 => "hour3",
            Timeframe::Hour4 => "hour4",
            Timeframe::Hour6 => "hour6",
            Timeframe::Hour12 => "hour12",
            Timeframe::Day1 => "day1",
            Timeframe::Week1 => "week1",
            Timeframe::Month1 => "month1",
            _ => "hour1",
        }
    }

    /// Generate authentication signature
    fn generate_auth_signature(secret: &str, timestamp: &str, api_key: &str) -> String {
        let message = format!("{api_key}{timestamp}");
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(message.as_bytes());
        let result = mac.finalize();
        hex::encode(result.into_bytes())
    }

    /// Parse ticker data
    fn parse_ticker(data: &BigoneWsTicker, symbol: &str) -> Ticker {
        Ticker {
            symbol: symbol.to_string(),
            timestamp: data.timestamp,
            datetime: data.timestamp.map(|ts| {
                chrono::DateTime::<Utc>::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            high: data.high,
            low: data.low,
            bid: data.bid.as_ref().and_then(|b| b.price),
            bid_volume: data.bid.as_ref().and_then(|b| b.quantity),
            ask: data.ask.as_ref().and_then(|b| b.price),
            ask_volume: data.ask.as_ref().and_then(|b| b.quantity),
            vwap: None,
            open: data.open,
            close: data.close,
            last: data.close,
            previous_close: None,
            change: data.daily_change,
            percentage: None,
            average: None,
            base_volume: data.volume,
            quote_volume: data.quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order book data
    fn parse_order_book(data: &BigoneWsOrderBook, symbol: &str) -> OrderBook {
        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|b| {
                Some(OrderBookEntry {
                    price: Decimal::from_str(&b.price).ok()?,
                    amount: Decimal::from_str(&b.quantity).ok()?,
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|a| {
                Some(OrderBookEntry {
                    price: Decimal::from_str(&a.price).ok()?,
                    amount: Decimal::from_str(&a.quantity).ok()?,
                })
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
        }
    }

    /// Parse trade data
    fn parse_trade(data: &BigoneWsTrade, symbol: &str) -> Trade {
        let price = Decimal::from_str(&data.price).unwrap_or_default();
        let amount = Decimal::from_str(&data.amount).unwrap_or_default();

        Trade {
            id: data.id.to_string(),
            order: None,
            timestamp: data.timestamp,
            datetime: data.timestamp.map(|ts| {
                chrono::DateTime::<Utc>::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            symbol: symbol.to_string(),
            trade_type: None,
            side: match data.taker_side.as_deref() {
                Some("ASK") => Some("sell".to_string()),
                Some("BID") => Some("buy".to_string()),
                _ => None,
            },
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
    fn parse_ohlcv(data: &BigoneWsKline) -> Option<OHLCV> {
        Some(OHLCV {
            timestamp: data.timestamp?,
            open: Decimal::from_str(&data.open).ok()?,
            high: Decimal::from_str(&data.high).ok()?,
            low: Decimal::from_str(&data.low).ok()?,
            close: Decimal::from_str(&data.close).ok()?,
            volume: Decimal::from_str(&data.volume).ok()?,
        })
    }

    /// Parse order data
    fn parse_order(data: &BigoneWsOrder, symbol: &str) -> Order {
        let status = match data.state.as_deref() {
            Some("PENDING") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("CANCELLED") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("BID") => OrderSide::Buy,
            Some("ASK") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let amount = data.amount.as_ref().and_then(|a| Decimal::from_str(a).ok()).unwrap_or_default();
        let filled = data.filled_amount.as_ref().and_then(|f| Decimal::from_str(f).ok()).unwrap_or_default();

        Order {
            id: data.id.map(|id| id.to_string()).unwrap_or_default(),
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
            average: data.avg_deal_price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
            amount,
            filled,
            remaining: Some(amount - filled),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: data.post_only,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance data
    fn parse_balance(data: &BigoneWsBalance) -> Option<(String, Balance)> {
        let currency = data.asset_symbol.clone()?;
        let free = data.balance.as_ref().and_then(|b| Decimal::from_str(b).ok());
        let used = data.locked_balance.as_ref().and_then(|l| Decimal::from_str(l).ok());
        let total = match (free, used) {
            (Some(f), Some(u)) => Some(f + u),
            _ => None,
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

        // Handle ticker update
        if let Some(ticker_data) = json.get("marketTickerUpdate") {
            if let Ok(data) = serde_json::from_value::<BigoneWsTickerUpdate>(ticker_data.clone()) {
                if let Some(ticker_inner) = data.ticker {
                    let symbol = data.market.as_ref()
                        .map(|m| Self::to_unified_symbol(m))
                        .unwrap_or_default();
                    let ticker = Self::parse_ticker(&ticker_inner, &symbol);
                    let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                        symbol,
                        ticker,
                    }));
                }
            }
        }

        // Handle depth update
        if let Some(depth_data) = json.get("marketDepthUpdate") {
            if let Ok(data) = serde_json::from_value::<BigoneWsDepthUpdate>(depth_data.clone()) {
                if let Some(depth_inner) = data.depth {
                    let symbol = data.market.as_ref()
                        .map(|m| Self::to_unified_symbol(m))
                        .unwrap_or_default();
                    let order_book = Self::parse_order_book(&depth_inner, &symbol);
                    let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                        symbol,
                        order_book,
                        is_snapshot: true,
                    }));
                }
            }
        }

        // Handle trade update
        if let Some(trade_data) = json.get("marketTradeUpdate") {
            if let Ok(data) = serde_json::from_value::<BigoneWsTradeUpdate>(trade_data.clone()) {
                let symbol = data.market.as_ref()
                    .map(|m| Self::to_unified_symbol(m))
                    .unwrap_or_default();
                let trades: Vec<Trade> = data.trades.iter()
                    .map(|t| Self::parse_trade(t, &symbol))
                    .collect();
                if !trades.is_empty() {
                    let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                        symbol,
                        trades,
                    }));
                }
            }
        }

        // Handle kline update
        if let Some(kline_data) = json.get("marketKlineUpdate") {
            if let Ok(data) = serde_json::from_value::<BigoneWsKlineUpdate>(kline_data.clone()) {
                if let Some(kline_inner) = data.kline {
                    let symbol = data.market.as_ref()
                        .map(|m| Self::to_unified_symbol(m))
                        .unwrap_or_default();
                    if let Some(ohlcv) = Self::parse_ohlcv(&kline_inner) {
                        let _ = event_tx.send(WsMessage::Ohlcv(crate::types::WsOhlcvEvent {
                            symbol,
                            timeframe: Timeframe::Hour1, // Default, actual timeframe from subscription
                            ohlcv,
                        }));
                    }
                }
            }
        }

        // Handle order update
        if let Some(order_data) = json.get("orderStateChangeUpdate") {
            if let Ok(data) = serde_json::from_value::<BigoneWsOrderUpdate>(order_data.clone()) {
                if let Some(order_inner) = data.order {
                    let symbol = order_inner.asset_pair_name.as_ref()
                        .map(|m| Self::to_unified_symbol(m))
                        .unwrap_or_default();
                    let order = Self::parse_order(&order_inner, &symbol);
                    let _ = event_tx.send(WsMessage::Order(WsOrderEvent {
                        order,
                    }));
                }
            }
        }

        // Handle balance update
        if let Some(balance_data) = json.get("accountAssetChangeUpdate") {
            if let Ok(data) = serde_json::from_value::<BigoneWsBalanceUpdate>(balance_data.clone()) {
                if let Some(balance_inner) = data.asset {
                    if let Some((currency, balance)) = Self::parse_balance(&balance_inner) {
                        let mut balances = Balances::new();
                        balances.add(&currency, balance);
                        let _ = event_tx.send(WsMessage::Balance(WsBalanceEvent {
                            balances,
                        }));
                    }
                }
            }
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

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Authenticate if required
        if requires_auth {
            if let (Some(api_key), Some(api_secret)) = (&self.api_key, &self.api_secret) {
                let timestamp = Utc::now().timestamp_millis().to_string();
                let sign = Self::generate_auth_signature(api_secret, &timestamp, api_key);

                let auth_msg = serde_json::json!({
                    "requestId": format!("auth_{}", Utc::now().timestamp_millis()),
                    "authenticateCustomerRequest": {
                        "apiKey": api_key,
                        "timestamp": timestamp,
                        "signature": sign
                    }
                });

                ws_client.send(&auth_msg.to_string())?;
            }
        }

        // Build subscription message based on channel
        let sub_msg = match channel {
            "ticker" => {
                serde_json::json!({
                    "requestId": format!("ticker_{}", Utc::now().timestamp_millis()),
                    "subscribeMarketTickerRequest": {
                        "market": market_id
                    }
                })
            }
            "depth" => {
                serde_json::json!({
                    "requestId": format!("depth_{}", Utc::now().timestamp_millis()),
                    "subscribeMarketDepthRequest": {
                        "market": market_id
                    }
                })
            }
            "trade" => {
                serde_json::json!({
                    "requestId": format!("trade_{}", Utc::now().timestamp_millis()),
                    "subscribeMarketTradeRequest": {
                        "market": market_id
                    }
                })
            }
            "orders" => {
                serde_json::json!({
                    "requestId": format!("orders_{}", Utc::now().timestamp_millis()),
                    "subscribeOrderStateChangeRequest": {
                        "market": market_id
                    }
                })
            }
            "account" => {
                serde_json::json!({
                    "requestId": format!("account_{}", Utc::now().timestamp_millis()),
                    "subscribeAccountAssetChangeRequest": {}
                })
            }
            _ if channel.starts_with("kline") => {
                let period = channel.replace("kline", "");
                serde_json::json!({
                    "requestId": format!("kline_{}", Utc::now().timestamp_millis()),
                    "subscribeMarketKlineRequest": {
                        "market": market_id,
                        "period": period
                    }
                })
            }
            _ => return Err(crate::errors::CcxtError::NotSupported {
                feature: format!("Channel: {channel}"),
            }),
        };

        ws_client.send(&sub_msg.to_string())?;

        // Spawn message handler
        let subscriptions = self.subscriptions.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Message(msg) => {
                        let _ = Self::process_message(&msg, &event_tx);
                    }
                    WsEvent::Connected => {
                        // Connection established
                    }
                    WsEvent::Disconnected => {
                        // Handle disconnection
                        break;
                    }
                    WsEvent::Error(e) => {
                        // Handle error
                        eprintln!("BigONE WebSocket error: {e}");
                    }
                    WsEvent::Ping | WsEvent::Pong => {
                        // Heartbeat
                    }
                }
            }

            // Cleanup subscriptions
            let mut subs = subscriptions.write().await;
            subs.clear();
        });

        self.ws_client = Some(ws_client);
        Ok(event_rx)
    }

    /// Subscribe to order updates (private channel)
    pub async fn watch_orders(&mut self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let market_id = Self::format_symbol(symbol);
        self.subscribe_stream("orders", &market_id, true).await
    }

    /// Subscribe to balance updates (private channel)
    pub async fn watch_balance(&mut self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_stream("account", "", true).await
    }
}

impl Default for BigoneWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for BigoneWs {
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
impl WsExchange for BigoneWs {
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
        ws.subscribe_stream("trade", &market_id, false).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        let channel = format!("kline{}", Self::format_interval(timeframe));
        ws.subscribe_stream(&channel, &market_id, false).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let mut ws_client = WsClient::new(WsConfig {
                url: WS_URL.to_string(),
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

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsTickerUpdate {
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    ticker: Option<BigoneWsTicker>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsTicker {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    close: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    quote_volume: Option<Decimal>,
    #[serde(default)]
    daily_change: Option<Decimal>,
    #[serde(default)]
    bid: Option<BigoneWsBidAsk>,
    #[serde(default)]
    ask: Option<BigoneWsBidAsk>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsBidAsk {
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    quantity: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsDepthUpdate {
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    depth: Option<BigoneWsOrderBook>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsOrderBook {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<BigoneWsOrderBookLevel>,
    #[serde(default)]
    asks: Vec<BigoneWsOrderBookLevel>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsOrderBookLevel {
    price: String,
    quantity: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsTradeUpdate {
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    trades: Vec<BigoneWsTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsTrade {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    price: String,
    #[serde(default)]
    amount: String,
    #[serde(default)]
    taker_side: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsKlineUpdate {
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    kline: Option<BigoneWsKline>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsKline {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    open: String,
    #[serde(default)]
    high: String,
    #[serde(default)]
    low: String,
    #[serde(default)]
    close: String,
    #[serde(default)]
    volume: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsOrderUpdate {
    #[serde(default)]
    order: Option<BigoneWsOrder>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsOrder {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    asset_pair_name: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    filled_amount: Option<String>,
    #[serde(default)]
    avg_deal_price: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(rename = "type")]
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    post_only: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsBalanceUpdate {
    #[serde(default)]
    asset: Option<BigoneWsBalance>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneWsBalance {
    #[serde(default)]
    asset_symbol: Option<String>,
    #[serde(default)]
    balance: Option<String>,
    #[serde(default)]
    locked_balance: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        let ws = BigoneWs::default();
        assert!(ws.api_key.is_none());
        assert!(ws.api_secret.is_none());
    }

    #[test]
    fn test_clone() {
        let ws = BigoneWs::new().with_credentials("api_key", "secret");
        let cloned = ws.clone();
        assert_eq!(cloned.api_key, ws.api_key);
        assert_eq!(cloned.api_secret, ws.api_secret);
    }

    #[test]
    fn test_create_with_credentials() {
        let ws = BigoneWs::new().with_credentials("test_key", "test_secret");
        assert_eq!(ws.api_key, Some("test_key".to_string()));
        assert_eq!(ws.api_secret, Some("test_secret".to_string()));
    }

    #[test]
    fn test_format_symbol() {
        assert_eq!(BigoneWs::format_symbol("BTC/USDT"), "BTC-USDT");
        assert_eq!(BigoneWs::format_symbol("ETH/BTC"), "ETH-BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BigoneWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
        assert_eq!(BigoneWs::to_unified_symbol("ETH-BTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(BigoneWs::format_interval(Timeframe::Minute1), "min1");
        assert_eq!(BigoneWs::format_interval(Timeframe::Hour1), "hour1");
        assert_eq!(BigoneWs::format_interval(Timeframe::Day1), "day1");
    }

    #[test]
    fn test_generate_auth_signature() {
        let signature = BigoneWs::generate_auth_signature("test_secret", "1234567890", "test_key");
        assert!(!signature.is_empty());
    }

    #[test]
    fn test_parse_ticker() {
        let data = BigoneWsTicker {
            timestamp: Some(1234567890000),
            open: Some(Decimal::from_str("100.0").unwrap()),
            high: Some(Decimal::from_str("110.0").unwrap()),
            low: Some(Decimal::from_str("90.0").unwrap()),
            close: Some(Decimal::from_str("105.0").unwrap()),
            volume: Some(Decimal::from_str("1000.0").unwrap()),
            quote_volume: Some(Decimal::from_str("100000.0").unwrap()),
            daily_change: Some(Decimal::from_str("5.0").unwrap()),
            bid: Some(BigoneWsBidAsk {
                price: Some(Decimal::from_str("104.0").unwrap()),
                quantity: Some(Decimal::from_str("10.0").unwrap()),
            }),
            ask: Some(BigoneWsBidAsk {
                price: Some(Decimal::from_str("106.0").unwrap()),
                quantity: Some(Decimal::from_str("10.0").unwrap()),
            }),
        };

        let ticker = BigoneWs::parse_ticker(&data, "BTC/USDT");
        assert_eq!(ticker.symbol, "BTC/USDT");
        assert_eq!(ticker.high, Some(Decimal::from_str("110.0").unwrap()));
        assert_eq!(ticker.low, Some(Decimal::from_str("90.0").unwrap()));
    }

    #[test]
    fn test_parse_order_book() {
        let data = BigoneWsOrderBook {
            timestamp: Some(1234567890000),
            bids: vec![
                BigoneWsOrderBookLevel {
                    price: "100.0".to_string(),
                    quantity: "10.0".to_string(),
                },
            ],
            asks: vec![
                BigoneWsOrderBookLevel {
                    price: "101.0".to_string(),
                    quantity: "5.0".to_string(),
                },
            ],
        };

        let order_book = BigoneWs::parse_order_book(&data, "BTC/USDT");
        assert_eq!(order_book.symbol, "BTC/USDT");
        assert_eq!(order_book.bids.len(), 1);
        assert_eq!(order_book.asks.len(), 1);
    }

    #[test]
    fn test_parse_trade() {
        let data = BigoneWsTrade {
            id: 12345,
            price: "100.0".to_string(),
            amount: "1.0".to_string(),
            taker_side: Some("BID".to_string()),
            timestamp: Some(1234567890000),
        };

        let trade = BigoneWs::parse_trade(&data, "BTC/USDT");
        assert_eq!(trade.symbol, "BTC/USDT");
        assert_eq!(trade.id, "12345");
    }

    #[test]
    fn test_parse_ohlcv() {
        let data = BigoneWsKline {
            timestamp: Some(1234567890000),
            open: "100.0".to_string(),
            high: "110.0".to_string(),
            low: "90.0".to_string(),
            close: "105.0".to_string(),
            volume: "1000.0".to_string(),
        };

        let ohlcv = BigoneWs::parse_ohlcv(&data);
        assert!(ohlcv.is_some());
        let ohlcv = ohlcv.unwrap();
        assert_eq!(ohlcv.timestamp, 1234567890000);
    }

    #[test]
    fn test_parse_balance() {
        let data = BigoneWsBalance {
            asset_symbol: Some("BTC".to_string()),
            balance: Some("10.0".to_string()),
            locked_balance: Some("1.0".to_string()),
        };

        let result = BigoneWs::parse_balance(&data);
        assert!(result.is_some());
        let (currency, balance) = result.unwrap();
        assert_eq!(currency, "BTC");
        assert_eq!(balance.total, Some(Decimal::from_str("11.0").unwrap()));
    }
}
