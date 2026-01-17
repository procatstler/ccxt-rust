//! P2B Exchange Implementation
//!
//! CCXT p2b.ts to Rust port

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha512 = Hmac<Sha512>;

/// P2B Exchange
pub struct P2b {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    nonce_counter: RwLock<i64>,
}

impl P2b {
    const BASE_URL_PUBLIC: &'static str = "https://api.p2pb2b.com/api/v2/public";
    const BASE_URL_PRIVATE: &'static str = "https://api.p2pb2b.com/api/v2";
    const RATE_LIMIT_MS: u64 = 100; // 100ms between requests

    /// Create new P2B instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL_PUBLIC, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL_PRIVATE, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
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
            fetch_order: false,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL_PUBLIC.into());
        api_urls.insert("private".into(), Self::BASE_URL_PRIVATE.into());

        let urls = ExchangeUrls {
            logo: Some(
                "https://github.com/ccxt/ccxt/assets/43336371/8da13a80-1f0a-49be-bb90-ff8b25164755"
                    .into(),
            ),
            api: api_urls,
            www: Some("https://p2pb2b.com/".into()),
            doc: vec!["https://github.com/P2B-team/p2b-api-docs/blob/master/api-doc.md".into()],
            fees: Some("https://p2pb2b.com/fee-schedule/".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());

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
            nonce_counter: RwLock::new(0),
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
            format!("{path}?{query}")
        } else {
            path.to_string()
        };

        let response: P2bResponse<T> = self.public_client.get(&url, None, None).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_else(|| "Unknown error".into()),
            });
        }

        response.result.ok_or_else(|| CcxtError::ExchangeError {
            message: "Missing result in response".into(),
        })
    }

    /// Private API call
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        mut params: HashMap<String, serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        // Generate nonce
        let nonce = {
            let mut counter = self.nonce_counter.write().unwrap();
            *counter += 1;
            Utc::now().timestamp_millis() + *counter
        };

        // Add required parameters
        params.insert(
            "request".into(),
            serde_json::json!(format!("/api/v2/{}", path)),
        );
        params.insert("nonce".into(), serde_json::json!(nonce.to_string()));

        // Create payload (base64 encoded JSON)
        let body = serde_json::to_string(&params).map_err(|e| CcxtError::BadRequest {
            message: format!("Failed to serialize params: {e}"),
        })?;
        let payload = general_purpose::STANDARD.encode(body.as_bytes());

        // Create HMAC-SHA512 signature
        let mut mac =
            HmacSha512::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        // Set headers
        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/json".into());
        headers.insert("X-TXC-APIKEY".into(), api_key.to_string());
        headers.insert("X-TXC-PAYLOAD".into(), payload);
        headers.insert("X-TXC-SIGNATURE".into(), signature);

        let url = format!("/{path}");
        let json_body: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let response: P2bResponse<T> = self
            .private_client
            .post(&url, Some(json_body), Some(headers))
            .await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_else(|| "Unknown error".into()),
            });
        }

        response.result.ok_or_else(|| CcxtError::ExchangeError {
            message: "Missing result in response".into(),
        })
    }

    /// Parse market from exchange data
    fn parse_market(&self, data: &P2bMarket) -> Market {
        let base = data.stock.clone();
        let quote = data.money.clone();
        let symbol = format!("{base}/{quote}");
        let limits = &data.limits;

        Market {
            id: data.name.clone(),
            lowercase_id: Some(data.name.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: data.stock.clone(),
            quote_id: data.money.clone(),
            settle: None,
            settle_id: None,
            active: true,
            market_type: MarketType::Spot,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            index: false,
            contract: false,
            linear: None,
            inverse: None,
            sub_type: None,
            taker: Some(Decimal::new(2, 3)), // 0.2%
            maker: Some(Decimal::new(2, 3)), // 0.2%
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            underlying: None,
            underlying_id: None,
            precision: MarketPrecision {
                amount: limits.step_size.parse().ok(),
                price: limits.tick_size.parse().ok(),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                leverage: MinMax {
                    min: None,
                    max: None,
                },
                amount: MinMax {
                    min: limits.min_amount.parse().ok(),
                    max: limits.max_amount.parse().ok(),
                },
                price: MinMax {
                    min: limits.min_price.parse().ok(),
                    max: limits.max_price.parse().ok(),
                },
                cost: MinMax {
                    min: limits.min_total.parse().ok(),
                    max: None,
                },
            },
            margin_modes: None,
            created: None,
            info: serde_json::to_value(data).unwrap_or_default(),
            tier_based: false,
            percentage: true,
        }
    }

    /// Parse ticker
    fn parse_ticker(&self, data: &P2bTickerData, symbol: &str, timestamp: Option<i64>) -> Ticker {
        let ts = timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(ts),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|s| s.parse().ok()),
            low: data.low.as_ref().and_then(|s| s.parse().ok()),
            bid: data.bid.as_ref().and_then(|s| s.parse().ok()),
            bid_volume: None,
            ask: data.ask.as_ref().and_then(|s| s.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: data.open.as_ref().and_then(|s| s.parse().ok()),
            close: data.last.as_ref().and_then(|s| s.parse().ok()),
            last: data.last.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: None,
            percentage: data.change.as_ref().and_then(|s| s.parse().ok()),
            average: None,
            base_volume: data.volume.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: data.deal.as_ref().and_then(|s| s.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse trade
    fn parse_trade(&self, data: &P2bTrade, symbol: &str) -> Trade {
        let timestamp = data.time.or(data.deal_time).map(|t| (t * 1000.0) as i64);

        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.amount.parse().unwrap_or_default();

        Trade {
            id: data
                .id
                .as_ref()
                .or(data.deal_id.as_ref())
                .map(|s| s.to_string())
                .unwrap_or_default(),
            order: data
                .deal_order_id
                .as_ref()
                .cloned()
                .or_else(|| data.deal_order_id_num.as_ref().map(|n| n.to_string())),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.side.clone().or_else(|| data.type_field.clone()),
            taker_or_maker: data.role.as_ref().and_then(|r| {
                if r == "maker" || r == "1" {
                    Some(crate::types::TakerOrMaker::Maker)
                } else if r == "taker" || r == "2" {
                    Some(crate::types::TakerOrMaker::Taker)
                } else {
                    None
                }
            }),
            price,
            amount,
            cost: data.deal.as_ref().and_then(|s| s.parse().ok()),
            fee: data
                .fee
                .as_ref()
                .or(data.deal_fee.as_ref())
                .map(|f| crate::types::Fee {
                    cost: f.parse().ok(),
                    currency: None,
                    rate: None,
                }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order
    fn parse_order(&self, data: &P2bOrder, symbol: &str) -> Order {
        let timestamp = data.timestamp.or(data.ctime).map(|t| (t * 1000.0) as i64);

        let status = if data
            .left
            .as_ref()
            .map(|l| l.parse::<Decimal>().unwrap_or_default())
            .unwrap_or_default()
            == Decimal::ZERO
        {
            OrderStatus::Closed
        } else {
            OrderStatus::Open
        };

        let price: Option<Decimal> = data.price.parse().ok();
        let amount: Decimal = data.amount.parse().unwrap_or_default();
        let filled: Decimal = data
            .deal_stock
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let remaining = Some(amount - filled);

        Order {
            id: data
                .id
                .as_ref()
                .or(data.order_id.as_ref())
                .map(|s| s.to_string())
                .unwrap_or_default(),
            client_order_id: None,
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: data.ftime.map(|t| (t * 1000.0) as i64),
            last_update_timestamp: data.ftime.map(|t| (t * 1000.0) as i64),
            status,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: if data.side.as_deref() == Some("buy") {
                OrderSide::Buy
            } else {
                OrderSide::Sell
            },
            price,
            average: None,
            amount,
            filled,
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: data.deal_money.as_ref().and_then(|s| s.parse().ok()),
            trades: Vec::new(),
            fee: data.deal_fee.as_ref().map(|f| crate::types::Fee {
                cost: f.parse().ok(),
                currency: None,
                rate: None,
            }),
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Convert symbol to market ID (BTC/USDT -> BTC_USDT)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_")
    }
}

#[async_trait]
impl Exchange for P2b {
    fn id(&self) -> ExchangeId {
        ExchangeId::P2b
    }

    fn name(&self) -> &str {
        "P2B"
    }

    fn version(&self) -> &str {
        "v2"
    }

    fn countries(&self) -> &[&str] {
        &["LT"] // Lithuania
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
        let data: Vec<P2bMarket> = self.public_get("/markets", None).await?;
        Ok(data.iter().map(|m| self.parse_market(m)).collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market".into(), market_id);

        let data: P2bTickerData = self.public_get("/ticker", Some(params)).await?;
        Ok(self.parse_ticker(&data, symbol, None))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: HashMap<String, P2bTickerWrapper> = self.public_get("/tickers", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for (market_id, wrapper) in response {
            if let Some(symbol) = markets_by_id.get(&market_id) {
                if let Some(filter) = symbols {
                    if !filter.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let timestamp = wrapper.at.map(|t| (t * 1000.0) as i64);
                let ticker = self.parse_ticker(&wrapper.ticker, symbol, timestamp);
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let data: P2bOrderBook = self.public_get("/depth/result", Some(params)).await?;

        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|b| {
                if b.len() >= 2 {
                    Some(OrderBookEntry {
                        price: b[0].parse().ok()?,
                        amount: b[1].parse().ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|a| {
                if a.len() >= 2 {
                    Some(OrderBookEntry {
                        price: a[0].parse().ok()?,
                        amount: a[1].parse().ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: None,
            datetime: None,
            nonce: None,
            bids,
            asks,
            checksum: None,
        })
    }

    async fn fetch_trades(
        &self,
        _symbol: &str,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        // P2B requires lastId parameter for fetching trades
        return Err(CcxtError::ArgumentsRequired {
            message: "P2B fetchTrades requires params['lastId'] parameter".into(),
        });
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.to_market_id(symbol);
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        params.insert("interval".into(), interval.clone());
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let data: Vec<Vec<serde_json::Value>> =
            self.public_get("/market/kline", Some(params)).await?;

        let ohlcv: Vec<OHLCV> = data
            .iter()
            .filter_map(|c| {
                if c.len() < 7 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].as_i64()? * 1000,
                    open: c[1].as_str()?.parse().ok()?,
                    high: c[3].as_str()?.parse().ok()?,
                    low: c[4].as_str()?.parse().ok()?,
                    close: c[2].as_str()?.parse().ok()?,
                    volume: c[5].as_str()?.parse().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let data: HashMap<String, P2bBalance> = self
            .private_post("account/balances", HashMap::new())
            .await?;

        let mut result = Balances::new();
        for (currency, balance) in data {
            let free: Option<Decimal> = balance.available.parse().ok();
            let used: Option<Decimal> = balance.freeze.parse().ok();
            let total = match (free, used) {
                (Some(f), Some(u)) => Some(f + u),
                _ => None,
            };

            result.add(
                &currency,
                Balance {
                    free,
                    used,
                    total,
                    debt: None,
                },
            );
        }

        Ok(result)
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        if order_type != OrderType::Limit {
            return Err(CcxtError::BadRequest {
                message: "P2B only supports limit orders".into(),
            });
        }

        let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Limit order requires price".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market".into(), serde_json::json!(market_id));
        params.insert(
            "side".into(),
            serde_json::json!(match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            }),
        );
        params.insert("amount".into(), serde_json::json!(amount.to_string()));
        params.insert("price".into(), serde_json::json!(price_val.to_string()));

        let data: P2bOrder = self.private_post("order/new", params).await?;
        Ok(self.parse_order(&data, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market".into(), serde_json::json!(market_id));
        params.insert("orderId".into(), serde_json::json!(id));

        let data: P2bOrder = self.private_post("order/cancel", params).await?;
        Ok(self.parse_order(&data, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "P2B fetchOpenOrders requires a symbol".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market".into(), serde_json::json!(market_id));
        if let Some(l) = limit {
            params.insert("limit".into(), serde_json::json!(l));
        }

        let data: Vec<P2bOrder> = self.private_post("orders", params).await?;
        Ok(data.iter().map(|o| self.parse_order(o, symbol)).collect())
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let until = Utc::now().timestamp_millis();
        let since = since.unwrap_or(until - 86400000); // Default to 24 hours ago

        if (until - since) > 86400000 {
            return Err(CcxtError::BadRequest {
                message: "P2B fetchClosedOrders time range cannot exceed 24 hours".into(),
            });
        }

        let mut params = HashMap::new();
        params.insert("startTime".into(), serde_json::json!(since / 1000));
        params.insert("endTime".into(), serde_json::json!(until / 1000));

        if let Some(sym) = symbol {
            let market_id = self.to_market_id(sym);
            params.insert("market".into(), serde_json::json!(market_id));
        }

        if let Some(l) = limit {
            params.insert("limit".into(), serde_json::json!(l));
        }

        let response: HashMap<String, Vec<P2bOrder>> =
            self.private_post("account/order_history", params).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut orders = Vec::new();

        for (market_id, market_orders) in response {
            let sym = markets_by_id
                .get(&market_id)
                .map(|s| s.as_str())
                .or(symbol)
                .unwrap_or(&market_id);

            for order in market_orders {
                orders.push(self.parse_order(&order, sym));
            }
        }

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "P2B fetchMyTrades requires a symbol".into(),
        })?;

        let until = Utc::now().timestamp_millis();
        let since = since.unwrap_or(until - 86400000);

        if (until - since) > 86400000 {
            return Err(CcxtError::BadRequest {
                message: "P2B fetchMyTrades time range cannot exceed 24 hours".into(),
            });
        }

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market".into(), serde_json::json!(market_id));
        params.insert("startTime".into(), serde_json::json!(since / 1000));
        params.insert("endTime".into(), serde_json::json!(until / 1000));
        if let Some(l) = limit {
            params.insert("limit".into(), serde_json::json!(l));
        }

        let response: P2bTradeHistory = self
            .private_post("account/market_deal_history", params)
            .await?;

        Ok(response
            .deals
            .iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect())
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        // Fetch open orders and find the matching one
        let orders = self.fetch_open_orders(Some(symbol), None, None).await?;
        orders
            .into_iter()
            .find(|o| o.id == id)
            .ok_or_else(|| CcxtError::OrderNotFound {
                order_id: id.to_string(),
            })
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
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let base_url = if api == "private" {
            Self::BASE_URL_PRIVATE
        } else {
            Self::BASE_URL_PUBLIC
        };

        let mut url = format!("{base_url}/{path}");
        let headers = HashMap::new();

        if api == "public" && !params.is_empty() {
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

// === P2B API Response Types ===

#[derive(Debug, Deserialize)]
struct P2bResponse<T> {
    success: bool,
    #[serde(rename = "errorCode")]
    error_code: Option<String>,
    message: Option<String>,
    result: Option<T>,
}

#[derive(Debug, Deserialize, Serialize)]
struct P2bMarket {
    name: String,
    stock: String,
    money: String,
    precision: P2bPrecision,
    limits: P2bLimits,
}

#[derive(Debug, Deserialize, Serialize)]
struct P2bPrecision {
    money: String,
    stock: String,
    fee: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct P2bLimits {
    min_amount: String,
    max_amount: String,
    step_size: String,
    min_price: String,
    max_price: String,
    tick_size: String,
    min_total: String,
}

#[derive(Debug, Deserialize)]
struct P2bTickerWrapper {
    at: Option<f64>,
    ticker: P2bTickerData,
}

#[derive(Debug, Deserialize, Serialize)]
struct P2bTickerData {
    bid: Option<String>,
    ask: Option<String>,
    low: Option<String>,
    high: Option<String>,
    last: Option<String>,
    #[serde(rename = "vol")]
    volume: Option<String>,
    deal: Option<String>,
    change: Option<String>,
    open: Option<String>,
}

#[derive(Debug, Deserialize)]
struct P2bOrderBook {
    asks: Vec<Vec<String>>,
    bids: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct P2bTrade {
    id: Option<String>,
    #[serde(rename = "type")]
    type_field: Option<String>,
    time: Option<f64>,
    amount: String,
    price: String,
    side: Option<String>,
    deal: Option<String>,
    fee: Option<String>,
    role: Option<String>,
    deal_id: Option<String>,
    deal_time: Option<f64>,
    deal_order_id: Option<String>,
    deal_order_id_num: Option<i64>,
    deal_fee: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct P2bOrder {
    #[serde(rename = "orderId")]
    order_id: Option<String>,
    id: Option<String>,
    market: Option<String>,
    price: String,
    side: Option<String>,
    #[serde(rename = "type")]
    type_field: Option<String>,
    timestamp: Option<f64>,
    ctime: Option<f64>,
    ftime: Option<f64>,
    #[serde(rename = "dealMoney")]
    deal_money: Option<String>,
    #[serde(rename = "dealStock")]
    deal_stock: Option<String>,
    amount: String,
    #[serde(rename = "takerFee")]
    taker_fee: Option<String>,
    #[serde(rename = "makerFee")]
    maker_fee: Option<String>,
    left: Option<String>,
    #[serde(rename = "dealFee")]
    deal_fee: Option<String>,
}

#[derive(Debug, Deserialize)]
struct P2bBalance {
    available: String,
    freeze: String,
}

#[derive(Debug, Deserialize)]
struct P2bTradeHistory {
    total: Option<i64>,
    deals: Vec<P2bTrade>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let p2b = P2b::new(config).unwrap();

        assert_eq!(p2b.to_market_id("BTC/USDT"), "BTC_USDT");
        assert_eq!(p2b.to_market_id("ETH/BTC"), "ETH_BTC");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let p2b = P2b::new(config).unwrap();

        assert_eq!(p2b.id(), ExchangeId::P2b);
        assert_eq!(p2b.name(), "P2B");
        assert!(p2b.has().spot);
        assert!(!p2b.has().margin);
        assert!(!p2b.has().create_market_order);
    }
}
