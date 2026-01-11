//! Luno Exchange Implementation
//!
//! CCXT luno.ts ported to Rust

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade, OHLCV,
};

/// Luno Exchange
pub struct Luno {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, i32>,
}

impl Luno {
    const BASE_URL: &'static str = "https://api.luno.com/api";
    const RATE_LIMIT_MS: u64 = 200; // 300 calls per minute = 5 calls per second

    /// Create new Luno instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            fetch_markets: true,
            fetch_currencies: false, // Requires authentication
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
            fetch_closed_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), "https://api.luno.com/api".into());
        api_urls.insert("private".into(), "https://api.luno.com/api".into());
        api_urls.insert(
            "exchange".into(),
            "https://api.luno.com/api/exchange".into(),
        );

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27766607-8c1a69d8-5ede-11e7-930c-540b5eb9be24.jpg".into()),
            api: api_urls,
            www: Some("https://www.luno.com".into()),
            doc: vec![
                "https://www.luno.com/en/api".into(),
                "https://npmjs.org/package/bitx".into(),
                "https://github.com/bausmeier/node-bitx".into(),
            ],
            fees: None,
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, 60);
        timeframes.insert(Timeframe::Minute5, 300);
        timeframes.insert(Timeframe::Minute15, 900);
        timeframes.insert(Timeframe::Minute30, 1800);
        timeframes.insert(Timeframe::Hour1, 3600);
        timeframes.insert(Timeframe::Hour3, 10800);
        timeframes.insert(Timeframe::Hour4, 14400);
        timeframes.insert(Timeframe::Day1, 86400);
        timeframes.insert(Timeframe::Day3, 259200);
        timeframes.insert(Timeframe::Week1, 604800);

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
            format!("{path}?{query}")
        } else {
            path.to_string()
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

        // Basic authentication
        let auth = BASE64.encode(format!("{api_key}:{secret}"));
        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), format!("Basic {auth}"));

        match method {
            "GET" => {
                let query: String = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");

                let url = if query.is_empty() {
                    path.to_string()
                } else {
                    format!("{path}?{query}")
                };

                self.private_client.get(&url, None, Some(headers)).await
            },
            "POST" => {
                // Use post_form for form-urlencoded data
                self.private_client
                    .post_form(path, &params, Some(headers))
                    .await
            },
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// Convert symbol to market ID (BTC/ZAR → XBTZAR)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// Parse ticker response
    fn parse_ticker(&self, data: &LunoTicker, symbol: &str) -> Ticker {
        let timestamp = data.timestamp;

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: None,
            low: None,
            bid: data.bid.parse().ok(),
            bid_volume: None,
            ask: data.ask.parse().ok(),
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last_trade.parse().ok(),
            last: data.last_trade.parse().ok(),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.rolling_24_hour_volume.parse().ok(),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order response
    fn parse_order(&self, data: &LunoOrder, symbol: &str) -> Order {
        let status = match data.state.as_str() {
            "PENDING" => OrderStatus::Open,
            "COMPLETE" => OrderStatus::Closed,
            _ => OrderStatus::Open,
        };

        let side = match data.order_type.as_str() {
            "ASK" | "SELL" => OrderSide::Sell,
            "BID" | "BUY" => OrderSide::Buy,
            _ => OrderSide::Buy,
        };

        let price: Option<Decimal> = data.limit_price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data
            .limit_volume
            .as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data
            .base
            .as_ref()
            .and_then(|b| b.parse().ok())
            .unwrap_or_default();
        let remaining = Some(amount - filled);
        let cost = data.counter.as_ref().and_then(|c| c.parse().ok());

        let fee = if let Some(fee_counter) = &data.fee_counter {
            fee_counter
                .parse::<Decimal>()
                .ok()
                .map(|cost| crate::types::Fee {
                    cost: Some(cost),
                    currency: None, // Would be quote currency
                    rate: None,
                })
        } else if let Some(fee_base) = &data.fee_base {
            fee_base
                .parse::<Decimal>()
                .ok()
                .map(|cost| crate::types::Fee {
                    cost: Some(cost),
                    currency: None, // Would be base currency
                    rate: None,
                })
        } else {
            None
        };

        Order {
            id: data.order_id.clone(),
            client_order_id: None,
            timestamp: Some(data.creation_timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.creation_timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: data.completed_timestamp,
            last_update_timestamp: data.completed_timestamp,
            status,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled,
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost,
            trades: Vec::new(),
            fee,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance response
    fn parse_balance(&self, balances: &[LunoBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let balance_val: Decimal = b.balance.parse().unwrap_or_default();
            let reserved_val: Decimal = b.reserved.parse().unwrap_or_default();
            let unconfirmed_val: Decimal = b.unconfirmed.parse().unwrap_or_default();

            let used = reserved_val + unconfirmed_val;
            let total = balance_val + unconfirmed_val;
            let free = total - used;

            let balance = Balance {
                free: Some(free),
                used: Some(used),
                total: Some(total),
                debt: None,
            };
            result.add(&b.asset, balance);
        }

        result
    }

    /// Parse trade response
    fn parse_trade(&self, data: &LunoTrade, symbol: &str) -> Trade {
        let timestamp = data.timestamp;

        // For public trades
        let side = if let Some(is_buy) = data.is_buy {
            if is_buy {
                Some("buy".into())
            } else {
                Some("sell".into())
            }
        } else {
            None
        };

        // For private trades
        let (side_private, taker_or_maker) = if let Some(order_type) = &data.order_type {
            let s = match order_type.as_str() {
                "ASK" | "SELL" => Some("sell".into()),
                "BID" | "BUY" => Some("buy".into()),
                _ => None,
            };

            let tom = if let Some(is_buy) = data.is_buy {
                if (s == Some("sell".into()) && is_buy) || (s == Some("buy".into()) && !is_buy) {
                    Some(TakerOrMaker::Maker)
                } else {
                    Some(TakerOrMaker::Taker)
                }
            } else {
                None
            };

            (s, tom)
        } else {
            (side, None)
        };

        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data
            .volume
            .as_ref()
            .and_then(|v| v.parse().ok())
            .or_else(|| data.base.as_ref().and_then(|b| b.parse().ok()))
            .unwrap_or_default();

        let fee = if let Some(fee_base) = &data.fee_base {
            fee_base
                .parse::<Decimal>()
                .ok()
                .filter(|&f| f > Decimal::ZERO)
                .map(|cost| crate::types::Fee {
                    cost: Some(cost),
                    currency: None,
                    rate: None,
                })
        } else if let Some(fee_counter) = &data.fee_counter {
            fee_counter
                .parse::<Decimal>()
                .ok()
                .filter(|&f| f > Decimal::ZERO)
                .map(|cost| crate::types::Fee {
                    cost: Some(cost),
                    currency: None,
                    rate: None,
                })
        } else {
            None
        };

        Trade {
            id: data.sequence.map(|s| s.to_string()).unwrap_or_default(),
            order: data.order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: side_private,
            taker_or_maker,
            price,
            amount,
            cost: data.counter.as_ref().and_then(|c| c.parse().ok()),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Luno {
    fn id(&self) -> ExchangeId {
        ExchangeId::Luno
    }

    fn name(&self) -> &str {
        "Luno"
    }

    fn version(&self) -> &str {
        "1"
    }

    fn countries(&self) -> &[&str] {
        &["GB", "SG", "ZA"]
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
        // Convert i32 to String
        static TIMEFRAMES_STR: std::sync::OnceLock<HashMap<Timeframe, String>> =
            std::sync::OnceLock::new();
        TIMEFRAMES_STR.get_or_init(|| {
            let mut map = HashMap::new();
            map.insert(Timeframe::Minute1, "60".to_string());
            map.insert(Timeframe::Minute5, "300".to_string());
            map.insert(Timeframe::Minute15, "900".to_string());
            map.insert(Timeframe::Minute30, "1800".to_string());
            map.insert(Timeframe::Hour1, "3600".to_string());
            map.insert(Timeframe::Hour3, "10800".to_string());
            map.insert(Timeframe::Hour4, "14400".to_string());
            map.insert(Timeframe::Day1, "86400".to_string());
            map.insert(Timeframe::Day3, "259200".to_string());
            map.insert(Timeframe::Week1, "604800".to_string());
            map
        })
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
        let response: LunoMarketsResponse = self
            .public_get("https://api.luno.com/api/exchange/1/markets", None)
            .await?;

        let mut markets = Vec::new();

        for market_info in response.markets {
            if market_info.trading_status != "ACTIVE" {
                continue;
            }

            let base = market_info.base_currency.clone();
            let quote = market_info.counter_currency.clone();
            let symbol = format!("{base}/{quote}");

            let _precision_amount = 10_i32.pow(market_info.volume_scale as u32);
            let _precision_price = 10_i32.pow(market_info.price_scale as u32);

            let market = Market {
                id: market_info.market_id.clone(),
                lowercase_id: Some(market_info.market_id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: market_info.base_currency.clone(),
                quote_id: market_info.counter_currency.clone(),
                settle: None,
                settle_id: None,
                active: market_info.trading_status == "ACTIVE",
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
                taker: Some(Decimal::new(1, 3)), // 0.1%
                maker: Some(Decimal::ZERO),
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(market_info.volume_scale),
                    price: Some(market_info.price_scale),
                    cost: None,
                    base: Some(market_info.volume_scale),
                    quote: Some(market_info.price_scale),
                },
                limits: MarketLimits {
                    leverage: MinMax {
                        min: None,
                        max: None,
                    },
                    amount: MinMax {
                        min: market_info.min_volume.parse().ok(),
                        max: market_info.max_volume.parse().ok(),
                    },
                    price: MinMax {
                        min: market_info.min_price.parse().ok(),
                        max: market_info.max_price.parse().ok(),
                    },
                    cost: MinMax {
                        min: None,
                        max: None,
                    },
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&market_info).unwrap_or_default(),
                tier_based: true,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);

        let response: LunoTicker = self.public_get("/1/ticker", Some(params)).await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: LunoTickersResponse = self.public_get("/1/tickers", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for data in response.tickers {
            if let Some(symbol) = markets_by_id.get(&data.pair) {
                // Filter by symbols if specified
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
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);

        let endpoint = if limit.is_some() && limit.unwrap() <= 100 {
            "/1/orderbook_top"
        } else {
            "/1/orderbook"
        };

        let response: LunoOrderBook = self.public_get(endpoint, Some(params)).await?;

        let bids: Vec<OrderBookEntry> = response
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: b.price.parse().unwrap_or_default(),
                amount: b.volume.parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: a.price.parse().unwrap_or_default(),
                amount: a.volume.parse().unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(response.timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(response.timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
            bids,
            asks,
            checksum: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);

        if let Some(s) = since {
            params.insert("since".into(), s.to_string());
        }

        let response: LunoTradesResponse = self.public_get("/1/trades", Some(params)).await?;

        let trades: Vec<Trade> = response
            .trades
            .iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect();

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.to_market_id(symbol);
        let duration = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);
        params.insert("duration".into(), duration.to_string());

        let since_val = if let Some(s) = since {
            s
        } else {
            let duration_ms = (*duration as i64) * 1000;
            Utc::now().timestamp_millis() - duration_ms
        };
        params.insert("since".into(), since_val.to_string());

        let response: LunoOHLCVResponse = self
            .public_get("https://api.luno.com/api/exchange/1/candles", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .candles
            .iter()
            .map(|c| OHLCV {
                timestamp: c.timestamp,
                open: c.open.parse().unwrap_or_default(),
                high: c.high.parse().unwrap_or_default(),
                low: c.low.parse().unwrap_or_default(),
                close: c.close.parse().unwrap_or_default(),
                volume: c.volume.parse().unwrap_or_default(),
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: LunoBalanceResponse =
            self.private_request("GET", "/1/balance", params).await?;

        Ok(self.parse_balance(&response.balance))
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);

        let response: LunoOrder = if order_type == OrderType::Market {
            params.insert(
                "type".into(),
                match side {
                    OrderSide::Buy => "BUY",
                    OrderSide::Sell => "SELL",
                }
                .into(),
            );

            if side == OrderSide::Buy {
                params.insert("counter_volume".into(), amount.to_string());
            } else {
                params.insert("base_volume".into(), amount.to_string());
            }

            self.private_request("POST", "/1/marketorder", params)
                .await?
        } else {
            params.insert("volume".into(), amount.to_string());
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
            params.insert(
                "type".into(),
                match side {
                    OrderSide::Buy => "BID",
                    OrderSide::Sell => "ASK",
                }
                .into(),
            );

            self.private_request("POST", "/1/postorder", params).await?
        };

        Ok(self.parse_order(&response, symbol))
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());

        let response: LunoStopOrderResponse =
            self.private_request("POST", "/1/stoporder", params).await?;

        Ok(Order {
            id: id.to_string(),
            info: serde_json::to_value(&response).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("id".into(), id.to_string());

        let response: LunoOrder = self
            .private_request("GET", &format!("/1/orders/{id}"), params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let symbol = markets_by_id
            .get(&response.pair)
            .cloned()
            .unwrap_or_else(|| response.pair.clone());

        Ok(self.parse_order(&response, &symbol))
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("pair".into(), self.to_market_id(s));
        }

        let response: LunoOrdersResponse =
            self.private_request("GET", "/1/listorders", params).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = response
            .orders
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.pair)
                    .cloned()
                    .unwrap_or_else(|| o.pair.clone());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("state".into(), "PENDING".to_string());

        if let Some(s) = symbol {
            params.insert("pair".into(), self.to_market_id(s));
        }

        let response: LunoOrdersResponse =
            self.private_request("GET", "/1/listorders", params).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = response
            .orders
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.pair)
                    .cloned()
                    .unwrap_or_else(|| o.pair.clone());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("state".into(), "COMPLETE".to_string());

        if let Some(s) = symbol {
            params.insert("pair".into(), self.to_market_id(s));
        }

        let response: LunoOrdersResponse =
            self.private_request("GET", "/1/listorders", params).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = response
            .orders
            .iter()
            .filter(|o| o.state == "COMPLETE")
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.pair)
                    .cloned()
                    .unwrap_or_else(|| o.pair.clone());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Luno fetchMyTrades requires a symbol".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);

        if let Some(s) = since {
            params.insert("since".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: LunoTradesResponse =
            self.private_request("GET", "/1/listtrades", params).await?;

        let trades: Vec<Trade> = response
            .trades
            .iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect();

        Ok(trades)
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        Some(self.to_market_id(symbol))
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
        let mut url = format!("{}{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();

        if api == "private" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();

            let auth = BASE64.encode(format!("{api_key}:{secret}"));
            headers.insert("Authorization".into(), format!("Basic {auth}"));
        }

        if !params.is_empty() {
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

// === Luno API Response Types ===

#[derive(Debug, Deserialize)]
struct LunoMarketsResponse {
    markets: Vec<LunoMarketInfo>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LunoMarketInfo {
    market_id: String,
    trading_status: String,
    base_currency: String,
    counter_currency: String,
    min_volume: String,
    max_volume: String,
    volume_scale: i32,
    min_price: String,
    max_price: String,
    price_scale: i32,
    fee_scale: i32,
}

#[derive(Debug, Deserialize, Serialize)]
struct LunoTicker {
    pair: String,
    timestamp: i64,
    bid: String,
    ask: String,
    last_trade: String,
    rolling_24_hour_volume: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct LunoTickersResponse {
    tickers: Vec<LunoTicker>,
}

#[derive(Debug, Deserialize)]
struct LunoOrderBook {
    timestamp: i64,
    bids: Vec<LunoOrderBookLevel>,
    asks: Vec<LunoOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
struct LunoOrderBookLevel {
    price: String,
    volume: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct LunoTrade {
    #[serde(default)]
    sequence: Option<i64>,
    timestamp: i64,
    price: String,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    is_buy: Option<bool>,
    // Private trade fields
    #[serde(default)]
    pair: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    #[serde(rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    counter: Option<String>,
    #[serde(default)]
    fee_base: Option<String>,
    #[serde(default)]
    fee_counter: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LunoTradesResponse {
    trades: Vec<LunoTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LunoOrder {
    order_id: String,
    creation_timestamp: i64,
    #[serde(default)]
    completed_timestamp: Option<i64>,
    #[serde(default)]
    expiration_timestamp: Option<i64>,
    #[serde(rename = "type")]
    order_type: String,
    state: String,
    #[serde(default)]
    limit_price: Option<String>,
    #[serde(default)]
    limit_volume: Option<String>,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    counter: Option<String>,
    #[serde(default)]
    fee_base: Option<String>,
    #[serde(default)]
    fee_counter: Option<String>,
    pair: String,
}

#[derive(Debug, Deserialize)]
struct LunoOrdersResponse {
    orders: Vec<LunoOrder>,
}

#[derive(Debug, Deserialize)]
struct LunoBalanceResponse {
    balance: Vec<LunoBalance>,
}

#[derive(Debug, Deserialize)]
struct LunoBalance {
    asset: String,
    balance: String,
    reserved: String,
    unconfirmed: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct LunoStopOrderResponse {
    success: bool,
}

#[derive(Debug, Deserialize)]
struct LunoOHLCVResponse {
    candles: Vec<LunoCandle>,
    duration: i32,
    pair: String,
}

#[derive(Debug, Deserialize)]
struct LunoCandle {
    timestamp: i64,
    open: String,
    close: String,
    high: String,
    low: String,
    volume: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let luno = Luno::new(config).unwrap();

        assert_eq!(luno.to_market_id("BTC/ZAR"), "BTCZAR");
        assert_eq!(luno.to_market_id("XBT/ZAR"), "XBTZAR");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let luno = Luno::new(config).unwrap();

        assert_eq!(luno.id(), ExchangeId::Luno);
        assert_eq!(luno.name(), "Luno");
        assert!(luno.has().spot);
        assert!(!luno.has().margin);
        assert!(!luno.has().swap);
    }
}
