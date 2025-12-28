//! BTCMarkets Exchange Implementation
//!
//! Australian cryptocurrency exchange

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

/// BTCMarkets exchange
pub struct Btcmarkets {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Btcmarkets {
    const BASE_URL: &'static str = "https://api.btcmarkets.net/v3";
    const RATE_LIMIT_MS: u64 = 40; // 25 requests per second

    /// Create new Btcmarkets instance
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
            logo: Some("https://user-images.githubusercontent.com/1294454/29142911-0e1acfc2-7d5c-11e7-98c4-07d9532b29d7.jpg".into()),
            api: api_urls,
            www: Some("https://www.btcmarkets.net".into()),
            doc: vec!["https://docs.btcmarkets.net/".into()],
            fees: Some("https://www.btcmarkets.net/fees".into()),
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
        body: Option<String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let body_str = body.as_deref().unwrap_or("");

        // Create message for signature: method\npath\ntimestamp\nbody
        let message = if body_str.is_empty() {
            format!("{method}\n{path}\n{timestamp}")
        } else {
            format!("{method}\n{path}\n{timestamp}\n{body_str}")
        };

        // Decode base64 secret
        let secret_bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            api_secret,
        )
        .map_err(|_| CcxtError::AuthenticationError {
            message: "Invalid API secret encoding".into(),
        })?;

        let mut mac = HmacSha512::new_from_slice(&secret_bytes).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(message.as_bytes());
        let signature = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            mac.finalize().into_bytes(),
        );

        let mut headers = HashMap::new();
        headers.insert("BM-AUTH-APIKEY".into(), api_key.to_string());
        headers.insert("BM-AUTH-TIMESTAMP".into(), timestamp);
        headers.insert("BM-AUTH-SIGNATURE".into(), signature);
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
            }
            "DELETE" => self.client.delete(path, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method {method}"),
            }),
        }
    }

    /// Parse market from API response
    fn parse_market(&self, data: &serde_json::Value) -> Option<Market> {
        let id = data.get("marketId")?.as_str()?;
        let base = data.get("baseAssetName")?.as_str()?;
        let quote = data.get("quoteAssetName")?.as_str()?;
        let symbol = format!("{base}/{quote}");

        let status = data
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("Online");

        Some(Market {
            id: id.to_string(),
            lowercase_id: Some(id.to_lowercase()),
            symbol: symbol.clone(),
            base: base.to_string(),
            quote: quote.to_string(),
            base_id: base.to_string(),
            quote_id: quote.to_string(),
            market_type: MarketType::Spot,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            index: false,
            active: status == "Online",
            contract: false,
            linear: None,
            inverse: None,
            sub_type: None,
            taker: data
                .get("takerFeeRate")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Decimal>().ok()),
            maker: data
                .get("makerFeeRate")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Decimal>().ok()),
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            settle: None,
            settle_id: None,
            precision: MarketPrecision {
                amount: data
                    .get("amountDecimals")
                    .and_then(|v| v.as_i64())
                    .map(|d| d as i32),
                price: data
                    .get("priceDecimals")
                    .and_then(|v| v.as_i64())
                    .map(|d| d as i32),
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

    /// Get market ID from symbol
    fn get_market_id(&self, symbol: &str) -> CcxtResult<String> {
        let markets = self.markets.read().unwrap();
        markets
            .get(symbol)
            .map(|m| m.id.clone())
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })
    }
}

#[async_trait]
impl Exchange for Btcmarkets {
    fn id(&self) -> ExchangeId {
        ExchangeId::Btcmarkets
    }

