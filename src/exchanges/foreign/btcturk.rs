//! BTCTurk Exchange Implementation
//!
//! CCXT btcturk.ts를 Rust로 포팅

#![allow(dead_code)]

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
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// BTCTurk 거래소
pub struct Btcturk {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    graph_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Btcturk {
    const BASE_URL: &'static str = "https://api.btcturk.com";
    const GRAPH_URL: &'static str = "https://graph-api.btcturk.com";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

    /// 새 BTCTurk 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(&format!("{}/api/v2", Self::BASE_URL), &config)?;
        let private_client = HttpClient::new(&format!("{}/api/v1", Self::BASE_URL), &config)?;
        let graph_client = HttpClient::new(&format!("{}/v1", Self::GRAPH_URL), &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: true,
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
            cancel_order: true,
            fetch_order: false,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), format!("{}/api/v2", Self::BASE_URL));
        api_urls.insert("private".into(), format!("{}/api/v1", Self::BASE_URL));
        api_urls.insert("graph".into(), format!("{}/v1", Self::GRAPH_URL));

        let urls = ExchangeUrls {
            logo: Some("https://github.com/user-attachments/assets/10e0a238-9f60-4b06-9dda-edfc7602f1d6".into()),
            api: api_urls,
            www: Some("https://www.btcturk.com".into()),
            doc: vec!["https://github.com/BTCTrader/broker-api-docs".into()],
            fees: None,
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".into());
        timeframes.insert(Timeframe::Minute15, "15".into());
        timeframes.insert(Timeframe::Minute30, "30".into());
        timeframes.insert(Timeframe::Hour1, "60".into());
        timeframes.insert(Timeframe::Hour4, "240".into());
        timeframes.insert(Timeframe::Day1, "1 d".into());
        timeframes.insert(Timeframe::Week1, "1 w".into());

        Ok(Self {
            config,
            public_client,
            private_client,
            graph_client,
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

    /// Graph API 호출
    async fn graph_get<T: serde::de::DeserializeOwned>(
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

        self.graph_client.get(&url, None, None).await
    }

    /// 비공개 API 호출
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

        let nonce = Utc::now().timestamp_millis().to_string();

        // Create signature: HMAC-SHA256(apiKey + nonce, secret)
        let auth = format!("{}{}", api_key, nonce);
        let secret_bytes = base64::decode(secret).map_err(|e| CcxtError::AuthenticationError {
            message: format!("Invalid secret key: {}", e),
        })?;

        let mut mac = HmacSha256::new_from_slice(&secret_bytes)
            .expect("HMAC can take key of any size");
        mac.update(auth.as_bytes());
        let signature = base64::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("X-PCK".into(), api_key.to_string());
        headers.insert("X-Stamp".into(), nonce);
        headers.insert("X-Signature".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        let url = if method == "GET" || method == "DELETE" {
            if params.is_empty() {
                path.to_string()
            } else {
                let query: String = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");
                format!("{path}?{query}")
            }
        } else {
            path.to_string()
        };

        match method {
            "GET" => self.private_client.get(&url, None, Some(headers)).await,
            "POST" => {
                let body = if !params.is_empty() {
                    Some(serde_json::to_string(&params).unwrap())
                } else {
                    None
                };
                self.private_client.post(&url, body.as_deref(), Some(headers)).await
            }
            "DELETE" => self.private_client.delete(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &BtcturkTicker, symbol: &str) -> Ticker {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high,
            low: data.low,
            bid: data.bid,
            bid_volume: None,
            ask: data.ask,
            ask_volume: None,
            vwap: None,
            open: data.open,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: data.daily,
            percentage: data.daily_percent,
            average: data.average,
            base_volume: data.volume,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 거래 응답 파싱
    fn parse_trade(&self, data: &BtcturkTrade, market: Option<&Market>) -> Trade {
        let timestamp = data.date.or(data.timestamp);
        let id = data.tid.as_ref().or(data.id.as_ref()).cloned().unwrap_or_default();
        let order = data.order_id.as_ref().map(|o| o.to_string());
        let market_id = data.pair.as_ref();

        let symbol = if let Some(mid) = market_id {
            let markets_by_id = self.markets_by_id.read().unwrap();
            markets_by_id.get(mid).cloned().unwrap_or_else(|| mid.clone())
        } else if let Some(m) = market {
            m.symbol.clone()
        } else {
            String::new()
        };

        let side = data.side.as_ref().or(data.order_type.as_ref()).map(|s| s.to_lowercase());
        let amount = data.amount.unwrap_or_default().abs();

        let fee = if let Some(fee_amt) = data.fee {
            Some(crate::types::Fee {
                cost: Some(fee_amt.abs()),
                currency: data.denominator_symbol.clone(),
                rate: None,
            })
        } else {
            None
        };

        Trade {
            id,
            order,
            timestamp,
            datetime: timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            symbol,
            trade_type: None,
            side,
            taker_or_maker: None,
            price: data.price.unwrap_or_default(),
            amount,
            cost: None,
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &BtcturkOrder, market: Option<&Market>) -> Order {
        let status = match data.status.as_deref() {
            Some("Untouched") => OrderStatus::Open,
            Some("Partial") => OrderStatus::Open,
            Some("Canceled") => OrderStatus::Canceled,
            Some("Closed") => OrderStatus::Closed,
            _ => OrderStatus::Open,
        };

        let order_type = match data.method.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            Some("stop_market") => OrderType::StopLoss,
            Some("stop_limit") => OrderType::StopLossLimit,
            _ => OrderType::Limit,
        };

        let side = match data.order_type.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let market_id = data.pair_symbol.as_ref();
        let symbol = if let Some(mid) = market_id {
            let markets_by_id = self.markets_by_id.read().unwrap();
            markets_by_id.get(mid).cloned().unwrap_or_else(|| mid.clone())
        } else if let Some(m) = market {
            m.symbol.clone()
        } else {
            String::new()
        };

        let price = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.amount.as_ref()
            .or(data.quantity.as_ref())
            .and_then(|a| a.parse().ok())
            .unwrap_or_default()
            .abs();
        let remaining = data.left_amount.as_ref().and_then(|r| r.parse().ok());
        let filled = remaining.map(|r| amount - r);

        let timestamp = data.update_time.or(data.datetime);

        Order {
            id: data.id.to_string(),
            client_order_id: data.order_client_id.clone(),
            timestamp,
            datetime: timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: data.update_time,
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled,
            remaining,
            stop_price: data.stop_price.as_ref().and_then(|p| p.parse().ok()),
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

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[BtcturkBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free: Option<Decimal> = b.free.parse().ok();
            let used: Option<Decimal> = b.locked.parse().ok();
            let total: Option<Decimal> = b.balance.parse().ok();

            let balance = Balance {
                free,
                used,
                total,
                debt: None,
            };
            result.add(&b.asset, balance);
        }

        result
    }
}

#[async_trait]
impl Exchange for Btcturk {
    fn id(&self) -> ExchangeId {
        ExchangeId::Btcturk
    }

    fn name(&self) -> &str {
        "BTCTurk"
    }

    fn version(&self) -> &str {
        "v2"
    }

    fn countries(&self) -> &[&str] {
        &["TR"]
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
        let response: BtcturkExchangeInfo = self
            .public_get("/server/exchangeinfo", None)
            .await?;

        let data = response.data;
        let symbols = data.symbols;
        let mut markets = Vec::new();

        for symbol_info in symbols {
            if symbol_info.status != "TRADING" {
                continue;
            }

            let base = symbol_info.numerator.clone();
            let quote = symbol_info.denominator.clone();
            let symbol = format!("{}/{}", base, quote);

            // Parse filters
            let mut min_price = None;
            let mut max_price = None;
            let mut min_amount = None;
            let mut max_amount = None;
            let mut min_cost = None;

            for filter in symbol_info.filters {
                if filter.filter_type == "PRICE_FILTER" {
                    min_price = filter.min_price.and_then(|p| p.parse().ok());
                    max_price = filter.max_price.and_then(|p| p.parse().ok());
                    min_amount = filter.min_amount.and_then(|p| p.parse().ok());
                    max_amount = filter.max_amount.and_then(|p| p.parse().ok());
                    min_cost = filter.min_exchange_value.and_then(|p| p.parse().ok());
                }
            }

            let market = Market {
                id: symbol_info.name.clone(),
                lowercase_id: Some(symbol_info.name.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: symbol_info.numerator.clone(),
                quote_id: symbol_info.denominator.clone(),
                settle: None,
                settle_id: None,
                active: symbol_info.status == "TRADING",
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
                taker: Some(Decimal::new(9, 4)), // 0.0009 = 0.09%
                maker: Some(Decimal::new(5, 4)), // 0.0005 = 0.05%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(symbol_info.numerator_scale),
                    price: Some(symbol_info.denominator_scale),
                    cost: None,
                    base: Some(symbol_info.numerator_scale),
                    quote: Some(symbol_info.denominator_scale),
                },
                limits: MarketLimits {
                    leverage: None,
                    amount: crate::types::MinMax { min: min_amount, max: max_amount },
                    price: crate::types::MinMax { min: min_price, max: max_price },
                    cost: crate::types::MinMax { min: min_cost, max: None },
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&symbol_info).unwrap_or_default(),
                tier_based: false,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let tickers = self.fetch_tickers(Some(&[symbol])).await?;
        tickers.get(symbol).cloned().ok_or_else(|| CcxtError::ExchangeError {
            message: format!("Ticker not found for symbol: {}", symbol),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: BtcturkTickerResponse = self
            .public_get("/ticker", None)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for data in response.data {
            if let Some(symbol) = markets_by_id.get(&data.pair) {
                // If symbols filter is specified, check if this symbol is included
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

    async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        let markets = self.markets.read().unwrap();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            message: format!("Market not found: {}", symbol),
        })?;

        let mut params = HashMap::new();
        params.insert("pairSymbol".into(), market.id.clone());

        let response: BtcturkOrderBookResponse = self
            .public_get("/orderbook", Some(params))
            .await?;

        let data = response.data;
        let timestamp = data.timestamp;

        let bids: Vec<OrderBookEntry> = data.bids.iter().map(|b| OrderBookEntry {
            price: b[0].parse().unwrap_or_default(),
            amount: b[1].parse().unwrap_or_default(),
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().map(|a| OrderBookEntry {
            price: a[0].parse().unwrap_or_default(),
            amount: a[1].parse().unwrap_or_default(),
        }).collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
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
        let markets = self.markets.read().unwrap();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            message: format!("Market not found: {}", symbol),
        })?;

        let mut params = HashMap::new();
        params.insert("pairSymbol".into(), market.id.clone());
        if let Some(l) = limit {
            params.insert("last".into(), l.to_string());
        }

        let response: BtcturkTradesResponse = self
            .public_get("/trades", Some(params))
            .await?;

        let trades: Vec<Trade> = response.data
            .iter()
            .map(|t| self.parse_trade(t, Some(market)))
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
        let markets = self.markets.read().unwrap();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            message: format!("Market not found: {}", symbol),
        })?;

        let interval = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {:?}", timeframe),
        })?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market.id.clone());
        params.insert("resolution".into(), interval.clone());

        let until = Utc::now().timestamp();
        params.insert("to".into(), until.to_string());

        if let Some(s) = since {
            params.insert("from".into(), (s / 1000).to_string());
        } else if let Some(l) = limit {
            let limit_val = l.min(11000);
            // Calculate from based on limit and timeframe
            let seconds = match timeframe {
                Timeframe::Minute1 => 60,
                Timeframe::Minute15 => 15 * 60,
                Timeframe::Minute30 => 30 * 60,
                Timeframe::Hour1 => 60 * 60,
                Timeframe::Hour4 => 4 * 60 * 60,
                Timeframe::Day1 => 24 * 60 * 60,
                Timeframe::Week1 => 7 * 24 * 60 * 60,
                _ => 60,
            };
            let from = until - (seconds * limit_val as i64);
            params.insert("from".into(), from.to_string());
        }

        let response: BtcturkOHLCVResponse = self
            .graph_get("/klines/history", Some(params))
            .await?;

        let timestamps = response.t.unwrap_or_default();
        let opens = response.o.unwrap_or_default();
        let highs = response.h.unwrap_or_default();
        let lows = response.l.unwrap_or_default();
        let closes = response.c.unwrap_or_default();
        let volumes = response.v.unwrap_or_default();

        let mut ohlcv_list = Vec::new();
        for i in 0..timestamps.len() {
            if i < opens.len() && i < highs.len() && i < lows.len() && i < closes.len() && i < volumes.len() {
                ohlcv_list.push(OHLCV {
                    timestamp: timestamps[i] * 1000, // Convert to milliseconds
                    open: opens[i],
                    high: highs[i],
                    low: lows[i],
                    close: closes[i],
                    volume: volumes[i],
                });
            }
        }

        Ok(ohlcv_list)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: BtcturkBalanceResponse = self
            .private_request("GET", "/users/balances", params)
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
        let markets = self.markets.read().unwrap();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            message: format!("Market not found: {}", symbol),
        })?;

        let mut params = HashMap::new();
        params.insert("orderType".into(), match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }.to_string());
        params.insert("orderMethod".into(), match order_type {
            OrderType::Limit => "limit",
            OrderType::Market => "market",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {:?}", order_type),
            }),
        }.to_string());
        params.insert("pairSymbol".into(), market.id.clone());
        params.insert("quantity".into(), amount.to_string());

        if order_type != OrderType::Market {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
        }

        // Generate newClientOrderId
        params.insert("newClientOrderId".into(), uuid::Uuid::new_v4().to_string());

        let response: BtcturkOrderResponse = self
            .private_request("POST", "/order", params)
            .await?;

        Ok(self.parse_order(&response.data, Some(market)))
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("id".into(), id.to_string());

        let response: BtcturkCancelResponse = self
            .private_request("DELETE", "/order", params)
            .await?;

        // BTCTurk returns simple success response, construct minimal order
        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: None,
            datetime: None,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: String::new(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price: None,
            average: None,
            amount: Decimal::ZERO,
            filled: None,
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
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            let markets = self.markets.read().unwrap();
            let market = markets.get(s).ok_or_else(|| CcxtError::BadSymbol {
                message: format!("Market not found: {}", s),
            })?;
            params.insert("pairSymbol".into(), market.id.clone());
        }

        let response: BtcturkOpenOrdersResponse = self
            .private_request("GET", "/openOrders", params)
            .await?;

        let data = response.data;
        let mut orders = Vec::new();

        // Combine bids and asks
        for bid in data.bids {
            orders.push(self.parse_order(&bid, None));
        }
        for ask in data.asks {
            orders.push(self.parse_order(&ask, None));
        }

        Ok(orders)
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "BTCTurk fetchOrders requires a symbol".into(),
        })?;

