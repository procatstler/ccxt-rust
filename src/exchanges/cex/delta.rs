//! Delta Exchange Implementation
//!
//! Indian derivatives exchange

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

type HmacSha256 = Hmac<Sha256>;

/// Delta Exchange
pub struct Delta {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Delta {
    const BASE_URL: &'static str = "https://api.delta.exchange/v2";
    const RATE_LIMIT_MS: u64 = 50; // 20 requests per second

    /// Create a new Delta instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            spot: false,
            margin: false,
            swap: true,
            future: true,
            option: true,
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
            logo: Some("https://user-images.githubusercontent.com/1294454/99450025-3be60a00-2931-11eb-9302-f4fd8d8589aa.jpg".into()),
            api: api_urls,
            www: Some("https://www.delta.exchange".into()),
            doc: vec!["https://docs.delta.exchange".into()],
            fees: Some("https://www.delta.exchange/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour2, "2h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Hour6, "6h".into());
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
        body: Option<serde_json::Value>,
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

        let timestamp = Utc::now().timestamp().to_string();
        let body_str = body
            .as_ref()
            .map(|b| serde_json::to_string(b).unwrap_or_default())
            .unwrap_or_default();

        // Signature: method + timestamp + path + body
        let signature_payload = format!("{method}{timestamp}{path}{body_str}");

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(signature_payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("api-key".into(), api_key.to_string());
        headers.insert("timestamp".into(), timestamp);
        headers.insert("signature".into(), signature);
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
}

#[async_trait]
impl Exchange for Delta {
    fn id(&self) -> ExchangeId {
        ExchangeId::Delta
    }

