//! Foxbit Exchange Implementation
//!
//! Brazilian cryptocurrency exchange with BRL trading pairs

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

/// Foxbit exchange
pub struct Foxbit {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Foxbit {
    const BASE_URL: &'static str = "https://api.foxbit.com.br/api/v3";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

    /// Create new Foxbit instance
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
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27991413-11b40d42-647f-11e7-91ee-78ced874dd09.jpg".into()),
            api: api_urls,
            www: Some("https://foxbit.com.br/".into()),
            doc: vec!["https://docs.foxbit.com.br/".into()],
            fees: Some("https://foxbit.com.br/taxas/".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());
        timeframes.insert(Timeframe::Week1, "1w".into());

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
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
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

        let timestamp = Utc::now().timestamp_millis().to_string();

        let body_params = params.unwrap_or_default();
        let body_string = if !body_params.is_empty() {
            serde_json::to_string(&body_params).map_err(|e| CcxtError::ExchangeError {
                message: format!("Failed to serialize params: {e}"),
            })?
        } else {
            String::new()
        };

        let message = format!(
            "{}{}{}{}",
            timestamp,
            method.to_uppercase(),
            path,
            body_string
        );

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(message.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("X-FB-ACCESS-KEY".into(), api_key.to_string());
        headers.insert("X-FB-ACCESS-TIMESTAMP".into(), timestamp);
        headers.insert("X-FB-ACCESS-SIGNATURE".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => self.client.get(path, None, Some(headers)).await,
            "POST" => {
                let body_value = if body_string.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&body_string).unwrap_or(serde_json::Value::Null))
                };
                self.client.post(path, body_value, Some(headers)).await
            },
            "DELETE" => self.client.delete(path, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }
}

#[async_trait]
impl Exchange for Foxbit {
    fn id(&self) -> ExchangeId {
        ExchangeId::Foxbit
    }

