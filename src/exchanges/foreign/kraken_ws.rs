//! Kraken WebSocket Implementation
//!
//! Kraken 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    TakerOrMaker, Ticker, Timeframe, Trade, WsBalanceEvent,
    WsExchange, WsMessage, WsMyTradeEvent, WsOrderBookEvent,
    WsOrderEvent, WsTickerEvent, WsTradeEvent,
};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

const WS_PUBLIC_URL: &str = "wss://ws.kraken.com";
const WS_PRIVATE_URL: &str = "wss://ws-auth.kraken.com";
const REST_API_URL: &str = "https://api.kraken.com";

/// Kraken WebSocket 클라이언트
pub struct KrakenWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    // Private channel authentication
    api_key: Option<String>,
    api_secret: Option<String>,
    ws_token: Option<String>,
    config: Option<ExchangeConfig>,
}

impl Clone for KrakenWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
            ws_token: self.ws_token.clone(),
            config: self.config.clone(),
        }
    }
}

impl KrakenWs {
    /// 새 Kraken WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: None,
            api_secret: None,
            ws_token: None,
            config: None,
        }
    }

    /// ExchangeConfig로 새 인스턴스 생성
    pub fn with_config(config: ExchangeConfig) -> Self {
        let api_key = config.api_key().map(|s| s.to_string());
        let api_secret = config.secret().map(|s| s.to_string());
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key,
            api_secret,
            ws_token: None,
            config: Some(config),
        }
    }

    /// 자격 증명으로 새 인스턴스 생성
    pub fn with_credentials(api_key: &str, api_secret: &str) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: Some(api_key.to_string()),
            api_secret: Some(api_secret.to_string()),
            ws_token: None,
            config: None,
        }
    }

    /// 자격 증명 설정
    pub fn set_credentials(&mut self, api_key: &str, api_secret: &str) {
        self.api_key = Some(api_key.to_string());
        self.api_secret = Some(api_secret.to_string());
    }

    /// API 키 반환
    pub fn get_api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Kraken API 서명 생성 (HMAC-SHA512)
    fn sign_request(secret: &str, path: &str, nonce: u64, body: &str) -> CcxtResult<String> {
        // SHA256(nonce + body)
        let mut sha256 = Sha256::new();
        sha256.update(format!("{nonce}{body}"));
        let sha256_digest = sha256.finalize();

        // path + sha256_digest
        let mut message = path.as_bytes().to_vec();
        message.extend_from_slice(&sha256_digest);

        // Decode secret from base64
        let decoded_secret = BASE64.decode(secret)
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Failed to decode secret: {e}"),
            })?;

        // HMAC-SHA512
        type HmacSha512 = Hmac<Sha512>;
        let mut mac = HmacSha512::new_from_slice(&decoded_secret)
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("HMAC error: {e}"),
            })?;
        mac.update(&message);
        let signature = mac.finalize().into_bytes();

        Ok(BASE64.encode(signature))
    }

    /// WebSocket 토큰 획득 (REST API GetWebSocketsToken)
    async fn get_websocket_token(&self) -> CcxtResult<String> {
        let api_key = self.api_key.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required for private channels".into(),
        })?;
        let api_secret = self.api_secret.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required for private channels".into(),
        })?;

        let nonce = Utc::now().timestamp_millis() as u64 * 1000;
        let body = format!("nonce={nonce}");
        let path = "/0/private/GetWebSocketsToken";
        let signature = Self::sign_request(api_secret, path, nonce, &body)?;

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{REST_API_URL}{path}"))
            .header("API-Key", api_key)
            .header("API-Sign", &signature)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: format!("{REST_API_URL}{path}"),
                message: e.to_string()
            })?;

        let result: KrakenWsTokenResponse = response
            .json()
            .await
            .map_err(|e| CcxtError::ParseError {
                data_type: "KrakenWsTokenResponse".to_string(),
                message: e.to_string()
            })?;

        if let Some(errors) = &result.error {
            if !errors.is_empty() {
                return Err(CcxtError::AuthenticationError {
                    message: errors.join(", "),
                });
            }
        }

        result.result
            .and_then(|r| r.token)
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "No token in response".into(),
            })
    }

    /// 심볼을 Kraken WebSocket 형식으로 변환 (BTC/USD -> XBT/USD)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("BTC", "XBT")
    }

    /// Kraken 심볼을 통합 심볼로 변환 (XBT/USD -> BTC/USD)
    fn to_unified_symbol(kraken_symbol: &str) -> String {
        kraken_symbol.replace("XBT", "BTC")
    }

    /// Timeframe을 Kraken 포맷으로 변환 (분 단위)
    fn format_interval(timeframe: Timeframe) -> i32 {
        match timeframe {
            Timeframe::Minute1 => 1,
            Timeframe::Minute5 => 5,
            Timeframe::Minute15 => 15,
            Timeframe::Minute30 => 30,
            Timeframe::Hour1 => 60,
            Timeframe::Hour4 => 240,
            Timeframe::Day1 => 1440,
            Timeframe::Week1 => 10080,
            _ => 1,
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &KrakenTickerData, symbol: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: data.h.as_ref().and_then(|h| h.get(1)).and_then(|v| Decimal::from_str(v).ok()),
            low: data.l.as_ref().and_then(|l| l.get(1)).and_then(|v| Decimal::from_str(v).ok()),
            bid: data.b.as_ref().and_then(|b| b.first()).and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: data.b.as_ref().and_then(|b| b.get(2)).and_then(|v| Decimal::from_str(v).ok()),
            ask: data.a.as_ref().and_then(|a| a.first()).and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: data.a.as_ref().and_then(|a| a.get(2)).and_then(|v| Decimal::from_str(v).ok()),
            vwap: data.p.as_ref().and_then(|p| p.get(1)).and_then(|v| Decimal::from_str(v).ok()),
            open: data.o.as_ref().and_then(|o| o.first()).and_then(|v| Decimal::from_str(v).ok()),
            close: data.c.as_ref().and_then(|c| c.first()).and_then(|v| Decimal::from_str(v).ok()),
            last: data.c.as_ref().and_then(|c| c.first()).and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.v.as_ref().and_then(|v| v.get(1)).and_then(|val| Decimal::from_str(val).ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &KrakenOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<Vec<String>>| -> Vec<OrderBookEntry> {
            entries.iter().filter_map(|e| {
                if e.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&e[0]).ok()?,
                        amount: Decimal::from_str(&e[1]).ok()?,
                    })
                } else {
                    None
                }
            }).collect()
        };

        let bids = data.bs.as_ref().map(parse_entries)
            .or_else(|| data.b.as_ref().map(parse_entries))
            .unwrap_or_default();

        let asks = data.as_.as_ref().map(parse_entries)
            .or_else(|| data.a.as_ref().map(parse_entries))
            .unwrap_or_default();

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trades(data: &[Vec<String>], symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);

        let trades: Vec<Trade> = data.iter().filter_map(|t| {
            if t.len() < 6 {
                return None;
            }

            let price = Decimal::from_str(&t[0]).ok()?;
            let amount = Decimal::from_str(&t[1]).ok()?;
            let timestamp = t[2].parse::<f64>().ok().map(|ts| (ts * 1000.0) as i64)?;
            let side = match t[3].as_str() {
                "b" => Some("buy".to_string()),
                "s" => Some("sell".to_string()),
                _ => None,
            };

            Some(Trade {
                id: t[2].clone(),
                order: None,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: unified_symbol.clone(),
                trade_type: None,
                side,
                taker_or_maker: None,
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: Vec::new(),
                info: serde_json::Value::Array(t.iter().map(|s| serde_json::Value::String(s.clone())).collect()),
            })
        }).collect();

        WsTradeEvent { symbol: unified_symbol, trades }
    }

    /// 주문 업데이트 파싱
    fn parse_order_update(data: &KrakenOrderData, order_id: &str) -> WsOrderEvent {
        let timestamp = data.opentm.as_ref()
            .and_then(|t| t.parse::<f64>().ok())
            .map(|t| (t * 1000.0) as i64)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = match data.descr.as_ref().and_then(|d| d.order_type.as_deref()) {
            Some(t) if t.contains("buy") => OrderSide::Buy,
            Some(t) if t.contains("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.descr.as_ref().and_then(|d| d.ordertype.as_deref()) {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            Some("stop-loss") => OrderType::StopLoss,
            Some("take-profit") => OrderType::TakeProfit,
            Some("stop-loss-limit") => OrderType::StopLossLimit,
            Some("take-profit-limit") => OrderType::TakeProfitLimit,
            _ => OrderType::Limit,
        };

        let status = match data.status.as_deref() {
            Some("pending") => OrderStatus::Open,
            Some("open") => OrderStatus::Open,
            Some("closed") => OrderStatus::Closed,
            Some("canceled") | Some("cancelled") => OrderStatus::Canceled,
            Some("expired") => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let symbol = data.descr.as_ref()
            .and_then(|d| d.pair.clone())
            .map(|p| Self::to_unified_symbol(&p))
            .unwrap_or_default();

        let price = data.descr.as_ref()
            .and_then(|d| d.price.as_ref())
            .and_then(|p| Decimal::from_str(p).ok());

        let amount = data.vol.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let filled = data.vol_exec.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let remaining = amount - filled;

        let cost = data.cost.as_ref()
            .and_then(|c| Decimal::from_str(c).ok());

        let average = data.avg_price.as_ref()
            .and_then(|a| Decimal::from_str(a).ok());

        let fee_cost = data.fee.as_ref()
            .and_then(|f| Decimal::from_str(f).ok());

        let fee = fee_cost.map(|c| Fee {
            currency: None,
            cost: Some(c),
            rate: None,
        });

        let last_trade_ts = data.closetm.as_ref()
            .and_then(|t| t.parse::<f64>().ok())
            .map(|t| (t * 1000.0) as i64);

        let order = Order {
            id: order_id.to_string(),
            client_order_id: data.userref.map(|r| r.to_string()),
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()),
            last_trade_timestamp: last_trade_ts,
            last_update_timestamp: Some(timestamp),
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force: None,
            side,
            price,
            average,
            amount,
            filled,
            remaining: Some(remaining),
            stop_price: None,
            cost,
            trades: Vec::new(),
            fee,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// 내 체결 파싱
    fn parse_my_trade(data: &KrakenTradeData, trade_id: &str) -> (String, Trade) {
        let timestamp = data.time.as_ref()
            .and_then(|t| t.parse::<f64>().ok())
            .map(|t| (t * 1000.0) as i64)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let symbol = data.pair.as_ref()
            .map(|p| Self::to_unified_symbol(p))
            .unwrap_or_default();

        let side = match data.trade_type.as_deref() {
            Some("b") | Some("buy") => Some("buy".to_string()),
            Some("s") | Some("sell") => Some("sell".to_string()),
            _ => None,
        };

        let taker_or_maker = match data.ordertype.as_deref() {
            Some("m") | Some("maker") => Some(TakerOrMaker::Maker),
            Some("t") | Some("taker") => Some(TakerOrMaker::Taker),
            _ => None,
        };

        let price = data.price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok())
            .unwrap_or_default();

        let amount = data.vol.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let cost = data.cost.as_ref()
            .and_then(|c| Decimal::from_str(c).ok())
            .or_else(|| Some(price * amount));

        let fee_cost = data.fee.as_ref()
            .and_then(|f| Decimal::from_str(f).ok());

        let fee = fee_cost.map(|c| Fee {
            currency: None,
            cost: Some(c),
            rate: None,
        });

        let trade = Trade {
            id: trade_id.to_string(),
            order: data.ordertxid.clone(),
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()),
            symbol: symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker,
            price,
            amount,
            cost,
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        (symbol, trade)
    }

    /// 잔고 업데이트 파싱
    fn parse_balance_update(data: &[KrakenBalanceData]) -> WsBalanceEvent {
        let timestamp = Utc::now().timestamp_millis();
        let mut balances = Balances::new();

        for item in data {
            if let Some(ref currency) = item.asset {
                let free = item.balance.as_ref()
                    .and_then(|b| Decimal::from_str(b).ok())
                    .unwrap_or_default();
                let used = item.hold_trade.as_ref()
                    .and_then(|h| Decimal::from_str(h).ok())
                    .unwrap_or_default();

                let balance = Balance::new(free, used);
                balances.add(currency, balance);
            }
        }

        balances.timestamp = Some(timestamp);
        balances.datetime = chrono::DateTime::from_timestamp_millis(timestamp)
            .map(|dt| dt.to_rfc3339());

        WsBalanceEvent { balances }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Parse the JSON message
        // Kraken sends array format: [channelID, data, "channelName", "pair"]
        // Private format: [data, "channelName", {"sequence": n}]

        // First, check if it's a JSON object (status messages)
        if msg.trim().starts_with('{') {
            // Try to parse as subscription status
            if let Ok(status) = serde_json::from_str::<KrakenWsStatus>(msg) {
                if status.status.as_deref() == Some("subscribed") {
                    return Some(WsMessage::Subscribed {
                        channel: status.subscription.as_ref()
                            .and_then(|s| s.name.clone())
                            .unwrap_or_default(),
                        symbol: status.pair.clone(),
                    });
                }
            }
            return None;
        }

        // Try to parse as array data
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(msg) {
            // Private channels: [[data], "channelName", {...}]
            // Check for private channel format (second element is the channel name string)
            if arr.len() >= 2 {
                if let Some(channel_name) = arr.get(1).and_then(|v| v.as_str()) {
                    // Handle private channels
                    match channel_name {
                        "openOrders" => {
                            if let Some(data_arr) = arr.first().and_then(|v| v.as_array()) {
                                for item in data_arr {
                                    if let Some(obj) = item.as_object() {
                                        for (order_id, order_data) in obj {
                                            if let Ok(order) = serde_json::from_value::<KrakenOrderData>(order_data.clone()) {
                                                return Some(WsMessage::Order(Self::parse_order_update(&order, order_id)));
                                            }
                                        }
                                    }
                                }
                            }
                            return None;
                        }
                        "ownTrades" => {
                            if let Some(data_arr) = arr.first().and_then(|v| v.as_array()) {
                                let mut trades_by_symbol: HashMap<String, Vec<Trade>> = HashMap::new();
                                for item in data_arr {
                                    if let Some(obj) = item.as_object() {
                                        for (trade_id, trade_data) in obj {
                                            if let Ok(trade) = serde_json::from_value::<KrakenTradeData>(trade_data.clone()) {
                                                let (symbol, parsed_trade) = Self::parse_my_trade(&trade, trade_id);
                                                trades_by_symbol.entry(symbol).or_default().push(parsed_trade);
                                            }
                                        }
                                    }
                                }
                                // Return first symbol's trades
                                if let Some((symbol, trades)) = trades_by_symbol.into_iter().next() {
                                    return Some(WsMessage::MyTrade(WsMyTradeEvent { symbol, trades }));
                                }
                            }
                            return None;
                        }
                        "balances" => {
                            // Kraken WebSocket v2 balances format
                            if let Some(data_arr) = arr.first().and_then(|v| v.as_array()) {
                                let balance_items: Vec<KrakenBalanceData> = data_arr.iter()
                                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                                    .collect();
                                if !balance_items.is_empty() {
                                    return Some(WsMessage::Balance(Self::parse_balance_update(&balance_items)));
                                }
                            }
                            return None;
                        }
                        _ => {}
                    }
                }
            }

            // Public channels: [channelID, data, "channelName", "pair"]
            if arr.len() >= 4 {
                let channel_name = arr.get(arr.len() - 2).and_then(|v| v.as_str());
                let pair = arr.last().and_then(|v| v.as_str()).unwrap_or("");

                match channel_name {
                    Some("ticker") => {
                        if let Some(data) = arr.get(1) {
                            if let Ok(ticker_data) = serde_json::from_value::<KrakenTickerData>(data.clone()) {
                                return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data, pair)));
                            }
                        }
                    }
                    Some(name) if name.starts_with("book") => {
                        if let Some(data) = arr.get(1) {
                            if let Ok(ob_data) = serde_json::from_value::<KrakenOrderBookData>(data.clone()) {
                                let is_snapshot = ob_data.bs.is_some() || ob_data.as_.is_some();
                                return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, pair, is_snapshot)));
                            }
                        }
                    }
                    Some("trade") => {
                        if let Some(data) = arr.get(1) {
                            if let Ok(trade_data) = serde_json::from_value::<Vec<Vec<String>>>(data.clone()) {
                                return Some(WsMessage::Trade(Self::parse_trades(&trade_data, pair)));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        pairs: Vec<String>,
        channel: &str,
        depth: Option<i32>,
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
        });

        let mut ws_rx = ws_client.connect().await?;

        // Build subscription message
        let mut subscription = serde_json::json!({
            "name": channel
        });

        if let Some(d) = depth {
            subscription["depth"] = serde_json::json!(d);
        }

        let subscribe_msg = serde_json::json!({
            "event": "subscribe",
            "pair": pairs,
            "subscription": subscription
        });

        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, pairs.join(","));
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // 이벤트 처리 태스크
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
                        if let Some(ws_msg) = Self::process_message(&msg) {
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

    /// Private 채널 구독 (인증 필요)
    async fn subscribe_private_stream(
        &mut self,
        channel: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Get WebSocket token
        let token = self.get_websocket_token().await?;
        self.ws_token = Some(token.clone());

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PRIVATE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Build subscription message with token
        let subscribe_msg = serde_json::json!({
            "event": "subscribe",
            "subscription": {
                "name": channel,
                "token": token
            }
        });

        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("private:{channel}");
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // 이벤트 처리 태스크
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
                        if let Some(ws_msg) = Self::process_message(&msg) {
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
}

impl Default for KrakenWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for KrakenWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let kraken_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![kraken_symbol], "ticker", None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let kraken_symbols: Vec<String> = symbols.iter()
            .map(|s| Self::format_symbol(s))
            .collect();
        client.subscribe_stream(kraken_symbols, "ticker", None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let kraken_symbol = Self::format_symbol(symbol);
        let depth = match limit.unwrap_or(10) {
            1..=10 => 10,
            11..=25 => 25,
            26..=100 => 100,
            101..=500 => 500,
            _ => 1000,
        };
        client.subscribe_stream(vec![kraken_symbol], &format!("book-{depth}"), Some(depth)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let kraken_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![kraken_symbol], "trade", None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let kraken_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        client.subscribe_stream(vec![kraken_symbol], &format!("ohlc-{interval}"), None).await
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

    /// WebSocket 인증 (Private 채널용 토큰 획득)
    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        let token = self.get_websocket_token().await?;
        self.ws_token = Some(token);
        Ok(())
    }

    /// 잔고 변경 구독 (인증 필요)
    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.as_deref().unwrap_or(""),
            self.api_secret.as_deref().unwrap_or(""),
        );
        client.subscribe_private_stream("balances").await
    }

    /// 주문 변경 구독 (인증 필요)
    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.as_deref().unwrap_or(""),
            self.api_secret.as_deref().unwrap_or(""),
        );
        client.subscribe_private_stream("openOrders").await
    }

    /// 내 체결 내역 구독 (인증 필요)
    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.as_deref().unwrap_or(""),
            self.api_secret.as_deref().unwrap_or(""),
        );
        client.subscribe_private_stream("ownTrades").await
    }
}

