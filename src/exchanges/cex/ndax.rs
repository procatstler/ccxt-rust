//! NDAX Exchange Implementation
//!
//! CCXT ndax.ts를 Rust로 포팅

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::Hmac;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, TimeInForce, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// NDAX 거래소
pub struct Ndax {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    oms_id: i32,
    session_token: RwLock<Option<String>>,
}

impl Ndax {
    const BASE_URL: &'static str = "https://api.ndax.io:8443/AP";
    const TEST_URL: &'static str = "https://ndaxmarginstaging.cdnhop.net:8443/AP";
    const RATE_LIMIT_MS: u64 = 1000;

    /// 새 NDAX 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let base_url = if config.is_sandbox() {
            Self::TEST_URL
        } else {
            Self::BASE_URL
        };

        let public_client = HttpClient::new(base_url, &config)?;
        let private_client = HttpClient::new(base_url, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            fetch_markets: true,
            fetch_currencies: true,
            fetch_ticker: true,
            fetch_tickers: false,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            create_stop_limit_order: true,
            create_stop_market_order: true,
            cancel_order: true,
            cancel_all_orders: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: false,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            fetch_deposit_address: true,
            fetch_ledger: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), base_url.into());
        api_urls.insert("private".into(), base_url.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/108623144-67a3ef00-744e-11eb-8140-75c6b851e945.jpg".into()),
            api: api_urls,
            www: Some("https://ndax.io".into()),
            doc: vec!["https://apidoc.ndax.io/".into()],
            fees: Some("https://ndax.io/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "60".into());
        timeframes.insert(Timeframe::Minute5, "300".into());
        timeframes.insert(Timeframe::Minute15, "900".into());
        timeframes.insert(Timeframe::Minute30, "1800".into());
        timeframes.insert(Timeframe::Hour1, "3600".into());
        timeframes.insert(Timeframe::Hour2, "7200".into());
        timeframes.insert(Timeframe::Hour4, "14400".into());
        timeframes.insert(Timeframe::Hour6, "21600".into());
        timeframes.insert(Timeframe::Hour12, "43200".into());
        timeframes.insert(Timeframe::Day1, "86400".into());
        timeframes.insert(Timeframe::Week1, "604800".into());
        timeframes.insert(Timeframe::Month1, "2419200".into());

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
            oms_id: 1,
            session_token: RwLock::new(None),
        })
    }

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let url = if let Some(p) = params {
            let query: String = p
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{endpoint}?{query}")
        } else {
            endpoint.to_string()
        };

        self.public_client.get(&url, None, None).await
    }

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        endpoint: &str,
        params: HashMap<String, Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let session_token = self
            .session_token
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Session token required. Call sign_in first.".into(),
            })?;

        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/json".into());
        headers.insert("Authorization".into(), format!("Bearer {session_token}"));

        let body = serde_json::to_value(&params).unwrap_or(Value::Object(serde_json::Map::new()));

        match method {
            "GET" => {
                let query: String = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v.to_string().trim_matches('"')))
                    .collect::<Vec<_>>()
                    .join("&");
                let url = if query.is_empty() {
                    endpoint.to_string()
                } else {
                    format!("{endpoint}?{query}")
                };
                self.private_client.get(&url, None, Some(headers)).await
            }
            "POST" => {
                self.private_client
                    .post(endpoint, Some(body), Some(headers))
                    .await
            }
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 마켓 정보 파싱
    fn parse_market(&self, market_data: &NdaxMarket) -> Market {
        let id = market_data.instrument_id.to_string();
        let base_id = market_data.product1.to_string();
        let quote_id = market_data.product2.to_string();
        let base = market_data.product1_symbol.clone();
        let quote = market_data.product2_symbol.clone();
        let symbol = format!("{base}/{quote}");

        let active = market_data.session_status == "Running" && !market_data.is_disable;

        Market {
            id: id.clone(),
            lowercase_id: Some(id.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: base_id.clone(),
            quote_id: quote_id.clone(),
            settle: None,
            settle_id: None,
            active,
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
            taker: Some(Decimal::new(25, 4)), // 0.0025 = 0.25%
            maker: Some(Decimal::new(2, 3)),  // 0.002 = 0.2%
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            precision: MarketPrecision {
                amount: market_data.quantity_increment.map(|d| d.to_string().len() as i32 - 2),
                price: market_data.price_increment.map(|d| d.to_string().len() as i32 - 2),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: MinMax {
                    min: market_data.minimum_quantity,
                    max: None,
                },
                price: MinMax {
                    min: market_data.minimum_price,
                    max: None,
                },
                cost: MinMax::default(),
                leverage: MinMax::default(),
            },
            margin_modes: None,
            created: None,
            info: serde_json::to_value(market_data).unwrap_or_default(),
            tier_based: false,
            percentage: true,
        }
    }

    /// 티커 파싱
    fn parse_ticker(&self, data: &NdaxTicker, symbol: &str) -> Ticker {
        let timestamp = data.time_stamp;

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.session_high,
            low: data.session_low,
            bid: data.best_bid,
            bid_volume: None,
            ask: data.best_offer,
            ask_volume: None,
            vwap: None,
            open: data.session_open,
            close: data.last_traded_px,
            last: data.last_traded_px,
            previous_close: None,
            change: data.rolling24hr_px_change,
            percentage: data.rolling24hr_px_change_percent,
            average: None,
            base_volume: data.rolling24hr_volume,
            quote_volume: data.rolling24hr_notional,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 파싱
    fn parse_order(&self, data: &NdaxOrder, symbol: Option<&str>) -> Order {
        let status = match data.status.as_str() {
            "Accepted" | "Working" => OrderStatus::Open,
            "FullyExecuted" => OrderStatus::Closed,
            "Canceled" | "Cancelled" => OrderStatus::Canceled,
            "Rejected" => OrderStatus::Rejected,
            "Expired" => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let order_id = data.order_id.map(|id| id.to_string()).unwrap_or_default();
        let symbol_str = symbol.map(String::from).unwrap_or_default();

        Order {
            id: order_id.clone(),
            client_order_id: data.client_order_id.map(|id| id.to_string()),
            timestamp: None,
            datetime: None,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol_str,
            order_type: OrderType::Limit,
            time_in_force: Some(TimeInForce::GTC),
            side: OrderSide::Buy,
            price: None,
            average: None,
            amount: Decimal::ZERO,
            filled: Decimal::ZERO,
            remaining: None,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 거래 파싱
    fn parse_trade(&self, data: &NdaxTrade, symbol: &str) -> Trade {
        let timestamp = data.time_stamp;
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.quantity.parse().unwrap_or_default();
        let cost = price * amount;

        Trade {
            id: data.trade_id.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: if data.side == 0 {
                Some("buy".into())
            } else {
                Some("sell".into())
            },
            taker_or_maker: None,
            price,
            amount,
            cost: Some(cost),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 파싱
    fn parse_balance(&self, balances: &[NdaxBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free = b.amount;
            let total = b.total;
            let used = total.map(|t| t - free.unwrap_or(Decimal::ZERO));

            let balance = Balance {
                free,
                used,
                total,
                debt: None,
            };

            if let Some(currency) = &b.product_symbol {
                result.add(currency, balance);
            }
        }

        result
    }
}

#[async_trait]
impl Exchange for Ndax {
    fn id(&self) -> ExchangeId {
        ExchangeId::Ndax
    }

    fn name(&self) -> &str {
        "NDAX"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["CA"]
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
        let mut params = HashMap::new();
        params.insert("omsId".into(), self.oms_id.to_string());

        let response: Vec<NdaxMarket> = self
            .public_get("/GetInstruments", Some(params))
            .await?;

        let markets: Vec<Market> = response.iter().map(|m| self.parse_market(m)).collect();

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets
                .get(symbol)
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("omsId".into(), self.oms_id.to_string());
        params.insert("InstrumentId".into(), market_id);

        let response: NdaxTicker = self.public_get("/GetLevel1", Some(params)).await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets
                .get(symbol)
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("omsId".into(), self.oms_id.to_string());
        params.insert("InstrumentId".into(), market_id);
        params.insert("Depth".into(), limit.unwrap_or(100).to_string());

        let response: Vec<Vec<Value>> = self.public_get("/GetL2Snapshot", Some(params)).await?;

        let mut bids = Vec::new();
        let mut asks = Vec::new();
        let mut timestamp: Option<i64> = None;
        let mut nonce: Option<i64> = None;

        for level in response {
            if level.len() < 10 {
                continue;
            }

            // Update timestamp and nonce
            if let Some(ts) = level.get(2).and_then(|v| v.as_i64()) {
                timestamp = Some(timestamp.map_or(ts, |t| t.max(ts)));
            }
            if let Some(n) = level.first().and_then(|v| v.as_i64()) {
                nonce = Some(nonce.map_or(n, |existing| existing.max(n)));
            }

            // Parse price and amount
            let price: Decimal = level
                .get(6)
                .and_then(|v| v.as_f64())
                .and_then(|p| Decimal::try_from(p).ok())
                .unwrap_or_default();

            let amount: Decimal = level
                .get(8)
                .and_then(|v| v.as_f64())
                .and_then(|a| Decimal::try_from(a).ok())
                .unwrap_or_default();

            // Side: 0 = bid, 1 = ask
            let side = level.get(9).and_then(|v| v.as_i64()).unwrap_or(0);

            let entry = OrderBookEntry { price, amount };

            if side == 0 {
                bids.push(entry);
            } else {
                asks.push(entry);
            }
        }

        // Sort bids descending, asks ascending
        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            nonce,
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
            let market = markets
                .get(symbol)
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("omsId".into(), self.oms_id.to_string());
        params.insert("InstrumentId".into(), market_id);
        params.insert("Count".into(), limit.unwrap_or(100).to_string());

        let response: Vec<NdaxTrade> = self.public_get("/GetLastTrades", Some(params)).await?;

        let trades: Vec<Trade> = response.iter().map(|t| self.parse_trade(t, symbol)).collect();

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
            let market = markets
                .get(symbol)
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?;
            market.id.clone()
        };

        let interval = self.timeframes.get(&timeframe).ok_or_else(|| {
            CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            }
        })?;

        let mut params = HashMap::new();
        params.insert("omsId".into(), self.oms_id.to_string());
        params.insert("InstrumentId".into(), market_id);
        params.insert("Interval".into(), interval.clone());

        // Calculate time range
        let now = Utc::now().timestamp_millis();
        let duration_secs: i64 = interval.parse().unwrap_or(60);
        let duration_ms = duration_secs * 1000;

        if let Some(s) = since {
            let from_date = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S").to_string())
                .unwrap_or_default();
            params.insert("FromDate".into(), from_date);

            if let Some(l) = limit {
                let to_ts = s + (l as i64 * duration_ms);
                let to_date = chrono::DateTime::from_timestamp_millis(to_ts)
                    .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S").to_string())
                    .unwrap_or_default();
                params.insert("ToDate".into(), to_date);
            } else {
                let to_date = chrono::DateTime::from_timestamp_millis(now)
                    .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S").to_string())
                    .unwrap_or_default();
                params.insert("ToDate".into(), to_date);
            }
        } else if let Some(l) = limit {
            let from_ts = now - (l as i64 * duration_ms);
            let from_date = chrono::DateTime::from_timestamp_millis(from_ts)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S").to_string())
                .unwrap_or_default();
            params.insert("FromDate".into(), from_date);

            let to_date = chrono::DateTime::from_timestamp_millis(now)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S").to_string())
                .unwrap_or_default();
            params.insert("ToDate".into(), to_date);
        }

        let response: Vec<Vec<Value>> = self
            .public_get("/GetTickerHistory", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|candle| {
                if candle.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: candle[0].as_i64()?,
                    open: candle[3].as_f64().and_then(|v| Decimal::try_from(v).ok())?,
                    high: candle[1].as_f64().and_then(|v| Decimal::try_from(v).ok())?,
                    low: candle[2].as_f64().and_then(|v| Decimal::try_from(v).ok())?,
                    close: candle[4].as_f64().and_then(|v| Decimal::try_from(v).ok())?,
                    volume: candle[5].as_f64().and_then(|v| Decimal::try_from(v).ok())?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let mut params = HashMap::new();
        params.insert("omsId".into(), Value::from(self.oms_id));
        params.insert("AccountId".into(), Value::from(0)); // Will be set from account

        let response: Vec<NdaxBalance> = self
            .private_request("GET", "/GetAccountPositions", params)
            .await?;

        Ok(self.parse_balance(&response))
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
            let market = markets
                .get(symbol)
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?;
            market.id.clone()
        };

        let order_type_id = match order_type {
            OrderType::Market => 1,
            OrderType::Limit => 2,
            OrderType::StopLoss => 3,
            OrderType::StopLossLimit => 4,
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type: {order_type:?}"),
                })
            }
        };

        let side_id = match side {
            OrderSide::Buy => 0,
            OrderSide::Sell => 1,
        };

        let mut params = HashMap::new();
        params.insert("omsId".into(), Value::from(self.oms_id));
        params.insert("InstrumentId".into(), Value::from(market_id.parse::<i32>().unwrap_or(0)));
        params.insert("AccountId".into(), Value::from(0)); // Will be set from account
        params.insert("TimeInForce".into(), Value::from(1)); // GTC
        params.insert("Side".into(), Value::from(side_id));
        params.insert("Quantity".into(), Value::from(amount.to_string()));
        params.insert("OrderType".into(), Value::from(order_type_id));

        if let Some(p) = price {
            params.insert("LimitPrice".into(), Value::from(p.to_string()));
        }

        let response: NdaxOrder = self.private_request("POST", "/SendOrder", params).await?;

        Ok(self.parse_order(&response, Some(symbol)))
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("omsId".into(), Value::from(self.oms_id));
        params.insert("OrderId".into(), Value::from(id.parse::<i64>().unwrap_or(0)));
        params.insert("AccountId".into(), Value::from(0)); // Will be set from account

        let response: NdaxOrder = self.private_request("POST", "/CancelOrder", params).await?;

        Ok(self.parse_order(&response, None))
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("omsId".into(), Value::from(self.oms_id));
        params.insert("OrderId".into(), Value::from(id.parse::<i64>().unwrap_or(0)));

        let response: NdaxOrder = self
            .private_request("GET", "/GetOrderStatus", params)
            .await?;

        Ok(self.parse_order(&response, None))
    }

    async fn fetch_open_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("omsId".into(), Value::from(self.oms_id));
        params.insert("AccountId".into(), Value::from(0)); // Will be set from account

        let response: Vec<NdaxOrder> = self
            .private_request("GET", "/GetOpenOrders", params)
            .await?;

        let orders: Vec<Order> = response.iter().map(|o| self.parse_order(o, None)).collect();

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
        _api: &str,
        method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{}{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();

        if let Some(token) = self.session_token.read().unwrap().as_ref() {
            headers.insert("Authorization".into(), format!("Bearer {token}"));
        }

        if method == "POST" {
            headers.insert("Content-Type".into(), "application/json".into());
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

    async fn fetch_my_trades(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();
        params.insert("omsId".into(), Value::from(self.oms_id));
        params.insert("AccountId".into(), Value::from(0)); // Will be set from account

        let response: Vec<NdaxTrade> = self
            .private_request("GET", "/GetTradesHistory", params)
            .await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| self.parse_trade(t, ""))
            .collect();

        Ok(trades)
    }

    async fn fetch_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("omsId".into(), Value::from(self.oms_id));
        params.insert("AccountId".into(), Value::from(0)); // Will be set from account

        let response: Vec<NdaxOrder> = self
            .private_request("GET", "/GetOrdersHistory", params)
            .await?;

        let orders: Vec<Order> = response.iter().map(|o| self.parse_order(o, None)).collect();

        Ok(orders)
    }
}

// === NDAX API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct NdaxMarket {
    #[serde(rename = "OMSId")]
    oms_id: i32,
    instrument_id: i32,
    symbol: String,
    product1: i32,
    product1_symbol: String,
    product2: i32,
    product2_symbol: String,
    session_status: String,
    #[serde(default)]
    is_disable: bool,
    #[serde(default)]
    quantity_increment: Option<Decimal>,
    #[serde(default)]
    price_increment: Option<Decimal>,
    #[serde(default)]
    minimum_quantity: Option<Decimal>,
    #[serde(default)]
    minimum_price: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct NdaxTicker {
    #[serde(rename = "OMSId")]
    oms_id: i32,
    instrument_id: i32,
    #[serde(default)]
    best_bid: Option<Decimal>,
    #[serde(default)]
    best_offer: Option<Decimal>,
    #[serde(default)]
    last_traded_px: Option<Decimal>,
    #[serde(default)]
    session_open: Option<Decimal>,
    #[serde(default)]
    session_high: Option<Decimal>,
    #[serde(default)]
    session_low: Option<Decimal>,
    #[serde(default)]
    rolling24hr_volume: Option<Decimal>,
    #[serde(default)]
    rolling24hr_notional: Option<Decimal>,
    #[serde(default)]
    rolling24hr_px_change: Option<Decimal>,
    #[serde(default)]
    rolling24hr_px_change_percent: Option<Decimal>,
    #[serde(rename = "TimeStamp")]
    time_stamp: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct NdaxOrder {
    #[serde(default)]
    status: String,
    #[serde(rename = "OrderId")]
    #[serde(default)]
    order_id: Option<i64>,
    #[serde(rename = "ClientOrderId")]
    #[serde(default)]
    client_order_id: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct NdaxTrade {
    trade_id: i64,
    price: String,
    quantity: String,
    side: i32,
    #[serde(rename = "TimeStamp")]
    time_stamp: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct NdaxBalance {
    #[serde(default)]
    product_symbol: Option<String>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    total: Option<Decimal>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let ndax = Ndax::new(config).unwrap();

        assert_eq!(ndax.id(), ExchangeId::Ndax);
        assert_eq!(ndax.name(), "NDAX");
        assert!(ndax.has().spot);
        assert!(!ndax.has().margin);
        assert!(!ndax.has().swap);
    }
}
