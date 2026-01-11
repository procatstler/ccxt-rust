//! One Trading Exchange Implementation
//!
//! CCXT onetrading.ts를 Rust로 포팅

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade, OHLCV,
};

/// One Trading 거래소
pub struct Onetrading {
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

impl Onetrading {
    const BASE_URL: &'static str = "https://api.onetrading.com/fast";
    const RATE_LIMIT_MS: u64 = 300; // rate limit in ms

    /// 새 Onetrading 인스턴스 생성
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
            fetch_currencies: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: false,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            cancel_order: true,
            cancel_all_orders: true,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_order_trades: true,
            fetch_trading_fees: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some(
                "https://github.com/ccxt/ccxt/assets/43336371/bdbc26fd-02f2-4ca7-9f1e-17333690bb1c"
                    .into(),
            ),
            api: api_urls,
            www: Some("https://onetrading.com/".into()),
            doc: vec!["https://docs.onetrading.com".into()],
            fees: Some("https://onetrading.com/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1/MINUTES".into());
        timeframes.insert(Timeframe::Minute5, "5/MINUTES".into());
        timeframes.insert(Timeframe::Minute15, "15/MINUTES".into());
        timeframes.insert(Timeframe::Minute30, "30/MINUTES".into());
        timeframes.insert(Timeframe::Hour1, "1/HOURS".into());
        timeframes.insert(Timeframe::Hour4, "4/HOURS".into());
        timeframes.insert(Timeframe::Day1, "1/DAYS".into());
        timeframes.insert(Timeframe::Week1, "1/WEEKS".into());
        timeframes.insert(Timeframe::Month1, "1/MONTHS".into());

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

    /// 공개 API 호출
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

    /// 비공개 API 호출
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

        let mut headers = HashMap::new();
        headers.insert("Accept".into(), "application/json".into());
        headers.insert("Authorization".into(), format!("Bearer {api_key}"));

        if method == "POST" {
            headers.insert("Content-Type".into(), "application/json".into());
            let body = serde_json::to_value(&params).map_err(|e| CcxtError::ExchangeError {
                message: format!("Failed to serialize params: {e}"),
            })?;
            self.private_client
                .post(path, Some(body), Some(headers))
                .await
        } else {
            let query: String = if !params.is_empty() {
                params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&")
            } else {
                String::new()
            };

            let url = if query.is_empty() {
                path.to_string()
            } else {
                format!("{path}?{query}")
            };

            match method {
                "GET" => self.private_client.get(&url, None, Some(headers)).await,
                "DELETE" => self.private_client.delete(&url, None, Some(headers)).await,
                _ => Err(CcxtError::NotSupported {
                    feature: format!("HTTP method: {method}"),
                }),
            }
        }
    }