        let markets = self.markets.read().unwrap();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            message: format!("Market not found: {}", symbol),
        })?;

        let mut params = HashMap::new();
        params.insert("pairSymbol".into(), market.id.clone());

        if let Some(l) = limit {
            params.insert("last".into(), l.to_string());
        }
        if let Some(s) = since {
            params.insert("startTime".into(), (s / 1000).to_string());
        }

        let response: BtcturkAllOrdersResponse = self
            .private_request("GET", "/allOrders", params)
            .await?;

        let orders: Vec<Order> = response.data
            .iter()
            .map(|o| self.parse_order(o, Some(market)))
            .collect();

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let params = HashMap::new();

        let response: BtcturkMyTradesResponse = self
            .private_request("GET", "/users/transactions/trade", params)
            .await?;

        let trades: Vec<Trade> = response.data
            .iter()
            .map(|t| self.parse_trade(t, None))
            .collect();

        Ok(trades)
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
        body: Option<&str>,
    ) -> SignedRequest {
        let base_url = match api {
            "graph" => Self::GRAPH_URL,
            "private" => Self::BASE_URL,
            _ => Self::BASE_URL,
        };

        let api_path = match api {
            "public" => "/api/v2",
            "private" => "/api/v1",
            "graph" => "/v1",
            _ => "/api/v2",
        };

        let mut url = format!("{}{}{}", base_url, api_path, path);
        let mut headers = HashMap::new();

        if api == "private" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();

            let nonce = Utc::now().timestamp_millis().to_string();
            let auth = format!("{}{}", api_key, nonce);

            if let Ok(secret_bytes) = base64::decode(secret) {
                let mut mac = HmacSha256::new_from_slice(&secret_bytes).unwrap();
                mac.update(auth.as_bytes());
                let signature = base64::encode(mac.finalize().into_bytes());

                headers.insert("X-PCK".into(), api_key.to_string());
                headers.insert("X-Stamp".into(), nonce);
                headers.insert("X-Signature".into(), signature);
                headers.insert("Content-Type".into(), "application/json".into());
            }
        }

        if (method == "GET" || method == "DELETE") && !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{}?{}", url, query);
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: body.map(String::from),
        }
    }
}

