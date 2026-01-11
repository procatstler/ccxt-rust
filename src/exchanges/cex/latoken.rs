//! LATOKEN Exchange Implementation
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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

#[allow(dead_code)]
type HmacSha256 = Hmac<Sha256>;

/// LATOKEN exchange
pub struct Latoken {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    currencies: RwLock<HashMap<String, LatokenCurrency>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Latoken {
    const BASE_URL: &'static str = "https://api.latoken.com/v2";
    const RATE_LIMIT_MS: u64 = 50; // 20 requests per second

    /// Create new LATOKEN instance
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
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_my_trades: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/61511972-24c39f00-aa01-11e9-9f7c-471f1d6e5214.jpg".into()),
            api: api_urls,
            www: Some("https://latoken.com".into()),
            doc: vec![
                "https://api.latoken.com/doc/v2/".into(),
            ],
            fees: Some("https://latoken.com/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());
        timeframes.insert(Timeframe::Week1, "1w".into());
        timeframes.insert(Timeframe::Month1, "1M".into());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            currencies: RwLock::new(HashMap::new()),
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
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let body_str = body.as_ref()
            .map(|b| serde_json::to_string(b).unwrap_or_default())
            .unwrap_or_default();

        let signature_payload = format!("{method}{path}{timestamp}{body_str}");

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError { message: "Invalid secret key".into() })?;
        mac.update(signature_payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("X-LA-APIKEY".into(), api_key.to_string());
        headers.insert("X-LA-SIGNATURE".into(), signature);
        headers.insert("X-LA-DIGEST".into(), "HMAC-SHA256".to_string());
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => self.client.get(path, None, Some(headers)).await,
            "POST" => self.client.post(path, body, Some(headers)).await,
            "DELETE" => self.client.delete(path, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// Get currency code from ID
    fn get_currency_code(&self, id: &str) -> String {
        let currencies = self.currencies.read().unwrap();
        currencies.get(id)
            .and_then(|c| c.tag.clone())
            .unwrap_or_else(|| id.to_string())
    }
}

#[async_trait]
impl Exchange for Latoken {
    fn id(&self) -> ExchangeId {
        ExchangeId::Latoken
    }

    fn name(&self) -> &str {
        "LATOKEN"
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

        // First load currencies
        let currencies: Vec<LatokenCurrency> = self.public_get("/currency", None).await?;

        {
            let mut currency_cache = self.currencies.write().unwrap();
            for currency in currencies {
                if let Some(id) = &currency.id {
                    currency_cache.insert(id.clone(), currency);
                }
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
        let response: Vec<LatokenPair> = self.public_get("/pair", None).await?;

        let mut markets = Vec::new();
        for data in &response {
            if let (Some(id), Some(base_id), Some(quote_id)) =
                (&data.id, &data.base_currency, &data.quote_currency) {

                let base = self.get_currency_code(base_id);
                let quote = self.get_currency_code(quote_id);
                let symbol = format!("{base}/{quote}");

                let market = Market {
                    id: id.clone(),
                    lowercase_id: Some(id.to_lowercase()),
                    symbol: symbol.clone(),
                    base: base.clone(),
                    quote: quote.clone(),
                    base_id: base_id.clone(),
                    quote_id: quote_id.clone(),
                    market_type: MarketType::Spot,
                    spot: true,
                    margin: false,
                    swap: false,
                    future: false,
                    option: false,
                    index: false,
                    active: data.status.as_deref() == Some("PAIR_STATUS_ACTIVE"),
                    contract: false,
                    linear: None,
                    inverse: None,
                    sub_type: None,
                    taker: Some(Decimal::new(1, 3)), // 0.1%
                    maker: Some(Decimal::new(1, 3)),
                    contract_size: None,
                    expiry: None,
                    expiry_datetime: None,
                    strike: None,
                    option_type: None,
                    settle: None,
                    settle_id: None,
                    precision: MarketPrecision {
                        amount: data.quantity_decimals,
                        price: data.price_decimals,
                        cost: data.cost_decimals,
                        base: None,
                        quote: None,
                    },
                    limits: MarketLimits {
                        amount: crate::types::MinMax {
                            min: data.min_order_quantity.as_ref().and_then(|s| s.parse().ok()),
                            max: None,
                        },
                        price: crate::types::MinMax::default(),
                        cost: crate::types::MinMax {
                            min: data.min_order_cost_usd.as_ref().and_then(|s| s.parse().ok()),
                            max: None,
                        },
                        leverage: crate::types::MinMax::default(),
                    },
                    margin_modes: None,
                    created: None,
                    info: serde_json::to_value(data).unwrap_or_default(),
                    tier_based: false,
                    percentage: true,
                };
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
        let markets = self.load_markets(false).await?;
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let path = format!("/ticker/{}/{}", market.base_id, market.quote_id);
        let response: LatokenTicker = self.public_get(&path, None).await?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: response.high.as_ref().and_then(|s| s.parse().ok()),
            low: response.low.as_ref().and_then(|s| s.parse().ok()),
            bid: response.best_bid.as_ref().and_then(|s| s.parse().ok()),
            bid_volume: None,
            ask: response.best_ask.as_ref().and_then(|s| s.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: response.open.as_ref().and_then(|s| s.parse().ok()),
            close: response.last_price.as_ref().and_then(|s| s.parse().ok()),
            last: response.last_price.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: response.change.as_ref().and_then(|s| s.parse().ok()),
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
        let markets = self.load_markets(false).await?;
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let path = format!("/book/{}/{}", market.base_id, market.quote_id);

        let mut params = HashMap::new();
        params.insert("limit".into(), limit.unwrap_or(100).to_string());

        let response: LatokenOrderBook = self.public_get(&path, Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<LatokenOrderBookLevel>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().filter_map(|e| {
                Some(OrderBookEntry {
                    price: e.price.as_ref()?.parse().ok()?,
                    amount: e.quantity.as_ref()?.parse().ok()?,
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
            bids: parse_entries(&response.bid),
            asks: parse_entries(&response.ask),
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let markets = self.load_markets(false).await?;
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let path = format!("/trade/history/{}/{}", market.base_id, market.quote_id);

        let mut params = HashMap::new();
        params.insert("limit".into(), limit.unwrap_or(100).to_string());

        let response: Vec<LatokenTrade> = self.public_get(&path, Some(params)).await?;

        let limit = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = response.iter()
            .take(limit)
            .map(|t| {
                let timestamp = t.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
                let price: Decimal = t.price.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
                let amount: Decimal = t.quantity.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();

                Trade {
                    id: t.id.clone().unwrap_or_default(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: t.maker_buyer.map(|b| if b { "sell" } else { "buy" }.to_string()),
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
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let markets = self.load_markets(false).await?;
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let resolution = self.timeframes.get(&timeframe).cloned().unwrap_or("1h".into());
        let path = "/tradingview/history".to_string();

        let now = Utc::now().timestamp();
        let mut params = HashMap::new();
        params.insert("symbol".into(), format!("{}/{}", market.base, market.quote));
        params.insert("resolution".into(), resolution);
        params.insert("from".into(), since.map(|s| s / 1000).unwrap_or(now - 86400 * 30).to_string());
        params.insert("to".into(), now.to_string());

        let response: LatokenOhlcvResponse = self.public_get(&path, Some(params)).await?;

        let len = response.t.len().min(limit.unwrap_or(500) as usize);
        let mut ohlcv = Vec::new();

        for i in 0..len {
            if let (Some(&t), Some(o), Some(h), Some(l), Some(c), Some(v)) = (
                response.t.get(i),
                response.o.get(i).and_then(|s| s.parse::<Decimal>().ok()),
                response.h.get(i).and_then(|s| s.parse::<Decimal>().ok()),
                response.l.get(i).and_then(|s| s.parse::<Decimal>().ok()),
                response.c.get(i).and_then(|s| s.parse::<Decimal>().ok()),
                response.v.get(i).and_then(|s| s.parse::<Decimal>().ok()),
            ) {
                ohlcv.push(OHLCV {
                    timestamp: t * 1000,
                    open: o,
                    high: h,
                    low: l,
                    close: c,
                    volume: v,
                });
            }
        }

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: Vec<LatokenBalance> = self.private_request("GET", "/auth/account", None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for balance_data in &response {
            if let Some(currency_id) = &balance_data.currency {
                let currency = self.get_currency_code(currency_id);
                let free: Decimal = balance_data.available.as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();
                let used: Decimal = balance_data.blocked.as_ref()
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
        let markets = self.load_markets(false).await?;
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let mut body = serde_json::json!({
            "baseCurrency": market.base_id,
            "quoteCurrency": market.quote_id,
            "side": match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            },
            "type": match order_type {
                OrderType::Limit => "limit",
                OrderType::Market => "market",
                _ => "limit",
            },
            "quantity": amount.to_string(),
            "condition": "GTC",
        });

        if order_type == OrderType::Limit {
            let p = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Price is required for limit orders".into(),
            })?;
            body["price"] = serde_json::json!(p.to_string());
        }

        let response: LatokenOrder = self.private_request("POST", "/auth/order/place", Some(body)).await?;

        let timestamp = Utc::now().timestamp_millis();
        let status = match response.status.as_deref() {
            Some("ORDER_STATUS_PLACED") | Some("ORDER_STATUS_PARTIALLY_FILLED") => OrderStatus::Open,
            Some("ORDER_STATUS_FILLED") => OrderStatus::Closed,
            Some("ORDER_STATUS_CANCELLED") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        Ok(Order {
            id: response.id.clone().unwrap_or_default(),
            client_order_id: response.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None, // LaToken uses string condition
            side,
            price: response.price.as_ref().and_then(|s| s.parse().ok()),
            average: None,
            amount,
            filled: response.filled.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
            remaining: None,
            cost: response.filled_cost.as_ref().and_then(|s| s.parse().ok()),
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
        let body = serde_json::json!({
            "id": id,
        });

        let response: LatokenOrder = self.private_request("POST", "/auth/order/cancel", Some(body)).await?;

        let timestamp = response.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let base = self.get_currency_code(response.base_currency.as_deref().unwrap_or(""));
        let quote = self.get_currency_code(response.quote_currency.as_deref().unwrap_or(""));
        let symbol = format!("{base}/{quote}");

        Ok(Order {
            id: response.id.clone().unwrap_or_else(|| id.to_string()),
            client_order_id: response.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol,
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price: response.price.as_ref().and_then(|s| s.parse().ok()),
            average: None,
            amount: response.quantity.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
            filled: response.filled.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
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
        let path = format!("/auth/order/getOrder/{id}");
        let response: LatokenOrder = self.private_request("GET", &path, None).await?;

        let timestamp = response.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let status = match response.status.as_deref() {
            Some("ORDER_STATUS_PLACED") | Some("ORDER_STATUS_PARTIALLY_FILLED") => OrderStatus::Open,
            Some("ORDER_STATUS_FILLED") => OrderStatus::Closed,
            Some("ORDER_STATUS_CANCELLED") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let side = match response.side.as_deref() {
            Some("ORDER_SIDE_BUY") => OrderSide::Buy,
            Some("ORDER_SIDE_SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match response.order_type.as_deref() {
            Some("ORDER_TYPE_LIMIT") => OrderType::Limit,
            Some("ORDER_TYPE_MARKET") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let base = self.get_currency_code(response.base_currency.as_deref().unwrap_or(""));
        let quote = self.get_currency_code(response.quote_currency.as_deref().unwrap_or(""));
        let symbol = format!("{base}/{quote}");

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
            symbol,
            order_type,
            time_in_force: None, // LaToken uses string condition
            side,
            price: response.price.as_ref().and_then(|s| s.parse().ok()),
            average: response.filled_cost.as_ref().zip(response.filled.as_ref())
                .and_then(|(cost, qty)| {
                    let c: Decimal = cost.parse().ok()?;
                    let q: Decimal = qty.parse().ok()?;
                    if q > Decimal::ZERO { Some(c / q) } else { None }
                }),
            amount: response.quantity.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
            filled: response.filled.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
            remaining: None,
            cost: response.filled_cost.as_ref().and_then(|s| s.parse().ok()),
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
        _symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("limit".into(), limit.unwrap_or(100).to_string());

        let path = format!("/auth/order/active?{}",
            params.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        );

        let response: Vec<LatokenOrder> = self.private_request("GET", &path, None).await?;

        let orders: Vec<Order> = response.iter()
            .map(|o| {
                let timestamp = o.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
                let status = match o.status.as_deref() {
                    Some("ORDER_STATUS_PLACED") | Some("ORDER_STATUS_PARTIALLY_FILLED") => OrderStatus::Open,
                    Some("ORDER_STATUS_FILLED") => OrderStatus::Closed,
                    Some("ORDER_STATUS_CANCELLED") => OrderStatus::Canceled,
                    _ => OrderStatus::Open,
                };

                let side = match o.side.as_deref() {
                    Some("ORDER_SIDE_BUY") => OrderSide::Buy,
                    Some("ORDER_SIDE_SELL") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                let order_type = match o.order_type.as_deref() {
                    Some("ORDER_TYPE_LIMIT") => OrderType::Limit,
                    Some("ORDER_TYPE_MARKET") => OrderType::Market,
                    _ => OrderType::Limit,
                };

                let base = self.get_currency_code(o.base_currency.as_deref().unwrap_or(""));
                let quote = self.get_currency_code(o.quote_currency.as_deref().unwrap_or(""));
                let symbol = format!("{base}/{quote}");

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
                    status,
                    symbol,
                    order_type,
                    time_in_force: None, // LaToken uses string condition
                    side,
                    price: o.price.as_ref().and_then(|s| s.parse().ok()),
                    average: o.filled_cost.as_ref().zip(o.filled.as_ref())
                        .and_then(|(cost, qty)| {
                            let c: Decimal = cost.parse().ok()?;
                            let q: Decimal = qty.parse().ok()?;
                            if q > Decimal::ZERO { Some(c / q) } else { None }
                        }),
                    amount: o.quantity.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
                    filled: o.filled.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
                    remaining: None,
                    cost: o.filled_cost.as_ref().and_then(|s| s.parse().ok()),
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

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
struct LatokenCurrency {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct LatokenPair {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "baseCurrency")]
    base_currency: Option<String>,
    #[serde(default, rename = "quoteCurrency")]
    quote_currency: Option<String>,
    #[serde(default, rename = "pricePrecision")]
    price_decimals: Option<i32>,
    #[serde(default, rename = "quantityPrecision")]
    quantity_decimals: Option<i32>,
    #[serde(default, rename = "costPrecision")]
    cost_decimals: Option<i32>,
    #[serde(default, rename = "minOrderQuantity")]
    min_order_quantity: Option<String>,
    #[serde(default, rename = "minOrderCostUsd")]
    min_order_cost_usd: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct LatokenTicker {
    #[serde(default, rename = "baseCurrency")]
    base_currency: Option<String>,
    #[serde(default, rename = "quoteCurrency")]
    quote_currency: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default, rename = "lastPrice")]
    last_price: Option<String>,
    #[serde(default, rename = "bestBid")]
    best_bid: Option<String>,
    #[serde(default, rename = "bestAsk")]
    best_ask: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    change: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct LatokenOrderBook {
    #[serde(default)]
    bid: Vec<LatokenOrderBookLevel>,
    #[serde(default)]
    ask: Vec<LatokenOrderBookLevel>,
}

#[derive(Debug, Default, Deserialize)]
struct LatokenOrderBookLevel {
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct LatokenTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default, rename = "makerBuyer")]
    maker_buyer: Option<bool>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct LatokenOhlcvResponse {
    #[serde(default)]
    t: Vec<i64>,
    #[serde(default)]
    o: Vec<String>,
    #[serde(default)]
    h: Vec<String>,
    #[serde(default)]
    l: Vec<String>,
    #[serde(default)]
    c: Vec<String>,
    #[serde(default)]
    v: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct LatokenOrder {
    #[serde(default)]
    id: Option<String>,
    #[serde(default, rename = "clientOrderId")]
    client_order_id: Option<String>,
    #[serde(default, rename = "baseCurrency")]
    base_currency: Option<String>,
    #[serde(default, rename = "quoteCurrency")]
    quote_currency: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    filled: Option<String>,
    #[serde(default, rename = "filledCost")]
    filled_cost: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    condition: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct LatokenBalance {
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    blocked: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Latoken::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Latoken);
        assert_eq!(exchange.name(), "LATOKEN");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_timeframes() {
        let config = ExchangeConfig::new();
        let exchange = Latoken::new(config).unwrap();
        assert!(exchange.timeframes().contains_key(&Timeframe::Minute1));
        assert!(exchange.timeframes().contains_key(&Timeframe::Hour1));
        assert!(exchange.timeframes().contains_key(&Timeframe::Day1));
    }
}
