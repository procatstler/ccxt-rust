//! Bithumb WebSocket Implementation
//!
//! Bithumb 실시간 데이터 스트리밍
//!
//! Private channels (authenticated):
//! - ordersub: Order status updates
//! - assetsub: Asset/balance updates

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Ticker,
    Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsOrderBookEvent, WsOrderEvent,
    WsTickerEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://pubwss.bithumb.com/pub/ws";
const WS_PRIVATE_URL: &str = "wss://wss.bithumb.com/websocket/ws";

/// Bithumb WebSocket 클라이언트
pub struct BithumbWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl BithumbWs {
    /// 새 Bithumb WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: None,
            api_secret: None,
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
        }
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
    }

    /// 심볼을 Bithumb 형식으로 변환 (BTC/KRW -> BTC_KRW)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Bithumb 심볼을 통합 심볼로 변환 (BTC_KRW -> BTC/KRW)
    fn to_unified_symbol(bithumb_symbol: &str) -> String {
        bithumb_symbol.replace("_", "/")
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BithumbTickerData, symbol: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data
            .date
            .as_ref()
            .and_then(|d| d.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let close_price: Option<Decimal> = data.close_price.as_ref().and_then(|v| v.parse().ok());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price.as_ref().and_then(|v| v.parse().ok()),
            low: data.low_price.as_ref().and_then(|v| v.parse().ok()),
            bid: data.buy_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: None,
            ask: data.sell_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: data.open_price.as_ref().and_then(|v| v.parse().ok()),
            close: close_price,
            last: close_price,
            previous_close: data.prev_close_price.as_ref().and_then(|v| v.parse().ok()),
            change: data.chg_amt.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.chg_rate.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.value.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent {
            symbol: unified_symbol,
            ticker,
        }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BithumbOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data
            .datetime
            .as_ref()
            .and_then(|d| d.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data
            .list
            .iter()
            .filter(|item| item.order_type.as_deref() == Some("bid"))
            .filter_map(|item| {
                Some(OrderBookEntry {
                    price: item.price.as_ref()?.parse().ok()?,
                    amount: item.quantity.as_ref()?.parse().ok()?,
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .list
            .iter()
            .filter(|item| item.order_type.as_deref() == Some("ask"))
            .filter_map(|item| {
                Some(OrderBookEntry {
                    price: item.price.as_ref()?.parse().ok()?,
                    amount: item.quantity.as_ref()?.parse().ok()?,
                })
            })
            .collect();

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
            checksum: None,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot: true, // Bithumb sends snapshots
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &BithumbTradeData, symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data
            .cont_date_time
            .as_ref()
            .and_then(|d| d.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data
            .cont_price
            .as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data
            .cont_qty
            .as_ref()
            .and_then(|q| q.parse().ok())
            .unwrap_or_default();

        let side = match data.buy_sell_gb.as_deref() {
            Some("1") => "sell", // 매도
            Some("2") => "buy",  // 매수
            _ => "buy",
        };

        let trades = vec![Trade {
            id: data.cont_no.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
            trade_type: None,
            side: Some(side.to_string()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent {
            symbol: unified_symbol,
            trades,
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str, subscribed_symbol: Option<&str>) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<BithumbWsResponse>(msg) {
            let symbol = subscribed_symbol.unwrap_or("");

            // Status message (subscribe confirmation)
            if response.status.as_deref() == Some("0000") {
                return Some(WsMessage::Subscribed {
                    channel: response.resmsg.clone().unwrap_or_default(),
                    symbol: Some(symbol.to_string()),
                });
            }

            // Data message
            if let (Some(msg_type), Some(content)) = (&response.msg_type, &response.content) {
                match msg_type.as_str() {
                    "ticker" => {
                        if let Ok(ticker_data) =
                            serde_json::from_value::<BithumbTickerData>(content.clone())
                        {
                            return Some(WsMessage::Ticker(Self::parse_ticker(
                                &ticker_data,
                                symbol,
                            )));
                        }
                    },
                    "orderbookdepth" => {
                        if let Ok(ob_data) =
                            serde_json::from_value::<BithumbOrderBookData>(content.clone())
                        {
                            return Some(WsMessage::OrderBook(Self::parse_order_book(
                                &ob_data, symbol,
                            )));
                        }
                    },
                    "transaction" => {
                        if let Ok(trade_data) =
                            serde_json::from_value::<BithumbTradeWrapper>(content.clone())
                        {
                            if let Some(trade) = trade_data.list.first() {
                                return Some(WsMessage::Trade(Self::parse_trade(trade, symbol)));
                            }
                        }
                    },
                    _ => {},
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        msg_type: &str,
        symbols: Vec<String>,
        channel: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        let subscribe_msg = serde_json::json!({
            "type": msg_type,
            "symbols": symbols,
            "tickTypes": ["30M"]  // 30분 단위 (Bithumb 기본)
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbols.join(","));
            self.subscriptions
                .write()
                .await
                .insert(key, channel.to_string());
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let subscribed_symbol = symbols.first().cloned();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    },
                    WsEvent::Disconnected => {
                        let _ = tx.send(WsMessage::Disconnected);
                    },
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) =
                            Self::process_message(&msg, subscribed_symbol.as_deref())
                        {
                            let _ = tx.send(ws_msg);
                        }
                    },
                    WsEvent::Error(err) => {
                        let _ = tx.send(WsMessage::Error(err));
                    },
                    _ => {},
                }
            }
        });

        Ok(event_rx)
    }

    /// HMAC-SHA512 서명 생성 (Private 인증용)
    fn create_signature(api_secret: &str, message: &str) -> CcxtResult<String> {
        type HmacSha512 = Hmac<Sha512>;
        let mut mac = HmacSha512::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".to_string(),
            }
        })?;
        mac.update(message.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Private WebSocket 구독 (ordersub, assetsub)
    async fn subscribe_private_stream(
        &mut self,
        channel: &str,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required for private channels".to_string(),
            })?;
        let api_secret =
            self.api_secret
                .as_ref()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API secret required for private channels".to_string(),
                })?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PRIVATE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // Bithumb private auth requires timestamp and signature
        let timestamp = Utc::now().timestamp_millis().to_string();
        let message = format!("{api_key}{timestamp}");
        let signature = Self::create_signature(api_secret, &message)?;

        // Authentication message
        let auth_msg = serde_json::json!({
            "type": "auth",
            "apiKey": api_key,
            "timestamp": timestamp,
            "signature": signature
        });
        ws_client.send(&auth_msg.to_string())?;

        // Build subscription based on channel type
        let subscribe_msg = match channel {
            "ordersub" => {
                if let Some(sym) = symbol {
                    let formatted_symbol = Self::format_symbol(sym);
                    serde_json::json!({
                        "type": "ordersub",
                        "symbols": [formatted_symbol]
                    })
                } else {
                    serde_json::json!({
                        "type": "ordersub"
                    })
                }
            },
            "assetsub" => {
                serde_json::json!({
                    "type": "assetsub"
                })
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Unknown private channel: {channel}"),
                });
            },
        };

        ws_client.send(&subscribe_msg.to_string())?;
        self.ws_client = Some(ws_client);

        // Save subscription
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or("all"));
            self.subscriptions
                .write()
                .await
                .insert(key, channel.to_string());
        }

        // Event processing task
        let tx = event_tx.clone();
        let channel_clone = channel.to_string();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    },
                    WsEvent::Disconnected => {
                        let _ = tx.send(WsMessage::Disconnected);
                    },
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_private_message(&msg, &channel_clone) {
                            let _ = tx.send(ws_msg);
                        }
                    },
                    WsEvent::Error(err) => {
                        let _ = tx.send(WsMessage::Error(err));
                    },
                    _ => {},
                }
            }
        });

        Ok(event_rx)
    }

    /// Private 메시지 파싱
    fn process_private_message(msg: &str, channel: &str) -> Option<WsMessage> {
        match channel {
            "ordersub" => {
                if let Ok(order_data) = serde_json::from_str::<BithumbOrderUpdateData>(msg) {
                    if order_data.msg_type.as_deref() == Some("ordersub") {
                        return Some(WsMessage::Order(Self::parse_order(&order_data)));
                    }
                }
            },
            "assetsub" => {
                if let Ok(asset_data) = serde_json::from_str::<BithumbAssetUpdateData>(msg) {
                    if asset_data.msg_type.as_deref() == Some("assetsub") {
                        return Some(WsMessage::Balance(Self::parse_balance(&asset_data)));
                    }
                }
            },
            _ => {},
        }
        None
    }

    /// 주문 데이터 파싱
    fn parse_order(data: &BithumbOrderUpdateData) -> WsOrderEvent {
        let symbol = data
            .symbol
            .as_ref()
            .map(|s| Self::to_unified_symbol(s))
            .unwrap_or_default();

        let timestamp = data
            .timestamp
            .as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.order_status.as_deref() {
            Some("placed") | Some("pending") => OrderStatus::Open,
            Some("completed") | Some("filled") => OrderStatus::Closed,
            Some("cancelled") | Some("canceled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let side = match data.order_type.as_deref() {
            Some("bid") | Some("buy") => OrderSide::Buy,
            Some("ask") | Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.price_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let order = Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: Some(timestamp),
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price: data.price.as_ref().and_then(|p| p.parse().ok()),
            average: data.avg_price.as_ref().and_then(|p| p.parse().ok()),
            amount: data
                .units
                .as_ref()
                .and_then(|u| u.parse().ok())
                .unwrap_or_default(),
            filled: data
                .units_remaining
                .as_ref()
                .and_then(|u| u.parse::<Decimal>().ok())
                .map(|remaining| {
                    data.units
                        .as_ref()
                        .and_then(|u| u.parse::<Decimal>().ok())
                        .unwrap_or_default()
                        - remaining
                })
                .unwrap_or_default(),
            remaining: data.units_remaining.as_ref().and_then(|u| u.parse().ok()),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            reduce_only: None,
            post_only: None,
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// 잔고 데이터 파싱
    fn parse_balance(data: &BithumbAssetUpdateData) -> WsBalanceEvent {
        let currency = data.currency.clone().unwrap_or_default();
        let timestamp = data
            .timestamp
            .as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let balance = Balance {
            free: data.available.as_ref().and_then(|v| v.parse().ok()),
            used: data.in_use.as_ref().and_then(|v| v.parse().ok()),
            total: data.total.as_ref().and_then(|v| v.parse().ok()),
            debt: None,
        };

        let mut balances = Balances::new();
        balances.add(currency, balance);
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(
            chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        );
        balances.info = serde_json::to_value(data).unwrap_or_default();

        WsBalanceEvent { balances }
    }
}

impl Default for BithumbWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BithumbWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let symbols = vec![Self::format_symbol(symbol)];
        client.subscribe_stream("ticker", symbols, "ticker").await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        client
            .subscribe_stream("ticker", formatted, "tickers")
            .await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let symbols = vec![Self::format_symbol(symbol)];
        client
            .subscribe_stream("orderbookdepth", symbols, "orderBook")
            .await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let symbols = vec![Self::format_symbol(symbol)];
        client
            .subscribe_stream("transaction", symbols, "trades")
            .await
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Bithumb does not support OHLCV streaming via WebSocket
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watch_ohlcv".to_string(),
        })
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

    // === Private Channel Methods ===

    async fn watch_orders(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key
                .clone()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API key required".into(),
                })?,
            self.api_secret
                .clone()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API secret required".into(),
                })?,
        );
        client.subscribe_private_stream("ordersub", symbol).await
    }

    async fn watch_my_trades(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Bithumb's ordersub channel includes trade execution info
        let mut client = Self::with_credentials(
            self.api_key
                .clone()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API key required".into(),
                })?,
            self.api_secret
                .clone()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API secret required".into(),
                })?,
        );
        client.subscribe_private_stream("ordersub", symbol).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key
                .clone()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API key required".into(),
                })?,
            self.api_secret
                .clone()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "API secret required".into(),
                })?,
        );
        client.subscribe_private_stream("assetsub", None).await
    }
}