// === BTCTurk API Response Types ===

#[derive(Debug, Deserialize)]
struct BtcturkExchangeInfo {
    data: BtcturkExchangeData,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BtcturkExchangeData {
    symbols: Vec<BtcturkSymbol>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BtcturkSymbol {
    id: String,
    name: String,
    status: String,
    numerator: String,
    denominator: String,
    numerator_scale: i32,
    denominator_scale: i32,
    filters: Vec<BtcturkFilter>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BtcturkFilter {
    filter_type: String,
    #[serde(default)]
    min_price: Option<String>,
    #[serde(default)]
    max_price: Option<String>,
    #[serde(default)]
    min_amount: Option<String>,
    #[serde(default)]
    max_amount: Option<String>,
    #[serde(default)]
    min_exchange_value: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BtcturkTickerResponse {
    data: Vec<BtcturkTicker>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BtcturkTicker {
    pair: String,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    bid: Option<Decimal>,
    #[serde(default)]
    ask: Option<Decimal>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    average: Option<Decimal>,
    #[serde(default)]
    daily: Option<Decimal>,
    #[serde(default)]
    daily_percent: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct BtcturkOrderBookResponse {
    data: BtcturkOrderBookData,
}

#[derive(Debug, Deserialize)]
struct BtcturkOrderBookData {
    timestamp: i64,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct BtcturkTradesResponse {
    data: Vec<BtcturkTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BtcturkTrade {
    #[serde(default)]
    pair: Option<String>,
    #[serde(default)]
    date: Option<i64>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    tid: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    order_id: Option<i64>,
    #[serde(default)]
    fee: Option<Decimal>,
    #[serde(default)]
    denominator_symbol: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BtcturkOHLCVResponse {
    #[serde(default)]
    t: Option<Vec<i64>>,
    #[serde(default)]
    o: Option<Vec<Decimal>>,
    #[serde(default)]
    h: Option<Vec<Decimal>>,
    #[serde(default)]
    l: Option<Vec<Decimal>>,
    #[serde(default)]
    c: Option<Vec<Decimal>>,
    #[serde(default)]
    v: Option<Vec<Decimal>>,
}

#[derive(Debug, Deserialize)]
struct BtcturkBalanceResponse {
    data: Vec<BtcturkBalance>,
}

#[derive(Debug, Deserialize)]
struct BtcturkBalance {
    asset: String,
    balance: String,
    locked: String,
    free: String,
}

#[derive(Debug, Deserialize)]
struct BtcturkOrderResponse {
    data: BtcturkOrder,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BtcturkOrder {
    id: i64,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    pair_symbol: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    order_client_id: Option<String>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    update_time: Option<i64>,
    #[serde(default)]
    datetime: Option<i64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    left_amount: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcturkCancelResponse {
    success: bool,
    message: String,
}

#[derive(Debug, Deserialize)]
struct BtcturkOpenOrdersResponse {
    data: BtcturkOpenOrdersData,
}

#[derive(Debug, Deserialize)]
struct BtcturkOpenOrdersData {
    #[serde(default)]
    bids: Vec<BtcturkOrder>,
    #[serde(default)]
    asks: Vec<BtcturkOrder>,
}

#[derive(Debug, Deserialize)]
struct BtcturkAllOrdersResponse {
    data: Vec<BtcturkOrder>,
}

#[derive(Debug, Deserialize)]
struct BtcturkMyTradesResponse {
    data: Vec<BtcturkTrade>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let btcturk = Btcturk::new(config).unwrap();

        assert_eq!(btcturk.id(), ExchangeId::Btcturk);
        assert_eq!(btcturk.name(), "BTCTurk");
        assert!(btcturk.has().spot);
        assert!(!btcturk.has().margin);
        assert!(!btcturk.has().swap);
    }
}
