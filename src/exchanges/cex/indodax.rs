//! Indodax Exchange Implementation
//!
//! Indonesian cryptocurrency exchange (formerly Bitcoin Indonesia)

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha512;
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
type HmacSha512 = Hmac<Sha512>;

/// Indodax exchange
pub struct Indodax {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Indodax {
    const BASE_URL: &'static str = "https://indodax.com/api";
    const PRIVATE_URL: &'static str = "https://indodax.com/tapi";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

    /// Create new Indodax instance
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
            create_market_order: false,
            cancel_order: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::PRIVATE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/51840849/87070508-9358c880-c221-11ea-8dc5-5391afbbb422.jpg".into()),
            api: api_urls,
            www: Some("https://www.indodax.com".into()),
            doc: vec!["https://github.com/btcid/indodax-official-api-docs".into()],
            fees: Some("https://www.indodax.com/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".to_string());
        timeframes.insert(Timeframe::Minute5, "5".to_string());
        timeframes.insert(Timeframe::Minute15, "15".to_string());
        timeframes.insert(Timeframe::Minute30, "30".to_string());
        timeframes.insert(Timeframe::Hour1, "60".to_string());
        timeframes.insert(Timeframe::Hour4, "240".to_string());
        timeframes.insert(Timeframe::Day1, "1D".to_string());

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

    /// Public API request
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Private API request
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let nonce = Utc::now().timestamp_millis();

        let mut post_params = params.unwrap_or_default();
        post_params.insert("method".into(), method.to_string());
        post_params.insert("nonce".into(), nonce.to_string());

        let query_string: String = post_params.iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        let mut mac = HmacSha512::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError { message: "Invalid secret key".into() })?;
        mac.update(query_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("Key".into(), api_key.to_string());
        headers.insert("Sign".into(), signature);
        headers.insert("Content-Type".into(), "application/x-www-form-urlencoded".into());

        // Use custom base URL for private API
        let client = HttpClient::new(Self::PRIVATE_URL, &self.config)?;
        let response: IndodaxResponse<T> = client.post_form("", &post_params, Some(headers)).await?;

        if response.success == 1 {
            response.data.ok_or_else(|| CcxtError::ExchangeError {
                message: "No data in response".into(),
            })
        } else {
            Err(CcxtError::ExchangeError {
                message: response.error.unwrap_or_else(|| "Unknown error".into()),
            })
        }
    }

    /// Convert symbol to market ID (BTC/IDR -> btc_idr)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace('/', "_").to_lowercase()
    }

    /// Get market ID from symbol
    fn get_market_id(&self, symbol: &str) -> CcxtResult<String> {
        let markets = self.markets.read().unwrap();
        if let Some(market) = markets.get(symbol) {
            Ok(market.id.clone())
        } else {
            Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })
        }
    }
}

#[async_trait]
impl Exchange for Indodax {
    fn id(&self) -> ExchangeId {
        ExchangeId::Indodax
    }

