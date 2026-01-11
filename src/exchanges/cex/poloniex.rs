//! Poloniex Exchange Implementation
//!
//! Poloniex 거래소 API 구현

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Fee, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade,
    OHLCV,
};

const BASE_URL: &str = "https://api.poloniex.com";
const RATE_LIMIT_MS: u64 = 5;

/// Poloniex 거래소 구조체
pub struct Poloniex {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Poloniex {
    /// 새 Poloniex 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let mut api_urls = HashMap::new();
        api_urls.insert("spot".into(), BASE_URL.into());
        api_urls.insert("swap".into(), BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27766817-e9456312-5ee6-11e7-9b3c-b628ca5626a5.jpg".into()),
            api: api_urls,
            www: Some("https://www.poloniex.com".into()),
            doc: vec!["https://api-docs.poloniex.com/spot/".into()],
            fees: Some("https://poloniex.com/fees".into()),
        };

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
            swap: true,
            future: true,
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
            fetch_deposits: false,
            fetch_withdrawals: false,
            withdraw: false,
            fetch_deposit_address: false,
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
        timeframes.insert(Timeframe::Minute1, "MINUTE_1".to_string());
        timeframes.insert(Timeframe::Minute5, "MINUTE_5".to_string());
        timeframes.insert(Timeframe::Minute15, "MINUTE_15".to_string());
        timeframes.insert(Timeframe::Minute30, "MINUTE_30".to_string());
        timeframes.insert(Timeframe::Hour1, "HOUR_1".to_string());
        timeframes.insert(Timeframe::Hour2, "HOUR_2".to_string());
        timeframes.insert(Timeframe::Hour4, "HOUR_4".to_string());
        timeframes.insert(Timeframe::Hour12, "HOUR_12".to_string());
        timeframes.insert(Timeframe::Day1, "DAY_1".to_string());
        timeframes.insert(Timeframe::Week1, "WEEK_1".to_string());

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

    /// Poloniex 심볼을 통합 심볼로 변환
    fn to_symbol(&self, poloniex_symbol: &str) -> String {
        // Poloniex uses format like BTC_USDT
        poloniex_symbol.replace("_", "/")
    }