    fn name(&self) -> &str {
        "Delta Exchange"
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
        let response: DeltaResponse<Vec<DeltaProduct>> = self.public_get("/products", None).await?;

        if !response.success.unwrap_or(false) {
            return Err(CcxtError::ExchangeError {
                message: response
                    .error
                    .as_ref()
                    .and_then(|e| e.message.clone())
                    .unwrap_or_else(|| "Unknown error".into()),
            });
        }

        let data = response.result.unwrap_or_default();
        let mut markets = Vec::new();

        for product in &data {
            let id = product.symbol.clone().unwrap_or_default();
            let base = product
                .underlying_asset
                .as_ref()
                .and_then(|a| a.symbol.clone())
                .unwrap_or_default();
            let quote = product
                .quoting_asset
                .as_ref()
                .and_then(|a| a.symbol.clone())
                .unwrap_or_else(|| "USD".to_string());

            let product_type = product.product_type.as_deref().unwrap_or("");
            let is_perpetual = product_type == "perpetual_futures";
            let is_future = product_type == "futures";
            let is_option = product_type == "call_options" || product_type == "put_options";

            let symbol = format!("{base}/{quote}");

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                market_type: if is_perpetual {
                    MarketType::Swap
                } else if is_future {
                    MarketType::Future
                } else {
                    MarketType::Option
                },
                spot: false,
                margin: false,
                swap: is_perpetual,
                future: is_future,
                option: is_option,
                index: false,
                active: product.state.as_deref() == Some("live"),
                contract: true,
                linear: Some(!product.is_quanto.unwrap_or(false)),
                inverse: Some(product.is_quanto.unwrap_or(false)),
                sub_type: Some(if is_perpetual { "linear" } else { "inverse" }.to_string()),
                taker: product
                    .taker_commission_rate
                    .as_ref()
                    .and_then(|s| s.parse().ok()),
                maker: product
                    .maker_commission_rate
                    .as_ref()
                    .and_then(|s| s.parse().ok()),
                contract_size: product.contract_value.as_ref().and_then(|s| s.parse().ok()),
                expiry: product.settlement_time.as_ref().and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(s)
                        .ok()
                        .map(|dt| dt.timestamp_millis())
                }),
                expiry_datetime: product.settlement_time.clone(),
                strike: product.strike_price.as_ref().and_then(|s| s.parse().ok()),
                option_type: if product_type == "call_options" {
                    Some("call".to_string())
                } else if product_type == "put_options" {
                    Some("put".to_string())
                } else {
                    None
                },
                underlying: None,
                underlying_id: None,
                settle: Some(quote.clone()),
                settle_id: Some(quote.clone()),
                precision: MarketPrecision {
                    amount: Some(8),
                    price: product.tick_size.as_ref().and_then(|s| {
                        let tick: f64 = s.parse().ok()?;
                        Some(-(tick.log10() as i32))
                    }),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(product).unwrap_or_default(),
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
        let market_id = self.market_id(symbol).unwrap_or_else(|| symbol.to_string());
        let path = format!("/tickers/{market_id}");

        let response: DeltaResponse<DeltaTicker> = self.public_get(&path, None).await?;

        let data = response.result.ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: data.high.as_ref().and_then(|s| s.parse().ok()),
            low: data.low.as_ref().and_then(|s| s.parse().ok()),
            bid: data.best_bid.as_ref().and_then(|s| s.parse().ok()),
            bid_volume: None,
            ask: data.best_ask.as_ref().and_then(|s| s.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: data.open.as_ref().and_then(|s| s.parse().ok()),
            close: data.close.as_ref().and_then(|s| s.parse().ok()),
            last: data.close.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: None,
            percentage: data
                .price_change_percent_24h
                .as_ref()
                .and_then(|s| s.parse().ok()),
            average: None,
            base_volume: data.volume.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: data.turnover.as_ref().and_then(|s| s.parse().ok()),
            index_price: data.spot_price.as_ref().and_then(|s| s.parse().ok()),
            mark_price: data.mark_price.as_ref().and_then(|s| s.parse().ok()),
            info: serde_json::to_value(&data).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.market_id(symbol).unwrap_or_else(|| symbol.to_string());
        let path = format!("/l2orderbook/{market_id}");

        let mut params = HashMap::new();
        params.insert("depth".into(), limit.unwrap_or(20).to_string());

        let response: DeltaResponse<DeltaOrderBook> = self.public_get(&path, Some(params)).await?;

        let data = response.result.ok_or_else(|| CcxtError::ParseError {
            data_type: "OrderBook".to_string(),
            message: "No data".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        let bids: Vec<OrderBookEntry> = data
            .buy
            .iter()
            .filter_map(|b| {
                Some(OrderBookEntry {
                    price: b.price.as_ref()?.parse().ok()?,
                    amount: b.size.as_ref()?.parse().ok()?,
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .sell
            .iter()
            .filter_map(|a| {
                Some(OrderBookEntry {
                    price: a.price.as_ref()?.parse().ok()?,
                    amount: a.size.as_ref()?.parse().ok()?,
                })
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            bids,
            asks,
            checksum: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.market_id(symbol).unwrap_or_else(|| symbol.to_string());
        let path = format!("/trades/{market_id}");

        let response: DeltaResponse<Vec<DeltaTrade>> = self.public_get(&path, None).await?;

        let data = response.result.unwrap_or_default();
        let limit = limit.unwrap_or(100) as usize;

        let trades: Vec<Trade> = data
            .iter()
            .take(limit)
            .map(|t| {
                let timestamp = t.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
                let price: Decimal = t
                    .price
                    .as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();
                let amount: Decimal = t
                    .size
                    .as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();

                Trade {
                    id: t.id.map(|i| i.to_string()).unwrap_or_default(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: t
                        .buyer_role
                        .as_ref()
                        .map(|r| if r == "taker" { "buy" } else { "sell" }.to_string()),
                    taker_or_maker: None, // Delta uses string buyer_role
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
        let market_id = self.market_id(symbol).unwrap_or_else(|| symbol.to_string());
        let resolution = self
            .timeframes
            .get(&timeframe)
            .cloned()
            .unwrap_or_else(|| "1h".into());
        let path = "/history/candles".to_string();

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("resolution".into(), resolution);

        if let Some(s) = since {
            params.insert("start".into(), (s / 1000).to_string());
        }
        if let Some(_l) = limit {
            let end = Utc::now().timestamp();
            params.insert("end".into(), end.to_string());
        }

        let response: DeltaResponse<Vec<DeltaCandle>> =
            self.public_get(&path, Some(params)).await?;

        let data = response.result.unwrap_or_default();
        let ohlcv: Vec<OHLCV> = data
            .iter()
            .filter_map(|c| {
                Some(OHLCV {
                    timestamp: c.time? * 1000,
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
        let response: DeltaResponse<Vec<DeltaBalance>> = self
            .private_request("GET", "/wallet/balances", None)
            .await?;

        let data = response.result.unwrap_or_default();

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for balance_data in &data {
            if let Some(asset) = balance_data.asset.as_ref().and_then(|a| a.symbol.clone()) {
                let free: Decimal = balance_data
                    .available_balance
                    .as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();
                let total: Decimal = balance_data
                    .balance
                    .as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();

                balances.currencies.insert(
                    asset,
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
        let market_id = self.market_id(symbol).unwrap_or_else(|| symbol.to_string());

        let mut body = serde_json::json!({
            "product_symbol": market_id,
            "side": match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            },
            "order_type": match order_type {
                OrderType::Limit => "limit_order",
                OrderType::Market => "market_order",
                OrderType::StopLimit => "stop_limit_order",
                OrderType::StopMarket => "stop_market_order",
                _ => "limit_order",
            },
            "size": amount.to_string().parse::<i64>().unwrap_or(0),
        });

        if let Some(p) = price {
            body["limit_price"] = serde_json::json!(p.to_string());
        }

        let response: DeltaResponse<DeltaOrder> =
            self.private_request("POST", "/orders", Some(body)).await?;

        let data = response.result.ok_or_else(|| CcxtError::ExchangeError {
            message: response
                .error
                .as_ref()
                .and_then(|e| e.message.clone())
                .unwrap_or_else(|| "Order creation failed".into()),
        })?;

        let timestamp = data
            .created_at
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Order {
            id: data.id.map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: data.created_at.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None, // Delta uses string time_in_force
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
        let body = serde_json::json!({
            "id": id.parse::<i64>().unwrap_or(0),
        });

        let response: DeltaResponse<DeltaOrder> = self
            .private_request("DELETE", "/orders", Some(body))
            .await?;

        let data = response.result.ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: data.product_symbol.clone().unwrap_or_default(),
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

        let response: DeltaResponse<DeltaOrder> = self.private_request("GET", &path, None).await?;

        let data = response.result.ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })?;

        let timestamp = data
            .created_at
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.state.as_deref() {
            Some("open") | Some("pending") => OrderStatus::Open,
            Some("closed") | Some("filled") => OrderStatus::Closed,
            Some("cancelled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit_order") => OrderType::Limit,
            Some("market_order") => OrderType::Market,
            Some("stop_limit_order") => OrderType::StopLimit,
            Some("stop_market_order") => OrderType::StopMarket,
            _ => OrderType::Limit,
        };

        let amount: Decimal = data
            .size
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let unfilled: Decimal = data
            .unfilled_size
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        Ok(Order {
            id: data.id.map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: data.created_at.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: data.product_symbol.clone().unwrap_or_default(),
            order_type,
            time_in_force: None, // Delta uses string time_in_force
            side,
            price: data.limit_price.as_ref().and_then(|s| s.parse().ok()),
            average: data
                .average_fill_price
                .as_ref()
                .and_then(|s| s.parse().ok()),
            amount,
            filled: amount - unfilled,
            remaining: Some(unfilled),
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&data).unwrap_or_default(),
            stop_price: data.stop_price.as_ref().and_then(|s| s.parse().ok()),
            trigger_price: data.stop_price.as_ref().and_then(|s| s.parse().ok()),
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: data.reduce_only,
            post_only: data.post_only,
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("state".into(), "open".to_string());

        if let Some(s) = symbol {
            params.insert(
                "product_symbol".into(),
                self.market_id(s).unwrap_or_else(|| s.to_string()),
            );
        }

        let query = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let path = format!("/orders?{query}");

        let response: DeltaResponse<Vec<DeltaOrder>> =
            self.private_request("GET", &path, None).await?;

        let data = response.result.unwrap_or_default();
        let orders: Vec<Order> = data
            .iter()
            .map(|o| {
                let timestamp = o
                    .created_at
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let side = match o.side.as_deref() {
                    Some("buy") => OrderSide::Buy,
                    Some("sell") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                let order_type = match o.order_type.as_deref() {
                    Some("limit_order") => OrderType::Limit,
                    Some("market_order") => OrderType::Market,
                    Some("stop_limit_order") => OrderType::StopLimit,
                    Some("stop_market_order") => OrderType::StopMarket,
                    _ => OrderType::Limit,
                };

                let amount: Decimal = o
                    .size
                    .as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();
                let unfilled: Decimal = o
                    .unfilled_size
                    .as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();

                Order {
                    id: o.id.map(|i| i.to_string()).unwrap_or_default(),
                    client_order_id: o.client_order_id.clone(),
                    timestamp: Some(timestamp),
                    datetime: o.created_at.clone(),
                    last_trade_timestamp: None,
                    last_update_timestamp: None,
                    status: OrderStatus::Open,
                    symbol: o.product_symbol.clone().unwrap_or_default(),
                    order_type,
                    time_in_force: None, // Delta uses string time_in_force
                    side,
                    price: o.limit_price.as_ref().and_then(|s| s.parse().ok()),
                    average: o.average_fill_price.as_ref().and_then(|s| s.parse().ok()),
                    amount,
                    filled: amount - unfilled,
                    remaining: Some(unfilled),
                    cost: None,
                    trades: Vec::new(),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(o).unwrap_or_default(),
                    stop_price: o.stop_price.as_ref().and_then(|s| s.parse().ok()),
                    trigger_price: o.stop_price.as_ref().and_then(|s| s.parse().ok()),
                    take_profit_price: None,
                    stop_loss_price: None,
                    reduce_only: o.reduce_only,
                    post_only: o.post_only,
                }
            })
            .collect();

        Ok(orders)
    }
}

// === Response Types ===

#[derive(Debug, Default, Deserialize)]
struct DeltaResponse<T> {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    result: Option<T>,
    #[serde(default)]
    error: Option<DeltaError>,
}

#[derive(Debug, Default, Deserialize)]
struct DeltaError {
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct DeltaProduct {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    product_type: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    underlying_asset: Option<DeltaAsset>,
    #[serde(default)]
    quoting_asset: Option<DeltaAsset>,
    #[serde(default)]
    contract_value: Option<String>,
    #[serde(default)]
    tick_size: Option<String>,
    #[serde(default)]
    strike_price: Option<String>,
    #[serde(default)]
    settlement_time: Option<String>,
    #[serde(default)]
    is_quanto: Option<bool>,
    #[serde(default)]
    maker_commission_rate: Option<String>,
    #[serde(default)]
    taker_commission_rate: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct DeltaAsset {
    #[serde(default)]
    symbol: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct DeltaTicker {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    best_bid: Option<String>,
    #[serde(default)]
    best_ask: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    turnover: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    spot_price: Option<String>,
    #[serde(default)]
    price_change_percent_24h: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DeltaOrderBook {
    #[serde(default)]
    buy: Vec<DeltaOrderBookLevel>,
    #[serde(default)]
    sell: Vec<DeltaOrderBookLevel>,
}

#[derive(Debug, Default, Deserialize)]
struct DeltaOrderBookLevel {
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct DeltaTrade {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    buyer_role: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct DeltaCandle {
    #[serde(default)]
    time: Option<i64>,
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
struct DeltaOrder {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    product_symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    limit_price: Option<String>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    unfilled_size: Option<String>,
    #[serde(default)]
    average_fill_price: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    time_in_force: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    reduce_only: Option<bool>,
    #[serde(default)]
    post_only: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct DeltaBalance {
    #[serde(default)]
    asset: Option<DeltaAsset>,
    #[serde(default)]
    balance: Option<String>,
    #[serde(default)]
    available_balance: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Delta::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Delta);
        assert_eq!(exchange.name(), "Delta Exchange");
        assert!(exchange.has().swap);
        assert!(exchange.has().future);
    }
}