// === Bithumb WebSocket Types ===

#[derive(Debug, Deserialize)]
struct BithumbWsResponse {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    resmsg: Option<String>,
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    #[serde(default)]
    content: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BithumbTickerData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    open_price: Option<String>,
    #[serde(default)]
    close_price: Option<String>,
    #[serde(default)]
    high_price: Option<String>,
    #[serde(default)]
    low_price: Option<String>,
    #[serde(default)]
    buy_price: Option<String>,
    #[serde(default)]
    sell_price: Option<String>,
    #[serde(default)]
    prev_close_price: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    chg_amt: Option<String>,
    #[serde(default)]
    chg_rate: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BithumbOrderBookData {
    #[serde(default)]
    datetime: Option<String>,
    #[serde(default)]
    list: Vec<BithumbOrderBookItem>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BithumbOrderBookItem {
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BithumbTradeWrapper {
    #[serde(default)]
    list: Vec<BithumbTradeData>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BithumbTradeData {
    #[serde(default)]
    cont_no: Option<String>,
    #[serde(default)]
    cont_price: Option<String>,
    #[serde(default)]
    cont_qty: Option<String>,
    #[serde(default)]
    buy_sell_gb: Option<String>,
    #[serde(default)]
    cont_date_time: Option<String>,
}

// === Private Channel Types ===

/// 주문 업데이트 데이터 (ordersub channel)
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BithumbOrderUpdateData {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    /// 주문 ID
    #[serde(default)]
    order_id: Option<String>,
    /// 심볼 (ex: BTC_KRW)
    #[serde(default)]
    symbol: Option<String>,
    /// 주문 타입 (bid: 매수, ask: 매도)
    #[serde(default)]
    order_type: Option<String>,
    /// 가격 타입 (limit, market)
    #[serde(default)]
    price_type: Option<String>,
    /// 주문 상태 (placed, completed, cancelled)
    #[serde(default)]
    order_status: Option<String>,
    /// 주문 가격
    #[serde(default)]
    price: Option<String>,
    /// 평균 체결가
    #[serde(default)]
    avg_price: Option<String>,
    /// 주문 수량
    #[serde(default)]
    units: Option<String>,
    /// 미체결 수량
    #[serde(default)]
    units_remaining: Option<String>,
    /// 타임스탬프
    #[serde(default)]
    timestamp: Option<String>,
}

/// 자산 업데이트 데이터 (assetsub channel)
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BithumbAssetUpdateData {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    /// 화폐 코드 (ex: BTC, KRW)
    #[serde(default)]
    currency: Option<String>,
    /// 사용 가능 잔고
    #[serde(default)]
    available: Option<String>,
    /// 사용 중 잔고 (주문 등)
    #[serde(default)]
    in_use: Option<String>,
    /// 총 잔고
    #[serde(default)]
    total: Option<String>,
    /// 타임스탬프
    #[serde(default)]
    timestamp: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BithumbWs::format_symbol("BTC/KRW"), "BTC_KRW");
        assert_eq!(BithumbWs::format_symbol("ETH/KRW"), "ETH_KRW");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BithumbWs::to_unified_symbol("BTC_KRW"), "BTC/KRW");
        assert_eq!(BithumbWs::to_unified_symbol("ETH_KRW"), "ETH/KRW");
    }

    #[test]
    fn test_with_credentials() {
        let ws = BithumbWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.api_key.is_some());
        assert!(ws.api_secret.is_some());
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = BithumbWs::new();
        assert!(ws.api_key.is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.api_key.is_some());
        assert!(ws.api_secret.is_some());
    }