    fn name(&self) -> &str {
        "BTCMarkets"
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

        let markets_data = response.as_array().cloned().unwrap_or_default();

        let mut markets = Vec::new();
        for data in markets_data {
            if let Some(market) = self.parse_market(&data) {
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
        let market_id = self.get_market_id(symbol)?;
        let path = format!("/markets/{market_id}/ticker");

        let response: BtcmarketsTicker = self.public_get(&path, None).await?;
        let timestamp = Utc::now().timestamp_millis();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: response.high24h,
            low: response.low24h,
            bid: response.best_bid,
            bid_volume: None,
            ask: response.best_ask,
            ask_volume: None,
            vwap: None,
            open: None,
            close: response.last_price,
            last: response.last_price,
            previous_close: None,
            change: None,
            percentage: response.price_pct_24h,
            average: None,
            base_volume: response.volume24h,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.get_market_id(symbol)?;
        let mut path = format!("/markets/{market_id}/orderbook");
        if let Some(l) = limit {
            path = format!("{path}?level={l}");
        }

        let response: BtcmarketsOrderBook = self.public_get(&path, None).await?;
        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<Vec<String>>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().filter_map(|e| {
                if e.len() >= 2 {
                    let price = e[0].parse::<Decimal>().ok()?;
                    let amount = e[1].parse::<Decimal>().ok()?;
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
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
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
        let market_id = self.get_market_id(symbol)?;
        let mut path = format!("/markets/{market_id}/trades");
        if let Some(l) = limit {
            path = format!("{path}?limit={l}");
        }

        let response: Vec<BtcmarketsTrade> = self.public_get(&path, None).await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| {
                let timestamp = chrono::DateTime::parse_from_rfc3339(&t.timestamp)
                    .ok()
                    .map(|dt| dt.timestamp_millis());
                Trade {
                    id: t.id.to_string(),
                    order: None,
                    timestamp,
                    datetime: Some(t.timestamp.clone()),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(t.side.to_lowercase()),
                    taker_or_maker: None,
                    price: t.price.unwrap_or_default(),
                    amount: t.amount.unwrap_or_default(),
                    cost: t.price.as_ref().and_then(|p| t.amount.as_ref().map(|a| p * a)),
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
        let market_id = self.get_market_id(symbol)?;
        let tf = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::NotSupported {
                feature: "Timeframe not supported".to_string(),
            })?;

        let mut path = format!("/markets/{market_id}/candles?timeWindow={tf}");
        if let Some(s) = since {
            let since_str = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            path = format!("{path}&from={since_str}");
        }
        if let Some(l) = limit {
            path = format!("{path}&limit={l}");
        }

        let response: Vec<BtcmarketsCandle> = self.public_get(&path, None).await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                let timestamp = chrono::DateTime::parse_from_rfc3339(&c.timestamp)
                    .ok()
                    .map(|dt| dt.timestamp_millis())?;
                Some(OHLCV {
                    timestamp,
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
        let response: Vec<BtcmarketsBalance> =
            self.private_request("GET", "/accounts/me/balances", None)
                .await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for balance in response {
            let currency = balance.asset_name.to_uppercase();
            let free = balance.available.unwrap_or_default();
            let used = balance.locked.unwrap_or_default();

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
        let market_id = self.get_market_id(symbol)?;

        let side_str = match side {
            OrderSide::Buy => "Bid",
            OrderSide::Sell => "Ask",
        };

        let type_str = match order_type {
            OrderType::Market => "Market",
            OrderType::Limit => "Limit",
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type {order_type:?}"),
                })
            }
        };

        let mut order_data = serde_json::json!({
            "marketId": market_id,
            "amount": amount.to_string(),
            "side": side_str,
            "type": type_str
        });

        if let Some(p) = price {
            order_data["price"] = serde_json::json!(p.to_string());
        }

        let body = serde_json::to_string(&order_data).map_err(|e| CcxtError::ExchangeError {
            message: format!("Failed to serialize order: {e}"),
        })?;

        let response: BtcmarketsOrder = self.private_request("POST", "/orders", Some(body)).await?;

        let timestamp = chrono::DateTime::parse_from_rfc3339(&response.creation_time)
            .ok()
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match response.status.as_deref() {
            Some("Placed") | Some("Accepted") | Some("Partially Matched") => OrderStatus::Open,
            Some("Fully Matched") => OrderStatus::Closed,
            Some("Cancelled") | Some("Partially Cancelled") | Some("Failed") => {
                OrderStatus::Canceled
            }
            _ => OrderStatus::Open,
        };

        let open_amount = response.open_amount;
        let client_order_id = response.client_order_id.clone();

        Ok(Order {
            id: response.order_id.clone(),
            client_order_id,
            timestamp: Some(timestamp),
            datetime: Some(response.creation_time.clone()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: open_amount,
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
        let path = format!("/orders/{id}");
        let response: BtcmarketsOrder = self.private_request("DELETE", &path, None).await?;

        let timestamp = chrono::DateTime::parse_from_rfc3339(&response.creation_time)
            .ok()
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let amount = response.amount.unwrap_or_default();
        let price = response.price;
        let open_amount = response.open_amount;
        let client_order_id = response.client_order_id.clone();

        Ok(Order {
            id: response.order_id.clone(),
            client_order_id,
            timestamp: Some(timestamp),
            datetime: Some(response.creation_time.clone()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price,
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: open_amount,
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

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let path = format!("/orders/{id}");
        let response: BtcmarketsOrder = self.private_request("GET", &path, None).await?;

        let timestamp = chrono::DateTime::parse_from_rfc3339(&response.creation_time)
            .ok()
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match response.status.as_deref() {
            Some("Placed") | Some("Accepted") | Some("Partially Matched") => OrderStatus::Open,
            Some("Fully Matched") => OrderStatus::Closed,
            Some("Cancelled") | Some("Partially Cancelled") | Some("Failed") => {
                OrderStatus::Canceled
            }
            _ => OrderStatus::Open,
        };

        let side_str = response.side.as_deref().unwrap_or("Bid");
        let side = if side_str == "Bid" {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        };

        let type_str = response.order_type.as_deref().unwrap_or("Limit");
        let order_type = if type_str == "Market" {
            OrderType::Market
        } else {
            OrderType::Limit
        };

        let amount = response.amount.unwrap_or_default();
        let open_amount_value = response.open_amount.unwrap_or_default();
        let filled = amount - open_amount_value;
        let price = response.price;
        let client_order_id = response.client_order_id.clone();

        Ok(Order {
            id: response.order_id.clone(),
            client_order_id,
            timestamp: Some(timestamp),
            datetime: Some(response.creation_time.clone()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled,
            remaining: Some(open_amount_value),
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
        let mut path = "/orders?status=open".to_string();
        if let Some(sym) = symbol {
            let market_id = self.get_market_id(sym)?;
            path = format!("{path}&marketId={market_id}");
        }

        let response: Vec<BtcmarketsOrder> = self.private_request("GET", &path, None).await?;

        let markets = self.markets.read().unwrap();
        let markets_by_id = self.markets_by_id.read().unwrap();

        let mut orders = Vec::new();
        for data in response {
            let market_id = data.market_id.as_deref().unwrap_or("");
            if let Some(sym) = markets_by_id.get(market_id) {
                if let Some(_market) = markets.get(sym) {
                    let timestamp = chrono::DateTime::parse_from_rfc3339(&data.creation_time)
                        .ok()
                        .map(|dt| dt.timestamp_millis())
                        .unwrap_or_else(|| Utc::now().timestamp_millis());

                    let side_str = data.side.as_deref().unwrap_or("Bid");
                    let side = if side_str == "Bid" {
                        OrderSide::Buy
                    } else {
                        OrderSide::Sell
                    };

                    let type_str = data.order_type.as_deref().unwrap_or("Limit");
                    let order_type = if type_str == "Market" {
                        OrderType::Market
                    } else {
                        OrderType::Limit
                    };

                    let amount = data.amount.unwrap_or_default();
                    let open_amount_value = data.open_amount.unwrap_or_default();
                    let filled = amount - open_amount_value;
                    let price = data.price;
                    let client_order_id = data.client_order_id.clone();

                    orders.push(Order {
                        id: data.order_id.clone(),
                        client_order_id,
                        timestamp: Some(timestamp),
                        datetime: Some(data.creation_time.clone()),
                        last_trade_timestamp: None,
                        last_update_timestamp: None,
                        status: OrderStatus::Open,
                        symbol: sym.to_string(),
                        order_type,
                        time_in_force: None,
                        side,
                        price,
                        average: None,
                        amount,
                        filled,
                        remaining: Some(open_amount_value),
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
                    });
                }
            }
        }

        Ok(orders)
    }
}

// === Response Types ===

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BtcmarketsTicker {
    #[serde(default)]
    market_id: Option<String>,
    #[serde(default)]
    best_bid: Option<Decimal>,
    #[serde(default)]
    best_ask: Option<Decimal>,
    #[serde(default)]
    last_price: Option<Decimal>,
    #[serde(default)]
    volume24h: Option<Decimal>,
    #[serde(default)]
    price_pct_24h: Option<Decimal>,
    #[serde(default)]
    low24h: Option<Decimal>,
    #[serde(default)]
    high24h: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct BtcmarketsOrderBook {
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcmarketsTrade {
    #[serde(default)]
    id: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    side: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcmarketsCandle {
    #[serde(default)]
    timestamp: String,
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
#[serde(rename_all = "camelCase")]
struct BtcmarketsBalance {
    #[serde(default)]
    asset_name: String,
    #[serde(default)]
    available: Option<Decimal>,
    #[serde(default)]
    locked: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BtcmarketsOrder {
    #[serde(default)]
    order_id: String,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    market_id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    creation_time: String,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    open_amount: Option<Decimal>,
    #[serde(default)]
    status: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Btcmarkets::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Btcmarkets);
        assert_eq!(exchange.name(), "BTCMarkets");
        assert!(exchange.has().spot);
        assert!(exchange.has().fetch_ohlcv);
    }

    #[test]
    fn test_timeframes() {
        let config = ExchangeConfig::new();
        let exchange = Btcmarkets::new(config).unwrap();
        assert!(exchange.timeframes().contains_key(&Timeframe::Minute1));
        assert!(exchange.timeframes().contains_key(&Timeframe::Hour1));
        assert!(exchange.timeframes().contains_key(&Timeframe::Day1));
    }
}