    /// 심볼 → 마켓 ID 변환 (BTC/EUR → BTC_EUR)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &OnetradingTicker, symbol: &str) -> Ticker {
        let timestamp = chrono::DateTime::parse_from_rfc3339(&data.time)
            .ok()
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(data.time.clone()),
            high: data.high.as_ref().and_then(|s| s.parse().ok()),
            low: data.low.as_ref().and_then(|s| s.parse().ok()),
            bid: data.best_bid.as_ref().and_then(|s| s.parse().ok()),
            bid_volume: None,
            ask: data.best_ask.as_ref().and_then(|s| s.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last_price.as_ref().and_then(|s| s.parse().ok()),
            last: data.last_price.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: data.price_change.as_ref().and_then(|s| s.parse().ok()),
            percentage: data
                .price_change_percentage
                .as_ref()
                .and_then(|s| s.parse().ok()),
            average: None,
            base_volume: data.base_volume.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: data.quote_volume.as_ref().and_then(|s| s.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &OnetradingOrder, symbol: &str) -> Order {
        let status = match data.status.as_deref() {
            Some("OPEN") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Open,
            Some("FILLED_FULLY") => OrderStatus::Closed,
            Some("FILLED_CLOSED") => OrderStatus::Canceled,
            Some("FILLED_REJECTED") => OrderStatus::Rejected,
            Some("REJECTED") => OrderStatus::Rejected,
            Some("CLOSED") => OrderStatus::Canceled,
            Some("FAILED") => OrderStatus::Rejected,
            Some("STOP_TRIGGERED") => OrderStatus::Open,
            Some("DONE") => OrderStatus::Closed,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            Some("STOP") => OrderType::StopLossLimit,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = data
            .time_in_force
            .as_ref()
            .and_then(|tif| match tif.as_str() {
                "GOOD_TILL_CANCELLED" => Some(TimeInForce::GTC),
                "GOOD_TILL_TIME" => Some(TimeInForce::GTT),
                "IMMEDIATE_OR_CANCELLED" => Some(TimeInForce::IOC),
                "FILL_OR_KILL" => Some(TimeInForce::FOK),
                _ => None,
            });

        let timestamp = data
            .time
            .as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis());

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data
            .amount
            .as_ref()
            .and_then(|a| a.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data
            .filled_amount
            .as_ref()
            .and_then(|f| f.parse().ok())
            .unwrap_or_default();
        let remaining = Some(amount - filled);
        let average = data.average_price.as_ref().and_then(|p| p.parse().ok());

        Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_id.clone(),
            timestamp,
            datetime: data.time.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: data
                .time_last_updated
                .as_ref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|dt| dt.timestamp_millis()),
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
            stop_price: data.trigger_price.as_ref().and_then(|p| p.parse().ok()),
            trigger_price: data.trigger_price.as_ref().and_then(|p| p.parse().ok()),
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: data.is_post_only,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 거래 응답 파싱
    fn parse_trade(&self, data: &OnetradingTrade, symbol: &str) -> Trade {
        // Use nested trade data if available, otherwise use flattened data from OnetradingTrade
        let trade_id = data
            .trade
            .as_ref()
            .and_then(|t| t.trade_id.clone())
            .or_else(|| data.trade_id.clone());
        let order_id = data
            .trade
            .as_ref()
            .and_then(|t| t.order_id.clone())
            .or_else(|| data.order_id.clone());
        let _instrument_code = data
            .trade
            .as_ref()
            .and_then(|t| t.instrument_code.clone())
            .or_else(|| data.instrument_code.clone());
        let amount = data
            .trade
            .as_ref()
            .and_then(|t| t.amount.clone())
            .or_else(|| data.amount.clone());
        let side = data
            .trade
            .as_ref()
            .and_then(|t| t.side.clone())
            .or_else(|| data.side.clone());
        let price = data
            .trade
            .as_ref()
            .and_then(|t| t.price.clone())
            .or_else(|| data.price.clone());
        let time = data
            .trade
            .as_ref()
            .and_then(|t| t.time.clone())
            .or_else(|| data.time.clone());
        let fee_data = data.fee.as_ref();

        let timestamp = time
            .as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis());

        let side_str = match side.as_deref() {
            Some("BUY") => Some("buy".into()),
            Some("SELL") => Some("sell".into()),
            _ => None,
        };

        let price: Decimal = price
            .as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = amount
            .as_ref()
            .and_then(|a| a.parse().ok())
            .unwrap_or_default();

        let fee = fee_data.map(|f| crate::types::Fee {
            cost: f.fee_amount.as_ref().and_then(|a| a.parse().ok()),
            currency: f.fee_currency.clone(),
            rate: f.fee_percentage.as_ref().and_then(|p| p.parse().ok()),
        });

        let taker_or_maker = fee_data.and_then(|f| {
            f.fee_type.as_ref().and_then(|t| match t.as_str() {
                "TAKER" => Some(TakerOrMaker::Taker),
                "MAKER" => Some(TakerOrMaker::Maker),
                _ => None,
            })
        });

        Trade {
            id: trade_id.unwrap_or_default(),
            order: order_id,
            timestamp,
            datetime: time,
            symbol: symbol.to_string(),
            trade_type: None,
            side: side_str,
            taker_or_maker,
            price,
            amount,
            cost: Some(price * amount),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[OnetradingBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free: Option<Decimal> = b.available.as_ref().and_then(|s| s.parse().ok());
            let used: Option<Decimal> = b.locked.as_ref().and_then(|s| s.parse().ok());
            let total = match (free, used) {
                (Some(f), Some(u)) => Some(f + u),
                _ => None,
            };

            let balance = Balance {
                free,
                used,
                total,
                debt: None,
            };
            result.add(&b.currency_code, balance);
        }

        result
    }
}

#[async_trait]
impl Exchange for Onetrading {
    fn id(&self) -> ExchangeId {
        ExchangeId::Onetrading
    }

    fn name(&self) -> &str {
        "One Trading"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["AT"] // Austria
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
        let response: Vec<OnetradingMarket> = self.public_get("/v1/instruments", None).await?;

        let mut markets = Vec::new();

        for instrument in response {
            if instrument.state != Some("ACTIVE".to_string()) {
                continue;
            }

            let base = instrument
                .base
                .as_ref()
                .and_then(|b| b.code.clone())
                .unwrap_or_default();
            let quote = instrument
                .quote
                .as_ref()
                .and_then(|q| q.code.clone())
                .unwrap_or_default();
            let id = instrument
                .id
                .clone()
                .unwrap_or_else(|| format!("{base}_{quote}"));
            let symbol = format!("{base}/{quote}");

            let amount_precision = instrument.amount_precision.unwrap_or(8);
            let price_precision = instrument.market_precision.unwrap_or(2);

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
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
                taker: Some(Decimal::new(15, 4)), // 0.0015
                maker: Some(Decimal::new(10, 4)), // 0.001
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(amount_precision),
                    price: Some(price_precision),
                    cost: None,
                    base: Some(amount_precision),
                    quote: Some(price_precision),
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax::default(),
                    price: crate::types::MinMax::default(),
                    cost: instrument
                        .min_size
                        .as_ref()
                        .and_then(|s| s.parse::<Decimal>().ok())
                        .map(|min| crate::types::MinMax {
                            min: Some(min),
                            max: None,
                        })
                        .unwrap_or_default(),
                    leverage: crate::types::MinMax::default(),
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&instrument).unwrap_or_default(),
                tier_based: true,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let path = format!("/v1/market-ticker/{market_id}");

        let response: OnetradingTicker = self.public_get(&path, None).await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<OnetradingTicker> = self.public_get("/v1/market-ticker", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();
        let mut tickers = HashMap::new();

        for data in response {
            if let Some(symbol) = markets_by_id.get(&data.instrument_code) {
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
        let path = format!("/v1/order-book/{market_id}");

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("depth".into(), l.to_string());
        }

        let response: OnetradingOrderBook = self.public_get(&path, Some(params)).await?;

        let timestamp = response
            .time
            .as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis());

        let bids: Vec<OrderBookEntry> = response
            .bids
            .unwrap_or_default()
            .iter()
            .filter_map(|b| {
                Some(OrderBookEntry {
                    price: b.price.as_ref()?.parse().ok()?,
                    amount: b.amount.as_ref()?.parse().ok()?,
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .asks
            .unwrap_or_default()
            .iter()
            .filter_map(|a| {
                Some(OrderBookEntry {
                    price: a.price.as_ref()?.parse().ok()?,
                    amount: a.amount.as_ref()?.parse().ok()?,
                })
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp,
            datetime: response.time.clone(),
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
        Err(CcxtError::NotSupported {
            feature: "fetchTrades".into(),
        })
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.to_market_id(symbol);
        let path = format!("/v1/candlesticks/{market_id}");

        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let parts: Vec<&str> = interval.split('/').collect();
        if parts.len() != 2 {
            return Err(CcxtError::BadRequest {
                message: format!("Invalid timeframe format: {interval}"),
            });
        }

        let mut params = HashMap::new();
        params.insert("period".into(), parts[0].to_string());
        params.insert("unit".into(), parts[1].to_string());

        if let Some(s) = since {
            let from = chrono::DateTime::from_timestamp_millis(s)
                .ok_or_else(|| CcxtError::BadRequest {
                    message: "Invalid timestamp".into(),
                })?
                .to_rfc3339();
            params.insert("from".into(), from);
        }

        if let Some(l) = limit {
            // Calculate 'to' based on 'from' and limit
            if let Some(from_str) = params.get("from") {
                let from_dt = chrono::DateTime::parse_from_rfc3339(from_str).map_err(|e| {
                    CcxtError::BadRequest {
                        message: format!("Invalid from timestamp: {e}"),
                    }
                })?;

                let duration_secs = match timeframe {
                    Timeframe::Minute1 => 60 * l as i64,
                    Timeframe::Minute5 => 300 * l as i64,
                    Timeframe::Minute15 => 900 * l as i64,
                    Timeframe::Minute30 => 1800 * l as i64,
                    Timeframe::Hour1 => 3600 * l as i64,
                    Timeframe::Hour4 => 14400 * l as i64,
                    Timeframe::Day1 => 86400 * l as i64,
                    Timeframe::Week1 => 604800 * l as i64,
                    Timeframe::Month1 => 2592000 * l as i64,
                    _ => {
                        return Err(CcxtError::NotSupported {
                            feature: format!("Timeframe: {timeframe:?}"),
                        })
                    },
                };

                let to_dt = from_dt + chrono::Duration::seconds(duration_secs);
                params.insert("to".into(), to_dt.to_rfc3339());
            }
        }

        let response: OnetradingCandlestickResponse = self.public_get(&path, Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response
            .candlesticks
            .unwrap_or_default()
            .iter()
            .filter_map(|c| {
                let timestamp = chrono::DateTime::parse_from_rfc3339(c.time.as_ref()?)
                    .ok()?
                    .timestamp_millis();

                Some(OHLCV {
                    timestamp,
                    open: c.open.as_ref()?.parse().ok()?,
                    high: c.high.as_ref()?.parse().ok()?,
                    low: c.low.as_ref()?.parse().ok()?,
                    close: c.close.as_ref()?.parse().ok()?,
                    volume: c.total_amount.as_ref()?.parse().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: OnetradingBalanceResponse = self
            .private_request("GET", "/v1/account/balances", HashMap::new())
            .await?;

        Ok(self.parse_balance(&response.balances.unwrap_or_default()))
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
        params.insert("instrument_code".into(), market_id);
        params.insert(
            "side".into(),
            match side {
                OrderSide::Buy => "BUY",
                OrderSide::Sell => "SELL",
            }
            .into(),
        );
        params.insert(
            "type".into(),
            match order_type {
                OrderType::Limit => "LIMIT",
                OrderType::Market => "MARKET",
                OrderType::StopLossLimit => "STOP",
                _ => {
                    return Err(CcxtError::NotSupported {
                        feature: format!("Order type: {order_type:?}"),
                    })
                },
            }
            .into(),
        );
        params.insert("amount".into(), amount.to_string());

        if order_type == OrderType::Limit || order_type == OrderType::StopLossLimit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
        }

        let response: OnetradingOrder = self
            .private_request("POST", "/v1/account/orders", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let path = format!("/v1/account/orders/{id}");

        let response: serde_json::Value = self
            .private_request("DELETE", &path, HashMap::new())
            .await?;

        // Onetrading returns empty body on cancel, construct minimal order
        Ok(Order {
            id: id.to_string(),
            symbol: symbol.to_string(),
            status: OrderStatus::Canceled,
            info: response,
            ..Default::default()
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let path = format!("/v1/account/orders/{id}");

        let response: OnetradingOrderResponse =
            self.private_request("GET", &path, HashMap::new()).await?;

        Ok(self.parse_order(&response.order, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("instrument_code".into(), self.to_market_id(s));
        }

        let response: OnetradingOrderHistoryResponse = self
            .private_request("GET", "/v1/account/orders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let orders: Vec<Order> = response
            .order_history
            .unwrap_or_default()
            .iter()
            .map(|order_data| {
                let order = &order_data.order;
                let sym = order
                    .instrument_code
                    .as_ref()
                    .and_then(|ic| markets_by_id.get(ic).cloned())
                    .unwrap_or_else(|| symbol.unwrap_or("").to_string());
                self.parse_order(order, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("with_cancelled_and_rejected".into(), "true".into());

        if let Some(s) = symbol {
            params.insert("instrument_code".into(), self.to_market_id(s));
        }

        if let Some(s) = since {
            let from = chrono::DateTime::from_timestamp_millis(s)
                .ok_or_else(|| CcxtError::BadRequest {
                    message: "Invalid timestamp".into(),
                })?
                .to_rfc3339();
            params.insert("from".into(), from);
        }

        if let Some(l) = limit {
            params.insert("max_page_size".into(), l.to_string());
        }

        let response: OnetradingOrderHistoryResponse = self
            .private_request("GET", "/v1/account/orders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let orders: Vec<Order> = response
            .order_history
            .unwrap_or_default()
            .iter()
            .filter(|order_data| {
                matches!(
                    order_data.order.status.as_deref(),
                    Some("FILLED_FULLY") | Some("FILLED_CLOSED") | Some("CLOSED") | Some("DONE")
                )
            })
            .map(|order_data| {
                let order = &order_data.order;
                let sym = order
                    .instrument_code
                    .as_ref()
                    .and_then(|ic| markets_by_id.get(ic).cloned())
                    .unwrap_or_else(|| symbol.unwrap_or("").to_string());
                self.parse_order(order, &sym)
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
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("instrument_code".into(), self.to_market_id(s));
        }

        if let Some(s) = since {
            let from = chrono::DateTime::from_timestamp_millis(s)
                .ok_or_else(|| CcxtError::BadRequest {
                    message: "Invalid timestamp".into(),
                })?
                .to_rfc3339();
            params.insert("from".into(), from);
        }

        if let Some(l) = limit {
            params.insert("max_page_size".into(), l.to_string());
        }

        let response: OnetradingTradeHistoryResponse = self
            .private_request("GET", "/v1/account/trades", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let trades: Vec<Trade> = response
            .trade_history
            .unwrap_or_default()
            .iter()
            .map(|trade_data| {
                let sym = trade_data
                    .trade
                    .as_ref()
                    .and_then(|t| t.instrument_code.clone())
                    .or_else(|| trade_data.instrument_code.clone())
                    .and_then(|ic| markets_by_id.get(&ic).cloned())
                    .unwrap_or_else(|| symbol.unwrap_or("").to_string());
                self.parse_trade(trade_data, &sym)
            })
            .collect();

        Ok(trades)
    }

    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("instrument_code".into(), self.to_market_id(s));
        }

        let response: Vec<String> = self
            .private_request("DELETE", "/v1/account/orders", params)
            .await?;

        // Return minimal order structures with canceled IDs
        let orders: Vec<Order> = response
            .iter()
            .map(|id| Order {
                id: id.clone(),
                symbol: symbol.unwrap_or("").to_string(),
                status: OrderStatus::Canceled,
                info: serde_json::Value::String(id.clone()),
                ..Default::default()
            })
            .collect();

        Ok(orders)
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
            headers.insert("Accept".into(), "application/json".into());
            headers.insert("Authorization".into(), format!("Bearer {api_key}"));

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

// === Onetrading API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct OnetradingMarket {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    base: Option<OnetradingAsset>,
    #[serde(default)]
    quote: Option<OnetradingAsset>,
    #[serde(default)]
    amount_precision: Option<i32>,
    #[serde(default)]
    market_precision: Option<i32>,
    #[serde(default)]
    min_size: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OnetradingAsset {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    precision: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OnetradingTicker {
    instrument_code: String,
    time: String,
    #[serde(default)]
    last_price: Option<String>,
    #[serde(default)]
    best_bid: Option<String>,
    #[serde(default)]
    best_ask: Option<String>,
    #[serde(default)]
    price_change: Option<String>,
    #[serde(default)]
    price_change_percentage: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    base_volume: Option<String>,
    #[serde(default)]
    quote_volume: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OnetradingOrderBook {
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    bids: Option<Vec<OnetradingOrderBookEntry>>,
    #[serde(default)]
    asks: Option<Vec<OnetradingOrderBookEntry>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OnetradingOrderBookEntry {
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct OnetradingOrder {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    instrument_code: Option<String>,
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    time_last_updated: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    average_price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    filled_amount: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    time_in_force: Option<String>,
    #[serde(default)]
    is_post_only: Option<bool>,
    #[serde(default)]
    trigger_price: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OnetradingOrderResponse {
    order: OnetradingOrder,
}

#[derive(Debug, Deserialize)]
struct OnetradingOrderHistoryResponse {
    #[serde(default)]
    order_history: Option<Vec<OnetradingOrderData>>,
}

#[derive(Debug, Deserialize)]
struct OnetradingOrderData {
    order: OnetradingOrder,
}

#[derive(Debug, Deserialize, Serialize)]
struct OnetradingTrade {
    #[serde(default)]
    trade: Option<OnetradingTradeData>,
    #[serde(default)]
    fee: Option<OnetradingFee>,
    // Flattened trade data for direct access
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    instrument_code: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OnetradingTradeData {
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    instrument_code: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OnetradingFee {
    #[serde(default)]
    fee_amount: Option<String>,
    #[serde(default)]
    fee_currency: Option<String>,
    #[serde(default)]
    fee_percentage: Option<String>,
    #[serde(default)]
    fee_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OnetradingTradeHistoryResponse {
    #[serde(default)]
    trade_history: Option<Vec<OnetradingTrade>>,
}

#[derive(Debug, Deserialize)]
struct OnetradingBalanceResponse {
    #[serde(default)]
    balances: Option<Vec<OnetradingBalance>>,
}

#[derive(Debug, Deserialize)]
struct OnetradingBalance {
    currency_code: String,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    locked: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OnetradingCandlestickResponse {
    #[serde(default)]
    candlesticks: Option<Vec<OnetradingCandlestick>>,
}

#[derive(Debug, Deserialize)]
struct OnetradingCandlestick {
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    total_amount: Option<String>,
    #[serde(default)]
    volume: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let onetrading = Onetrading::new(config).unwrap();

        assert_eq!(onetrading.to_market_id("BTC/EUR"), "BTC_EUR");
        assert_eq!(onetrading.to_market_id("ETH/CHF"), "ETH_CHF");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let onetrading = Onetrading::new(config).unwrap();

        assert_eq!(onetrading.id(), ExchangeId::Onetrading);
        assert_eq!(onetrading.name(), "One Trading");
        assert!(onetrading.has().spot);
        assert!(!onetrading.has().margin);
        assert!(!onetrading.has().swap);
    }
}
