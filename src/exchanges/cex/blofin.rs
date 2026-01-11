//! Blofin Exchange Implementation
//!
//! Options-focused derivatives exchange

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
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
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, TimeInForce, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// Blofin exchange - options and derivatives focused platform
pub struct Blofin {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Blofin {
    const BASE_URL: &'static str = "https://openapi.blofin.com";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second
    const VERSION: &'static str = "v1";

    /// Create new Blofin instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: false,
            margin: false,
            swap: true,
            future: false,
            option: true,
            fetch_markets: true,
            fetch_currencies: false,
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
            cancel_all_orders: false,
            fetch_order: false,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: false,
            fetch_deposit_address: false,
            ws: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/user-attachments/assets/518cdf80-f05d-4821-a3e3-d48ceb41d73b".into()),
            api: api_urls,
            www: Some("https://www.blofin.com".into()),
            doc: vec!["https://blofin.com/docs".into()],
            fees: None,
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute3, "3m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1H".into());
        timeframes.insert(Timeframe::Hour2, "2H".into());
        timeframes.insert(Timeframe::Hour4, "4H".into());
        timeframes.insert(Timeframe::Hour6, "6H".into());
        timeframes.insert(Timeframe::Hour8, "8H".into());
        timeframes.insert(Timeframe::Hour12, "12H".into());
        timeframes.insert(Timeframe::Day1, "1D".into());
        timeframes.insert(Timeframe::Day3, "3D".into());
        timeframes.insert(Timeframe::Week1, "1W".into());
        timeframes.insert(Timeframe::Month1, "1M".into());

