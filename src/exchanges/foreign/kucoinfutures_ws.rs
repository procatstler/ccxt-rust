//! KuCoin Futures WebSocket Implementation
//!
//! KuCoin Futures 실시간 데이터 스트리밍
//! Note: KuCoin Futures requires a token from REST API before connecting to WebSocket

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Position, PositionSide, MarginMode, Ticker, Timeframe, Trade, OHLCV, TakerOrMaker,
    WsBalanceEvent, WsExchange, WsMessage, WsMyTradeEvent, WsOrderBookEvent, WsOrderEvent,
    WsPositionEvent, WsTickerEvent, WsTradeEvent, WsOhlcvEvent,
};

/// KuCoin Futures WebSocket 클라이언트
pub struct KucoinfuturesWs {
    config: ExchangeConfig,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    connect_id: String,
}

impl Clone for KucoinfuturesWs {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            connect_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

/// KuCoin Futures WebSocket token response
#[derive(Debug, Deserialize)]
struct KfWsTokenResponse {
    code: String,
    data: Option<KfWsTokenData>,
}

#[derive(Debug, Deserialize)]
struct KfWsTokenData {
    token: String,
    #[serde(rename = "instanceServers")]
    instance_servers: Vec<KfWsInstanceServer>,
}

#[derive(Debug, Deserialize)]
struct KfWsInstanceServer {
    endpoint: String,
    protocol: String,
    encrypt: bool,
    #[serde(rename = "pingInterval")]
    ping_interval: i64,
    #[serde(rename = "pingTimeout")]
    ping_timeout: i64,
}

/// KuCoin Futures ticker data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsTickerData {
    symbol: String,
    sequence: Option<i64>,
    side: Option<String>,
    price: Option<String>,
    size: Option<i64>,
    #[serde(rename = "tradeId")]
    trade_id: Option<String>,
    #[serde(rename = "bestBidSize")]
    best_bid_size: Option<i64>,
    #[serde(rename = "bestBidPrice")]
    best_bid_price: Option<String>,
    #[serde(rename = "bestAskPrice")]
    best_ask_price: Option<String>,
    #[serde(rename = "bestAskSize")]
    best_ask_size: Option<i64>,
    ts: Option<i64>,
}

/// KuCoin Futures order book data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsOrderBookData {
    sequence: Option<i64>,
    change: Option<String>,
    timestamp: Option<i64>,
}

/// KuCoin Futures order book snapshot
#[derive(Debug, Deserialize, Serialize)]
struct KfWsOrderBookSnapshot {
    sequence: Option<i64>,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
    ts: Option<i64>,
}

/// KuCoin Futures trade data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsTradeData {
    sequence: Option<i64>,
    #[serde(rename = "tradeId")]
    trade_id: String,
    #[serde(rename = "makerOrderId")]
    maker_order_id: Option<String>,
    #[serde(rename = "takerOrderId")]
    taker_order_id: Option<String>,
    side: String,
    price: String,
    size: String,
    ts: i64,
    symbol: Option<String>,
}

/// KuCoin Futures OHLCV data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsOhlcvData {
    symbol: String,
    candles: Vec<String>,
    time: i64,
}

/// WebSocket message wrapper
#[derive(Debug, Deserialize)]
struct KfWsMessage {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    topic: Option<String>,
    subject: Option<String>,
    data: Option<serde_json::Value>,
}

// ============================================================================
// Private Channel Data Structures
// ============================================================================

/// KuCoin Futures order update from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsOrderData {
    symbol: String,
    #[serde(rename = "orderId")]
    order_id: String,
    #[serde(rename = "clientOid")]
    client_oid: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<String>,
    side: String,
    price: Option<String>,
    size: Option<String>,
    #[serde(rename = "filledSize")]
    filled_size: Option<String>,
    #[serde(rename = "filledValue")]
    filled_value: Option<String>,
    status: String,
    #[serde(rename = "orderTime")]
    order_time: Option<i64>,
    #[serde(rename = "ts")]
    timestamp: Option<i64>,
    #[serde(rename = "reduceOnly")]
    reduce_only: Option<bool>,
    #[serde(rename = "leverage")]
    leverage: Option<String>,
}

/// KuCoin Futures balance update from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsBalanceData {
    #[serde(rename = "availableBalance")]
    available_balance: Option<String>,
    #[serde(rename = "holdBalance")]
    hold_balance: Option<String>,
    currency: String,
    #[serde(rename = "timestamp")]
    timestamp: Option<i64>,
}

