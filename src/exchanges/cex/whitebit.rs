//! WhiteBit Exchange Implementation
//!
//! European cryptocurrency exchange

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

/// WhiteBit exchange
pub struct Whitebit {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Whitebit {
    const BASE_URL: &'static str = "https://whitebit.com/api/v4";
    const RATE_LIMIT_MS: u64 = 50; // 20 requests per second

    /// Create new WhiteBit instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            spot: true,
            margin: true,
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
            logo: Some("https://user-images.githubusercontent.com/1294454/66732963-8eb7dd00-ee66-11e9-849b-10d9282bb9e0.jpg".into()),
            api: api_urls,
            www: Some("https://whitebit.com".into()),
            doc: vec!["https://docs.whitebit.com/".into()],
            fees: Some("https://whitebit.com/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute3, "3m".into());
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
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
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

        let nonce = Utc::now().timestamp_millis();
        let mut request_body = body.unwrap_or(serde_json::json!({}));
        request_body["request"] = serde_json::json!(format!("/api/v4{}", path));
        request_body["nonce"] = serde_json::json!(nonce);

        let body_str = serde_json::to_string(&request_body)?;
        let payload = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, body_str.as_bytes());

        let mut mac = HmacSha512::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            })?;
        mac.update(payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("X-TXC-APIKEY".into(), api_key.to_string());
        headers.insert("X-TXC-PAYLOAD".into(), payload);
        headers.insert("X-TXC-SIGNATURE".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        self.client.post(path, Some(request_body), Some(headers)).await
    }

    /// Convert symbol (BTC/USDT -> BTC_USDT)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_")
    }
}

#[async_trait]
impl Exchange for Whitebit {
    fn id(&self) -> ExchangeId {
        ExchangeId::Whitebit
    }

