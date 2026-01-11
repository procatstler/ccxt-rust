//! Bitstamp Exchange Implementation
//!
//! European cryptocurrency exchange

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

/// Bitstamp exchange
pub struct Bitstamp {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Bitstamp {
    const BASE_URL: &'static str = "https://www.bitstamp.net/api/v2";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

    /// Create new Bitstamp instance
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
            fetch_ohlcv: true,
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
            logo: Some("https://user-images.githubusercontent.com/1294454/27786377-8c8ab57e-5fe9-11e7-8ea4-2b05b6bcceec.jpg".into()),
            api: api_urls,
            www: Some("https://www.bitstamp.net".into()),
            doc: vec![
                "https://www.bitstamp.net/api/".into(),
            ],
            fees: Some("https://www.bitstamp.net/fee-schedule/".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "60".into());
        timeframes.insert(Timeframe::Minute3, "180".into());
        timeframes.insert(Timeframe::Minute5, "300".into());
        timeframes.insert(Timeframe::Minute15, "900".into());
        timeframes.insert(Timeframe::Minute30, "1800".into());
        timeframes.insert(Timeframe::Hour1, "3600".into());
        timeframes.insert(Timeframe::Hour2, "7200".into());
        timeframes.insert(Timeframe::Hour4, "14400".into());
        timeframes.insert(Timeframe::Day1, "86400".into());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
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
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
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
        let timestamp = Utc::now().timestamp_millis().to_string();
        let content_type = "application/x-www-form-urlencoded";

        let body_params = params.unwrap_or_default();
        let body = if body_params.is_empty() {
            String::new()
        } else {
            body_params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        };

        // Bitstamp v2 signature format
        let message = format!(
            "BITSTAMP {api_key}POSTwww.bitstamp.net/api/v2{path}{content_type}{nonce}v2{body}"
        );

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(message.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("X-Auth".into(), format!("BITSTAMP {api_key}"));
        headers.insert("X-Auth-Signature".into(), signature.to_uppercase());
        headers.insert("X-Auth-Nonce".into(), nonce);
        headers.insert("X-Auth-Timestamp".into(), timestamp);
        headers.insert("X-Auth-Version".into(), "v2".into());
        headers.insert("Content-Type".into(), content_type.into());

        self.client
            .post_form(path, &body_params, Some(headers))
            .await
    }

    /// Convert symbol to market ID (BTC/USD -> btcusd)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "").to_lowercase()
    }
}