    /// 통합 심볼을 Poloniex 형식으로 변환
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// 마켓 데이터 파싱 (Spot)
    fn parse_spot_market(&self, data: &PoloniexSpotMarket) -> Market {
        let id = data.symbol.clone();
        let base = data.base_currency_name.clone();
        let quote = data.quote_currency_name.clone();
        let symbol = format!("{base}/{quote}");

        let trade_limit = &data.symbol_trade_limit;
        let price_precision = trade_limit.price_scale.unwrap_or(8) as i32;
        let amount_precision = trade_limit.quantity_scale.unwrap_or(8) as i32;

        Market {
            id: id.clone(),
            lowercase_id: Some(id.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: data.base_currency_name.clone(),
            quote_id: data.quote_currency_name.clone(),
            active: data.state == "NORMAL",
            market_type: MarketType::Spot,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            contract: false,
            linear: None,
            inverse: None,
            settle: None,
            settle_id: None,
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            taker: None,
            maker: None,
            precision: MarketPrecision {
                amount: Some(amount_precision),
                price: Some(price_precision),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: MinMax {
                    min: trade_limit
                        .min_quantity
                        .as_ref()
                        .and_then(|v| Decimal::from_str(v).ok()),
                    max: None,
                },
                price: MinMax::default(),
                cost: MinMax {
                    min: trade_limit
                        .min_amount
                        .as_ref()
                        .and_then(|v| Decimal::from_str(v).ok()),
                    max: None,
                },
                leverage: MinMax::default(),
            },
            index: false,
            sub_type: None,
            margin_modes: None,
            created: data.tradable_start_time,
            tier_based: true,
            percentage: true,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 티커 데이터 파싱
    fn parse_ticker(&self, symbol: &str, data: &PoloniexTicker) -> Ticker {
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: data.low.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: data.bid.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: data
                .bid_quantity
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            ask: data.ask.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: data
                .ask_quantity
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            close: data.close.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            last: data.close.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: data
                .daily_change
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            percentage: None,
            average: None,
            base_volume: data
                .quantity
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: data.amount.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            index_price: None,
            mark_price: data
                .mark_price
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// OHLCV 데이터 파싱
    fn parse_ohlcv(&self, data: &[serde_json::Value]) -> Option<OHLCV> {
        if data.len() < 14 {
            return None;
        }

        // Spot format: [low, high, open, close, amount, quantity, buyTakerAmount, buyTakerQuantity,
        //               tradeCount, ts, weightedAverage, interval, startTime, closeTime]
        Some(OHLCV {
            timestamp: data[12].as_i64()?,
            open: Decimal::from_str(data[2].as_str()?).ok()?,
            high: Decimal::from_str(data[1].as_str()?).ok()?,
            low: Decimal::from_str(data[0].as_str()?).ok()?,
            close: Decimal::from_str(data[3].as_str()?).ok()?,
            volume: Decimal::from_str(data[5].as_str()?).ok()?,
        })
    }

    /// 체결 데이터 파싱
    fn parse_trade(&self, symbol: &str, data: &PoloniexTrade) -> Option<Trade> {
        let price = Decimal::from_str(&data.price).ok()?;
        let amount = Decimal::from_str(&data.quantity).ok()?;
        let timestamp = data.ts;
        let side = match data.taker_side.as_str() {
            "BUY" => Some("buy".to_string()),
            "SELL" => Some("sell".to_string()),
            _ => None,
        };

        Some(Trade {
            id: data.id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side,
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// 내 체결 데이터 파싱
    fn parse_my_trade(&self, data: &PoloniexMyTrade) -> Option<Trade> {
        let price = Decimal::from_str(&data.price).ok()?;
        let amount = Decimal::from_str(&data.quantity).ok()?;
        let timestamp = data.create_time?;
        let side = match data.side.as_deref() {
            Some("BUY") => Some("buy".to_string()),
            Some("SELL") => Some("sell".to_string()),
            _ => None,
        };

        let fee_cost = data
            .fee_amount
            .as_ref()
            .and_then(|f| Decimal::from_str(f).ok());
        let fee = fee_cost.map(|cost| Fee {
            cost: Some(cost),
            currency: data.fee_currency.clone(),
            rate: None,
        });

        Some(Trade {
            id: data.id.clone(),
            order: data.order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: data
                .symbol
                .as_ref()
                .map(|s| self.to_symbol(s))
                .unwrap_or_default(),
            trade_type: None,
            side,
            taker_or_maker: data.role.as_ref().and_then(|r| match r.as_str() {
                "TAKER" | "taker" => Some(TakerOrMaker::Taker),
                "MAKER" | "maker" => Some(TakerOrMaker::Maker),
                _ => None,
            }),
            price,
            amount,
            cost: Some(price * amount),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// 주문 데이터 파싱
    fn parse_order(&self, data: &PoloniexOrder) -> CcxtResult<Order> {
        let status = match data.state.as_deref() {
            Some("NEW") => OrderStatus::Open,
            Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("PENDING_CANCEL") => OrderStatus::Open,
            Some("PARTIALLY_CANCELED") => OrderStatus::Canceled,
            Some("CANCELED") => OrderStatus::Canceled,
            Some("FAILED") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let symbol = data
            .symbol
            .as_ref()
            .map(|s| self.to_symbol(s))
            .unwrap_or_default();

        let side = match data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("MARKET") => OrderType::Market,
            Some("LIMIT") => OrderType::Limit,
            Some("LIMIT_MAKER") => OrderType::Limit,
            _ => OrderType::Limit,
        };

        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());

        let amount = data
            .quantity
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let filled = data
            .filled_quantity
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let remaining = amount - filled;
        let cost = data
            .filled_amount
            .as_ref()
            .and_then(|c| Decimal::from_str(c).ok());
        let average = if filled > Decimal::ZERO {
            cost.map(|c| c / filled)
        } else {
            None
        };

        let timestamp = data.create_time;

        Ok(Order {
            id: data.id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: None,
            last_update_timestamp: data.update_time,
            symbol,
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
            post_only: None,
            time_in_force: data.time_in_force.as_ref().and_then(|t| match t.as_str() {
                "GTC" => Some(TimeInForce::GTC),
                "IOC" => Some(TimeInForce::IOC),
                "FOK" => Some(TimeInForce::FOK),
                "PO" => Some(TimeInForce::PO),
                _ => None,
            }),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// 잔고 데이터 파싱
    fn parse_balance(&self, data: &[PoloniexAccountBalance]) -> Balances {
        let mut currencies = HashMap::new();

        for account in data {
            for balance in &account.balances {
                let currency = balance.currency.clone();
                let free = Decimal::from_str(&balance.available).unwrap_or_default();
                let used = Decimal::from_str(&balance.hold).unwrap_or_default();
                let total = free + used;

                currencies.insert(
                    currency,
                    Balance {
                        free: Some(free),
                        used: Some(used),
                        total: Some(total),
                        debt: None,
                    },
                );
            }
        }

        Balances {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// API 서명 생성
    fn sign_request(
        &self,
        method: &str,
        path: &str,
        timestamp: &str,
        body: &str,
    ) -> CcxtResult<String> {
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?;

        // Sign format: METHOD\n/path\nrequestBody=body&signTimestamp=timestamp
        let mut auth = format!("{method}\n{path}\n");

        if method == "POST" || method == "PUT" || method == "DELETE" {
            if !body.is_empty() {
                auth.push_str(&format!("requestBody={body}&"));
            }
            auth.push_str(&format!("signTimestamp={timestamp}"));
        } else {
            // GET request
            if !body.is_empty() {
                auth.push_str(&format!("{body}&signTimestamp={timestamp}"));
            } else {
                auth.push_str(&format!("signTimestamp={timestamp}"));
            }
        }

        // HMAC-SHA256
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|e| {
            CcxtError::AuthenticationError {
                message: format!("HMAC error: {e}"),
            }
        })?;
        mac.update(auth.as_bytes());
        let signature = mac.finalize().into_bytes();

        Ok(BASE64.encode(signature))
    }

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned + Default>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Private API 요청
    async fn private_request<T: for<'de> Deserialize<'de> + Default>(
        &self,
        method: &str,
        endpoint: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let path = endpoint;

        let (body, query) = if method == "POST" || method == "PUT" || method == "DELETE" {
            let body = if params.is_empty() {
                String::new()
            } else {
                serde_json::to_string(&params).unwrap_or_default()
            };
            (body, String::new())
        } else {
            // GET request
            let query = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            (String::new(), query)
        };

        let signature = self.sign_request(method, path, &timestamp, &query)?;

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        headers.insert("key".to_string(), api_key.to_string());
        headers.insert("signTimestamp".to_string(), timestamp);
        headers.insert("signature".to_string(), signature);

        let full_path = if !query.is_empty() && method == "GET" {
            format!("{path}?{query}")
        } else {
            path.to_string()
        };

        let json_body = if !body.is_empty() {
            Some(serde_json::from_str(&body).unwrap_or(serde_json::json!({})))
        } else {
            None
        };

        match method {
            "GET" => self.client.get(&full_path, None, Some(headers)).await,
            "POST" => self.client.post(&full_path, json_body, Some(headers)).await,
            "PUT" => {
                self.client
                    .put_json(&full_path, json_body, Some(headers))
                    .await
            },
            "DELETE" => {
                self.client
                    .delete_json(&full_path, json_body, Some(headers))
                    .await
            },
            _ => Err(CcxtError::BadRequest {
                message: format!("Unsupported method: {method}"),
            }),
        }
    }
}

#[async_trait]
impl Exchange for Poloniex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Poloniex
    }

    fn name(&self) -> &str {
        "Poloniex"
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
        Some(self.to_market_id(symbol))
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        Some(self.to_symbol(market_id))
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

        let response: Vec<PoloniexSpotMarket> = self.public_get("/markets", None).await?;

        let mut markets_map = HashMap::new();
        let mut markets_by_id_map = HashMap::new();

        for data in response {
            let market = self.parse_spot_market(&data);
            markets_by_id_map.insert(data.symbol.clone(), market.symbol.clone());
            markets_map.insert(market.symbol.clone(), market);
        }

        {
            let mut m = self.markets.write().map_err(|_| CcxtError::ExchangeError {
                message: "Failed to acquire write lock".into(),
            })?;
            *m = markets_map.clone();
        }
        {
            let mut m = self
                .markets_by_id
                .write()
                .map_err(|_| CcxtError::ExchangeError {
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
        let poloniex_symbol = self.to_market_id(symbol);
        let path = format!("/markets/{poloniex_symbol}/ticker24h");

        let response: PoloniexTicker = self.public_get(&path, None).await?;
        Ok(self.parse_ticker(symbol, &response))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<PoloniexTicker> = self.public_get("/markets/ticker24h", None).await?;

        let mut tickers = HashMap::new();
        for data in response {
            if let Some(sym) = &data.symbol {
                let unified_symbol = self.to_symbol(sym);
                if let Some(requested_symbols) = symbols {
                    if !requested_symbols.contains(&unified_symbol.as_str()) {
                        continue;
                    }
                }
                let ticker = self.parse_ticker(&unified_symbol, &data);
                tickers.insert(unified_symbol, ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let poloniex_symbol = self.to_market_id(symbol);
        let path = format!("/markets/{poloniex_symbol}/orderBook");

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: PoloniexOrderBook = self.public_get(&path, Some(params)).await?;

        let timestamp = response
            .time
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = response
            .bids
            .iter()
            .step_by(2)
            .enumerate()
            .filter_map(|(i, price_str)| {
                let amount_str = response.bids.get(i * 2 + 1)?;
                Some(OrderBookEntry {
                    price: Decimal::from_str(price_str).ok()?,
                    amount: Decimal::from_str(amount_str).ok()?,
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .asks
            .iter()
            .step_by(2)
            .enumerate()
            .filter_map(|(i, price_str)| {
                let amount_str = response.asks.get(i * 2 + 1)?;
                Some(OrderBookEntry {
                    price: Decimal::from_str(price_str).ok()?,
                    amount: Decimal::from_str(amount_str).ok()?,
                })
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
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
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let poloniex_symbol = self.to_market_id(symbol);
        let path = format!("/markets/{poloniex_symbol}/trades");

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<PoloniexTrade> = self.public_get(&path, Some(params)).await?;

        let mut trades = Vec::new();
        for trade_data in response {
            if let Some(trade) = self.parse_trade(symbol, &trade_data) {
                if let Some(s) = since {
                    if let Some(ts) = trade.timestamp {
                        if ts < s {
                            continue;
                        }
                    }
                }
                trades.push(trade);
            }
        }

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let poloniex_symbol = self.to_market_id(symbol);
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let path = format!("/markets/{poloniex_symbol}/candles");

        let mut params = HashMap::new();
        params.insert("interval".to_string(), interval.clone());
        if let Some(s) = since {
            params.insert("startTime".to_string(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<Vec<serde_json::Value>> = self.public_get(&path, Some(params)).await?;

        let mut ohlcv_list = Vec::new();
        for candle in response {
            if let Some(ohlcv) = self.parse_ohlcv(&candle) {
                ohlcv_list.push(ohlcv);
            }
        }

        Ok(ohlcv_list)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: Vec<PoloniexAccountBalance> = self
            .private_request("GET", "/accounts/balances", HashMap::new())
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
        let poloniex_symbol = self.to_market_id(symbol);

        let type_str = match order_type {
            OrderType::Market => "MARKET",
            OrderType::Limit => "LIMIT",
            _ => {
                return Err(CcxtError::BadRequest {
                    message: format!("Unsupported order type: {order_type:?}"),
                })
            },
        };

        let side_str = match side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };

        let mut request_params = HashMap::new();
        request_params.insert("symbol".to_string(), poloniex_symbol);
        request_params.insert("side".to_string(), side_str.to_string());
        request_params.insert("type".to_string(), type_str.to_string());
        request_params.insert("quantity".to_string(), amount.to_string());

        if let Some(p) = price {
            request_params.insert("price".to_string(), p.to_string());
        }

        let response: PoloniexOrder = self
            .private_request("POST", "/orders", request_params)
            .await?;

        self.parse_order(&response)
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("/orders/{id}");

        let response: PoloniexOrder = self
            .private_request("DELETE", &path, HashMap::new())
            .await?;

        self.parse_order(&response)
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("/orders/{id}");

        let response: PoloniexOrder = self.private_request("GET", &path, HashMap::new()).await?;

        self.parse_order(&response)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(sym) = symbol {
            let poloniex_symbol = self.to_market_id(sym);
            params.insert("symbol".to_string(), poloniex_symbol);
        }

        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<PoloniexOrder> = self.private_request("GET", "/orders", params).await?;

        let mut orders = Vec::new();
        for order_data in response {
            orders.push(self.parse_order(&order_data)?);
        }

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("state".to_string(), "FILLED".to_string());

        if let Some(sym) = symbol {
            let poloniex_symbol = self.to_market_id(sym);
            params.insert("symbol".to_string(), poloniex_symbol);
        }

        if let Some(s) = since {
            params.insert("startTime".to_string(), s.to_string());
        }

        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<PoloniexOrder> = self
            .private_request("GET", "/orders/history", params)
            .await?;

        let mut orders = Vec::new();
        for order_data in response {
            orders.push(self.parse_order(&order_data)?);
        }

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();

        if let Some(sym) = symbol {
            let poloniex_symbol = self.to_market_id(sym);
            params.insert("symbol".to_string(), poloniex_symbol);
        }

        if let Some(s) = since {
            params.insert("startTime".to_string(), s.to_string());
        }

        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<PoloniexMyTrade> = self.private_request("GET", "/trades", params).await?;

        let mut trades = Vec::new();
        for trade_data in response {
            if let Some(trade) = self.parse_my_trade(&trade_data) {
                trades.push(trade);
            }
        }

        Ok(trades)
    }

    fn sign(
        &self,
        path: &str,
        _api: &str,
        method: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        let mut result_headers = headers.unwrap_or_default();

        if self.config.api_key().is_some() && self.config.secret().is_some() {
            let timestamp = Utc::now().timestamp_millis().to_string();
            let body_str = body.unwrap_or("");

            if let Ok(signature) = self.sign_request(method, path, &timestamp, body_str) {
                if let Some(api_key) = self.config.api_key() {
                    result_headers
                        .insert("Content-Type".to_string(), "application/json".to_string());
                    result_headers.insert("key".to_string(), api_key.to_string());
                    result_headers.insert("signTimestamp".to_string(), timestamp);
                    result_headers.insert("signature".to_string(), signature);
                }
            }
        }

        let url = if params.is_empty() {
            format!("{BASE_URL}{path}")
        } else {
            let query = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{BASE_URL}{path}?{query}")
        };

        SignedRequest {
            url,
            method: method.to_string(),
            headers: result_headers,
            body: body.map(|s| s.to_string()),
        }
    }
}

// === Poloniex Response Types ===

#[derive(Debug, Default, Deserialize, Serialize)]
struct PoloniexSpotMarket {
    symbol: String,
    #[serde(rename = "baseCurrencyName")]
    base_currency_name: String,
    #[serde(rename = "quoteCurrencyName")]
    quote_currency_name: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    state: String,
    #[serde(rename = "visibleStartTime")]
    visible_start_time: Option<i64>,
    #[serde(rename = "tradableStartTime")]
    tradable_start_time: Option<i64>,
    #[serde(rename = "symbolTradeLimit")]
    symbol_trade_limit: PoloniexTradeLimit,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PoloniexTradeLimit {
    symbol: Option<String>,
    #[serde(rename = "priceScale")]
    price_scale: Option<u32>,
    #[serde(rename = "quantityScale")]
    quantity_scale: Option<u32>,
    #[serde(rename = "amountScale")]
    amount_scale: Option<u32>,
    #[serde(rename = "minQuantity")]
    min_quantity: Option<String>,
    #[serde(rename = "minAmount")]
    min_amount: Option<String>,
    #[serde(rename = "highestBid")]
    highest_bid: Option<String>,
    #[serde(rename = "lowestAsk")]
    lowest_ask: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PoloniexTicker {
    symbol: Option<String>,
    open: Option<String>,
    low: Option<String>,
    high: Option<String>,
    close: Option<String>,
    quantity: Option<String>,
    amount: Option<String>,
    #[serde(rename = "tradeCount")]
    trade_count: Option<i64>,
    #[serde(rename = "startTime")]
    start_time: Option<i64>,
    #[serde(rename = "closeTime")]
    close_time: Option<i64>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "dailyChange")]
    daily_change: Option<String>,
    bid: Option<String>,
    #[serde(rename = "bidQuantity")]
    bid_quantity: Option<String>,
    ask: Option<String>,
    #[serde(rename = "askQuantity")]
    ask_quantity: Option<String>,
    ts: Option<i64>,
    #[serde(rename = "markPrice")]
    mark_price: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PoloniexOrderBook {
    time: Option<i64>,
    scale: Option<String>,
    asks: Vec<String>,
    bids: Vec<String>,
    ts: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PoloniexTrade {
    id: String,
    price: String,
    quantity: String,
    amount: Option<String>,
    #[serde(rename = "takerSide")]
    taker_side: String,
    ts: i64,
    #[serde(rename = "createTime")]
    create_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PoloniexMyTrade {
    id: String,
    symbol: Option<String>,
    #[serde(rename = "orderId")]
    order_id: Option<String>,
    side: Option<String>,
    #[serde(rename = "type")]
    trade_type: Option<String>,
    price: String,
    quantity: String,
    #[serde(rename = "feeAmount")]
    fee_amount: Option<String>,
    #[serde(rename = "feeCurrency")]
    fee_currency: Option<String>,
    role: Option<String>,
    #[serde(rename = "createTime")]
    create_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PoloniexOrder {
    id: Option<String>,
    #[serde(rename = "clientOrderId")]
    client_order_id: Option<String>,
    symbol: Option<String>,
    state: Option<String>,
    #[serde(rename = "accountType")]
    account_type: Option<String>,
    side: Option<String>,
    #[serde(rename = "type", alias = "orderType")]
    order_type: Option<String>,
    #[serde(rename = "timeInForce")]
    time_in_force: Option<String>,
    quantity: Option<String>,
    price: Option<String>,
    #[serde(rename = "avgPrice")]
    avg_price: Option<String>,
    amount: Option<String>,
    #[serde(rename = "filledQuantity")]
    filled_quantity: Option<String>,
    #[serde(rename = "filledAmount")]
    filled_amount: Option<String>,
    #[serde(rename = "createTime")]
    create_time: Option<i64>,
    #[serde(rename = "updateTime")]
    update_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PoloniexAccountBalance {
    #[serde(rename = "accountId")]
    account_id: Option<String>,
    #[serde(rename = "accountType")]
    account_type: Option<String>,
    balances: Vec<PoloniexBalance>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PoloniexBalance {
    currency: String,
    available: String,
    hold: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_poloniex() -> Poloniex {
        Poloniex::new(ExchangeConfig::default()).unwrap()
    }

    #[test]
    fn test_to_symbol() {
        let poloniex = create_test_poloniex();
        assert_eq!(poloniex.to_symbol("BTC_USDT"), "BTC/USDT");
        assert_eq!(poloniex.to_symbol("ETH_BTC"), "ETH/BTC");
    }

    #[test]
    fn test_to_market_id() {
        let poloniex = create_test_poloniex();
        assert_eq!(poloniex.to_market_id("BTC/USDT"), "BTC_USDT");
        assert_eq!(poloniex.to_market_id("ETH/BTC"), "ETH_BTC");
    }

    #[test]
    fn test_exchange_id() {
        let poloniex = create_test_poloniex();
        assert_eq!(poloniex.id(), ExchangeId::Poloniex);
        assert_eq!(poloniex.name(), "Poloniex");
    }

    #[test]
    fn test_has_features() {
        let poloniex = create_test_poloniex();
        let features = poloniex.has();
        assert!(features.fetch_markets);
        assert!(features.fetch_ticker);
        assert!(features.fetch_order_book);
        assert!(features.fetch_balance);
        assert!(features.create_order);
    }

    #[test]
    fn test_timeframes() {
        let poloniex = create_test_poloniex();
        let timeframes = poloniex.timeframes();
        assert_eq!(
            timeframes.get(&Timeframe::Minute1),
            Some(&"MINUTE_1".to_string())
        );
        assert_eq!(
            timeframes.get(&Timeframe::Hour1),
            Some(&"HOUR_1".to_string())
        );
        assert_eq!(timeframes.get(&Timeframe::Day1), Some(&"DAY_1".to_string()));
    }
}