        Ok(Self {
            config,
            public_client,
            private_client,
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

        let url = if let Some(p) = params {
            let query: String = p
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("/api/{}/{path}?{query}", Self::VERSION)
        } else {
            format!("/api/{}/{path}", Self::VERSION)
        };

        self.public_client.get(&url, None, None).await
    }

    /// Private API call
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;
        let password = self.config.password().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Password required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let mut request = format!("/api/{}/{path}", Self::VERSION);

        let mut headers = HashMap::new();
        headers.insert("ACCESS-KEY".into(), api_key.to_string());
        headers.insert("ACCESS-PASSPHRASE".into(), password.to_string());
        headers.insert("ACCESS-TIMESTAMP".into(), timestamp.clone());
        headers.insert("ACCESS-NONCE".into(), timestamp.clone());

        let mut sign_body = String::new();
        let url = if method == "GET" {
            if !params.is_empty() {
                let query: String = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");
                request = format!("{request}?{query}");
            }
            request.clone()
        } else {
            if !params.is_empty() {
                sign_body = serde_json::to_string(&params).unwrap_or_default();
                headers.insert("Content-Type".into(), "application/json".into());
            }
            request.clone()
        };

        // Sign: request + method + timestamp + timestamp + body
        let auth_string = format!("{request}{method}{timestamp}{timestamp}{sign_body}");
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(auth_string.as_bytes());
        let signature = general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        headers.insert("ACCESS-SIGN".into(), signature);

        match method {
            "GET" => self.private_client.get(&url, None, Some(headers)).await,
            "POST" => {
                let body = if !params.is_empty() {
                    serde_json::from_str(&sign_body).ok()
                } else {
                    None
                };
                self.private_client.post(&url, body, Some(headers)).await
            }
            "DELETE" => self.private_client.delete(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// Parse ticker response
    fn parse_ticker(&self, data: &BlofinTicker, symbol: &str) -> Ticker {
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high24h,
            low: data.low24h,
            bid: data.bid_px,
            bid_volume: data.bid_sz,
            ask: data.ask_px,
            ask_volume: data.ask_sz,
            vwap: None,
            open: data.open24h,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.vol24h,
            quote_volume: data.vol_ccy24h,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order response
    fn parse_order(&self, data: &BlofinOrder, symbol: &str) -> Order {
        let status = match data.state.as_str() {
            "live" => OrderStatus::Open,
            "partially_filled" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "canceled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_str() {
            "market" => OrderType::Market,
            "limit" => OrderType::Limit,
            "post_only" => OrderType::Limit,
            "fok" => OrderType::Limit,
            "ioc" => OrderType::Limit,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = match data.order_type.as_str() {
            "fok" => Some(TimeInForce::FOK),
            "ioc" => Some(TimeInForce::IOC),
            _ => None,
        };

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.size.parse().unwrap_or_default();
        let filled: Decimal = data.filled_size.parse().unwrap_or_default();
        let remaining = Some(amount - filled);
        let average: Option<Decimal> = data.average_price.as_ref().and_then(|p| p.parse().ok());

        let cost = average.map(|avg| avg * filled);

        Order {
            id: data.order_id.clone(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(data.create_time),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.create_time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: Some(data.update_time),
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force,
            side,
            price,
            average,
            amount,
            filled,
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: data.tp_order_price.as_ref().and_then(|p| p.parse().ok()),
            stop_loss_price: data.sl_order_price.as_ref().and_then(|p| p.parse().ok()),
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: Some(data.reduce_only == "true"),
            post_only: Some(data.order_type == "post_only"),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance response
    fn parse_balance(&self, balances: &[BlofinBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let total: Option<Decimal> = b.equity.parse().ok();
            let free: Option<Decimal> = b.available_balance.parse().ok();
            let used = match (total, free) {
                (Some(t), Some(f)) => Some(t - f),
                _ => None,
            };

            let balance = Balance {
                free,
                used,
                total,
                debt: None,
            };
            result.add(&b.currency, balance);
        }

        result
    }
}

#[async_trait]
impl Exchange for Blofin {
    fn id(&self) -> ExchangeId {
        ExchangeId::Blofin
    }

    fn name(&self) -> &str {
        "BloFin"
    }

    fn version(&self) -> &str {
        Self::VERSION
    }

    fn countries(&self) -> &[&str] {
        &["US"]
    }

    fn rate_limit(&self) -> u64 {
        Self::RATE_LIMIT_MS
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
            let markets = self.markets.read().unwrap();
            if !reload && !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let markets_vec = self.fetch_markets().await?;
        let mut markets_map = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for market in markets_vec {
            markets_by_id.insert(market.id.clone(), market.symbol.clone());
            markets_map.insert(market.symbol.clone(), market);
        }

        {
            let mut markets = self.markets.write().unwrap();
            *markets = markets_map.clone();
        }
        {
            let mut by_id = self.markets_by_id.write().unwrap();
            *by_id = markets_by_id;
        }

        Ok(markets_map)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response: BlofinResponse<Vec<BlofinMarket>> = self
            .public_get("market/instruments", None)
            .await?;

        let mut markets = Vec::new();

        for instrument in response.data {
            if instrument.state != "live" {
                continue;
            }

            let base = instrument.base_ccy.clone();
            let quote = instrument.quote_ccy.clone();
            let symbol = format!("{base}/{quote}");

            let (market_type, swap, future, option) = match instrument.inst_type.as_str() {
                "SWAP" => (MarketType::Swap, true, false, false),
                "FUTURES" => (MarketType::Future, false, true, false),
                "OPTION" => (MarketType::Option, false, false, true),
                _ => (MarketType::Swap, true, false, false),
            };

            let market = Market {
                id: instrument.inst_id.clone(),
                lowercase_id: Some(instrument.inst_id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: instrument.base_ccy.clone(),
                quote_id: instrument.quote_ccy.clone(),
                settle: Some(instrument.settle_ccy.clone()),
                settle_id: Some(instrument.settle_ccy.clone()),
                active: true,
                market_type,
                spot: false,
                margin: false,
                swap,
                future,
                option,
                index: false,
                contract: swap || future || option,
                linear: Some(true),
                inverse: Some(false),
                sub_type: None,
                taker: Some(Decimal::new(6, 4)), // 0.0006
                maker: Some(Decimal::new(2, 4)), // 0.0002
                contract_size: instrument.ct_val.parse().ok(),
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: instrument.lot_sz.parse().ok(),
                    price: instrument.tick_sz.parse().ok(),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&instrument).unwrap_or_default(),
                tier_based: false,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);

        let response: BlofinResponse<Vec<BlofinTicker>> = self
            .public_get("market/tickers", Some(params))
            .await?;

        let ticker_data = response.data.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "No ticker data returned".into(),
        })?;

        Ok(self.parse_ticker(ticker_data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: BlofinResponse<Vec<BlofinTicker>> = self
            .public_get("market/tickers", None)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for data in response.data {
            if let Some(symbol) = markets_by_id.get(&data.inst_id) {
                if let Some(filter) = symbols {
                    if !filter.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let ticker = self.parse_ticker(&data, symbol);
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);
        if let Some(l) = limit {
            params.insert("sz".into(), l.to_string());
        }

        let response: BlofinResponse<Vec<BlofinOrderBook>> = self
            .public_get("market/books", Some(params))
            .await?;

        let book_data = response.data.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "No order book data returned".into(),
        })?;

        let bids: Vec<OrderBookEntry> = book_data
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: b[0].parse().unwrap_or_default(),
                amount: b[1].parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = book_data
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: a[0].parse().unwrap_or_default(),
                amount: a[1].parse().unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(book_data.ts),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(book_data.ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
            bids,
            asks,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BlofinResponse<Vec<BlofinTrade>> = self
            .public_get("market/trades", Some(params))
            .await?;

        let trades: Vec<Trade> = response
            .data
            .iter()
            .map(|t| Trade {
                id: t.trade_id.clone(),
                order: None,
                timestamp: Some(t.ts),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(t.ts)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(t.side.clone()),
                taker_or_maker: None,
                price: t.px.parse().unwrap_or_default(),
                amount: t.sz.parse().unwrap_or_default(),
                cost: Some(
                    t.px.parse::<Decimal>().unwrap_or_default()
                        * t.sz.parse::<Decimal>().unwrap_or_default(),
                ),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(t).unwrap_or_default(),
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
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let interval = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {timeframe:?}"),
        })?;

        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);
        params.insert("bar".into(), interval.clone());
        if let Some(s) = since {
            params.insert("after".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1440).to_string());
        }

        let response: BlofinResponse<Vec<Vec<String>>> = self
            .public_get("market/candles", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .data
            .iter()
            .filter_map(|c| {
                if c.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].parse().ok()?,
                    open: c[1].parse().ok()?,
                    high: c[2].parse().ok()?,
                    low: c[3].parse().ok()?,
                    close: c[4].parse().ok()?,
                    volume: c[5].parse().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: BlofinResponse<Vec<BlofinBalance>> = self
            .private_request("GET", "asset/balances", params)
            .await?;

        Ok(self.parse_balance(&response.data))
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);
        params.insert("side".into(), match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }.to_string());
        params.insert("orderType".into(), match order_type {
            OrderType::Limit => "limit",
            OrderType::Market => "market",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {order_type:?}"),
            }),
        }.to_string());
        params.insert("size".into(), amount.to_string());

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
        }

        let response: BlofinResponse<Vec<BlofinOrder>> = self
            .private_request("POST", "trade/order", params)
            .await?;

        let order_data = response.data.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "No order data returned".into(),
        })?;

        Ok(self.parse_order(order_data, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: BlofinResponse<Vec<BlofinOrder>> = self
            .private_request("POST", "trade/cancel-order", params)
            .await?;

        let order_data = response.data.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "No order data returned".into(),
        })?;

        Ok(self.parse_order(order_data, symbol))
    }

    async fn fetch_order(&self, _id: &str, _symbol: &str) -> CcxtResult<Order> {
        Err(CcxtError::NotSupported {
            feature: "fetchOrder".into(),
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                markets
                    .get(s)
                    .map(|m| m.id.clone())
                    .ok_or_else(|| CcxtError::BadSymbol {
                        symbol: s.to_string(),
                    })?
            };
            params.insert("instId".into(), market_id);
        }

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BlofinResponse<Vec<BlofinOrder>> = self
            .private_request("GET", "trade/orders-pending", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = response
            .data
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.inst_id)
                    .cloned()
                    .unwrap_or_else(|| o.inst_id.clone());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        let markets = self.markets.read().unwrap();
        markets.get(symbol).map(|m| m.id.clone())
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let markets_by_id = self.markets_by_id.read().unwrap();
        markets_by_id.get(market_id).cloned()
    }

    fn sign(
        &self,
        path: &str,
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{}/api/{}/{}", Self::BASE_URL, Self::VERSION, path);
        let mut headers = HashMap::new();

        if api == "private" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();
            let password = self.config.password().unwrap_or_default();

            let timestamp = Utc::now().timestamp_millis().to_string();
            let mut request = format!("/api/{}/{}", Self::VERSION, path);

            headers.insert("ACCESS-KEY".into(), api_key.to_string());
            headers.insert("ACCESS-PASSPHRASE".into(), password.to_string());
            headers.insert("ACCESS-TIMESTAMP".into(), timestamp.clone());
            headers.insert("ACCESS-NONCE".into(), timestamp.clone());

            let mut sign_body = String::new();

            if method == "GET" {
                if !params.is_empty() {
                    let query: String = params
                        .iter()
                        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                        .collect::<Vec<_>>()
                        .join("&");
                    url = format!("{url}?{query}");
                    request = format!("{request}?{query}");
                }
            } else if !params.is_empty() {
                sign_body = serde_json::to_string(params).unwrap_or_default();
                headers.insert("Content-Type".into(), "application/json".into());
            }

            let auth_string = format!("{request}{method}{timestamp}{timestamp}{sign_body}");
            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(auth_string.as_bytes());
            let signature = general_purpose::STANDARD.encode(mac.finalize().into_bytes());
            headers.insert("ACCESS-SIGN".into(), signature);
        } else if !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}?{query}");
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        }
    }
}