// === Kraken WebSocket Types ===

#[derive(Debug, Deserialize)]
struct KrakenWsStatus {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    pair: Option<String>,
    #[serde(default)]
    subscription: Option<KrakenWsSubscription>,
}

#[derive(Debug, Deserialize)]
struct KrakenWsSubscription {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    depth: Option<i32>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KrakenTickerData {
    #[serde(default)]
    a: Option<Vec<String>>, // ask [price, wholeLotVolume, lotVolume]
    #[serde(default)]
    b: Option<Vec<String>>, // bid [price, wholeLotVolume, lotVolume]
    #[serde(default)]
    c: Option<Vec<String>>, // close [price, lotVolume]
    #[serde(default)]
    v: Option<Vec<String>>, // volume [today, last24Hours]
    #[serde(default)]
    p: Option<Vec<String>>, // vwap [today, last24Hours]
    #[serde(default)]
    t: Option<Vec<i64>>, // trade count [today, last24Hours]
    #[serde(default)]
    l: Option<Vec<String>>, // low [today, last24Hours]
    #[serde(default)]
    h: Option<Vec<String>>, // high [today, last24Hours]
    #[serde(default)]
    o: Option<Vec<String>>, // open [today, last24Hours]
}

#[derive(Debug, Default, Deserialize)]
struct KrakenOrderBookData {
    #[serde(default)]
    bs: Option<Vec<Vec<String>>>, // bids snapshot
    #[serde(default, rename = "as")]
    as_: Option<Vec<Vec<String>>>, // asks snapshot
    #[serde(default)]
    b: Option<Vec<Vec<String>>>, // bids update
    #[serde(default)]
    a: Option<Vec<Vec<String>>>, // asks update
}

// === Private Channel Types ===

/// WebSocket Token Response
#[derive(Debug, Deserialize)]
struct KrakenWsTokenResponse {
    #[serde(default)]
    error: Option<Vec<String>>,
    #[serde(default)]
    result: Option<KrakenWsTokenResult>,
}

#[derive(Debug, Deserialize)]
struct KrakenWsTokenResult {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    expires: Option<i64>,
}

/// 주문 데이터 (openOrders)
#[derive(Debug, Default, Deserialize, Serialize)]
struct KrakenOrderData {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    userref: Option<i64>,
    #[serde(default)]
    opentm: Option<String>,  // open timestamp (as string)
    #[serde(default)]
    closetm: Option<String>, // close timestamp (as string)
    #[serde(default)]
    vol: Option<String>,      // volume (amount)
    #[serde(default)]
    vol_exec: Option<String>, // executed volume (filled)
    #[serde(default)]
    cost: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    avg_price: Option<String>,
    #[serde(default)]
    descr: Option<KrakenOrderDescr>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KrakenOrderDescr {
    #[serde(default)]
    pair: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>, // buy/sell
    #[serde(default)]
    ordertype: Option<String>,  // limit/market
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    price2: Option<String>,     // secondary price (stop, limit)
    #[serde(default)]
    leverage: Option<String>,
}

/// 체결 데이터 (ownTrades)
#[derive(Debug, Default, Deserialize, Serialize)]
struct KrakenTradeData {
    #[serde(default)]
    ordertxid: Option<String>,  // order ID
    #[serde(default)]
    pair: Option<String>,
    #[serde(default)]
    time: Option<String>,       // timestamp as string
    #[serde(default, rename = "type")]
    trade_type: Option<String>, // buy/sell
    #[serde(default)]
    ordertype: Option<String>,  // maker/taker
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    vol: Option<String>,
    #[serde(default)]
    cost: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    margin: Option<String>,
}

/// 잔고 데이터 (balances)
#[derive(Debug, Default, Deserialize, Serialize)]
struct KrakenBalanceData {
    #[serde(default)]
    asset: Option<String>,      // currency code
    #[serde(default)]
    balance: Option<String>,    // total balance
    #[serde(default)]
    hold_trade: Option<String>, // held for trading
    #[serde(default)]
    hold_deposit: Option<String>, // held for deposit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(KrakenWs::format_symbol("BTC/USD"), "XBT/USD");
        assert_eq!(KrakenWs::format_symbol("ETH/BTC"), "ETH/XBT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(KrakenWs::to_unified_symbol("XBT/USD"), "BTC/USD");
        assert_eq!(KrakenWs::to_unified_symbol("ETH/XBT"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(KrakenWs::format_interval(Timeframe::Minute1), 1);
        assert_eq!(KrakenWs::format_interval(Timeframe::Hour1), 60);
        assert_eq!(KrakenWs::format_interval(Timeframe::Day1), 1440);
    }

    #[test]
    fn test_with_credentials() {
        let client = KrakenWs::with_credentials("test_api_key", "dGVzdF9zZWNyZXQ=");
        assert_eq!(client.get_api_key(), Some("test_api_key"));
    }

    #[test]
    fn test_set_credentials() {
        let mut client = KrakenWs::new();
        assert_eq!(client.get_api_key(), None);
        client.set_credentials("new_key", "bmV3X3NlY3JldA==");
        assert_eq!(client.get_api_key(), Some("new_key"));
    }

    #[test]
    fn test_debug_json_parsing() {
        let msg = r#"[[{"OQCLML-BW3P3-BUCMWZ":{"status":"open"}}],"openOrders",{"sequence":1}]"#;

        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(msg) {
            println!("Array length: {}", arr.len());
            for (i, v) in arr.iter().enumerate() {
                let type_str = match v {
                    serde_json::Value::Array(_) => "Array".to_string(),
                    serde_json::Value::Object(_) => "Object".to_string(),
                    serde_json::Value::String(s) => format!("String({})", s),
                    _ => "Other".to_string(),
                };
                println!("arr[{}] type: {:?}", i, type_str);
            }

            // Test arr[1] as string
            if let Some(s) = arr.get(1).and_then(|v| v.as_str()) {
                println!("arr[1] as_str: {}", s);
            } else {
                println!("arr[1] is NOT a string!");
            }

            // Test arr[0] as array
            if let Some(data_arr) = arr.first().and_then(|v| v.as_array()) {
                println!("arr[0] is an array with {} elements", data_arr.len());
                for item in data_arr {
                    if let Some(obj) = item.as_object() {
                        for (key, _val) in obj {
                            println!("Found key: {}", key);
                        }
                    }
                }
            }
        } else {
            println!("Failed to parse JSON");
        }
    }

    #[test]
    fn test_process_message_open_orders() {
        // Kraken openOrders format: [[{"order-id": {order data}}], "openOrders", {"sequence": 1}]
        let msg = r#"[[{"OQCLML-BW3P3-BUCMWZ":{"avg_price":"0.00000","cost":"0.00000","descr":{"pair":"XBT/USD","type":"buy","ordertype":"limit","price":"50000.00000","leverage":"none"},"fee":"0.00000","opentm":"1616663113.987","status":"open","vol":"0.00100000","vol_exec":"0.00000000"}}],"openOrders",{"sequence":1}]"#;

        let result = KrakenWs::process_message(msg);
        assert!(result.is_some(), "process_message returned None");
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "OQCLML-BW3P3-BUCMWZ");
            assert_eq!(event.order.symbol, "BTC/USD");
            assert_eq!(event.order.status, OrderStatus::Open);
            assert_eq!(event.order.side, OrderSide::Buy);
            assert_eq!(event.order.order_type, OrderType::Limit);
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_message_open_orders_filled() {
        let msg = r#"[[{"OQCLML-BW3P3-BUCMWZ":{"avg_price":"50500.00000","cost":"50.50000","descr":{"pair":"XBT/USD","type":"sell","ordertype":"market","price":"0.00000"},"fee":"0.13130","opentm":"1616663113.987","closetm":"1616663115.123","status":"closed","vol":"0.00100000","vol_exec":"0.00100000"}}],"openOrders",{"sequence":2}]"#;

        let result = KrakenWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "OQCLML-BW3P3-BUCMWZ");
            assert_eq!(event.order.status, OrderStatus::Closed);
            assert_eq!(event.order.side, OrderSide::Sell);
            assert_eq!(event.order.order_type, OrderType::Market);
            assert!(event.order.average.is_some());
            assert!(event.order.fee.is_some());
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_message_own_trades() {
        // Kraken ownTrades format: [[{"trade-id": {trade data}}], "ownTrades", {"sequence": 1}]
        let msg = r#"[[{"TDLH43-DVQXD-2KHVYY":{"ordertxid":"OQCLML-BW3P3-BUCMWZ","pair":"XBT/USD","time":"1616663115.123","type":"b","ordertype":"m","price":"50500.00000","vol":"0.00100000","cost":"50.50000","fee":"0.13130"}}],"ownTrades",{"sequence":1}]"#;

        let result = KrakenWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::MyTrade(event)) = result {
            assert_eq!(event.symbol, "BTC/USD");
            assert_eq!(event.trades.len(), 1);
            let trade = &event.trades[0];
            assert_eq!(trade.id, "TDLH43-DVQXD-2KHVYY");
            assert_eq!(trade.order, Some("OQCLML-BW3P3-BUCMWZ".to_string()));
            assert_eq!(trade.side, Some("buy".to_string()));
        } else {
            panic!("Expected MyTrade message");
        }
    }

    #[test]
    fn test_process_message_own_trades_sell() {
        let msg = r#"[[{"TDLH44-ABCDE-3KHVYY":{"ordertxid":"OQCLML-BW3P3-BUCMWZ","pair":"ETH/XBT","time":"1616663116.456","type":"s","ordertype":"t","price":"0.03500000","vol":"1.50000000","cost":"0.05250000","fee":"0.00013650"}}],"ownTrades",{"sequence":2}]"#;

        let result = KrakenWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::MyTrade(event)) = result {
            assert_eq!(event.symbol, "ETH/BTC");
            assert_eq!(event.trades.len(), 1);
            let trade = &event.trades[0];
            assert_eq!(trade.side, Some("sell".to_string()));
            assert_eq!(trade.taker_or_maker, Some(TakerOrMaker::Taker));
        } else {
            panic!("Expected MyTrade message");
        }
    }

