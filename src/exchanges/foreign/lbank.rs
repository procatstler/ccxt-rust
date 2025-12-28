//! LBank Exchange Implementation
//!
//! Chinese cryptocurrency exchange

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

type HmacSha256 = Hmac<Sha256>;

/// LBank exchange
pub struct Lbank {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Lbank {
    const BASE_URL: &'static str = "https://api.lbank.info/v2";
    const RATE_LIMIT_MS: u64 = 50; // 20 requests per second

    /// Create new LBank instance
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
            fetch_my_trades: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/38063602-9605e28a-3302-11e8-81be-64b1e53c4f48.jpg".into()),
            api: api_urls,
            www: Some("https://www.lbank.info".into()),
            doc: vec!["https://www.lbank.info/en-US/docs/index.html".into()],
            fees: Some("https://www.lbank.info/fees.html".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "minute1".into());
        timeframes.insert(Timeframe::Minute5, "minute5".into());
        timeframes.insert(Timeframe::Minute15, "minute15".into());
        timeframes.insert(Timeframe::Minute30, "minute30".into());
        timeframes.insert(Timeframe::Hour1, "hour1".into());
        timeframes.insert(Timeframe::Hour4, "hour4".into());
        timeframes.insert(Timeframe::Hour8, "hour8".into());
        timeframes.insert(Timeframe::Day1, "day1".into());
        timeframes.insert(Timeframe::Week1, "week1".into());

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
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();

        let mut request_params = params.unwrap_or_default();
        request_params.insert("api_key".into(), api_key.to_string());
        request_params.insert("timestamp".into(), timestamp);

        // Sort parameters and create signature string
        let mut sorted_keys: Vec<_> = request_params.keys().collect();
        sorted_keys.sort();

        let sign_str: String = sorted_keys
            .iter()
            .map(|k| format!("{}={}", k, request_params.get(*k).unwrap()))
            .collect::<Vec<_>>()
            .join("&");

        let sign_str = format!("{sign_str}&secret_key={api_secret}");

        let mut mac = HmacSha256::new_from_slice(sign_str.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError { message: "Invalid secret key".into() })?;
        mac.update(sign_str.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes()).to_uppercase();

        request_params.insert("sign".into(), signature);

        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/x-www-form-urlencoded".into());

        self.client.post_form(path, &request_params, Some(headers)).await
    }

    /// Convert symbol to market ID (BTC/USD -> btc_usd)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_").to_lowercase()
    }
}

#[async_trait]
impl Exchange for Lbank {
    fn id(&self) -> ExchangeId {
        ExchangeId::Lbank
    }

