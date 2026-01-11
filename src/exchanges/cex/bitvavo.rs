//! Bitvavo Exchange Implementation
//!
//! Dutch cryptocurrency exchange with EUR trading pairs

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

/// Bitvavo 거래소
pub struct Bitvavo {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Bitvavo {
    const BASE_URL: &'static str = "https://api.bitvavo.com/v2";
    const RATE_LIMIT_MS: u64 = 60; // 1000 requests per minute = ~60ms per request

    /// 새 Bitvavo 인스턴스 생성
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
            logo: Some("https://user-images.githubusercontent.com/1294454/169202626-bd130fc5-fcf9-41bb-8d97-6093225c73cd.jpg".into()),
            api: api_urls,
            www: Some("https://bitvavo.com/".into()),
            doc: vec!["https://docs.bitvavo.com/".into()],
            fees: Some("https://bitvavo.com/en/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".to_string());
        timeframes.insert(Timeframe::Minute5, "5m".to_string());
        timeframes.insert(Timeframe::Minute15, "15m".to_string());
        timeframes.insert(Timeframe::Minute30, "30m".to_string());
        timeframes.insert(Timeframe::Hour1, "1h".to_string());
        timeframes.insert(Timeframe::Hour2, "2h".to_string());
        timeframes.insert(Timeframe::Hour4, "4h".to_string());
        timeframes.insert(Timeframe::Hour6, "6h".to_string());
        timeframes.insert(Timeframe::Hour8, "8h".to_string());
        timeframes.insert(Timeframe::Hour12, "12h".to_string());
        timeframes.insert(Timeframe::Day1, "1d".to_string());

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

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// 비공개 API 호출
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

        let timestamp = Utc::now().timestamp_millis().to_string();
        let body_str = body.unwrap_or("");