    #[test]
    fn test_process_message_subscribed() {
        let msg = r#"{"channelName":"openOrders","event":"subscriptionStatus","status":"subscribed","subscription":{"name":"openOrders"}}"#;

        let result = KrakenWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Subscribed { channel, .. }) = result {
            assert_eq!(channel, "openOrders");
        } else {
            panic!("Expected Subscribed message");
        }
    }

    #[test]
    fn test_parse_order_update() {
        let data = KrakenOrderData {
            status: Some("open".to_string()),
            userref: Some(123456),
            opentm: Some("1616663113.987".to_string()),
            closetm: None,
            vol: Some("0.00100000".to_string()),
            vol_exec: Some("0.00000000".to_string()),
            cost: Some("0.00000".to_string()),
            fee: Some("0.00000".to_string()),
            avg_price: None,
            descr: Some(KrakenOrderDescr {
                pair: Some("XBT/USD".to_string()),
                order_type: Some("buy".to_string()),
                ordertype: Some("limit".to_string()),
                price: Some("50000.00000".to_string()),
                price2: None,
                leverage: None,
            }),
        };

        let event = KrakenWs::parse_order_update(&data, "OTEST-ID123-ABCDEF");
        assert_eq!(event.order.id, "OTEST-ID123-ABCDEF");
        assert_eq!(event.order.client_order_id, Some("123456".to_string()));
        assert_eq!(event.order.symbol, "BTC/USD");
        assert_eq!(event.order.status, OrderStatus::Open);
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
        assert!(event.order.price.is_some());
    }