    fn name(&self) -> &str {
        "Indodax"
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
        let response: IndodaxPairsResponse = self.public_get("/pairs", None).await?;

        let mut markets = Vec::new();
        for pair in response.0 {
            let symbol = format!("{}/{}", pair.traded_currency.to_uppercase(), pair.base_currency.to_uppercase());
            let id = pair.id.clone();
            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: pair.traded_currency.to_uppercase(),
                quote: pair.base_currency.to_uppercase(),
                base_id: pair.traded_currency.clone(),
                quote_id: pair.base_currency.clone(),
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
                taker: Some(Decimal::new(3, 3)), // 0.003 = 0.3%
                maker: Some(Decimal::new(3, 3)), // 0.003 = 0.3%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
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
                info: serde_json::json!(pair),
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
        let market_id = self.get_market_id(symbol)?;
        let path = format!("/ticker/{market_id}");

        let response: IndodaxTickerResponse = self.public_get(&path, None).await?;
        let timestamp = response.ticker.server_time * 1000;
        let info = serde_json::to_value(&response).unwrap_or_default();
        let ticker = response.ticker;

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: ticker.high,
            low: ticker.low,
            bid: ticker.buy,
            bid_volume: None,
            ask: ticker.sell,
            ask_volume: None,
            vwap: None,
            open: None,
            close: ticker.last,
            last: ticker.last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: ticker.vol_base,
            quote_volume: ticker.vol_quote,
            index_price: None,
            mark_price: None,
            info,
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.get_market_id(symbol)?;
        let path = format!("/depth/{market_id}");

        let response: IndodaxDepthResponse = self.public_get(&path, None).await?;
        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<Vec<Decimal>>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().filter_map(|e| {
                if e.len() >= 2 {
                    Some(OrderBookEntry {
                        price: e[0],
                        amount: e[1],
                    })
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
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            bids: parse_entries(&response.buy),
            asks: parse_entries(&response.sell),
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.get_market_id(symbol)?;
        let path = format!("/trades/{market_id}");

        let response: Vec<IndodaxTrade> = self.public_get(&path, None).await?;

        let limit = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = response.iter()
            .take(limit)
            .map(|t| {
                let timestamp = t.date * 1000;
                Trade {
                    id: t.tid.to_string(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(t.trade_type.clone()),
                    taker_or_maker: None,
                    price: t.price,
                    amount: t.amount,
                    cost: Some(t.price * t.amount),
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
        // Extract needed data and drop the lock before any await
        let chart_symbol = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            format!("{}_{}", market.base, market.quote)
        };

        let tf = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::NotSupported {
            feature: "Timeframe not supported".to_string(),
        })?.clone();

        self.rate_limiter.throttle(1.0).await;

        // Use a custom client for the chart API
        let temp_config = ExchangeConfig::new();
        let chart_client = HttpClient::new("https://indodax.com", &temp_config)?;

        let response: IndodaxChartResponse = chart_client.get("/tradingview/history", Some({
            let mut params = HashMap::new();
            params.insert("symbol".to_string(), chart_symbol);
            params.insert("resolution".to_string(), tf.to_string());
            params.insert("countback".to_string(), limit.unwrap_or(500).to_string());
            params
        }), None).await?;

        if response.s != "ok" {
            return Err(CcxtError::ExchangeError {
                message: "Failed to fetch OHLCV data".into(),
            });
        }

        let mut ohlcv = Vec::new();
        let len = response.t.len().min(response.o.len()).min(response.h.len())
            .min(response.l.len()).min(response.c.len()).min(response.v.len());

        for i in 0..len {
            ohlcv.push(OHLCV {
                timestamp: response.t[i] * 1000,
                open: response.o[i],
                high: response.h[i],
                low: response.l[i],
                close: response.c[i],
                volume: response.v[i],
            });
        }

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: IndodaxBalanceData = self.private_request("getInfo", None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for (currency, amount) in &response.balance {
            let free = *amount;
            let used = response.balance_hold.get(currency).copied().unwrap_or_default();
            balances.currencies.insert(
                currency.to_uppercase(),
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
        if order_type == OrderType::Market {
            return Err(CcxtError::NotSupported {
                feature: "Market orders".to_string(),
            });
        }

        let market_id = self.get_market_id(symbol)?;
        let price = price.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Price is required for limit orders".into(),
        })?;

        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);
        params.insert("type".into(), match side {
            OrderSide::Buy => "buy".into(),
            OrderSide::Sell => "sell".into(),
        });
        params.insert("price".into(), price.to_string());

        // For buy orders, send IDR amount; for sell orders, send crypto amount
        // Extract values and drop the lock before await
        let (base_key, quote_key) = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            (market.base.to_lowercase(), market.quote.to_lowercase())
        };

        if side == OrderSide::Buy {
            let cost = amount * price;
            params.insert(quote_key, cost.to_string());
        } else {
            params.insert(base_key, amount.to_string());
        }

        let response: IndodaxOrderResponse = self.private_request("trade", Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: response.order_id.to_string(),
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
            price: Some(price),
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
        let market_id = self.get_market_id(symbol)?;

        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);
        params.insert("order_id".into(), id.to_string());
        params.insert("type".into(), "buy".into()); // Required but can be either buy or sell

        let _response: IndodaxCancelResponse = self.private_request("cancelOrder", Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
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
        let market_id = self.get_market_id(symbol)?;

        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);
        params.insert("order_id".into(), id.to_string());

        let response: IndodaxOrderDetail = self.private_request("getOrder", Some(params)).await?;
        let order = response.order;

        let timestamp = order.submit_time.unwrap_or_else(|| Utc::now().timestamp()) * 1000;
        let status = if order.status.as_ref() == Some(&"cancelled".to_string()) {
            OrderStatus::Canceled
        } else if order.remain_idr.as_ref().unwrap_or(&Decimal::ZERO) == &Decimal::ZERO {
            OrderStatus::Closed
        } else {
            OrderStatus::Open
        };

        let side = match order.order_type.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side,
            price: order.price,
            average: None,
            amount: Decimal::ZERO,
            filled: Decimal::ZERO,
            remaining: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&order).unwrap_or_default(),
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
        let symbols_to_query: Vec<String> = if let Some(sym) = symbol {
            vec![sym.to_string()]
        } else {
            let markets = self.markets.read().unwrap();
            markets.keys().cloned().collect()
        };

        let mut all_orders = Vec::new();
        for sym in symbols_to_query {
            let market_id = match self.get_market_id(&sym) {
                Ok(id) => id,
                Err(_) => continue,
            };

            let mut params = HashMap::new();
            params.insert("pair".into(), market_id);

            match self.private_request::<IndodaxOpenOrdersResponse>("openOrders", Some(params)).await {
                Ok(response) => {
                    for order_data in response.orders {
                        let timestamp = order_data.submit_time.unwrap_or_else(|| Utc::now().timestamp()) * 1000;
                        let side = match order_data.order_type.as_deref() {
                            Some("buy") => OrderSide::Buy,
                            Some("sell") => OrderSide::Sell,
                            _ => OrderSide::Buy,
                        };

                        all_orders.push(Order {
                            id: order_data.order_id.as_ref().unwrap_or(&String::new()).clone(),
                            client_order_id: None,
                            timestamp: Some(timestamp),
                            datetime: Some(
                                chrono::DateTime::from_timestamp_millis(timestamp)
                                    .map(|dt| dt.to_rfc3339())
                                    .unwrap_or_default(),
                            ),
                            last_trade_timestamp: None,
                            last_update_timestamp: None,
                            status: OrderStatus::Open,
                            symbol: sym.clone(),
                            order_type: OrderType::Limit,
                            time_in_force: None,
                            side,
                            price: order_data.price,
                            average: None,
                            amount: Decimal::ZERO,
                            filled: Decimal::ZERO,
                            remaining: None,
                            cost: None,
                            trades: Vec::new(),
                            fee: None,
                            fees: Vec::new(),
                            info: serde_json::to_value(&order_data).unwrap_or_default(),
                            stop_price: None,
                            trigger_price: None,
                            take_profit_price: None,
                            stop_loss_price: None,
                            reduce_only: None,
                            post_only: None,
                        });
                    }
                }
                Err(_) => continue,
            }
        }

        Ok(all_orders)
    }
}

// === Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxResponse<T> {
    success: i32,
    #[serde(rename = "return")]
    data: Option<T>,
    error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxPairsResponse(Vec<IndodaxPair>);

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxPair {
    id: String,
    traded_currency: String,
    base_currency: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxTickerResponse {
    ticker: IndodaxTicker,
}

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxTicker {
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    buy: Option<Decimal>,
    #[serde(default)]
    sell: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default, rename = "vol_btc")]
    vol_base: Option<Decimal>,
    #[serde(default, rename = "vol_idr")]
    vol_quote: Option<Decimal>,
    #[serde(default)]
    server_time: i64,
}

#[derive(Debug, Deserialize)]
struct IndodaxDepthResponse {
    #[serde(default)]
    buy: Vec<Vec<Decimal>>,
    #[serde(default)]
    sell: Vec<Vec<Decimal>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxTrade {
    #[serde(default)]
    tid: i64,
    #[serde(default)]
    date: i64,
    #[serde(default)]
    price: Decimal,
    #[serde(default)]
    amount: Decimal,
    #[serde(default, rename = "type")]
    trade_type: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxChartResponse {
    s: String,
    #[serde(default)]
    t: Vec<i64>,
    #[serde(default)]
    o: Vec<Decimal>,
    #[serde(default)]
    h: Vec<Decimal>,
    #[serde(default)]
    l: Vec<Decimal>,
    #[serde(default)]
    c: Vec<Decimal>,
    #[serde(default)]
    v: Vec<Decimal>,
}

#[derive(Debug, Deserialize)]
struct IndodaxBalanceData {
    #[serde(default)]
    balance: HashMap<String, Decimal>,
    #[serde(default)]
    balance_hold: HashMap<String, Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxOrderResponse {
    #[serde(default)]
    order_id: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxCancelResponse {
    #[serde(default)]
    order_id: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxOrderDetail {
    order: IndodaxOrderData,
}

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxOrderData {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    submit_time: Option<i64>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    order_idr: Option<Decimal>,
    #[serde(default)]
    remain_idr: Option<Decimal>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IndodaxOpenOrdersResponse {
    #[serde(default)]
    orders: Vec<IndodaxOrderData>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Indodax::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Indodax);
        assert_eq!(exchange.name(), "Indodax");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Indodax::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/IDR"), "btc_idr");
    }
}
