//! HitBTC Exchange Implementation
//!
//! HitBTC 거래소 API 구현

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade, Fee, OHLCV,
};

const BASE_URL: &str = "https://api.hitbtc.com/api/3";
const RATE_LIMIT_MS: u64 = 4; // 3.333ms rate limit (300 requests per second)

/// HitBTC 거래소 구조체
pub struct Hitbtc {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Hitbtc {
    /// 새 HitBTC 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), BASE_URL.into());
        api_urls.insert("private".into(), BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27766555-8eaec20e-5edc-11e7-9c5b-6dc69fc42f5e.jpg".into()),
            api: api_urls,
            www: Some("https://hitbtc.com".into()),
            doc: vec![
                "https://api.hitbtc.com".into(),
                "https://github.com/hitbtc-com/hitbtc-api/blob/master/APIv2.md".into(),
            ],
            fees: Some("https://hitbtc.com/fees-and-limits".into()),
        };

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: true,
            swap: true,
            future: false,
            option: false,
            fetch_markets: true,
            fetch_currencies: true,
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
            cancel_all_orders: true,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            fetch_deposit_address: true,
            ws: false,
            watch_ticker: false,
            watch_tickers: false,
            watch_order_book: false,
            watch_trades: false,
            watch_ohlcv: false,
            watch_balance: false,
            watch_orders: false,
            watch_my_trades: false,
            ..Default::default()
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "M1".to_string());
        timeframes.insert(Timeframe::Minute3, "M3".to_string());
        timeframes.insert(Timeframe::Minute5, "M5".to_string());
        timeframes.insert(Timeframe::Minute15, "M15".to_string());
        timeframes.insert(Timeframe::Minute30, "M30".to_string());
        timeframes.insert(Timeframe::Hour1, "H1".to_string());
        timeframes.insert(Timeframe::Hour4, "H4".to_string());
        timeframes.insert(Timeframe::Day1, "D1".to_string());
        timeframes.insert(Timeframe::Week1, "D7".to_string());
        timeframes.insert(Timeframe::Month1, "1M".to_string());

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