    #[test]
    fn test_parse_my_trade() {
        let data = KrakenTradeData {
            ordertxid: Some("OQCLML-BW3P3-BUCMWZ".to_string()),
            pair: Some("XBT/USD".to_string()),
            time: Some("1616663115.123".to_string()),
            trade_type: Some("b".to_string()),
            ordertype: Some("m".to_string()),
            price: Some("50500.00000".to_string()),
            vol: Some("0.00100000".to_string()),
            cost: Some("50.50000".to_string()),
            fee: Some("0.13130".to_string()),
            margin: None,
        };

        let (symbol, trade) = KrakenWs::parse_my_trade(&data, "TDLH43-DVQXD-2KHVYY");
        assert_eq!(symbol, "BTC/USD");
        assert_eq!(trade.id, "TDLH43-DVQXD-2KHVYY");
        assert_eq!(trade.order, Some("OQCLML-BW3P3-BUCMWZ".to_string()));
        assert_eq!(trade.side, Some("buy".to_string()));
        assert_eq!(trade.taker_or_maker, Some(TakerOrMaker::Maker));
        assert!(trade.fee.is_some());
    }

    #[test]
    fn test_order_status_mapping() {
        let test_cases = vec![
            ("pending", OrderStatus::Open),
            ("open", OrderStatus::Open),
            ("closed", OrderStatus::Closed),
            ("canceled", OrderStatus::Canceled),
            ("cancelled", OrderStatus::Canceled),
            ("expired", OrderStatus::Expired),
            ("unknown", OrderStatus::Open),
        ];

        for (status_str, expected) in test_cases {
            let data = KrakenOrderData {
                status: Some(status_str.to_string()),
                ..Default::default()
            };
            let event = KrakenWs::parse_order_update(&data, "test");
            assert_eq!(event.order.status, expected, "Failed for status: {}", status_str);
        }
    }

