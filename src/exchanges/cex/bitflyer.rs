//! bitFlyer Exchange Implementation
//!
//! Japanese cryptocurrency exchange

#![allow(clippy::manual_strip)]

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

/// bitFlyer Exchange
pub struct Bitflyer {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Bitflyer {
    const BASE_URL: &'static str = "https://api.bitflyer.com/v1";
    const RATE_LIMIT_MS: u64 = 200; // 5 requests per second

    /// Create new bitFlyer instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            spot: true,
            margin: false,
            swap: false,
            future: true,
            option: false,
            fetch_markets: true,
            fetch_ticker: true,
            fetch_tickers: false,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: false,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            cancel_order: true,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/28051642-56154182-660e-11e7-9b0d-6042d1e6edd8.jpg".into()),
            api: api_urls,
            www: Some("https://bitflyer.com".into()),
            doc: vec!["https://lightning.bitflyer.com/docs".into()],
            fees: Some("https://bitflyer.com/en-us/commission".into()),
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
        body: Option<String>,
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

        let timestamp = Utc::now().timestamp_millis().to_string();
        let body_str = body.unwrap_or_default();

        let sign_str = format!("{timestamp}{method}{path}{body_str}");

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(sign_str.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("ACCESS-KEY".into(), api_key.to_string());
        headers.insert("ACCESS-TIMESTAMP".into(), timestamp);
        headers.insert("ACCESS-SIGN".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => {
                let full_path = if body_str.is_empty() {
                    path.to_string()
                } else {
                    format!("{path}?{body_str}")
                };
                self.client.get(&full_path, None, Some(headers)).await
            },
            "POST" => {
                let json_body = if body_str.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&body_str).unwrap_or(serde_json::json!({})))
                };
                self.client.post(path, json_body, Some(headers)).await
            },
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// Convert symbol to market ID (BTC/JPY -> BTC_JPY)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_")
    }
}