        // Message to sign: timestamp + method + path + body
        let message = format!("{timestamp}{method}{path}{body_str}");

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(message.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("BITVAVO-ACCESS-KEY".into(), api_key.to_string());
        headers.insert("BITVAVO-ACCESS-SIGNATURE".into(), signature);
        headers.insert("BITVAVO-ACCESS-TIMESTAMP".into(), timestamp);
        headers.insert("BITVAVO-ACCESS-WINDOW".into(), "10000".into());
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

    /// Parse market from API response
    fn parse_market(&self, data: &serde_json::Value) -> Option<Market> {
        let market_id = data.get("market")?.as_str()?;
        let base = data.get("base")?.as_str()?;
        let quote = data.get("quote")?.as_str()?;
        let symbol = format!("{}/{}", base.to_uppercase(), quote.to_uppercase());

        let status = data
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("trading");
        let active = status == "trading";

        let price_precision = data
            .get("pricePrecision")
            .and_then(|v| v.as_i64())
            .map(|p| p as i32);

        Some(Market {
            id: market_id.to_string(),
            lowercase_id: Some(market_id.to_lowercase()),
            symbol: symbol.clone(),
            base: base.to_uppercase(),
            quote: quote.to_uppercase(),
            base_id: base.to_string(),
            quote_id: quote.to_string(),
            market_type: MarketType::Spot,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            index: false,
            active,
            contract: false,
            linear: None,
            inverse: None,
            sub_type: None,
            taker: Some(Decimal::new(25, 4)), // 0.0025 = 0.25%
            maker: Some(Decimal::new(25, 4)), // 0.0025 = 0.25%
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            settle: None,
            settle_id: None,
            precision: MarketPrecision {
                amount: price_precision,
                price: price_precision,
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
impl Exchange for Bitvavo {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bitvavo
    }

    fn name(&self) -> &str {
        "Bitvavo"
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
        let response: serde_json::Value = self.public_get("/markets", None).await?;

        let markets_data = response
            .as_array()
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "Invalid markets response".into(),
            })?;

        let mut markets = Vec::new();
        for data in markets_data {
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

        let path = format!("/ticker/24h?market={market_id}");
        let response: BitvavoTicker = self.public_get(&path, None).await?;

        let timestamp = response
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: response.high,
            low: response.low,
            bid: response.bid,
            bid_volume: response.bid_size,
            ask: response.ask,
            ask_volume: response.ask_size,
            vwap: None,
            open: response.open,
            close: response.last,
            last: response.last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: response.volume,
            quote_volume: response.volume_quote,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.market_id(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("depth".into(), l.to_string());
        }

        let path = format!("/{market_id}/book");
        let response: BitvavoOrderBook = self.public_get(&path, Some(params)).await?;
        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<Vec<String>>| -> Vec<OrderBookEntry> {
            entries
                .iter()
                .filter_map(|e| {
                    if e.len() >= 2 {
                        let price = e[0].parse::<Decimal>().ok()?;
                        let amount = e[1].parse::<Decimal>().ok()?;
                        Some(OrderBookEntry { price, amount })
                    } else {
                        None
                    }
                })
                .collect()
        };

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: response.nonce,
            bids: parse_entries(&response.bids),
            asks: parse_entries(&response.asks),
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

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let path = format!("/{market_id}/trades");
        let response: Vec<BitvavoTrade> = self.public_get(&path, Some(params)).await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| {
                let timestamp = t.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
                Trade {
                    id: t.id.clone(),
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
                    amount: t.amount.unwrap_or_default(),
                    cost: t.price.and_then(|p| t.amount.map(|a| p * a)),
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
                feature: "Timeframe not supported".to_string(),
            })?;

        let mut params = HashMap::new();
        params.insert("interval".into(), tf.to_string());

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }
        if let Some(s) = since {
            params.insert("start".into(), s.to_string());
        }

        let path = format!("/{market_id}/candles");
        let response: Vec<Vec<serde_json::Value>> = self.public_get(&path, Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                if c.len() >= 6 {
                    Some(OHLCV {
                        timestamp: c[0].as_i64()?,
                        open: c[1].as_str().and_then(|s| s.parse::<Decimal>().ok())?,
                        high: c[2].as_str().and_then(|s| s.parse::<Decimal>().ok())?,
                        low: c[3].as_str().and_then(|s| s.parse::<Decimal>().ok())?,
                        close: c[4].as_str().and_then(|s| s.parse::<Decimal>().ok())?,
                        volume: c[5].as_str().and_then(|s| s.parse::<Decimal>().ok())?,
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: Vec<BitvavoBalance> = self.private_request("GET", "/balance", None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for balance_item in response {
            let currency = balance_item.symbol.to_uppercase();
            let free = balance_item.available.unwrap_or_default();
            let used = balance_item.in_order.unwrap_or_default();

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

        let mut order_data = serde_json::json!({
            "market": market_id,
            "side": match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            },
            "orderType": match order_type {
                OrderType::Market => "market",
                OrderType::Limit => "limit",
                _ => "limit",
            },
            "amount": amount.to_string(),
        });

        if order_type == OrderType::Limit {
            let p = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Price required for limit order".into(),
            })?;
            order_data["price"] = serde_json::json!(p.to_string());
        }

        let body = serde_json::to_string(&order_data).map_err(|e| CcxtError::ExchangeError {
            message: format!("Failed to serialize order: {e}"),
        })?;

        let response: BitvavoOrder = self.private_request("POST", "/order", Some(&body)).await?;

        let timestamp = response
            .created
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Order {
            id: response.order_id.clone(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: response.updated,
            status: match response.status.as_deref() {
                Some("new") | Some("open") | Some("partiallyFilled") => OrderStatus::Open,
                Some("filled") => OrderStatus::Closed,
                Some("canceled") | Some("cancelled") | Some("expired") | Some("rejected") => {
                    OrderStatus::Canceled
                },
                _ => OrderStatus::Open,
            },
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price: response.price,
            average: None,
            amount: response.amount.unwrap_or(amount),
            filled: response.filled_amount.unwrap_or_default(),
            remaining: response.amount_remaining,
            cost: response.filled_amount_quote,
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
        let market_id = self.market_id(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let path = format!("/order?market={market_id}&orderId={id}");
        let response: BitvavoOrder = self.private_request("DELETE", &path, None).await?;

        let timestamp = response
            .created
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Order {
            id: response.order_id.clone(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: response.updated,
            status: OrderStatus::Canceled,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: match response.side.as_deref() {
                Some("buy") => OrderSide::Buy,
                Some("sell") => OrderSide::Sell,
                _ => OrderSide::Buy,
            },
            price: response.price,
            average: None,
            amount: response.amount.unwrap_or_default(),
            filled: response.filled_amount.unwrap_or_default(),
            remaining: response.amount_remaining,
            cost: response.filled_amount_quote,
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

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.market_id(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let path = format!("/order?market={market_id}&orderId={id}");
        let response: BitvavoOrder = self.private_request("GET", &path, None).await?;

        let timestamp = response
            .created
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Order {
            id: response.order_id.clone(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: response.updated,
            status: match response.status.as_deref() {
                Some("new") | Some("open") | Some("partiallyFilled") => OrderStatus::Open,
                Some("filled") => OrderStatus::Closed,
                Some("canceled") | Some("cancelled") | Some("expired") | Some("rejected") => {
                    OrderStatus::Canceled
                },
                _ => OrderStatus::Open,
            },
            symbol: symbol.to_string(),
            order_type: match response.order_type.as_deref() {
                Some("market") => OrderType::Market,
                Some("limit") => OrderType::Limit,
                _ => OrderType::Limit,
            },
            time_in_force: None,
            side: match response.side.as_deref() {
                Some("buy") => OrderSide::Buy,
                Some("sell") => OrderSide::Sell,
                _ => OrderSide::Buy,
            },
            price: response.price,
            average: None,
            amount: response.amount.unwrap_or_default(),
            filled: response.filled_amount.unwrap_or_default(),
            remaining: response.amount_remaining,
            cost: response.filled_amount_quote,
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
        let mut path = "/ordersOpen".to_string();

        if let Some(sym) = symbol {
            let market_id = self.market_id(sym).ok_or_else(|| CcxtError::BadSymbol {
                symbol: sym.to_string(),
            })?;
            path = format!("{path}?market={market_id}");
        }

        let response: Vec<BitvavoOrder> = self.private_request("GET", &path, None).await?;

        let orders: Vec<Order> = response
            .iter()
            .map(|o| {
                let timestamp = o.created.unwrap_or_else(|| Utc::now().timestamp_millis());
                let order_symbol = if let Some(sym) = symbol {
                    sym.to_string()
                } else {
                    // Try to find symbol from market_id
                    o.market
                        .as_ref()
                        .and_then(|m| self.symbol(m))
                        .unwrap_or_default()
                };

                Order {
                    id: o.order_id.clone(),
                    client_order_id: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    last_trade_timestamp: None,
                    last_update_timestamp: o.updated,
                    status: OrderStatus::Open,
                    symbol: order_symbol,
                    order_type: match o.order_type.as_deref() {
                        Some("market") => OrderType::Market,
                        Some("limit") => OrderType::Limit,
                        _ => OrderType::Limit,
                    },
                    time_in_force: None,
                    side: match o.side.as_deref() {
                        Some("buy") => OrderSide::Buy,
                        Some("sell") => OrderSide::Sell,
                        _ => OrderSide::Buy,
                    },
                    price: o.price,
                    average: None,
                    amount: o.amount.unwrap_or_default(),
                    filled: o.filled_amount.unwrap_or_default(),
                    remaining: o.amount_remaining,
                    cost: o.filled_amount_quote,
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
struct BitvavoTicker {
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bid: Option<Decimal>,
    #[serde(default, rename = "bidSize")]
    bid_size: Option<Decimal>,
    #[serde(default)]
    ask: Option<Decimal>,
    #[serde(default, rename = "askSize")]
    ask_size: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default, rename = "volumeQuote")]
    volume_quote: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct BitvavoOrderBook {
    #[serde(default)]
    market: String,
    #[serde(default)]
    nonce: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitvavoTrade {
    #[serde(default)]
    id: String,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    side: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitvavoBalance {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    available: Option<Decimal>,
    #[serde(default, rename = "inOrder")]
    in_order: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitvavoOrder {
    #[serde(default, rename = "orderId")]
    order_id: String,
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    created: Option<i64>,
    #[serde(default)]
    updated: Option<i64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "orderType")]
    order_type: Option<String>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default, rename = "amountRemaining")]
    amount_remaining: Option<Decimal>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default, rename = "filledAmount")]
    filled_amount: Option<Decimal>,
    #[serde(default, rename = "filledAmountQuote")]
    filled_amount_quote: Option<Decimal>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Bitvavo::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Bitvavo);
        assert_eq!(exchange.name(), "Bitvavo");
        assert!(exchange.has().spot);
        assert!(exchange.has().fetch_ohlcv);
    }

    #[test]
    fn test_timeframes() {
        let config = ExchangeConfig::new();
        let exchange = Bitvavo::new(config).unwrap();
        let timeframes = exchange.timeframes();
        assert_eq!(timeframes.get(&Timeframe::Minute1), Some(&"1m".to_string()));
        assert_eq!(timeframes.get(&Timeframe::Hour1), Some(&"1h".to_string()));
        assert_eq!(timeframes.get(&Timeframe::Day1), Some(&"1d".to_string()));
    }
}