    #[test]
    fn test_order_type_mapping() {
        let test_cases = vec![
            ("limit", OrderType::Limit),
            ("market", OrderType::Market),
            ("stop-loss", OrderType::StopLoss),
            ("take-profit", OrderType::TakeProfit),
            ("stop-loss-limit", OrderType::StopLossLimit),
            ("take-profit-limit", OrderType::TakeProfitLimit),
        ];

        for (type_str, expected) in test_cases {
            let data = KrakenOrderData {
                descr: Some(KrakenOrderDescr {
                    ordertype: Some(type_str.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let event = KrakenWs::parse_order_update(&data, "test");
            assert_eq!(event.order.order_type, expected, "Failed for type: {}", type_str);
        }
    }

    #[test]
    fn test_clone() {
        let client = KrakenWs::with_credentials("test_key", "test_secret");
        let cloned = client.clone();
        assert_eq!(cloned.get_api_key(), Some("test_key"));
    }

    #[test]
    fn test_with_config() {
        let config = ExchangeConfig::new()
            .with_api_key("config_key")
            .with_api_secret("config_secret");
        let client = KrakenWs::with_config(config);
        assert_eq!(client.get_api_key(), Some("config_key"));
    }

    #[test]
    fn test_parse_balance_update() {
        let data = vec![
            KrakenBalanceData {
                asset: Some("XBT".to_string()),
                balance: Some("1.5".to_string()),
                hold_trade: Some("0.5".to_string()),
                hold_deposit: None,
            },
            KrakenBalanceData {
                asset: Some("USD".to_string()),
                balance: Some("10000.00".to_string()),
                hold_trade: Some("1000.00".to_string()),
                hold_deposit: None,
            },
        ];

        let event = KrakenWs::parse_balance_update(&data);
        assert_eq!(event.balances.currencies.len(), 2);

        let xbt = event.balances.get("XBT").unwrap();
        assert_eq!(xbt.free, Some(Decimal::from_str("1.5").unwrap()));
        assert_eq!(xbt.used, Some(Decimal::from_str("0.5").unwrap()));

        let usd = event.balances.get("USD").unwrap();
        assert_eq!(usd.free, Some(Decimal::from_str("10000.00").unwrap()));
        assert_eq!(usd.used, Some(Decimal::from_str("1000.00").unwrap()));
    }

    #[test]
    fn test_process_message_balance() {
        let msg = r#"[[{"asset":"XBT","balance":"1.5","hold_trade":"0.5"}],"balances",{"sequence":1}]"#;

        let result = KrakenWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Balance(event)) = result {
            assert_eq!(event.balances.currencies.len(), 1);
            let xbt = event.balances.get("XBT").unwrap();
            assert_eq!(xbt.free, Some(Decimal::from_str("1.5").unwrap()));
        } else {
            panic!("Expected Balance message");
        }
    }
}