/// KuCoin Futures position update from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsPositionData {
    symbol: String,
    #[serde(rename = "currentQty")]
    current_qty: Option<String>,
    #[serde(rename = "avgEntryPrice")]
    avg_entry_price: Option<String>,
    #[serde(rename = "markPrice")]
    mark_price: Option<String>,
    #[serde(rename = "unrealisedPnl")]
    unrealised_pnl: Option<String>,
    #[serde(rename = "realisedPnl")]
    realised_pnl: Option<String>,
    #[serde(rename = "liquidationPrice")]
    liquidation_price: Option<String>,
    #[serde(rename = "leverage")]
    leverage: Option<String>,
    #[serde(rename = "marginType")]
    margin_type: Option<String>,
    #[serde(rename = "isOpen")]
    is_open: Option<bool>,
    #[serde(rename = "maintMarginReq")]
    maint_margin_req: Option<String>,
    #[serde(rename = "timestamp")]
    timestamp: Option<i64>,
}

/// KuCoin Futures trade/fill update from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsFillData {
    symbol: String,
    #[serde(rename = "tradeId")]
    trade_id: String,
    #[serde(rename = "orderId")]
    order_id: String,
    #[serde(rename = "clientOid")]
    client_oid: Option<String>,
    side: String,
    price: String,
    size: String,
    #[serde(rename = "filledQty")]
    filled_qty: Option<String>,
    fee: Option<String>,
    #[serde(rename = "feeCurrency")]
    fee_currency: Option<String>,
    #[serde(rename = "liquidity")]
    liquidity: Option<String>,
    #[serde(rename = "ts")]
    timestamp: i64,
}

