//! Bitso Exchange Implementation
//!
//! Mexican cryptocurrency exchange supporting MXN trading pairs

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

#[allow(dead_code)]
type HmacSha256 = Hmac<Sha256>;

/// Bitso exchange
pub struct Bitso {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Bitso {
    const BASE_URL: &'static str = "https://api.bitso.com/v3";
    const RATE_LIMIT_MS: u64 = 1000; // 60 requests per minute

    /// Create new Bitso instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            fetch_markets: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: false,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            cancel_order: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27766335-715ce7aa-5ed5-11e7-88a8-173a27bb30fe.jpg".into()),
            api: api_urls,
            www: Some("https://bitso.com".into()),
            doc: vec!["https://bitso.com/api_info".into()],
            fees: Some("https://bitso.com/fees".into()),
        };

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes: HashMap::new(), // No OHLCV support via REST
        })
    }

    /// Public API call
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Private API call
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let api_secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        let nonce = Utc::now().timestamp_millis().to_string();
        let request_path = format!("/v3{path}");
        let body_str = body.unwrap_or("");

        let message = format!(
            "{}{}{}{}",
            nonce,
            method.to_uppercase(),
            request_path,
            body_str
        );

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(message.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let auth_header = format!("Bitso {api_key}:{nonce}:{signature}");

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), auth_header);
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => self.client.get(path, None, Some(headers)).await,
            "POST" => {
                let body_value = if body_str.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(body_str).unwrap_or(serde_json::Value::Null))
                };
                self.client.post(path, body_value, Some(headers)).await
            },
            "DELETE" => self.client.delete(path, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method {method}"),
            }),
        }
    }

    /// Convert symbol to market ID (BTC/MXN -> btc_mxn)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace('/', "_").to_lowercase()
    }

    /// Parse market from API response
    fn parse_market(&self, data: &serde_json::Value) -> Option<Market> {
        let id = data.get("book")?.as_str()?;
        let parts: Vec<&str> = id.split('_').collect();
        if parts.len() != 2 {
            return None;
        }

        let base = parts[0].to_uppercase();
        let quote = parts[1].to_uppercase();
        let symbol = format!("{base}/{quote}");

        Some(Market {
            id: id.to_string(),
            lowercase_id: Some(id.to_string()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: parts[0].to_string(),
            quote_id: parts[1].to_string(),
            market_type: MarketType::Spot,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            index: false,
            active: true,
            contract: false,
            linear: None,
            inverse: None,
            sub_type: None,
            taker: Some(Decimal::new(65, 4)), // 0.0065 = 0.65%
            maker: Some(Decimal::new(65, 4)), // 0.0065 = 0.65%
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            underlying: None,
            underlying_id: None,
            settle: None,
            settle_id: None,
            precision: MarketPrecision {
                amount: Some(8),
                price: Some(2),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits::default(),
            margin_modes: None,
            created: None,
            info: data.clone(),
            tier_based: false,
            percentage: true,
        })
    }
}

#[async_trait]
impl Exchange for Bitso {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bitso
    }

    fn name(&self) -> &str {
        "Bitso"
    }

    fn has(&self) -> &ExchangeFeatures {
        &self.features
    }

    fn urls(&self) -> &ExchangeUrls {
        &self.urls
    }

    fn timeframes(&self) -> &HashMap<Timeframe, String> {
        &self.timeframes
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        {
            let cache = self.markets.read().unwrap();
            if !reload && !cache.is_empty() {
                return Ok(cache.clone());
            }
        }

        let markets = self.fetch_markets().await?;
        let mut result = HashMap::new();

        let mut cache = self.markets.write().unwrap();
        let mut by_id = self.markets_by_id.write().unwrap();

        for market in markets {
            result.insert(market.symbol.clone(), market.clone());
            cache.insert(market.symbol.clone(), market.clone());
            by_id.insert(market.id.clone(), market.symbol.clone());
        }

        Ok(result)
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        let cache = self.markets.read().unwrap();
        cache.get(symbol).map(|m| m.id.clone())
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let by_id = self.markets_by_id.read().unwrap();
        by_id.get(market_id).cloned()
    }

    fn sign(
        &self,
        path: &str,
        _api: &str,
        method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        SignedRequest {
            url: path.to_string(),
            method: method.to_string(),
            headers: HashMap::new(),
            body: None,
        }
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        #[derive(Deserialize)]
        struct Response {
            success: bool,
            payload: Vec<serde_json::Value>,
        }

        let response: Response = self.public_get("/available_books", None).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: "Failed to fetch markets".into(),
            });
        }

        let mut markets = Vec::new();
        for data in &response.payload {
            if let Some(market) = self.parse_market(data) {
                markets.push(market);
            }
        }

        // Cache markets
        let mut cache = self.markets.write().unwrap();
        let mut by_id = self.markets_by_id.write().unwrap();
        for market in &markets {
            cache.insert(market.symbol.clone(), market.clone());
            by_id.insert(market.id.clone(), market.symbol.clone());
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.market_id(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        #[derive(Deserialize)]
        struct Response {
            success: bool,
            payload: BitsoTicker,
        }

        let mut params = HashMap::new();
        params.insert("book".into(), market_id);

        let response: Response = self.public_get("/ticker", Some(params)).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: "Failed to fetch ticker".into(),
            });
        }

        let data = response.payload;
        let timestamp = if let Some(ts_str) = &data.created_at {
            chrono::DateTime::parse_from_rfc3339(ts_str)
                .ok()
                .map(|dt| dt.timestamp_millis())
        } else {
            None
        };

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: timestamp.or(Some(Utc::now().timestamp_millis())),
            datetime: Some(Utc::now().to_rfc3339()),
            high: data.high,
            low: data.low,
            bid: data.bid,
            bid_volume: None,
            ask: data.ask,
            ask_volume: None,
            vwap: data.vwap,
            open: None,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&data).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.market_id(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        #[derive(Deserialize)]
        struct Response {
            success: bool,
            payload: BitsoOrderBook,
        }

        let mut params = HashMap::new();
        params.insert("book".into(), market_id);
        if let Some(_l) = limit {
            params.insert("aggregate".into(), "false".into());
        }

        let response: Response = self.public_get("/order_book", Some(params)).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: "Failed to fetch order book".into(),
            });
        }

        let data = response.payload;
        let timestamp = if let Some(ts_str) = &data.updated_at {
            chrono::DateTime::parse_from_rfc3339(ts_str)
                .ok()
                .map(|dt| dt.timestamp_millis())
        } else {
            None
        };

        let parse_entries = |entries: &Vec<BitsoOrderBookEntry>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().map(|e| OrderBookEntry {
                price: e.price,
                amount: e.amount,
            });
            if let Some(l) = limit {
                iter.take(l as usize).collect()
            } else {
                iter.collect()
            }
        };

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: timestamp.or(Some(Utc::now().timestamp_millis())),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: data.sequence,
            bids: parse_entries(&data.bids),
            asks: parse_entries(&data.asks),
            checksum: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.market_id(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        #[derive(Deserialize)]
        struct Response {
            success: bool,
            payload: Vec<BitsoTrade>,
        }

        let mut params = HashMap::new();
        params.insert("book".into(), market_id);

        let response: Response = self.public_get("/trades", Some(params)).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: "Failed to fetch trades".into(),
            });
        }

        let limit = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = response
            .payload
            .iter()
            .take(limit)
            .map(|t| {
                let timestamp = t
                    .created_at
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis());

                Trade {
                    id: t.tid.to_string(),
                    order: None,
                    timestamp,
                    datetime: timestamp.map(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: t.maker_side.clone(),
                    taker_or_maker: None,
                    price: t.price,
                    amount: t.amount,
                    cost: None,
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                }
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        Err(CcxtError::NotSupported {
            feature: "fetch_ohlcv".to_string(),
        })
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        #[derive(Deserialize)]
        struct Response {
            success: bool,
            payload: BitsoBalance,
        }

        let response: Response = self.private_request("GET", "/balance", None).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: "Failed to fetch balance".into(),
            });
        }

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for balance in response.payload.balances {
            let currency = balance.currency.to_uppercase();
            balances.currencies.insert(
                currency,
                Balance {
                    free: Some(balance.available),
                    used: Some(balance.locked),
                    total: Some(balance.total),
                    debt: None,
                },
            );
        }

        Ok(balances)
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let market_id = self.market_id(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let mut order_data = serde_json::json!({
            "book": market_id,
            "side": match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            },
            "type": match order_type {
                OrderType::Limit => "limit",
                OrderType::Market => "market",
                _ => return Err(CcxtError::NotSupported {
                    feature: format!("Order type {order_type:?}"),
                }),
            },
            "major": amount.to_string()
        });

        if order_type == OrderType::Limit {
            let p = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Price is required for limit orders".into(),
            })?;
            order_data["price"] = serde_json::json!(p.to_string());
        }

        let body = serde_json::to_string(&order_data).map_err(|e| CcxtError::ExchangeError {
            message: format!("Failed to serialize order: {e}"),
        })?;

        #[derive(Deserialize)]
        struct Response {
            success: bool,
            payload: BitsoOrderResponse,
        }

        let response: Response = self.private_request("POST", "/orders", Some(&body)).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: "Failed to create order".into(),
            });
        }

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: response.payload.oid.clone(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&response.payload).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        #[derive(Deserialize)]
        struct Response {
            success: bool,
        }

        let endpoint = format!("/orders/{id}");
        let response: Response = self.private_request("DELETE", &endpoint, None).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: "Failed to cancel order".into(),
            });
        }

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: _symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price: None,
            average: None,
            amount: Decimal::ZERO,
            filled: Decimal::ZERO,
            remaining: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::Value::Null,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        #[derive(Deserialize)]
        struct Response {
            success: bool,
            payload: Vec<BitsoOrderDetail>,
        }

        let endpoint = format!("/orders/{id}");
        let response: Response = self.private_request("GET", &endpoint, None).await?;

        if !response.success || response.payload.is_empty() {
            return Err(CcxtError::OrderNotFound {
                order_id: id.to_string(),
            });
        }

        let o = &response.payload[0];
        let timestamp = o
            .created_at
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis());

        let status = match o.status.as_deref() {
            Some("queued") | Some("active") | Some("open") | Some("partially_filled") => {
                OrderStatus::Open
            },
            Some("complete") | Some("completed") => OrderStatus::Closed,
            Some("cancelled") | Some("canceled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let side = match o.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match o.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let filled = o.original_amount - o.unfilled_amount;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp,
            datetime: timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: o
                .updated_at
                .as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis()),
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price: o.price,
            average: None,
            amount: o.original_amount,
            filled,
            remaining: Some(o.unfilled_amount),
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(o).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        #[derive(Deserialize)]
        struct Response {
            success: bool,
            payload: Vec<BitsoOrderDetail>,
        }

        let mut params: HashMap<String, String> = HashMap::new();
        if let Some(sym) = symbol {
            let market_id = self.market_id(sym).ok_or_else(|| CcxtError::BadSymbol {
                symbol: sym.to_string(),
            })?;
            params.insert("book".into(), market_id);
        }

        let response: Response = self.private_request("GET", "/open_orders", None).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: "Failed to fetch open orders".into(),
            });
        }

        let markets_by_id = self.markets_by_id.read().unwrap();
        let orders: Vec<Order> = response
            .payload
            .iter()
            .filter_map(|o| {
                let market_id = o.book.as_ref()?;
                let sym = markets_by_id.get(market_id)?;

                let timestamp = o
                    .created_at
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis());

                let side = match o.side.as_deref() {
                    Some("buy") => OrderSide::Buy,
                    Some("sell") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                let order_type = match o.order_type.as_deref() {
                    Some("limit") => OrderType::Limit,
                    Some("market") => OrderType::Market,
                    _ => OrderType::Limit,
                };

                let filled = o.original_amount - o.unfilled_amount;

                Some(Order {
                    id: o.oid.clone().unwrap_or_default(),
                    client_order_id: None,
                    timestamp,
                    datetime: timestamp.map(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
                    last_trade_timestamp: None,
                    last_update_timestamp: o
                        .updated_at
                        .as_ref()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.timestamp_millis()),
                    status: OrderStatus::Open,
                    symbol: sym.clone(),
                    order_type,
                    time_in_force: None,
                    side,
                    price: o.price,
                    average: None,
                    amount: o.original_amount,
                    filled,
                    remaining: Some(o.unfilled_amount),
                    cost: None,
                    trades: Vec::new(),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(o).unwrap_or_default(),
                    stop_price: None,
                    trigger_price: None,
                    take_profit_price: None,
                    stop_loss_price: None,
                    reduce_only: None,
                    post_only: None,
                })
            })
            .collect();

        Ok(orders)
    }
}

// === Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct BitsoTicker {
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    bid: Option<Decimal>,
    #[serde(default)]
    ask: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    vwap: Option<Decimal>,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitsoOrderBook {
    #[serde(default)]
    bids: Vec<BitsoOrderBookEntry>,
    #[serde(default)]
    asks: Vec<BitsoOrderBookEntry>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    sequence: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct BitsoOrderBookEntry {
    #[serde(default)]
    price: Decimal,
    #[serde(default)]
    amount: Decimal,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitsoTrade {
    #[serde(default)]
    tid: i64,
    #[serde(default)]
    price: Decimal,
    #[serde(default)]
    amount: Decimal,
    #[serde(default)]
    maker_side: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitsoBalance {
    #[serde(default)]
    balances: Vec<BitsoBalanceEntry>,
}

#[derive(Debug, Deserialize)]
struct BitsoBalanceEntry {
    #[serde(default)]
    currency: String,
    #[serde(default)]
    available: Decimal,
    #[serde(default)]
    locked: Decimal,
    #[serde(default)]
    total: Decimal,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitsoOrderResponse {
    #[serde(default)]
    oid: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitsoOrderDetail {
    #[serde(default)]
    oid: Option<String>,
    #[serde(default)]
    book: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    original_amount: Decimal,
    #[serde(default)]
    unfilled_amount: Decimal,
    #[serde(default)]
    status: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Bitso::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Bitso);
        assert_eq!(exchange.name(), "Bitso");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Bitso::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/MXN"), "btc_mxn");
    }
}