#[async_trait]
impl Exchange for Bitflyer {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bitflyer
    }

    fn name(&self) -> &str {
        "bitFlyer"
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
        let response: Vec<BitflyerMarket> = self.public_get("/markets", None).await?;

        let mut markets = Vec::new();
        for info in response {
            let id = info.product_code.clone().unwrap_or_default();

            // Parse market type and symbols
            let (base, quote, market_type, spot, future) = if id.starts_with("FX_") {
                // FX markets (swap/perpetual)
                let parts: Vec<&str> = id[3..].split('_').collect();
                let base = parts.first().unwrap_or(&"BTC").to_string();
                let quote = parts.get(1).unwrap_or(&"JPY").to_string();
                (base, quote, MarketType::Swap, false, false)
            } else if id.contains("_MAT") {
                // Futures markets
                let _base = id
                    .split('_')
                    .next()
                    .unwrap_or("BTC")
                    .chars()
                    .take(3)
                    .collect::<String>();
                (
                    "BTC".to_string(),
                    "JPY".to_string(),
                    MarketType::Future,
                    false,
                    true,
                )
            } else {
                // Spot markets
                let parts: Vec<&str> = id.split('_').collect();
                let base = parts.first().unwrap_or(&"BTC").to_string();
                let quote = parts.get(1).unwrap_or(&"JPY").to_string();
                (base, quote, MarketType::Spot, true, false)
            };

            let symbol = format!("{base}/{quote}");

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                market_type,
                spot,
                margin: false,
                swap: !spot && !future,
                future,
                option: false,
                index: false,
                active: true,
                contract: !spot,
                linear: if !spot { Some(true) } else { None },
                inverse: if !spot { Some(false) } else { None },
                sub_type: None,
                taker: Some(Decimal::new(15, 4)), // 0.0015 = 0.15%
                maker: Some(Decimal::new(15, 4)),
                contract_size: if !spot { Some(Decimal::ONE) } else { None },
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
                    price: Some(0),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&info).unwrap_or_default(),
                tier_based: false,
                percentage: true,
            };
            markets.push(market);
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
        let market_id = self
            .market_id(symbol)
            .unwrap_or_else(|| self.convert_to_market_id(symbol));

        let mut params = HashMap::new();
        params.insert("product_code".into(), market_id);

        let response: BitflyerTicker = self.public_get("/ticker", Some(params)).await?;

        let timestamp = response
            .timestamp
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: response.timestamp.clone(),
            high: None,
            low: None,
            bid: response.best_bid,
            bid_volume: response.best_bid_size,
            ask: response.best_ask,
            ask_volume: response.best_ask_size,
            vwap: None,
            open: None,
            close: response.ltp,
            last: response.ltp,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: response.volume,
            quote_volume: response.volume_by_product,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self
            .market_id(symbol)
            .unwrap_or_else(|| self.convert_to_market_id(symbol));

        let mut params = HashMap::new();
        params.insert("product_code".into(), market_id);

        let response: BitflyerOrderBook = self.public_get("/board", Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<BitflyerOrderBookLevel>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().filter_map(|e| {
                Some(OrderBookEntry {
                    price: e.price?,
                    amount: e.size?,
                })
            });
            if let Some(l) = limit {
                iter.take(l as usize).collect()
            } else {
                iter.collect()
            }
        };

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            checksum: None,
            bids: parse_entries(&response.bids),
            asks: parse_entries(&response.asks),
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self
            .market_id(symbol)
            .unwrap_or_else(|| self.convert_to_market_id(symbol));

        let mut params = HashMap::new();
        params.insert("product_code".into(), market_id);
        if let Some(l) = limit {
            params.insert("count".into(), l.to_string());
        }

        let response: Vec<BitflyerTrade> = self.public_get("/executions", Some(params)).await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| {
                let timestamp = t
                    .exec_date
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                Trade {
                    id: t.id.map(|i| i.to_string()).unwrap_or_default(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: t.exec_date.clone(),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: t.side.as_ref().map(|s| s.to_lowercase()),
                    taker_or_maker: None,
                    price: t.price.unwrap_or_default(),
                    amount: t.size.unwrap_or_default(),
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
        let response: Vec<BitflyerBalance> =
            self.private_request("GET", "/me/getbalance", None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for data in response {
            if let Some(currency) = data.currency_code {
                let total = data.amount.unwrap_or_default();
                let free = data.available.unwrap_or_default();

                balances.currencies.insert(
                    currency.to_uppercase(),
                    Balance {
                        free: Some(free),
                        used: Some(total - free),
                        total: Some(total),
                        debt: None,
                    },
                );
            }
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
        let market_id = self
            .market_id(symbol)
            .unwrap_or_else(|| self.convert_to_market_id(symbol));

        let mut request = serde_json::Map::new();
        request.insert("product_code".to_string(), serde_json::json!(market_id));
        request.insert(
            "child_order_type".to_string(),
            serde_json::json!(match order_type {
                OrderType::Limit => "LIMIT",
                OrderType::Market => "MARKET",
                _ => "LIMIT",
            }),
        );
        request.insert(
            "side".to_string(),
            serde_json::json!(match side {
                OrderSide::Buy => "BUY",
                OrderSide::Sell => "SELL",
            }),
        );
        request.insert(
            "size".to_string(),
            serde_json::json!(amount.to_string().parse::<f64>().unwrap_or(0.0)),
        );

        if let Some(p) = price {
            request.insert(
                "price".to_string(),
                serde_json::json!(p.to_string().parse::<f64>().unwrap_or(0.0)),
            );
        }

        let body = serde_json::to_string(&request).unwrap_or_default();
        let response: BitflyerOrderResponse = self
            .private_request("POST", "/me/sendchildorder", Some(body))
            .await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: response
                .child_order_acceptance_id
                .clone()
                .unwrap_or_default(),
            client_order_id: response.child_order_acceptance_id.clone(),
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
            info: serde_json::to_value(&response).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self
            .market_id(symbol)
            .unwrap_or_else(|| self.convert_to_market_id(symbol));

        let mut request = serde_json::Map::new();
        request.insert("product_code".to_string(), serde_json::json!(market_id));
        request.insert(
            "child_order_acceptance_id".to_string(),
            serde_json::json!(id),
        );

        let body = serde_json::to_string(&request).unwrap_or_default();
        let _response: serde_json::Value = self
            .private_request("POST", "/me/cancelchildorder", Some(body))
            .await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: id.to_string(),
            client_order_id: Some(id.to_string()),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: symbol.to_string(),
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
        // bitFlyer doesn't have a direct fetch order endpoint
        // We need to check open orders
        let orders = self.fetch_open_orders(Some(symbol), None, None).await?;

        orders
            .into_iter()
            .find(|o| {
                o.id == id
                    || o.client_order_id
                        .as_ref()
                        .map(|cid| cid == id)
                        .unwrap_or(false)
            })
            .ok_or_else(|| CcxtError::OrderNotFound {
                order_id: id.to_string(),
            })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut query_params = Vec::new();

        if let Some(s) = symbol {
            let market_id = self
                .market_id(s)
                .unwrap_or_else(|| self.convert_to_market_id(s));
            query_params.push(format!("product_code={market_id}"));
        }

        query_params.push("child_order_state=ACTIVE".to_string());

        if let Some(l) = limit {
            query_params.push(format!("count={l}"));
        }

        let query_string = query_params.join("&");
        let response: Vec<BitflyerOrder> = self
            .private_request("GET", &format!("/me/getchildorders?{query_string}"), None)
            .await?;

        let orders: Vec<Order> = response
            .iter()
            .map(|o| {
                let timestamp = o
                    .child_order_date
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let status = match o.child_order_state.as_deref() {
                    Some("ACTIVE") => OrderStatus::Open,
                    Some("COMPLETED") => OrderStatus::Closed,
                    Some("CANCELED") | Some("REJECTED") | Some("EXPIRED") => OrderStatus::Canceled,
                    _ => OrderStatus::Open,
                };

                let side = match o.side.as_deref() {
                    Some("BUY") => OrderSide::Buy,
                    Some("SELL") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                let order_type = match o.child_order_type.as_deref() {
                    Some("LIMIT") => OrderType::Limit,
                    Some("MARKET") => OrderType::Market,
                    _ => OrderType::Limit,
                };

                let sym = o
                    .product_code
                    .as_ref()
                    .map(|pc| pc.replace("_", "/"))
                    .unwrap_or_else(|| symbol.unwrap_or("").to_string());

                Order {
                    id: o.child_order_id.clone().unwrap_or_default(),
                    client_order_id: o.child_order_acceptance_id.clone(),
                    timestamp: Some(timestamp),
                    datetime: o.child_order_date.clone(),
                    last_trade_timestamp: None,
                    last_update_timestamp: None,
                    status,
                    symbol: sym,
                    order_type,
                    time_in_force: None,
                    side,
                    price: o.price,
                    average: o.average_price,
                    amount: o.size.unwrap_or_default(),
                    filled: o.executed_size.unwrap_or_default(),
                    remaining: o.outstanding_size,
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
                }
            })
            .collect();

        Ok(orders)
    }
}

// === Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct BitflyerMarket {
    #[serde(default)]
    product_code: Option<String>,
    #[serde(default)]
    market_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitflyerTicker {
    #[serde(default)]
    product_code: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    best_bid: Option<Decimal>,
    #[serde(default)]
    best_ask: Option<Decimal>,
    #[serde(default)]
    best_bid_size: Option<Decimal>,
    #[serde(default)]
    best_ask_size: Option<Decimal>,
    #[serde(default)]
    ltp: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    volume_by_product: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct BitflyerOrderBook {
    #[serde(default)]
    bids: Vec<BitflyerOrderBookLevel>,
    #[serde(default)]
    asks: Vec<BitflyerOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
struct BitflyerOrderBookLevel {
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    size: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitflyerTrade {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    size: Option<Decimal>,
    #[serde(default)]
    exec_date: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitflyerOrder {
    #[serde(default)]
    child_order_id: Option<String>,
    #[serde(default)]
    child_order_acceptance_id: Option<String>,
    #[serde(default)]
    product_code: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    child_order_type: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    average_price: Option<Decimal>,
    #[serde(default)]
    size: Option<Decimal>,
    #[serde(default)]
    executed_size: Option<Decimal>,
    #[serde(default)]
    outstanding_size: Option<Decimal>,
    #[serde(default)]
    total_commission: Option<Decimal>,
    #[serde(default)]
    child_order_state: Option<String>,
    #[serde(default)]
    child_order_date: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitflyerOrderResponse {
    #[serde(default)]
    child_order_acceptance_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitflyerBalance {
    #[serde(default)]
    currency_code: Option<String>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    available: Option<Decimal>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Bitflyer::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Bitflyer);
        assert_eq!(exchange.name(), "bitFlyer");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Bitflyer::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/JPY"), "BTC_JPY");
    }
}