    #[test]
    fn test_create_signature() {
        let signature = BithumbWs::create_signature("test_secret", "test_message").unwrap();
        // HMAC-SHA512 produces 128 character hex string
        assert_eq!(signature.len(), 128);
        // Should be valid hex
        assert!(signature.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_parse_order_update_data() {
        let json = r#"{
            "type": "ordersub",
            "orderId": "test-order-123",
            "symbol": "BTC_KRW",
            "orderType": "bid",
            "priceType": "limit",
            "orderStatus": "placed",
            "price": "50000000",
            "units": "0.001",
            "unitsRemaining": "0.001",
            "timestamp": "1704067200000"
        }"#;

        let data: BithumbOrderUpdateData = serde_json::from_str(json).unwrap();
        assert_eq!(data.msg_type.as_deref(), Some("ordersub"));
        assert_eq!(data.order_id.as_deref(), Some("test-order-123"));
        assert_eq!(data.symbol.as_deref(), Some("BTC_KRW"));
        assert_eq!(data.order_type.as_deref(), Some("bid"));
        assert_eq!(data.order_status.as_deref(), Some("placed"));
    }

    #[test]
    fn test_parse_asset_update_data() {
        let json = r#"{
            "type": "assetsub",
            "currency": "BTC",
            "available": "1.5",
            "inUse": "0.5",
            "total": "2.0",
            "timestamp": "1704067200000"
        }"#;

        let data: BithumbAssetUpdateData = serde_json::from_str(json).unwrap();
        assert_eq!(data.msg_type.as_deref(), Some("assetsub"));
        assert_eq!(data.currency.as_deref(), Some("BTC"));
        assert_eq!(data.available.as_deref(), Some("1.5"));
        assert_eq!(data.in_use.as_deref(), Some("0.5"));
        assert_eq!(data.total.as_deref(), Some("2.0"));
    }
}