// === Blofin API Response Types ===

#[derive(Debug, Deserialize)]
struct BlofinResponse<T> {
    code: String,
    msg: String,
    data: T,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlofinMarket {
    inst_id: String,
    inst_type: String,
    base_ccy: String,
    quote_ccy: String,
    settle_ccy: String,
    ct_val: String,
    lot_sz: String,
    tick_sz: String,
    state: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlofinTicker {
    inst_id: String,
    last: Option<Decimal>,
    open24h: Option<Decimal>,
    high24h: Option<Decimal>,
    low24h: Option<Decimal>,
    vol24h: Option<Decimal>,
    vol_ccy24h: Option<Decimal>,
    bid_px: Option<Decimal>,
    bid_sz: Option<Decimal>,
    ask_px: Option<Decimal>,
    ask_sz: Option<Decimal>,
    ts: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlofinOrderBook {
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
    ts: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlofinTrade {
    trade_id: String,
    inst_id: String,
    px: String,
    sz: String,
    side: String,
    ts: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlofinOrder {
    order_id: String,
    #[serde(default)]
    client_order_id: Option<String>,
    inst_id: String,
    side: String,
    order_type: String,
    #[serde(default)]
    price: Option<String>,
    size: String,
    filled_size: String,
    #[serde(default)]
    average_price: Option<String>,
    state: String,
    create_time: i64,
    update_time: i64,
    #[serde(default)]
    reduce_only: String,
    #[serde(default)]
    tp_order_price: Option<String>,
    #[serde(default)]
    sl_order_price: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlofinBalance {
    currency: String,
    equity: String,
    available_balance: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let blofin = Blofin::new(config).unwrap();

        assert_eq!(blofin.id(), ExchangeId::Blofin);
        assert_eq!(blofin.name(), "BloFin");
        assert!(blofin.has().swap);
        assert!(blofin.has().option);
        assert!(!blofin.has().spot);
    }
}