impl KucoinfuturesWs {
    /// 새 KuCoin Futures WebSocket 클라이언트 생성
    pub fn new(config: ExchangeConfig) -> Self {
        Self {
            config,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            connect_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// 설정으로 클라이언트 생성 (with_config 패턴)
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self::new(config)
    }

    /// 심볼을 KuCoin Futures 마켓 ID로 변환
    fn to_market_id(symbol: &str) -> String {
        // BTC/USD:BTC -> XBTUSDM (perpetual)
        // BTC/USD:USD -> XBTUSDTM (USDT-margined perpetual)
        // ETH/USD:USD -> ETHUSDTM
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() != 2 {
            return symbol.to_uppercase();
        }

        let base = parts[0];
        let quote_settle: Vec<&str> = parts[1].split(':').collect();

        // Handle BTC -> XBT conversion
        let base_formatted = if base == "BTC" { "XBT" } else { base };

        if quote_settle.len() >= 2 {
            let quote = quote_settle[0];
            let settle = quote_settle[1];

            // Check for expiry date
            let settle_parts: Vec<&str> = settle.split('-').collect();

            if settle_parts.len() > 1 {
                // Delivery futures: XBTMUSDM_240628
                format!("{}{}M_{}", base_formatted, quote, settle_parts[1])
            } else if quote == "USD" && settle == base {
                // Coin-margined perpetual: XBTUSDM
                format!("{}USDM", base_formatted)
            } else {
                // USDT-margined perpetual: XBTUSDTM
                format!("{}USDTM", base_formatted)
            }
        } else {
            format!("{}USDTM", base_formatted)
        }
    }

    /// 마켓 ID를 통합 심볼로 변환
    fn to_unified_symbol(market_id: &str) -> String {
        let upper = market_id.to_uppercase();

        if upper.ends_with("USDM") {
            // Coin-margined: XBTUSDM -> BTC/USD:BTC
            let base = if upper.starts_with("XBT") {
                "BTC"
            } else if upper.starts_with("ETH") {
                "ETH"
            } else {
                let base_end = upper.len() - 4;
                return format!("{}/USD:USD", &upper[..base_end]);
            };
            format!("{}/USD:{}", base, base)
        } else if upper.ends_with("USDTM") {
            // USDT-margined: XBTUSDTM -> BTC/USDT:USDT
            let base = if upper.starts_with("XBT") {
                "BTC"
            } else if upper.starts_with("ETH") {
                "ETH"
            } else {
                let base_end = upper.len() - 5;
                return format!("{}/USDT:USDT", &upper[..base_end]);
            };
            format!("{}/USDT:USDT", base)
        } else {
            market_id.to_string()
        }
    }

    /// Timeframe을 KuCoin Futures 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1min",
            Timeframe::Minute5 => "5min",
            Timeframe::Minute15 => "15min",
            Timeframe::Minute30 => "30min",
            Timeframe::Hour1 => "1hour",
            Timeframe::Hour2 => "2hour",
            Timeframe::Hour4 => "4hour",
            Timeframe::Hour8 => "8hour",
            Timeframe::Hour12 => "12hour",
            Timeframe::Day1 => "1day",
            Timeframe::Week1 => "1week",
            _ => "1min",
        }
    }

    /// Public 토큰 가져오기 (WebSocket 연결에 필요)
    async fn get_public_token() -> CcxtResult<(String, i64)> {
        let url = "https://api-futures.kucoin.com/api/v1/bullet-public";
        let client = reqwest::Client::new();
        let response = client
            .post(url)
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError { url: url.to_string(), message: e.to_string() })?;

        let token_response: KfWsTokenResponse = response
            .json()
            .await
            .map_err(|e| CcxtError::ParseError { data_type: "KfWsTokenResponse".to_string(), message: e.to_string() })?;

        let data = token_response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to get WebSocket token".into(),
        })?;

        let server = data.instance_servers.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "No WebSocket server available".into(),
        })?;

        let ws_url = format!("{}?token={}&connectId={}",
            server.endpoint,
            data.token,
            uuid::Uuid::new_v4()
        );

        Ok((ws_url, server.ping_interval))
    }

    /// Private 토큰 가져오기 (인증된 WebSocket 연결에 필요)
    async fn get_private_token(&self) -> CcxtResult<(String, i64)> {
        let url = "https://api-futures.kucoin.com/api/v1/bullet-private";
        let timestamp = Utc::now().timestamp_millis().to_string();

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required for private WebSocket".to_string(),
        })?;

        let api_secret = self.config.api_secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required for private WebSocket".to_string(),
        })?;

        let passphrase = self.config.password().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Passphrase required for KuCoin Futures private WebSocket".to_string(),
        })?;

        // Create signature: timestamp + method + endpoint
        let str_to_sign = format!("{}POST/api/v1/bullet-private", timestamp);

        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        type HmacSha256 = Hmac<Sha256>;

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Failed to create HMAC: {}", e),
            })?;
        mac.update(str_to_sign.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());

        // Sign passphrase as well
        let mut passphrase_mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Failed to create HMAC for passphrase: {}", e),
            })?;
        passphrase_mac.update(passphrase.as_bytes());
        let signed_passphrase = STANDARD.encode(passphrase_mac.finalize().into_bytes());

        let client = reqwest::Client::new();
        let response = client
            .post(url)
            .header("KC-API-KEY", api_key)
            .header("KC-API-SIGN", &signature)
            .header("KC-API-TIMESTAMP", &timestamp)
            .header("KC-API-PASSPHRASE", &signed_passphrase)
            .header("KC-API-KEY-VERSION", "2")
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError { url: url.to_string(), message: e.to_string() })?;

        let token_response: KfWsTokenResponse = response
            .json()
            .await
            .map_err(|e| CcxtError::ParseError { data_type: "KfWsTokenResponse".to_string(), message: e.to_string() })?;

        if token_response.code != "200000" {
            return Err(CcxtError::AuthenticationError {
                message: format!("Failed to get private token: {}", token_response.code),
            });
        }

        let data = token_response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to get WebSocket token data".into(),
        })?;

        let server = data.instance_servers.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "No WebSocket server available".into(),
        })?;

        let ws_url = format!("{}?token={}&connectId={}",
            server.endpoint,
            data.token,
            uuid::Uuid::new_v4()
        );

        Ok((ws_url, server.ping_interval))
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &KfWsTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.ts.map(|t| t / 1_000_000).unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: None,
            low: None,
            bid: data.best_bid_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.best_bid_size.map(|v| Decimal::from(v)),
            ask: data.best_ask_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.best_ask_size.map(|v| Decimal::from(v)),
            vwap: None,
            open: None,
            close: data.price.as_ref().and_then(|v| v.parse().ok()),
            last: data.price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: None,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 스냅샷 파싱
    fn parse_order_book_snapshot(data: &KfWsOrderBookSnapshot, market_id: &str) -> WsOrderBookEvent {
        let symbol = Self::to_unified_symbol(market_id);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: b[0].parse().ok()?,
                    amount: b[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: a[0].parse().ok()?,
                    amount: a[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let order_book = OrderBook {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.sequence,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol,
            order_book,
            is_snapshot: true,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &KfWsTradeData, market_id: &str) -> Trade {
        let symbol = data.symbol.as_ref().map(|s| Self::to_unified_symbol(s))
            .unwrap_or_else(|| Self::to_unified_symbol(market_id));
        let timestamp = data.ts;
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.size.parse().unwrap_or_default();

        Trade {
            id: data.trade_id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol,
            trade_type: None,
            side: Some(data.side.clone()),
            taker_or_maker: Some(TakerOrMaker::Taker),
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// OHLCV 메시지 파싱
    fn parse_ohlcv(data: &KfWsOhlcvData, topic: &str) -> Option<WsOhlcvEvent> {
        let symbol = Self::to_unified_symbol(&data.symbol);

        // Extract timeframe from topic: /contractMarket/limitCandle:XBTUSDTM_1min
        let timeframe = if let Some(interval_part) = topic.split('_').last() {
            match interval_part {
                "1min" => Timeframe::Minute1,
                "5min" => Timeframe::Minute5,
                "15min" => Timeframe::Minute15,
                "30min" => Timeframe::Minute30,
                "1hour" => Timeframe::Hour1,
                "2hour" => Timeframe::Hour2,
                "4hour" => Timeframe::Hour4,
                "8hour" => Timeframe::Hour8,
                "12hour" => Timeframe::Hour12,
                "1day" => Timeframe::Day1,
                "1week" => Timeframe::Week1,
                _ => Timeframe::Minute1,
            }
        } else {
            Timeframe::Minute1
        };

        // candles: ["1545184800000", "0.0105", "0.0105", "0.0105", "0.0105", "0"]
        // [timestamp, open, close, high, low, volume]
        if data.candles.len() < 6 {
            return None;
        }

        let ohlcv = OHLCV {
            timestamp: data.candles[0].parse().ok()?,
            open: data.candles[1].parse().ok()?,
            high: data.candles[3].parse().ok()?,
            low: data.candles[4].parse().ok()?,
            close: data.candles[2].parse().ok()?,
            volume: data.candles[5].parse().ok()?,
        };

        Some(WsOhlcvEvent { symbol, timeframe, ohlcv })
    }

    // ========================================================================
    // Private Channel Parse Functions
    // ========================================================================

    /// 주문 업데이트 파싱
    fn parse_order_update(data: &KfWsOrderData) -> WsOrderEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.timestamp.or(data.order_time).unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = match data.side.to_lowercase().as_str() {
            "buy" => OrderSide::Buy,
            _ => OrderSide::Sell,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            Some("stop") => OrderType::StopMarket,
            Some("stop_limit") => OrderType::StopLimit,
            _ => OrderType::Limit,
        };

        let status = match data.status.as_str() {
            "open" | "new" => OrderStatus::Open,
            "match" | "partially_filled" => OrderStatus::Open,
            "done" | "filled" => OrderStatus::Closed,
            "canceled" | "cancelled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let price = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount = data.size.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
        let filled = data.filled_size.as_ref().and_then(|f| f.parse().ok()).unwrap_or_default();

        let order = Order {
            id: data.order_id.clone(),
            client_order_id: data.client_oid.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.timestamp,
            symbol: symbol.clone(),
            order_type,
            side,
            price,
            average: None,
            amount,
            filled,
            remaining: Some(amount - filled),
            cost: data.filled_value.as_ref().and_then(|v| v.parse().ok()),
            status,
            fee: None,
            fees: Vec::new(),
            trades: Vec::new(),
            reduce_only: data.reduce_only,
            post_only: None,
            time_in_force: None,
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            trigger_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// 잔고 업데이트 파싱
    fn parse_balance_update(data: &KfWsBalanceData) -> WsBalanceEvent {
        let free: Decimal = data.available_balance.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let used: Decimal = data.hold_balance.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let total = free + used;
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let mut balances = HashMap::new();
        balances.insert(
            data.currency.clone(),
            Balance {
                free: Some(free),
                used: Some(used),
                total: Some(total),
                debt: None,
            },
        );

        WsBalanceEvent {
            balances: Balances {
                info: serde_json::to_value(data).unwrap_or_default(),
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                currencies: balances,
            },
        }
    }

    /// 포지션 업데이트 파싱
    fn parse_position_update(data: &KfWsPositionData) -> WsPositionEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let contracts: Decimal = data.current_qty.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        let side = if contracts > Decimal::ZERO {
            Some(PositionSide::Long)
        } else if contracts < Decimal::ZERO {
            Some(PositionSide::Short)
        } else {
            None
        };

        let margin_mode = match data.margin_type.as_deref() {
            Some("isolated") => Some(MarginMode::Isolated),
            Some("cross") => Some(MarginMode::Cross),
            _ => None,
        };

        let position = Position {
            id: None,
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_update_timestamp: Some(timestamp),
            hedged: Some(false),
            side,
            contracts: Some(contracts.abs()),
            contract_size: None,
            entry_price: data.avg_entry_price.as_ref().and_then(|v| v.parse().ok()),
            mark_price: data.mark_price.as_ref().and_then(|v| v.parse().ok()),
            last_price: None,
            notional: None,
            leverage: data.leverage.as_ref().and_then(|v| v.parse().ok()),
            collateral: None,
            initial_margin: None,
            maintenance_margin: data.maint_margin_req.as_ref().and_then(|v| v.parse().ok()),
            initial_margin_percentage: None,
            maintenance_margin_percentage: None,
            unrealized_pnl: data.unrealised_pnl.as_ref().and_then(|v| v.parse().ok()),
            realized_pnl: data.realised_pnl.as_ref().and_then(|v| v.parse().ok()),
            liquidation_price: data.liquidation_price.as_ref().and_then(|v| v.parse().ok()),
            margin_mode,
            margin_ratio: None,
            percentage: None,
            stop_loss_price: None,
            take_profit_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsPositionEvent { positions: vec![position] }
    }

    /// 체결 업데이트 파싱 (내 거래)
    fn parse_fill_update(data: &KfWsFillData) -> WsMyTradeEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.size.parse().unwrap_or_default();
        let timestamp = data.timestamp;

        let _side = match data.side.to_lowercase().as_str() {
            "buy" => OrderSide::Buy,
            _ => OrderSide::Sell,
        };

        let taker_or_maker = match data.liquidity.as_deref() {
            Some("maker") => TakerOrMaker::Maker,
            Some("taker") => TakerOrMaker::Taker,
            _ => TakerOrMaker::Taker,
        };

        let fee = data.fee.as_ref().and_then(|f| f.parse().ok()).map(|cost: Decimal| {
            crate::types::Fee {
                currency: data.fee_currency.clone(),
                cost: Some(cost),
                rate: None,
            }
        });

        let trade = Trade {
            id: data.trade_id.clone(),
            order: Some(data.order_id.clone()),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(data.side.clone()),
            taker_or_maker: Some(taker_or_maker),
            price,
            amount,
            cost: Some(price * amount),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsMyTradeEvent { symbol, trades: vec![trade] }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        let wrapper: KfWsMessage = serde_json::from_str(msg).ok()?;

        // Skip non-message types
        if wrapper.msg_type.as_deref() != Some("message") {
            return None;
        }

        let topic = wrapper.topic.as_ref()?;
        let data = wrapper.data.as_ref()?;
        let subject = wrapper.subject.as_deref();

        // Extract market ID from topic
        let market_id = topic.split(':').last().unwrap_or("");

        match subject {
            Some("ticker") => {
                if let Ok(ticker_data) = serde_json::from_value::<KfWsTickerData>(data.clone()) {
                    return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                }
            }
            Some("level2") => {
                if topic.contains("level2Depth") {
                    if let Ok(snapshot) = serde_json::from_value::<KfWsOrderBookSnapshot>(data.clone()) {
                        return Some(WsMessage::OrderBook(Self::parse_order_book_snapshot(&snapshot, market_id)));
                    }
                }
            }
            // Private channels - check first with specific topic conditions
            Some("orderChange") | Some("orderUpdate") => {
                if let Ok(order_data) = serde_json::from_value::<KfWsOrderData>(data.clone()) {
                    return Some(WsMessage::Order(Self::parse_order_update(&order_data)));
                }
            }
            Some("availableBalance") | Some("orderMargin") => {
                if let Ok(balance_data) = serde_json::from_value::<KfWsBalanceData>(data.clone()) {
                    return Some(WsMessage::Balance(Self::parse_balance_update(&balance_data)));
                }
            }
            Some("position.change") | Some("position") => {
                if let Ok(position_data) = serde_json::from_value::<KfWsPositionData>(data.clone()) {
                    return Some(WsMessage::Position(Self::parse_position_update(&position_data)));
                }
            }
            Some("match") => {
                // Check if this is a private trade fill first
                if topic.contains("/contractMarket/tradeOrders") {
                    if let Ok(fill_data) = serde_json::from_value::<KfWsFillData>(data.clone()) {
                        return Some(WsMessage::MyTrade(Self::parse_fill_update(&fill_data)));
                    }
                }
                // Otherwise it's a public trade
                if let Ok(trade_data) = serde_json::from_value::<KfWsTradeData>(data.clone()) {
                    let trade = Self::parse_trade(&trade_data, market_id);
                    let symbol = trade.symbol.clone();
                    return Some(WsMessage::Trade(WsTradeEvent {
                        symbol,
                        trades: vec![trade],
                    }));
                }
            }
            Some("candle") => {
                if let Ok(ohlcv_data) = serde_json::from_value::<KfWsOhlcvData>(data.clone()) {
                    if let Some(event) = Self::parse_ohlcv(&ohlcv_data, topic) {
                        return Some(WsMessage::Ohlcv(event));
                    }
                }
            }
            _ => {}
        }

        // Also try to match by topic for private channels
        if let Some(topic_str) = wrapper.topic.as_ref() {
            if topic_str.contains("/contractMarket/tradeOrders") {
                if let Ok(order_data) = serde_json::from_value::<KfWsOrderData>(data.clone()) {
                    return Some(WsMessage::Order(Self::parse_order_update(&order_data)));
                }
                if let Ok(fill_data) = serde_json::from_value::<KfWsFillData>(data.clone()) {
                    return Some(WsMessage::MyTrade(Self::parse_fill_update(&fill_data)));
                }
            }
            if topic_str.contains("/contractAccount/wallet") {
                if let Ok(balance_data) = serde_json::from_value::<KfWsBalanceData>(data.clone()) {
                    return Some(WsMessage::Balance(Self::parse_balance_update(&balance_data)));
                }
            }
            if topic_str.contains("/contract/position") {
                if let Ok(position_data) = serde_json::from_value::<KfWsPositionData>(data.clone()) {
                    return Some(WsMessage::Position(Self::parse_position_update(&position_data)));
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        topic: String,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // Get WebSocket URL with token
        let (ws_url, ping_interval) = Self::get_public_token().await?;

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: (ping_interval / 1000) as u64,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Build subscription message
        let request_id = uuid::Uuid::new_v4().to_string();
        let subscribe_msg = serde_json::json!({
            "id": request_id,
            "type": "subscribe",
            "topic": topic,
            "response": true
        });

        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            self.subscriptions.write().await.insert(topic.clone(), topic.clone());
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

    /// Private 채널 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_private_stream(
        &mut self,
        topic: String,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // Get Private WebSocket URL with authenticated token
        let (ws_url, ping_interval) = self.get_private_token().await?;

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: (ping_interval / 1000) as u64,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Build subscription message
        let request_id = uuid::Uuid::new_v4().to_string();
        let subscribe_msg = serde_json::json!({
            "id": request_id,
            "type": "subscribe",
            "topic": topic,
            "privateChannel": true,
            "response": true
        });

        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            self.subscriptions.write().await.insert(topic.clone(), topic.clone());
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

impl Default for KucoinfuturesWs {
    fn default() -> Self {
        Self::new(ExchangeConfig::default())
    }
}

#[async_trait]
impl WsExchange for KucoinfuturesWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        let topic = format!("/contractMarket/ticker:{}", market_id);
        client.subscribe_stream(topic).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_ids: Vec<String> = symbols.iter()
            .map(|s| Self::to_market_id(s))
            .collect();
        let topic = format!("/contractMarket/ticker:{}", market_ids.join(","));
        client.subscribe_stream(topic).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        let depth = limit.unwrap_or(50);
        let topic = format!("/contractMarket/level2Depth{}:{}", depth, market_id);
        client.subscribe_stream(topic).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        let topic = format!("/contractMarket/execution:{}", market_id);
        client.subscribe_stream(topic).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        let interval = Self::format_interval(timeframe);
        let topic = format!("/contractMarket/limitCandle:{}_{}", market_id, interval);
        client.subscribe_stream(topic).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ref ws_client) = self.ws_client {
            ws_client.close()?;
        }
        self.ws_client = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        if let Some(ref ws_client) = self.ws_client {
            ws_client.is_connected().await
        } else {
            false
        }
    }

    // ========================================================================
    // Private Channel Methods
    // ========================================================================

    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        // KuCoin Futures: /contractMarket/tradeOrders
        let topic = "/contractMarket/tradeOrders".to_string();
        client.subscribe_private_stream(topic).await
    }

    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        // KuCoin Futures uses tradeOrders for both order updates and fills
        let topic = "/contractMarket/tradeOrders".to_string();
        client.subscribe_private_stream(topic).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        // KuCoin Futures: /contractAccount/wallet
        let topic = "/contractAccount/wallet".to_string();
        client.subscribe_private_stream(topic).await
    }

    async fn watch_positions(&self, _symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        // KuCoin Futures: /contract/position
        let topic = "/contract/position".to_string();
        client.subscribe_private_stream(topic).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_market_id() {
        assert_eq!(KucoinfuturesWs::to_market_id("BTC/USD:BTC"), "XBTUSDM");
        assert_eq!(KucoinfuturesWs::to_market_id("BTC/USDT:USDT"), "XBTUSDTM");
        assert_eq!(KucoinfuturesWs::to_market_id("ETH/USD:ETH"), "ETHUSDM");
        assert_eq!(KucoinfuturesWs::to_market_id("ETH/USDT:USDT"), "ETHUSDTM");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(KucoinfuturesWs::to_unified_symbol("XBTUSDM"), "BTC/USD:BTC");
        assert_eq!(KucoinfuturesWs::to_unified_symbol("XBTUSDTM"), "BTC/USDT:USDT");
        assert_eq!(KucoinfuturesWs::to_unified_symbol("ETHUSDM"), "ETH/USD:ETH");
        assert_eq!(KucoinfuturesWs::to_unified_symbol("ETHUSDTM"), "ETH/USDT:USDT");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(KucoinfuturesWs::format_interval(Timeframe::Minute1), "1min");
        assert_eq!(KucoinfuturesWs::format_interval(Timeframe::Hour1), "1hour");
        assert_eq!(KucoinfuturesWs::format_interval(Timeframe::Day1), "1day");
    }

    // ========================================================================
    // Private Channel Tests
    // ========================================================================

    #[test]
    fn test_parse_order_update() {
        let json = r#"{
            "symbol": "XBTUSDTM",
            "orderId": "12345678901234567890",
            "clientOid": "my_order_123",
            "type": "limit",
            "side": "buy",
            "price": "50000.0",
            "size": "0.1",
            "filledSize": "0.05",
            "filledValue": "2500",
            "status": "open",
            "orderTime": 1700000000000,
            "ts": 1700000001000,
            "reduceOnly": false
        }"#;

        let data: KfWsOrderData = serde_json::from_str(json).unwrap();
        let event = KucoinfuturesWs::parse_order_update(&data);

        assert_eq!(event.order.id, "12345678901234567890");
        assert_eq!(event.order.symbol, "BTC/USDT:USDT");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
        assert_eq!(event.order.status, OrderStatus::Open);
        assert!(event.order.price.is_some());
    }

    #[test]
    fn test_parse_balance_update() {
        let json = r#"{
            "availableBalance": "10000.50",
            "holdBalance": "500.25",
            "currency": "USDT",
            "timestamp": 1700000000000
        }"#;

        let data: KfWsBalanceData = serde_json::from_str(json).unwrap();
        let event = KucoinfuturesWs::parse_balance_update(&data);

        let usdt_balance = event.balances.currencies.get("USDT").unwrap();
        assert!(usdt_balance.free.is_some());
        assert!(usdt_balance.used.is_some());
        assert!(usdt_balance.total.is_some());
    }

    #[test]
    fn test_parse_position_update() {
        let json = r#"{
            "symbol": "XBTUSDTM",
            "currentQty": "1.5",
            "avgEntryPrice": "50000.0",
            "markPrice": "51000.0",
            "unrealisedPnl": "1500.0",
            "realisedPnl": "200.0",
            "liquidationPrice": "40000.0",
            "leverage": "10",
            "marginType": "cross",
            "isOpen": true,
            "timestamp": 1700000000000
        }"#;

        let data: KfWsPositionData = serde_json::from_str(json).unwrap();
        let event = KucoinfuturesWs::parse_position_update(&data);

        assert!(!event.positions.is_empty());
        let position = &event.positions[0];
        assert_eq!(position.symbol, "BTC/USDT:USDT");
        assert_eq!(position.side, Some(PositionSide::Long));
        assert!(position.contracts.is_some());
        assert!(position.entry_price.is_some());
    }

    #[test]
    fn test_parse_position_short() {
        let json = r#"{
            "symbol": "ETHUSDTM",
            "currentQty": "-2.0",
            "avgEntryPrice": "2000.0",
            "markPrice": "1900.0",
            "unrealisedPnl": "200.0",
            "marginType": "isolated",
            "timestamp": 1700000000000
        }"#;

        let data: KfWsPositionData = serde_json::from_str(json).unwrap();
        let event = KucoinfuturesWs::parse_position_update(&data);

        assert!(!event.positions.is_empty());
        let position = &event.positions[0];
        assert_eq!(position.symbol, "ETH/USDT:USDT");
        assert_eq!(position.side, Some(PositionSide::Short));
        assert_eq!(position.margin_mode, Some(MarginMode::Isolated));
    }

    #[test]
    fn test_parse_fill_update() {
        let json = r#"{
            "symbol": "XBTUSDTM",
            "tradeId": "trade123456789",
            "orderId": "order987654321",
            "clientOid": "client_order_1",
            "side": "sell",
            "price": "51000.0",
            "size": "0.1",
            "fee": "2.55",
            "feeCurrency": "USDT",
            "liquidity": "taker",
            "ts": 1700000000000
        }"#;

        let data: KfWsFillData = serde_json::from_str(json).unwrap();
        let event = KucoinfuturesWs::parse_fill_update(&data);

        assert_eq!(event.symbol, "BTC/USDT:USDT");
        assert!(!event.trades.is_empty());
        let trade = &event.trades[0];
        assert_eq!(trade.id, "trade123456789");
        assert_eq!(trade.order, Some("order987654321".to_string()));
        assert!(trade.fee.is_some());
    }

    #[test]
    fn test_parse_fill_maker() {
        let json = r#"{
            "symbol": "ETHUSDM",
            "tradeId": "fill_abc",
            "orderId": "order_xyz",
            "side": "buy",
            "price": "2000.0",
            "size": "5.0",
            "fee": "0.5",
            "feeCurrency": "ETH",
            "liquidity": "maker",
            "ts": 1700000000000
        }"#;

        let data: KfWsFillData = serde_json::from_str(json).unwrap();
        let event = KucoinfuturesWs::parse_fill_update(&data);

        assert_eq!(event.symbol, "ETH/USD:ETH");
        assert!(!event.trades.is_empty());
        assert_eq!(event.trades[0].taker_or_maker, Some(TakerOrMaker::Maker));
    }

    #[test]
    fn test_private_process_message_order() {
        let json = r#"{
            "type": "message",
            "topic": "/contractMarket/tradeOrders",
            "subject": "orderChange",
            "data": {
                "symbol": "XBTUSDTM",
                "orderId": "123456",
                "side": "buy",
                "type": "limit",
                "price": "50000",
                "size": "0.1",
                "status": "open"
            }
        }"#;

        let result = KucoinfuturesWs::process_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "123456");
        }
    }

    #[test]
    fn test_private_process_message_balance() {
        let json = r#"{
            "type": "message",
            "topic": "/contractAccount/wallet",
            "subject": "availableBalance",
            "data": {
                "availableBalance": "5000.0",
                "holdBalance": "100.0",
                "currency": "USDT",
                "timestamp": 1700000000000
            }
        }"#;

        let result = KucoinfuturesWs::process_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("USDT"));
        }
    }

    #[test]
    fn test_private_process_message_position() {
        let json = r#"{
            "type": "message",
            "topic": "/contract/position",
            "subject": "position.change",
            "data": {
                "symbol": "XBTUSDTM",
                "currentQty": "2.0",
                "avgEntryPrice": "50000.0",
                "markPrice": "50500.0",
                "timestamp": 1700000000000
            }
        }"#;

        let result = KucoinfuturesWs::process_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Position(event)) = result {
            assert!(!event.positions.is_empty());
            assert_eq!(event.positions[0].symbol, "BTC/USDT:USDT");
        }
    }

    #[test]
    fn test_with_config() {
        let config = ExchangeConfig::default()
            .with_api_key("test_key")
            .with_api_secret("test_secret")
            .with_password("test_passphrase");
        let client = KucoinfuturesWs::with_config(config.clone());
        assert_eq!(client.config.api_key(), config.api_key());
        assert_eq!(client.config.api_secret(), config.api_secret());
        assert_eq!(client.config.password(), config.password());
    }

    #[test]
    fn test_clone() {
        let config = ExchangeConfig::default()
            .with_api_key("test_key");
        let client = KucoinfuturesWs::new(config);
        let cloned = client.clone();
        assert_eq!(cloned.config.api_key(), client.config.api_key());
    }

    #[test]
    fn test_order_status_mapping() {
        let test_cases = vec![
            ("open", OrderStatus::Open),
            ("new", OrderStatus::Open),
            ("match", OrderStatus::Open),
            ("done", OrderStatus::Closed),
            ("filled", OrderStatus::Closed),
            ("canceled", OrderStatus::Canceled),
            ("cancelled", OrderStatus::Canceled),
        ];

        for (status_str, expected) in test_cases {
            let json = format!(r#"{{
                "symbol": "XBTUSDTM",
                "orderId": "123",
                "side": "buy",
                "status": "{}"
            }}"#, status_str);

            let data: KfWsOrderData = serde_json::from_str(&json).unwrap();
            let event = KucoinfuturesWs::parse_order_update(&data);
            assert_eq!(event.order.status, expected, "Failed for status: {}", status_str);
        }
    }
}