    fn name(&self) -> &str {
        "Foxbit"
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
        let response: FoxbitMarketsResponse = self.public_get("/markets", None).await?;

        let mut markets = Vec::new();
        for market_data in response.data {
            let symbol = format!(
                "{}/{}",
                market_data.base.to_uppercase(),
                market_data.quote.to_uppercase()
            );
            let market = Market {
                id: market_data.symbol.clone(),
                lowercase_id: Some(market_data.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: market_data.base.to_uppercase(),
                quote: market_data.quote.to_uppercase(),
                base_id: market_data.base.clone(),
                quote_id: market_data.quote.clone(),
                market_type: MarketType::Spot,
                spot: true,
                margin: false,
                swap: false,
                future: false,
                option: false,
                index: false,
                active: market_data.active.unwrap_or(true),
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: market_data.taker_fee,
                maker: market_data.maker_fee,
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
                    amount: market_data.quantity_precision,
                    price: market_data.price_precision,
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&market_data).unwrap_or_default(),
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
        let market_id = self.market_id(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let path = format!("/markets/{market_id}/ticker");
        let response: FoxbitTickerResponse = self.public_get(&path, None).await?;
        let data = response.data;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: data.high,
            low: data.low,
            bid: data.best_bid,
            bid_volume: None,
            ask: data.best_ask,
            ask_volume: None,
            vwap: None,
            open: data.open,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: data.change,
            percentage: data.change_pct,
            average: None,
            base_volume: data.volume,
            quote_volume: data.volume_quote,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&data).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.market_id(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let mut path = format!("/markets/{market_id}/orderbook");
        if let Some(l) = limit {
            path = format!("{path}?depth={l}");
        }

        let response: FoxbitOrderBookResponse = self.public_get(&path, None).await?;
        let data = response.data;
        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<FoxbitOrderBookLevel>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().map(|e| OrderBookEntry {
                price: e.price,
                amount: e.quantity,
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
            bids: parse_entries(&data.bids),
            asks: parse_entries(&data.asks),
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

        let mut path = format!("/markets/{market_id}/trades");
        if let Some(l) = limit {
            path = format!("{path}?limit={l}");
        }

        let response: FoxbitTradesResponse = self.public_get(&path, None).await?;

        let trades: Vec<Trade> = response
            .data
            .iter()
            .map(|t| {
                let timestamp = t.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
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
                    side: t.side.clone(),
                    taker_or_maker: None,
                    price: t.price.unwrap_or_default(),
                    amount: t.quantity.unwrap_or_default(),
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
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.market_id(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let tf = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::NotSupported {
                feature: format!("Timeframe {timeframe:?}"),
            })?;

        let mut path = format!("/markets/{market_id}/candles?interval={tf}");
        if let Some(s) = since {
            path = format!("{path}&startTime={s}");
        }
        if let Some(l) = limit {
            path = format!("{path}&limit={l}");
        }

        let response: FoxbitCandlesResponse = self.public_get(&path, None).await?;

        let ohlcv: Vec<OHLCV> = response
            .data
            .iter()
            .filter_map(|c| {
                Some(OHLCV {
                    timestamp: c.timestamp?,
                    open: c.open?,
                    high: c.high?,
                    low: c.low?,
                    close: c.close?,
                    volume: c.volume?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: FoxbitBalanceResponse =
            self.private_request("GET", "/accounts", None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for account in response.data {
            let currency = account.currency_symbol.to_uppercase();
            let free = account.balance_available.unwrap_or_default();
            let used = account.balance_locked.unwrap_or_default();
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

        let mut params = HashMap::new();
        params.insert("market_symbol".into(), market_id.clone());
        params.insert(
            "side".into(),
            match side {
                OrderSide::Buy => "BUY".into(),
                OrderSide::Sell => "SELL".into(),
            },
        );
        params.insert(
            "type".into(),
            match order_type {
                OrderType::Limit => "LIMIT".into(),
                OrderType::Market => "MARKET".into(),
                _ => "LIMIT".into(),
            },
        );
        params.insert("quantity".into(), amount.to_string());

        if order_type == OrderType::Limit {
            let p = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Price is required for limit orders".into(),
            })?;
            params.insert("price".into(), p.to_string());
        }

        let response: FoxbitOrderResponse = self
            .private_request("POST", "/orders", Some(params))
            .await?;
        let data = response.data;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: data.id.clone(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: self.parse_order_status(&data.state),
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: data.avg_price,
            amount,
            filled: data.quantity_executed.unwrap_or_default(),
            remaining: Some(amount - data.quantity_executed.unwrap_or_default()),
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&data).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("/orders/{id}");
        let response: FoxbitOrderResponse = self.private_request("DELETE", &path, None).await?;
        let data = response.data;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: data.id.clone(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: String::new(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price: data.price,
            average: data.avg_price,
            amount: data.quantity.unwrap_or_default(),
            filled: data.quantity_executed.unwrap_or_default(),
            remaining: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&data).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("/orders/{id}");
        let response: FoxbitOrderResponse = self.private_request("GET", &path, None).await?;
        let data = response.data;

        let timestamp = data
            .created_at
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let side = match data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            _ => OrderType::Limit,
        };

        Ok(Order {
            id: data.id.clone(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: self.parse_order_status(&data.state),
            symbol: String::new(),
            order_type,
            time_in_force: None,
            side,
            price: data.price,
            average: data.avg_price,
            amount: data.quantity.unwrap_or_default(),
            filled: data.quantity_executed.unwrap_or_default(),
            remaining: data
                .quantity
                .as_ref()
                .and_then(|q| data.quantity_executed.as_ref().map(|e| q - e)),
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&data).unwrap_or_default(),
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
        let mut path = "/orders?state=ACTIVE".to_string();

        if let Some(sym) = symbol {
            let market_id = self.market_id(sym).ok_or_else(|| CcxtError::BadSymbol {
                symbol: sym.to_string(),
            })?;
            path = format!("{path}&market_symbol={market_id}");
        }

        let response: FoxbitOrdersResponse = self.private_request("GET", &path, None).await?;

        let orders: Vec<Order> = response
            .data
            .iter()
            .map(|o| {
                let timestamp = o
                    .created_at
                    .unwrap_or_else(|| Utc::now().timestamp_millis());
                let side = match o.side.as_deref() {
                    Some("BUY") => OrderSide::Buy,
                    Some("SELL") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                let order_type = match o.order_type.as_deref() {
                    Some("LIMIT") => OrderType::Limit,
                    Some("MARKET") => OrderType::Market,
                    _ => OrderType::Limit,
                };

                Order {
                    id: o.id.clone(),
                    client_order_id: o.client_order_id.clone(),
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    last_trade_timestamp: None,
                    last_update_timestamp: None,
                    status: self.parse_order_status(&o.state),
                    symbol: symbol.unwrap_or("").to_string(),
                    order_type,
                    time_in_force: None,
                    side,
                    price: o.price,
                    average: o.avg_price,
                    amount: o.quantity.unwrap_or_default(),
                    filled: o.quantity_executed.unwrap_or_default(),
                    remaining: o
                        .quantity
                        .as_ref()
                        .and_then(|q| o.quantity_executed.as_ref().map(|e| q - e)),
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

impl Foxbit {
    fn parse_order_status(&self, state: &Option<String>) -> OrderStatus {
        match state.as_deref() {
            Some("ACTIVE") | Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("FILLED") | Some("COMPLETED") => OrderStatus::Closed,
            Some("CANCELED") | Some("CANCELLED") | Some("REJECTED") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        }
    }
}

// === Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct FoxbitMarketsResponse {
    #[serde(default)]
    data: Vec<FoxbitMarketData>,
}

#[derive(Debug, Deserialize, Serialize)]
struct FoxbitMarketData {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    base: String,
    #[serde(default)]
    quote: String,
    #[serde(default)]
    active: Option<bool>,
    #[serde(default, rename = "makerFee")]
    maker_fee: Option<Decimal>,
    #[serde(default, rename = "takerFee")]
    taker_fee: Option<Decimal>,
    #[serde(default, rename = "quantityPrecision")]
    quantity_precision: Option<i32>,
    #[serde(default, rename = "pricePrecision")]
    price_precision: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct FoxbitTickerResponse {
    #[serde(default)]
    data: FoxbitTickerData,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct FoxbitTickerData {
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default, rename = "bestBid")]
    best_bid: Option<Decimal>,
    #[serde(default, rename = "bestAsk")]
    best_ask: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default, rename = "volumeQuote")]
    volume_quote: Option<Decimal>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    change: Option<Decimal>,
    #[serde(default, rename = "changePct")]
    change_pct: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct FoxbitOrderBookResponse {
    #[serde(default)]
    data: FoxbitOrderBookData,
}

#[derive(Debug, Default, Deserialize)]
struct FoxbitOrderBookData {
    #[serde(default)]
    bids: Vec<FoxbitOrderBookLevel>,
    #[serde(default)]
    asks: Vec<FoxbitOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
struct FoxbitOrderBookLevel {
    #[serde(default)]
    price: Decimal,
    #[serde(default)]
    quantity: Decimal,
}

#[derive(Debug, Deserialize, Serialize)]
struct FoxbitTradesResponse {
    #[serde(default)]
    data: Vec<FoxbitTradeData>,
}

#[derive(Debug, Deserialize, Serialize)]
struct FoxbitTradeData {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    quantity: Option<Decimal>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct FoxbitCandlesResponse {
    #[serde(default)]
    data: Vec<FoxbitCandleData>,
}

#[derive(Debug, Deserialize)]
struct FoxbitCandleData {
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
}

#[derive(Debug, Deserialize)]
struct FoxbitBalanceResponse {
    #[serde(default)]
    data: Vec<FoxbitAccountData>,
}

#[derive(Debug, Deserialize)]
struct FoxbitAccountData {
    #[serde(default)]
    currency_symbol: String,
    #[serde(default)]
    balance_available: Option<Decimal>,
    #[serde(default)]
    balance_locked: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct FoxbitOrderResponse {
    #[serde(default)]
    data: FoxbitOrderData,
}

#[derive(Debug, Deserialize, Serialize)]
struct FoxbitOrdersResponse {
    #[serde(default)]
    data: Vec<FoxbitOrderData>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct FoxbitOrderData {
    #[serde(default)]
    id: String,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    quantity: Option<Decimal>,
    #[serde(default)]
    quantity_executed: Option<Decimal>,
    #[serde(default)]
    avg_price: Option<Decimal>,
    #[serde(default)]
    created_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Foxbit::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Foxbit);
        assert_eq!(exchange.name(), "Foxbit");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_timeframes() {
        let config = ExchangeConfig::new();
        let exchange = Foxbit::new(config).unwrap();
        assert!(exchange.timeframes().contains_key(&Timeframe::Minute1));
        assert!(exchange.timeframes().contains_key(&Timeframe::Hour1));
    }
}