#[async_trait]
impl Exchange for Bitstamp {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bitstamp
    }

    fn name(&self) -> &str {
        "Bitstamp"
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
        let response: Vec<BitstampMarket> = self.public_get("/trading-pairs-info/", None).await?;

        let mut markets = Vec::new();
        for info in response {
            let id = info.url_symbol.clone().unwrap_or_default();
            let base = info.base_currency.clone().unwrap_or_default();
            let quote = info.counter_currency.clone().unwrap_or_default();
            let symbol = format!("{base}/{quote}");

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.to_lowercase(),
                quote_id: quote.to_lowercase(),
                market_type: MarketType::Spot,
                spot: true,
                margin: false,
                swap: false,
                future: false,
                option: false,
                index: false,
                active: info.trading == Some("Enabled".to_string()),
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: Some(Decimal::new(5, 3)), // 0.5%
                maker: Some(Decimal::new(5, 3)),
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                settle: None,
                settle_id: None,
                precision: MarketPrecision {
                    amount: info.base_decimals,
                    price: info.counter_decimals,
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax {
                        min: info.minimum_order.as_ref().and_then(|s| s.parse().ok()),
                        max: None,
                    },
                    price: crate::types::MinMax::default(),
                    cost: crate::types::MinMax::default(),
                    leverage: crate::types::MinMax::default(),
                },
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
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/ticker/{market_id}/");
        let response: BitstampTicker = self.public_get(&path, None).await?;

        let timestamp = response
            .timestamp
            .as_ref()
            .and_then(|s| s.parse::<i64>().ok())
            .map(|t| t * 1000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: response.high.as_ref().and_then(|s| s.parse().ok()),
            low: response.low.as_ref().and_then(|s| s.parse().ok()),
            bid: response.bid.as_ref().and_then(|s| s.parse().ok()),
            bid_volume: None,
            ask: response.ask.as_ref().and_then(|s| s.parse().ok()),
            ask_volume: None,
            vwap: response.vwap.as_ref().and_then(|s| s.parse().ok()),
            open: response.open.as_ref().and_then(|s| s.parse().ok()),
            close: response.last.as_ref().and_then(|s| s.parse().ok()),
            last: response.last.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: response.volume.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/order_book/{market_id}/");
        let response: BitstampOrderBook = self.public_get(&path, None).await?;

        let timestamp = response
            .timestamp
            .as_ref()
            .and_then(|s| s.parse::<i64>().ok())
            .map(|t| t * 1000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let parse_entries = |entries: &Vec<Vec<String>>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().filter_map(|e| {
                if e.len() >= 2 {
                    let price = e[0].parse().ok()?;
                    let amount = e[1].parse().ok()?;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
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
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
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
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/transactions/{market_id}/");
        let response: Vec<BitstampTrade> = self.public_get(&path, None).await?;

        let limit = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = response
            .iter()
            .take(limit)
            .map(|t| {
                let timestamp = t
                    .date
                    .as_ref()
                    .and_then(|s| s.parse::<i64>().ok())
                    .map(|ts| ts * 1000)
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let price: Decimal = t
                    .price
                    .as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();
                let amount: Decimal = t
                    .amount
                    .as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();

                let side = match t.trade_type.as_deref() {
                    Some("0") => "buy".to_string(),
                    Some("1") => "sell".to_string(),
                    _ => "buy".to_string(),
                };

                Trade {
                    id: t.tid.map(|tid| tid.to_string()).unwrap_or_default(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(side),
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
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
        symbol: &str,
        timeframe: Timeframe,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.convert_to_market_id(symbol);
        let step = self
            .timeframes
            .get(&timeframe)
            .cloned()
            .unwrap_or("3600".into());
        let path = format!("/ohlc/{market_id}/");

        let mut params = HashMap::new();
        params.insert("step".into(), step);
        params.insert("limit".into(), limit.unwrap_or(100).to_string());

        let response: BitstampOhlcvResponse = self.public_get(&path, Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response
            .data
            .ohlc
            .iter()
            .filter_map(|c| {
                Some(OHLCV {
                    timestamp: c.timestamp.as_ref()?.parse::<i64>().ok()? * 1000,
                    open: c.open.as_ref()?.parse().ok()?,
                    high: c.high.as_ref()?.parse().ok()?,
                    low: c.low.as_ref()?.parse().ok()?,
                    close: c.close.as_ref()?.parse().ok()?,
                    volume: c.volume.as_ref()?.parse().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: HashMap<String, serde_json::Value> =
            self.private_post("/balance/", None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for (key, value) in &response {
            if key.ends_with("_available") {
                let currency = key.replace("_available", "").to_uppercase();
                let free: Decimal = value
                    .as_str()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();

                let used_key = format!("{}_reserved", currency.to_lowercase());
                let used: Decimal = response
                    .get(&used_key)
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();

                balances.currencies.insert(
                    currency,
                    Balance {
                        free: Some(free),
                        used: Some(used),
                        total: Some(free + used),
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
        let market_id = self.convert_to_market_id(symbol);

        let order_side = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };

        let order_type_str = match order_type {
            OrderType::Market => "market",
            _ => "limit",
        };

        let path = format!("/{order_side}/{order_type_str}/{market_id}/");

        let mut params = HashMap::new();
        params.insert("amount".into(), amount.to_string());
        if let Some(p) = price {
            params.insert("price".into(), p.to_string());
        }

        let response: BitstampOrder = self.private_post(&path, Some(params)).await?;

        let timestamp = response
            .datetime
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Order {
            id: response.id.clone().unwrap_or_default(),
            client_order_id: response.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
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

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("id".into(), id.to_string());

        let response: BitstampOrder = self.private_post("/cancel_order/", Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: String::new(),
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
            info: serde_json::to_value(&response).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("id".into(), id.to_string());

        let response: BitstampOrder = self.private_post("/order_status/", Some(params)).await?;

        let timestamp = response
            .datetime
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match response.status.as_deref() {
            Some("Open") | Some("In Queue") => OrderStatus::Open,
            Some("Finished") => OrderStatus::Closed,
            Some("Canceled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match response.order_type.as_deref() {
            Some("0") => OrderType::Limit,
            Some("1") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let amount: Decimal = response
            .amount
            .as_ref()
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or_default();

        let side = if amount >= Decimal::ZERO {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        };

        Ok(Order {
            id: response.id.clone().unwrap_or_default(),
            client_order_id: response.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: response.currency_pair.clone().unwrap_or_default(),
            order_type,
            time_in_force: None,
            side,
            price: response.price.as_ref().and_then(|s| s.parse().ok()),
            average: None,
            amount: amount.abs(),
            filled: Decimal::ZERO,
            remaining: None,
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

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let path = if let Some(s) = symbol {
            format!("/open_orders/{}/", self.convert_to_market_id(s))
        } else {
            "/open_orders/all/".to_string()
        };

        let response: Vec<BitstampOrder> = self.private_post(&path, None).await?;

        let orders: Vec<Order> = response
            .iter()
            .map(|o| {
                let timestamp = o
                    .datetime
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let amount: Decimal = o
                    .amount
                    .as_ref()
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();

                let side = if amount >= Decimal::ZERO {
                    OrderSide::Buy
                } else {
                    OrderSide::Sell
                };

                let order_type = match o.order_type.as_deref() {
                    Some("0") => OrderType::Limit,
                    Some("1") => OrderType::Market,
                    _ => OrderType::Limit,
                };

                Order {
                    id: o.id.clone().unwrap_or_default(),
                    client_order_id: o.client_order_id.clone(),
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    last_trade_timestamp: None,
                    last_update_timestamp: None,
                    status: OrderStatus::Open,
                    symbol: o.currency_pair.clone().unwrap_or_default(),
                    order_type,
                    time_in_force: None,
                    side,
                    price: o.price.as_ref().and_then(|s| s.parse().ok()),
                    average: None,
                    amount: amount.abs(),
                    filled: Decimal::ZERO,
                    remaining: Some(amount.abs()),
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

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitstampMarket {
    #[serde(default)]
    url_symbol: Option<String>,
    #[serde(default)]
    base_currency: Option<String>,
    #[serde(default)]
    counter_currency: Option<String>,
    #[serde(default)]
    base_decimals: Option<i32>,
    #[serde(default)]
    counter_decimals: Option<i32>,
    #[serde(default)]
    minimum_order: Option<String>,
    #[serde(default)]
    trading: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitstampTicker {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    bid: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    vwap: Option<String>,
    #[serde(default)]
    volume: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BitstampOrderBook {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitstampTrade {
    #[serde(default)]
    tid: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default, rename = "type")]
    trade_type: Option<String>,
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BitstampOhlcvResponse {
    #[serde(default)]
    data: BitstampOhlcvData,
}

#[derive(Debug, Default, Deserialize)]
struct BitstampOhlcvData {
    #[serde(default)]
    ohlc: Vec<BitstampOhlcv>,
}

#[derive(Debug, Default, Deserialize)]
struct BitstampOhlcv {
    #[serde(default)]
    timestamp: Option<String>,
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

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitstampOrder {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    datetime: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    currency_pair: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Bitstamp::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Bitstamp);
        assert_eq!(exchange.name(), "Bitstamp");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Bitstamp::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/USD"), "btcusd");
    }
}