    fn name(&self) -> &str {
        "WhiteBit"
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
        let response: HashMap<String, WhitebitMarket> = self.public_get("/public/markets", None).await?;

        let mut markets = Vec::new();
        for (id, info) in response {
            let base = info.stock.clone().unwrap_or_default();
            let quote = info.money.clone().unwrap_or_default();
            let symbol = format!("{base}/{quote}");

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                market_type: MarketType::Spot,
                spot: true,
                margin: info.margin_trading.unwrap_or(false),
                swap: false,
                future: false,
                option: false,
                index: false,
                active: info.trade_disabled != Some(true),
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: info.taker_fee.as_ref().and_then(|s| s.parse().ok()),
                maker: info.maker_fee.as_ref().and_then(|s| s.parse().ok()),
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                settle: None,
                settle_id: None,
                precision: MarketPrecision {
                    amount: info.stock_prec,
                    price: info.money_prec,
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax {
                        min: info.min_amount.as_ref().and_then(|s| s.parse().ok()),
                        max: info.max_amount.as_ref().and_then(|s| s.parse().ok()),
                    },
                    price: crate::types::MinMax::default(),
                    cost: crate::types::MinMax {
                        min: info.min_total.as_ref().and_then(|s| s.parse().ok()),
                        max: info.max_total.as_ref().and_then(|s| s.parse().ok()),
                    },
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
        let response: HashMap<String, WhitebitTicker> = self.public_get("/public/ticker", None).await?;

        let market_id = self.convert_to_market_id(symbol);
        let ticker_data = response.get(&market_id).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: ticker_data.high.as_ref().and_then(|s| s.parse().ok()),
            low: ticker_data.low.as_ref().and_then(|s| s.parse().ok()),
            bid: ticker_data.bid.as_ref().and_then(|s| s.parse().ok()),
            bid_volume: None,
            ask: ticker_data.ask.as_ref().and_then(|s| s.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: ticker_data.open.as_ref().and_then(|s| s.parse().ok()),
            close: ticker_data.last.as_ref().and_then(|s| s.parse().ok()),
            last: ticker_data.last.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: ticker_data.change.as_ref().and_then(|s| s.parse().ok()),
            percentage: None,
            average: None,
            base_volume: ticker_data.volume.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: ticker_data.deal.as_ref().and_then(|s| s.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(ticker_data).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/public/orderbook/{market_id}");

        let mut params = HashMap::new();
        params.insert("limit".into(), limit.unwrap_or(100).to_string());
        params.insert("level".into(), "2".to_string());

        let response: WhitebitOrderBook = self.public_get(&path, Some(params)).await?;

        let timestamp = response.timestamp.map(|t| (t * 1000.0) as i64).unwrap_or_else(|| Utc::now().timestamp_millis());

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
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
            bids: parse_entries(&response.bids),
            asks: parse_entries(&response.asks),
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/public/trades/{market_id}");

        let mut params = HashMap::new();
        params.insert("limit".into(), limit.unwrap_or(100).to_string());

        let response: Vec<WhitebitTrade> = self.public_get(&path, Some(params)).await?;

        let limit = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = response.iter()
            .take(limit)
            .map(|t| {
                let timestamp = t.time.map(|time| (time * 1000.0) as i64).unwrap_or_else(|| Utc::now().timestamp_millis());
                let price: Decimal = t.price.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
                let amount: Decimal = t.amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();

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
                    side: t.trade_type.clone(),
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
        let interval = self.timeframes.get(&timeframe).cloned().unwrap_or("1h".into());

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        params.insert("interval".into(), interval);
        params.insert("limit".into(), limit.unwrap_or(100).to_string());
        if let Some(s) = since {
            params.insert("start".into(), (s / 1000).to_string());
        }

        let response: Vec<Vec<serde_json::Value>> = self.public_get("/public/kline", Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response.iter().filter_map(|c| {
            Some(OHLCV {
                timestamp: c.first()?.as_i64()? * 1000,
                open: c.get(1)?.as_str()?.parse().ok()?,
                close: c.get(2)?.as_str()?.parse().ok()?,
                high: c.get(3)?.as_str()?.parse().ok()?,
                low: c.get(4)?.as_str()?.parse().ok()?,
                volume: c.get(5)?.as_str()?.parse().ok()?,
            })
        }).collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: HashMap<String, WhitebitBalance> = self.private_post("/trade-account/balance", None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for (currency, data) in response {
            let free: Decimal = data.available.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
            let used: Decimal = data.freeze.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();

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

        let path = match order_type {
            OrderType::Limit => "/order/new",
            OrderType::Market => "/order/stock_market",
            _ => "/order/new",
        };

        let mut body = serde_json::json!({
            "market": market_id,
            "side": match side { OrderSide::Buy => "buy", OrderSide::Sell => "sell" },
            "amount": amount.to_string(),
        });

        if let Some(p) = price {
            body["price"] = serde_json::json!(p.to_string());
        }

        let response: WhitebitOrder = self.private_post(path, Some(body)).await?;

        let timestamp = response.ctime.map(|t| (t * 1000.0) as i64).unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Order {
            id: response.order_id.map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: response.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: response.mtime.map(|t| (t * 1000.0) as i64),
            status: OrderStatus::Open,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled: response.deal_stock.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
            remaining: response.left.as_ref().and_then(|s| s.parse().ok()),
            cost: response.deal_money.as_ref().and_then(|s| s.parse().ok()),
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
        let market_id = self.convert_to_market_id(symbol);

        let body = serde_json::json!({
            "market": market_id,
            "orderId": id.parse::<i64>().unwrap_or(0),
        });

        let response: WhitebitOrder = self.private_post("/order/cancel", Some(body)).await?;

        let timestamp = response.ctime.map(|t| (t * 1000.0) as i64).unwrap_or_else(|| Utc::now().timestamp_millis());

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
        // WhiteBit doesn't have a direct fetch order endpoint
        // We need to check open orders
        let orders = self.fetch_open_orders(Some(symbol), None, None).await?;

        orders.into_iter()
            .find(|o| o.id == id)
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
        let mut body = serde_json::json!({
            "limit": limit.unwrap_or(100),
        });

        if let Some(s) = symbol {
            body["market"] = serde_json::json!(self.convert_to_market_id(s));
        }

        let response: Vec<WhitebitOrder> = self.private_post("/orders", Some(body)).await?;

        let orders: Vec<Order> = response.iter()
            .map(|o| {
                let timestamp = o.ctime.map(|t| (t * 1000.0) as i64).unwrap_or_else(|| Utc::now().timestamp_millis());
                let side = match o.side.as_deref() {
                    Some("buy") => OrderSide::Buy,
                    Some("sell") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                let order_type = match o.order_type.as_deref() {
                    Some("limit") => OrderType::Limit,
                    Some("market") => OrderType::Market,
                    Some("stop_limit") => OrderType::StopLimit,
                    Some("stop_market") => OrderType::StopMarket,
                    _ => OrderType::Limit,
                };

                let sym = o.market.as_ref()
                    .map(|m| m.replace("_", "/"))
                    .unwrap_or_else(|| symbol.unwrap_or("").to_string());

                Order {
                    id: o.order_id.map(|i| i.to_string()).unwrap_or_default(),
                    client_order_id: o.client_order_id.clone(),
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    last_trade_timestamp: None,
                    last_update_timestamp: o.mtime.map(|t| (t * 1000.0) as i64),
                    status: OrderStatus::Open,
                    symbol: sym,
                    order_type,
                    time_in_force: None,
                    side,
                    price: o.price.as_ref().and_then(|s| s.parse().ok()),
                    average: o.deal_money.as_ref().zip(o.deal_stock.as_ref())
                        .and_then(|(money, stock)| {
                            let m: Decimal = money.parse().ok()?;
                            let s: Decimal = stock.parse().ok()?;
                            if s > Decimal::ZERO { Some(m / s) } else { None }
                        }),
                    amount: o.amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
                    filled: o.deal_stock.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
                    remaining: o.left.as_ref().and_then(|s| s.parse().ok()),
                    cost: o.deal_money.as_ref().and_then(|s| s.parse().ok()),
                    trades: Vec::new(),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(o).unwrap_or_default(),
                    stop_price: o.activation_price.as_ref().and_then(|s| s.parse().ok()),
                    trigger_price: o.activation_price.as_ref().and_then(|s| s.parse().ok()),
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
struct WhitebitMarket {
    #[serde(default)]
    stock: Option<String>,
    #[serde(default)]
    money: Option<String>,
    #[serde(default)]
    stock_prec: Option<i32>,
    #[serde(default)]
    money_prec: Option<i32>,
    #[serde(default)]
    min_amount: Option<String>,
    #[serde(default)]
    max_amount: Option<String>,
    #[serde(default)]
    min_total: Option<String>,
    #[serde(default)]
    max_total: Option<String>,
    #[serde(default)]
    maker_fee: Option<String>,
    #[serde(default)]
    taker_fee: Option<String>,
    #[serde(default)]
    trade_disabled: Option<bool>,
    #[serde(default)]
    margin_trading: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WhitebitTicker {
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    bid: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    deal: Option<String>,
    #[serde(default)]
    change: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WhitebitOrderBook {
    #[serde(default)]
    timestamp: Option<f64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WhitebitTrade {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default, rename = "type")]
    trade_type: Option<String>,
    #[serde(default)]
    time: Option<f64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WhitebitOrder {
    #[serde(default)]
    order_id: Option<i64>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    left: Option<String>,
    #[serde(default)]
    deal_stock: Option<String>,
    #[serde(default)]
    deal_money: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    ctime: Option<f64>,
    #[serde(default)]
    mtime: Option<f64>,
    #[serde(default)]
    activation_price: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WhitebitBalance {
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    freeze: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Whitebit::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Whitebit);
        assert_eq!(exchange.name(), "WhiteBit");
        assert!(exchange.has().spot);
        assert!(exchange.has().fetch_ohlcv);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Whitebit::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/USDT"), "BTC_USDT");
    }
}