    fn name(&self) -> &str {
        "LBank"
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
        let response: LbankResponse<Vec<LbankMarket>> = self.public_get("/accuracy.do", None).await?;

        if response.result != Some(true) {
            return Err(CcxtError::ExchangeError {
                message: response.error_code.map(|c| c.to_string()).unwrap_or_else(|| "Failed to fetch markets".into()),
            });
        }

        let data = response.data.unwrap_or_default();
        let mut markets = Vec::new();

        for market_data in &data {
            let id = market_data.symbol.clone().unwrap_or_default();
            let parts: Vec<&str> = id.split('_').collect();
            if parts.len() != 2 {
                continue;
            }

            let base = parts[0].to_uppercase();
            let quote = parts[1].to_uppercase();
            let symbol = format!("{base}/{quote}");

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.clone()),
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
                taker: Some(Decimal::new(1, 3)), // 0.1%
                maker: Some(Decimal::new(1, 3)), // 0.1%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                settle: None,
                settle_id: None,
                precision: MarketPrecision {
                    amount: market_data.quantity_accuracy,
                    price: market_data.price_accuracy,
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax {
                        min: market_data.min_tran_qty.as_ref().and_then(|s| s.parse().ok()),
                        max: None,
                    },
                    price: crate::types::MinMax::default(),
                    cost: crate::types::MinMax::default(),
                    leverage: crate::types::MinMax::default(),
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(market_data).unwrap_or_default(),
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
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: LbankResponse<Vec<LbankTicker>> = self.public_get("/ticker/24hr.do", Some(params)).await?;

        if response.result != Some(true) {
            return Err(CcxtError::ExchangeError {
                message: "Failed to fetch ticker".into(),
            });
        }

        let data = response.data.unwrap_or_default();
        let ticker_data = data.first().ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let timestamp = ticker_data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: ticker_data.high.as_ref().and_then(|s| s.parse().ok()),
            low: ticker_data.low.as_ref().and_then(|s| s.parse().ok()),
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: None,
            close: ticker_data.latest.as_ref().and_then(|s| s.parse().ok()),
            last: ticker_data.latest.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: ticker_data.change.as_ref().and_then(|s| s.parse().ok()),
            percentage: None,
            average: None,
            base_volume: ticker_data.vol.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: ticker_data.turnover.as_ref().and_then(|s| s.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(ticker_data).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("size".into(), limit.unwrap_or(60).to_string());

        let response: LbankResponse<LbankOrderBook> = self.public_get("/depth.do", Some(params)).await?;

        let data = response.data.ok_or_else(|| CcxtError::ParseError {
            data_type: "OrderBook".to_string(),
            message: "No data".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<Vec<String>>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().filter_map(|e| {
                if e.len() >= 2 {
                    Some(OrderBookEntry {
                        price: e[0].parse().ok()?,
                        amount: e[1].parse().ok()?,
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
            bids: parse_entries(&data.bids),
            asks: parse_entries(&data.asks),
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.convert_to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("size".into(), limit.unwrap_or(100).to_string());

        let response: LbankResponse<Vec<LbankTrade>> = self.public_get("/trades.do", Some(params)).await?;

        let data = response.data.unwrap_or_default();
        let limit = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = data.iter()
            .take(limit)
            .map(|t| {
                let timestamp = t.date_ms.unwrap_or_else(|| Utc::now().timestamp_millis());
                let price: Decimal = t.price.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
                let amount: Decimal = t.amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();

                Trade {
                    id: t.tid.clone().unwrap_or_default(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: t.direction.clone(),
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
        let market_id = self.convert_to_market_id(symbol);
        let interval = self.timeframes.get(&timeframe).cloned().ok_or_else(|| CcxtError::NotSupported {
            feature: format!("Timeframe {timeframe:?}"),
        })?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("type".into(), interval);
        params.insert("size".into(), limit.unwrap_or(100).to_string());
        if let Some(s) = since {
            params.insert("time".into(), (s / 1000).to_string());
        }

        let response: LbankResponse<Vec<Vec<serde_json::Value>>> = self.public_get("/kline.do", Some(params)).await?;

        let data = response.data.unwrap_or_default();
        let ohlcv: Vec<OHLCV> = data.iter()
            .filter_map(|c| {
                Some(OHLCV {
                    timestamp: c.first()?.as_i64()? * 1000,
                    open: c.get(1)?.as_f64().map(Decimal::try_from).and_then(|r| r.ok())?,
                    high: c.get(2)?.as_f64().map(Decimal::try_from).and_then(|r| r.ok())?,
                    low: c.get(3)?.as_f64().map(Decimal::try_from).and_then(|r| r.ok())?,
                    close: c.get(4)?.as_f64().map(Decimal::try_from).and_then(|r| r.ok())?,
                    volume: c.get(5)?.as_f64().map(Decimal::try_from).and_then(|r| r.ok())?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: LbankResponse<LbankBalanceData> = self.private_request("/user_info.do", None).await?;

        let data = response.data.ok_or_else(|| CcxtError::AuthenticationError {
            message: "Failed to fetch balance".into(),
        })?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for (currency, amount) in &data.free {
            let free: Decimal = amount.parse().unwrap_or_default();
            let used: Decimal = data.freeze.get(currency)
                .and_then(|s| s.parse().ok())
                .unwrap_or_default();

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
        let market_id = self.convert_to_market_id(symbol);

        let type_str = match (side, order_type) {
            (OrderSide::Buy, OrderType::Market) => "buy_market",
            (OrderSide::Buy, _) => "buy",
            (OrderSide::Sell, OrderType::Market) => "sell_market",
            (OrderSide::Sell, _) => "sell",
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("type".into(), type_str.to_string());
        params.insert("amount".into(), amount.to_string());
        if let Some(p) = price {
            params.insert("price".into(), p.to_string());
        }

        let response: LbankResponse<LbankOrderResult> = self.private_request("/create_order.do", Some(params)).await?;

        if response.result != Some(true) {
            return Err(CcxtError::ExchangeError {
                message: response.error_code.map(|c| c.to_string()).unwrap_or_else(|| "Failed to create order".into()),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ParseError {
            data_type: "Order".to_string(),
            message: "No data".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: data.order_id.clone().unwrap_or_default(),
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
            info: serde_json::to_value(&data).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("order_id".into(), id.to_string());

        let response: LbankResponse<serde_json::Value> = self.private_request("/cancel_order.do", Some(params)).await?;

        if response.result != Some(true) {
            return Err(CcxtError::ExchangeError {
                message: response.error_code.map(|c| c.to_string()).unwrap_or_else(|| "Failed to cancel order".into()),
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
        let market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("order_id".into(), id.to_string());

        let response: LbankResponse<LbankOrder> = self.private_request("/orders_info.do", Some(params)).await?;

        if response.result != Some(true) {
            return Err(CcxtError::OrderNotFound {
                order_id: id.to_string(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })?;

        let timestamp = data.create_time.unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.status.as_deref() {
            Some("0") | Some("-1") => OrderStatus::Open,
            Some("1") | Some("2") => OrderStatus::Closed,
            Some("4") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let side = match data.order_type.as_deref() {
            Some("buy") | Some("buy_market") => OrderSide::Buy,
            Some("sell") | Some("sell_market") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = if data.order_type.as_ref().map(|s| s.contains("market")).unwrap_or(false) {
            OrderType::Market
        } else {
            OrderType::Limit
        };

        Ok(Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.custom_id.clone(),
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
            order_type,
            time_in_force: None,
            side,
            price: data.price.as_ref().and_then(|s| s.parse().ok()),
            average: data.avg_price.as_ref().and_then(|s| s.parse().ok()),
            amount: data.amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
            filled: data.deal_amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
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

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let market_id = symbol.map(|s| self.convert_to_market_id(s)).unwrap_or_default();

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("current_page".into(), "1".to_string());
        params.insert("page_length".into(), limit.unwrap_or(100).to_string());

        let response: LbankResponse<LbankOrdersData> = self.private_request("/orders_info_no_deal.do", Some(params)).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data".into(),
        })?;

        let orders: Vec<Order> = data.orders.iter()
            .map(|o| {
                let timestamp = o.create_time.unwrap_or_else(|| Utc::now().timestamp_millis());

                let status = match o.status.as_deref() {
                    Some("0") | Some("-1") => OrderStatus::Open,
                    Some("1") | Some("2") => OrderStatus::Closed,
                    Some("4") => OrderStatus::Canceled,
                    _ => OrderStatus::Open,
                };

                let side = match o.order_type.as_deref() {
                    Some("buy") | Some("buy_market") => OrderSide::Buy,
                    Some("sell") | Some("sell_market") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                let order_type = if o.order_type.as_ref().map(|s| s.contains("market")).unwrap_or(false) {
                    OrderType::Market
                } else {
                    OrderType::Limit
                };

                let symbol_id = o.symbol.clone().unwrap_or_default();
                let parts: Vec<&str> = symbol_id.split('_').collect();
                let sym = if parts.len() == 2 {
                    format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
                } else {
                    symbol.unwrap_or("").to_string()
                };

                Order {
                    id: o.order_id.clone().unwrap_or_default(),
                    client_order_id: o.custom_id.clone(),
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    last_trade_timestamp: None,
                    last_update_timestamp: None,
                    status,
                    symbol: sym,
                    order_type,
                    time_in_force: None,
                    side,
                    price: o.price.as_ref().and_then(|s| s.parse().ok()),
                    average: o.avg_price.as_ref().and_then(|s| s.parse().ok()),
                    amount: o.amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
                    filled: o.deal_amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
                    remaining: None,
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

#[derive(Debug, Deserialize)]
struct LbankResponse<T> {
    #[serde(default)]
    result: Option<bool>,
    #[serde(default)]
    error_code: Option<i32>,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LbankMarket {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    quantity_accuracy: Option<i32>,
    #[serde(default)]
    price_accuracy: Option<i32>,
    #[serde(default)]
    min_tran_qty: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LbankTicker {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    latest: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    vol: Option<String>,
    #[serde(default)]
    turnover: Option<String>,
    #[serde(default)]
    change: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct LbankOrderBook {
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LbankTrade {
    #[serde(default)]
    tid: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    date_ms: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct LbankOrder {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    custom_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    deal_amount: Option<String>,
    #[serde(default)]
    avg_price: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    create_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct LbankOrdersData {
    #[serde(default)]
    orders: Vec<LbankOrder>,
}

#[derive(Debug, Default, Deserialize)]
struct LbankBalanceData {
    #[serde(default)]
    free: HashMap<String, String>,
    #[serde(default)]
    freeze: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct LbankOrderResult {
    #[serde(default)]
    order_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Lbank::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Lbank);
        assert_eq!(exchange.name(), "LBank");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Lbank::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/USD"), "btc_usd");
    }
}