    /// 마켓 데이터 파싱
    fn parse_market(&self, id: &str, data: &HitbtcMarket) -> Market {
        let market_type = data.market_type.as_deref().unwrap_or("spot");
        let spot = market_type == "spot";
        let swap = market_type == "futures" && data.expiry.is_none();
        let future = market_type == "futures" && data.expiry.is_some();
        let margin = spot && data.margin_trading.unwrap_or(false);

        let base_id = data.base_currency.clone()
            .or_else(|| data.underlying.clone())
            .unwrap_or_default();
        let quote_id = data.quote_currency.clone().unwrap_or_default();
        let base = base_id.clone();
        let quote = quote_id.clone();

        let symbol = if swap || future {
            format!("{}/{}:{}", base, quote, quote)
        } else {
            format!("{}/{}", base, quote)
        };

        let amount_precision = data.quantity_increment.as_ref()
            .and_then(|s| s.parse::<f64>().ok())
            .map(|v| {
                let s = format!("{}", v);
                s.split('.').nth(1).map(|d| d.len() as i32).unwrap_or(0)
            });

        let price_precision = data.tick_size.as_ref()
            .and_then(|s| s.parse::<f64>().ok())
            .map(|v| {
                let s = format!("{}", v);
                s.split('.').nth(1).map(|d| d.len() as i32).unwrap_or(0)
            });

        Market {
            id: id.to_string(),
            lowercase_id: Some(id.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: base_id.clone(),
            quote_id: quote_id.clone(),
            active: true,
            market_type: if spot {
                MarketType::Spot
            } else if swap {
                MarketType::Swap
            } else if future {
                MarketType::Future
            } else {
                MarketType::Spot
            },
            spot,
            margin,
            swap,
            future,
            option: false,
            contract: swap || future,
            linear: Some(true),
            inverse: Some(false),
            settle: if swap || future { Some(quote.clone()) } else { None },
            settle_id: if swap || future { Some(quote_id.clone()) } else { None },
            contract_size: if swap || future { Some(Decimal::ONE) } else { None },
            expiry: data.expiry,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            taker: data.take_rate.and_then(|s| Decimal::from_str(&s.to_string()).ok()),
            maker: data.make_rate.and_then(|s| Decimal::from_str(&s.to_string()).ok()),
            precision: MarketPrecision {
                amount: amount_precision,
                price: price_precision,
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: MinMax {
                    min: data.quantity_increment.as_ref()
                        .and_then(|s| Decimal::from_str(s).ok()),
                    max: None,
                },
                price: MinMax {
                    min: data.tick_size.as_ref()
                        .and_then(|s| Decimal::from_str(s).ok()),
                    max: None,
                },
                cost: MinMax::default(),
                leverage: MinMax {
                    min: Some(Decimal::ONE),
                    max: data.max_initial_leverage.as_ref()
                        .and_then(|s| Decimal::from_str(s).ok()),
                },
            },
            index: false,
            sub_type: None,
            margin_modes: None,
            created: None,
            tier_based: true,
            percentage: true,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 티커 데이터 파싱
    fn parse_ticker(&self, symbol: &str, data: &HitbtcTicker) -> Ticker {
        let timestamp = data.timestamp.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime: data.timestamp.clone(),
            high: data.high.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            low: data.low.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            bid: data.bid.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            bid_volume: None,
            ask: data.ask.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            ask_volume: None,
            vwap: None,
            open: data.open.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            close: data.last.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            last: data.last.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            quote_volume: data.volume_quote.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// OHLCV 데이터 파싱
    fn parse_ohlcv(&self, data: &HitbtcOHLCV) -> Option<OHLCV> {
        let timestamp = data.timestamp.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())?;

        Some(OHLCV {
            timestamp,
            open: Decimal::from_str(&data.open).ok()?,
            high: Decimal::from_str(&data.max).ok()?,
            low: Decimal::from_str(&data.min).ok()?,
            close: Decimal::from_str(&data.close).ok()?,
            volume: data.volume.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default(),
        })
    }

    /// 체결 데이터 파싱
    fn parse_trade(&self, symbol: &str, data: &HitbtcTrade) -> Option<Trade> {
        let timestamp = data.timestamp.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis());

        let price = Decimal::from_str(&data.price).ok()?;
        let amount = Decimal::from_str(&data.qty.as_ref().or(data.quantity.as_ref())?).ok()?;

        Some(Trade {
            id: data.id.clone().unwrap_or_default(),
            order: data.client_order_id.clone(),
            timestamp,
            datetime: data.timestamp.clone(),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.side.clone(),
            taker_or_maker: data.taker.map(|t| if t { TakerOrMaker::Taker } else { TakerOrMaker::Maker }),
            price,
            amount,
            cost: Some(price * amount),
            fee: data.fee.as_ref().map(|f| Fee {
                cost: Decimal::from_str(f).ok(),
                currency: None,
                rate: None,
            }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// 주문 데이터 파싱
    fn parse_order(&self, data: &HitbtcOrder) -> CcxtResult<Order> {
        let status = match data.status.as_deref() {
            Some("new") => OrderStatus::Open,
            Some("suspended") => OrderStatus::Open,
            Some("partiallyFilled") => OrderStatus::Open,
            Some("filled") => OrderStatus::Closed,
            Some("canceled") => OrderStatus::Canceled,
            Some("expired") => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("market") => OrderType::Market,
            Some("limit") => OrderType::Limit,
            Some("stopLimit") => OrderType::StopLossLimit,
            Some("stopMarket") => OrderType::StopLoss,
            Some("takeProfitLimit") => OrderType::TakeProfitLimit,
            Some("takeProfitMarket") => OrderType::TakeProfit,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let timestamp = data.created_at.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis());

        let amount = data.quantity.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();

        let filled = data.quantity_cumulative.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();

        let remaining = amount - filled;

        let price = data.price.as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let average = data.price_average.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .filter(|p| *p > Decimal::ZERO);

        let cost = average.map(|avg| avg * filled);

        Ok(Order {
            id: data.id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp,
            datetime: data.created_at.clone(),
            last_trade_timestamp: data.updated_at.as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis()),
            last_update_timestamp: None,
            symbol: data.symbol.clone().unwrap_or_default(),
            order_type,
            side,
            price,
            amount,
            cost,
            average,
            filled,
            remaining: Some(remaining),
            status,
            fee: None,
            fees: Vec::new(),
            trades: Vec::new(),
            reduce_only: None,
            post_only: data.post_only,
            time_in_force: data.time_in_force.as_ref().and_then(|t| match t.as_str() {
                "GTC" => Some(TimeInForce::GTC),
                "IOC" => Some(TimeInForce::IOC),
                "FOK" => Some(TimeInForce::FOK),
                "GTD" | "Day" => Some(TimeInForce::GTC),
                _ => None,
            }),
            stop_price: data.stop_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// 잔고 데이터 파싱
    fn parse_balance(&self, data: &[HitbtcBalance]) -> Balances {
        let mut currencies = HashMap::new();

        for entry in data {
            let currency = entry.currency.clone();
            let available = Decimal::from_str(&entry.available).unwrap_or_default();
            let reserved = Decimal::from_str(&entry.reserved).unwrap_or_default();
            let total = available + reserved;

            currencies.insert(currency, Balance {
                free: Some(available),
                used: Some(reserved),
                total: Some(total),
                debt: None,
            });
        }

        Balances {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// API 서명 생성 (HMAC-SHA256 with Basic Auth format)
    fn sign_request(&self, method: &str, path: &str, body: &str) -> CcxtResult<String> {
        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();

        // Payload: METHOD + /api/3/path + [?query or body] + timestamp
        let mut payload = vec![method.to_string(), path.to_string()];

        if !body.is_empty() {
            payload.push(body.to_string());
        }

        payload.push(timestamp.clone());
        let payload_string = payload.join("");

        // HMAC-SHA256 signature
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("HMAC error: {}", e),
            })?;
        mac.update(payload_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        // Authorization format: "HS256 " + base64(apiKey + ":" + signature + ":" + timestamp)
        let auth_payload = format!("{}:{}:{}", api_key, signature, timestamp);
        let encoded = BASE64.encode(auth_payload.as_bytes());

        Ok(format!("HS256 {}", encoded))
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
        endpoint: &str,
        body: Option<serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let path = format!("/api/3/{}", endpoint);
        let body_str = body.as_ref()
            .map(|b| serde_json::to_string(b).unwrap_or_default())
            .unwrap_or_default();

        let authorization = self.sign_request(method, &path, &body_str)?;

        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), authorization);
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        if method == "GET" {
            self.client.get(&path, None, Some(headers)).await
        } else if method == "POST" {
            self.client.post(&path, body, Some(headers)).await
        } else if method == "DELETE" {
            self.client.delete_json(&path, body, Some(headers)).await
        } else {
            Err(CcxtError::BadRequest {
                message: format!("HTTP method {} not supported", method),
            })
        }
    }
}

#[async_trait]
impl Exchange for Hitbtc {
    fn id(&self) -> ExchangeId {
        ExchangeId::Hitbtc
    }

    fn name(&self) -> &str {
        "HitBTC"
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

    fn market_id(&self, symbol: &str) -> Option<String> {
        Some(symbol.replace("/", "").replace(":", ""))
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let markets = self.markets_by_id.read().ok()?;
        markets.get(market_id).cloned()
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        {
            let markets = self.markets.read().map_err(|_| CcxtError::ExchangeError {
                message: "Failed to acquire read lock".into(),
            })?;
            if !reload && !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let response: HashMap<String, HitbtcMarket> = self.public_get("/public/symbol", None).await?;

        let mut markets_map = HashMap::new();
        let mut markets_by_id_map = HashMap::new();

        for (id, data) in response {
            if id.ends_with("_BQX") {
                continue; // Skip invalid symbols
            }
            let market = self.parse_market(&id, &data);
            markets_by_id_map.insert(id.clone(), market.symbol.clone());
            markets_map.insert(market.symbol.clone(), market);
        }

        {
            let mut m = self.markets.write().map_err(|_| CcxtError::ExchangeError {
                message: "Failed to acquire write lock".into(),
            })?;
            *m = markets_map.clone();
        }
        {
            let mut m = self.markets_by_id.write().map_err(|_| CcxtError::ExchangeError {
                message: "Failed to acquire write lock".into(),
            })?;
            *m = markets_by_id_map;
        }

        Ok(markets_map)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(false).await?;
        Ok(markets.into_values().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.load_markets(false).await?;
        let market_id = self.market_id(symbol).unwrap_or_else(|| symbol.to_string());

        let path = format!("/public/ticker/{}", market_id);
        let response: HitbtcTicker = self.public_get(&path, None).await?;

        Ok(self.parse_ticker(symbol, &response))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        if let Some(syms) = symbols {
            let market_ids: Vec<String> = syms.iter()
                .map(|s| self.market_id(s).unwrap_or_else(|| s.to_string()))
                .collect();
            params.insert("symbols".to_string(), market_ids.join(","));
        }

        let response: HashMap<String, HitbtcTicker> = self.public_get("/public/ticker", Some(params)).await?;

        let mut tickers = HashMap::new();
        for (market_id, data) in response {
            if let Some(symbol) = self.symbol(&market_id) {
                let ticker = self.parse_ticker(&symbol, &data);
                tickers.insert(symbol, ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;
        let market_id = self.market_id(symbol).unwrap_or_else(|| symbol.to_string());

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("depth".to_string(), l.to_string());
        }

        let path = format!("/public/orderbook/{}", market_id);
        let response: HitbtcOrderBook = self.public_get(&path, Some(params)).await?;

        let timestamp = response.timestamp.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis());

        let bids: Vec<OrderBookEntry> = response.bid.iter().filter_map(|b| {
            Some(OrderBookEntry {
                price: Decimal::from_str(&b.price).ok()?,
                amount: Decimal::from_str(&b.size).ok()?,
            })
        }).collect();

        let asks: Vec<OrderBookEntry> = response.ask.iter().filter_map(|a| {
            Some(OrderBookEntry {
                price: Decimal::from_str(&a.price).ok()?,
                amount: Decimal::from_str(&a.size).ok()?,
            })
        }).collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp,
            datetime: response.timestamp,
            nonce: None,
            bids,
            asks,
        })
    }

    async fn fetch_trades(&self, symbol: &str, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        self.load_markets(false).await?;
        let market_id = self.market_id(symbol).unwrap_or_else(|| symbol.to_string());

        let mut params = HashMap::new();
        if let Some(s) = since {
            params.insert("from".to_string(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(1000).to_string());
        }

        let path = format!("/public/trades/{}", market_id);
        let response: Vec<HitbtcTrade> = self.public_get(&path, Some(params)).await?;

        Ok(response.iter()
            .filter_map(|t| self.parse_trade(symbol, t))
            .collect())
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        self.load_markets(false).await?;
        let market_id = self.market_id(symbol).unwrap_or_else(|| symbol.to_string());

        let interval = self.timeframes.get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {:?}", timeframe),
            })?;

        let mut params = HashMap::new();
        params.insert("period".to_string(), interval.clone());
        if let Some(s) = since {
            let datetime = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("from".to_string(), datetime);
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(1000).to_string());
        }

        let path = format!("/public/candles/{}", market_id);
        let response: Vec<HitbtcOHLCV> = self.public_get(&path, Some(params)).await?;

        Ok(response.iter()
            .filter_map(|o| self.parse_ohlcv(o))
            .collect())
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: Vec<HitbtcBalance> = self
            .private_request("GET", "spot/balance", None)
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
        self.load_markets(false).await?;
        let market_id = self.market_id(symbol).unwrap_or_else(|| symbol.to_string());

        let type_str = match order_type {
            OrderType::Market => "market",
            OrderType::Limit => "limit",
            OrderType::StopLoss => "stopMarket",
            OrderType::StopLossLimit => "stopLimit",
            OrderType::TakeProfit => "takeProfitMarket",
            OrderType::TakeProfitLimit => "takeProfitLimit",
            _ => return Err(CcxtError::BadRequest {
                message: format!("Unsupported order type: {:?}", order_type),
            }),
        };

        let side_str = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };

        let mut request = serde_json::json!({
            "symbol": market_id,
            "type": type_str,
            "side": side_str,
            "quantity": amount.to_string(),
        });

        if let Some(p) = price {
            request["price"] = serde_json::json!(p.to_string());
        }

        let response: HitbtcOrder = self
            .private_request("POST", "spot/order", Some(request))
            .await?;

        self.parse_order(&response)
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("spot/order/{}", id);
        let response: HitbtcOrder = self
            .private_request("DELETE", &path, None)
            .await?;

        self.parse_order(&response)
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("client_order_id".to_string(), id.to_string());

        let response: Vec<HitbtcOrder> = self
            .private_request("GET", "spot/history/order", Some(serde_json::to_value(&params)?))
            .await?;

        response.into_iter()
            .next()
            .ok_or_else(|| CcxtError::OrderNotFound {
                order_id: id.to_string(),
            })
            .and_then(|order| self.parse_order(&order))
    }

    async fn fetch_open_orders(&self, symbol: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(sym) = symbol {
            let market_id = self.market_id(sym).unwrap_or_else(|| sym.to_string());
            params.insert("symbol".to_string(), market_id);
        }

        let response: Vec<HitbtcOrder> = self
            .private_request("GET", "spot/order", Some(serde_json::to_value(&params)?))
            .await?;

        response.iter()
            .map(|order| self.parse_order(order))
            .collect()
    }

    async fn fetch_closed_orders(&self, symbol: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(sym) = symbol {
            let market_id = self.market_id(sym).unwrap_or_else(|| sym.to_string());
            params.insert("symbol".to_string(), market_id);
        }
        if let Some(s) = since {
            let datetime = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("from".to_string(), datetime);
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<HitbtcOrder> = self
            .private_request("GET", "spot/history/order", Some(serde_json::to_value(&params)?))
            .await?;

        response.iter()
            .map(|order| self.parse_order(order))
            .collect()
    }

    fn sign(
        &self,
        path: &str,
        _api: &str,
        method: &str,
        _params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        let mut result_headers = headers.unwrap_or_default();
        result_headers.insert("Content-Type".to_string(), "application/json".to_string());

        let url = format!("{}{}", BASE_URL, path);

        SignedRequest {
            url,
            method: method.to_string(),
            headers: result_headers,
            body: body.map(|s| s.to_string()),
        }
    }
}

// === HitBTC Response Types ===

#[derive(Debug, Default, Deserialize, Serialize)]
struct HitbtcMarket {
    #[serde(default, rename = "type")]
    market_type: Option<String>,
    #[serde(default)]
    expiry: Option<i64>,
    #[serde(default)]
    underlying: Option<String>,
    #[serde(default)]
    base_currency: Option<String>,
    #[serde(default)]
    quote_currency: Option<String>,
    #[serde(default)]
    quantity_increment: Option<String>,
    #[serde(default)]
    tick_size: Option<String>,
    #[serde(default)]
    take_rate: Option<f64>,
    #[serde(default)]
    make_rate: Option<f64>,
    #[serde(default)]
    fee_currency: Option<String>,
    #[serde(default)]
    margin_trading: Option<bool>,
    #[serde(default)]
    max_initial_leverage: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct HitbtcTicker {
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    bid: Option<String>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    volume_quote: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct HitbtcOrderBookLevel {
    #[serde(default)]
    price: String,
    #[serde(default)]
    size: String,
}

#[derive(Debug, Default, Deserialize)]
struct HitbtcOrderBook {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    ask: Vec<HitbtcOrderBookLevel>,
    #[serde(default)]
    bid: Vec<HitbtcOrderBookLevel>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct HitbtcTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    price: String,
    #[serde(default)]
    qty: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    taker: Option<bool>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    fee: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct HitbtcOHLCV {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    open: String,
    #[serde(default)]
    close: String,
    #[serde(default)]
    min: String,
    #[serde(default)]
    max: String,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    volume_quote: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct HitbtcOrder {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    time_in_force: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    quantity_cumulative: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    price_average: Option<String>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    post_only: Option<bool>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct HitbtcBalance {
    #[serde(default)]
    currency: String,
    #[serde(default)]
    available: String,
    #[serde(default)]
    reserved: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_hitbtc() -> Hitbtc {
        Hitbtc::new(ExchangeConfig::default()).unwrap()
    }

    #[test]
    fn test_exchange_id() {
        let hitbtc = create_test_hitbtc();
        assert_eq!(hitbtc.id(), ExchangeId::Hitbtc);
        assert_eq!(hitbtc.name(), "HitBTC");
    }

    #[test]
    fn test_has_features() {
        let hitbtc = create_test_hitbtc();
        let features = hitbtc.has();
        assert!(features.fetch_markets);
        assert!(features.fetch_ticker);
        assert!(features.fetch_order_book);
        assert!(features.fetch_balance);
        assert!(features.create_order);
        assert!(features.cancel_order);
    }

    #[test]
    fn test_timeframes() {
        let hitbtc = create_test_hitbtc();
        let timeframes = hitbtc.timeframes();
        assert_eq!(timeframes.get(&Timeframe::Minute1), Some(&"M1".to_string()));
        assert_eq!(timeframes.get(&Timeframe::Hour1), Some(&"H1".to_string()));
        assert_eq!(timeframes.get(&Timeframe::Day1), Some(&"D1".to_string()));
    }

    #[test]
    fn test_market_id() {
        let hitbtc = create_test_hitbtc();
        assert_eq!(hitbtc.market_id("BTC/USDT"), Some("BTCUSDT".to_string()));
        assert_eq!(hitbtc.market_id("ETH/BTC"), Some("ETHBTC".to_string()));
    }
}
