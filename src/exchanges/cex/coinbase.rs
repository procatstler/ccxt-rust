//! Coinbase Exchange Implementation
//!
//! Coinbase Advanced Trade API 구현

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Fee, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade, Transaction,
    OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

const BASE_URL: &str = "https://api.coinbase.com";
const RATE_LIMIT_MS: u64 = 100;

/// Coinbase 거래소
pub struct Coinbase {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Coinbase {
    /// 새 Coinbase 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
            swap: false,
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
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: false,
            fetch_deposit_address: true,
            ws: true,
            watch_ticker: true,
            watch_tickers: false,
            watch_order_book: true,
            watch_trades: true,
            watch_ohlcv: false,
            watch_balance: false,
            watch_orders: false,
            watch_my_trades: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), BASE_URL.into());
        api_urls.insert("private".into(), BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/40811661-b6eceae2-653a-11e8-829e-10bfadb078cf.jpg".into()),
            api: api_urls,
            www: Some("https://www.coinbase.com".into()),
            doc: vec![
                "https://docs.cloud.coinbase.com/advanced-trade-api/docs".into(),
            ],
            fees: Some("https://www.coinbase.com/advanced-fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "ONE_MINUTE".into());
        timeframes.insert(Timeframe::Minute5, "FIVE_MINUTE".into());
        timeframes.insert(Timeframe::Minute15, "FIFTEEN_MINUTE".into());
        timeframes.insert(Timeframe::Minute30, "THIRTY_MINUTE".into());
        timeframes.insert(Timeframe::Hour1, "ONE_HOUR".into());
        timeframes.insert(Timeframe::Hour2, "TWO_HOUR".into());
        timeframes.insert(Timeframe::Hour6, "SIX_HOUR".into());
        timeframes.insert(Timeframe::Day1, "ONE_DAY".into());

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

    /// 심볼을 Coinbase 형식으로 변환 (BTC/USD -> BTC-USD)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Coinbase 심볼을 통합 형식으로 변환 (BTC-USD -> BTC/USD)
    fn to_symbol(&self, market_id: &str) -> String {
        market_id.replace("-", "/")
    }

    /// HMAC 서명 생성
    fn create_signature(
        &self,
        timestamp: &str,
        method: &str,
        path: &str,
        body: &str,
    ) -> CcxtResult<String> {
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        let message = format!("{timestamp}{method}{path}{body}");
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(message.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
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
    async fn private_get<T: serde::de::DeserializeOwned>(
        &self,
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

        let timestamp = Utc::now().timestamp().to_string();
        let full_path = if params.is_empty() {
            path.to_string()
        } else {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{path}?{query}")
        };

        let signature = self.create_signature(&timestamp, "GET", &full_path, "")?;

        let mut headers = HashMap::new();
        headers.insert("CB-ACCESS-KEY".to_string(), api_key.to_string());
        headers.insert("CB-ACCESS-SIGN".to_string(), signature);
        headers.insert("CB-ACCESS-TIMESTAMP".to_string(), timestamp);
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        self.client.get(&full_path, None, Some(headers)).await
    }

    /// 비공개 POST API 호출
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;

        let timestamp = Utc::now().timestamp().to_string();
        let body_str = body.to_string();
        let signature = self.create_signature(&timestamp, "POST", path, &body_str)?;

        let mut headers = HashMap::new();
        headers.insert("CB-ACCESS-KEY".to_string(), api_key.to_string());
        headers.insert("CB-ACCESS-SIGN".to_string(), signature);
        headers.insert("CB-ACCESS-TIMESTAMP".to_string(), timestamp);
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        self.client.post(path, Some(body), Some(headers)).await
    }

    /// 마켓 정보 파싱
    fn parse_market(&self, data: &CoinbaseProduct) -> Market {
        let base = data.base_currency_id.clone();
        let quote = data.quote_currency_id.clone();
        let symbol = format!("{base}/{quote}");

        Market {
            id: data.product_id.clone(),
            lowercase_id: Some(data.product_id.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: base.to_lowercase(),
            quote_id: quote.to_lowercase(),
            settle: None,
            settle_id: None,
            market_type: MarketType::Spot,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            index: false,
            active: data.status.as_ref().map(|s| s == "online").unwrap_or(true),
            contract: false,
            linear: None,
            inverse: None,
            sub_type: None,
            taker: Some(Decimal::new(60, 4)), // 0.006 = 0.6%
            maker: Some(Decimal::new(40, 4)), // 0.004 = 0.4%
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            percentage: true,
            tier_based: true,
            precision: MarketPrecision {
                amount: data
                    .base_increment
                    .as_ref()
                    .and_then(|v| v.parse::<Decimal>().ok())
                    .map(|d| {
                        let s = d.to_string();
                        if let Some(pos) = s.find('.') {
                            (s.len() - pos - 1) as i32
                        } else {
                            0
                        }
                    }),
                price: data
                    .quote_increment
                    .as_ref()
                    .and_then(|v| v.parse::<Decimal>().ok())
                    .map(|d| {
                        let s = d.to_string();
                        if let Some(pos) = s.find('.') {
                            (s.len() - pos - 1) as i32
                        } else {
                            0
                        }
                    }),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: crate::types::MinMax {
                    min: data.base_min_size.as_ref().and_then(|v| v.parse().ok()),
                    max: data.base_max_size.as_ref().and_then(|v| v.parse().ok()),
                },
                price: crate::types::MinMax {
                    min: data.quote_increment.as_ref().and_then(|v| v.parse().ok()),
                    max: None,
                },
                cost: crate::types::MinMax {
                    min: data.min_market_funds.as_ref().and_then(|v| v.parse().ok()),
                    max: data.max_market_funds.as_ref().and_then(|v| v.parse().ok()),
                },
                leverage: crate::types::MinMax::default(),
            },
            margin_modes: None,
            created: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 티커 파싱
    fn parse_ticker(&self, data: &CoinbaseTicker, symbol: &str) -> Ticker {
        let timestamp = data
            .time
            .as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low_24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.best_bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.best_bid_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.best_ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.best_ask_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open_24h.as_ref().and_then(|v| v.parse().ok()),
            close: data.price.as_ref().and_then(|v| v.parse().ok()),
            last: data.price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data
                .price_percentage_change_24h
                .as_ref()
                .and_then(|v| v.parse().ok()),
            percentage: data
                .price_percentage_change_24h
                .as_ref()
                .and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.volume_24h.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 파싱
    fn parse_order(&self, data: &CoinbaseOrder, symbol: &str) -> CcxtResult<Order> {
        let timestamp = data
            .created_time
            .as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis());

        let status = match data.status.as_deref() {
            Some("PENDING") | Some("OPEN") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("CANCELLED") | Some("EXPIRED") | Some("FAILED") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let side = match data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("MARKET") => OrderType::Market,
            Some("LIMIT") => OrderType::Limit,
            Some("STOP") | Some("STOP_LIMIT") => OrderType::StopLimit,
            _ => OrderType::Limit,
        };

        // Parse order configuration for price and amount
        let (price, amount) = if let Some(config) = &data.order_configuration {
            if let Some(limit) = &config.limit_limit_gtc {
                (
                    limit.limit_price.as_ref().and_then(|v| v.parse().ok()),
                    limit
                        .base_size
                        .as_ref()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or_default(),
                )
            } else if let Some(market) = &config.market_market_ioc {
                (
                    None,
                    market
                        .base_size
                        .as_ref()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or_default(),
                )
            } else {
                (None, Decimal::ZERO)
            }
        } else {
            (None, Decimal::ZERO)
        };

        let filled: Decimal = data
            .filled_size
            .as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        Ok(Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            trigger_price: None,
            average: data
                .average_filled_price
                .as_ref()
                .and_then(|v| v.parse().ok()),
            amount,
            filled,
            remaining: Some(amount - filled),
            cost: data
                .total_value_after_fees
                .as_ref()
                .and_then(|v| v.parse().ok()),
            trades: Vec::new(),
            reduce_only: None,
            post_only: None,
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            fee: data
                .total_fees
                .as_ref()
                .and_then(|v| v.parse().ok())
                .map(|cost| Fee {
                    cost: Some(cost),
                    currency: None,
                    rate: None,
                }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// 체결 파싱
    fn parse_trade_data(&self, data: &CoinbaseFill, symbol: &str) -> Trade {
        let timestamp = data
            .trade_time
            .as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = match data.side.as_deref() {
            Some("BUY") => Some("buy".to_string()),
            Some("SELL") => Some("sell".to_string()),
            _ => None,
        };

        let price: Decimal = data
            .price
            .as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data
            .size
            .as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: data.order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side,
            taker_or_maker: data.liquidity_indicator.as_ref().map(|l| {
                if l == "MAKER" {
                    TakerOrMaker::Maker
                } else {
                    TakerOrMaker::Taker
                }
            }),
            price,
            amount,
            cost: Some(price * amount),
            fee: data
                .commission
                .as_ref()
                .and_then(|v| v.parse().ok())
                .map(|cost| Fee {
                    cost: Some(cost),
                    currency: None,
                    rate: None,
                }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 파싱
    fn parse_balance_data(&self, data: &CoinbaseAccount) -> (String, Balance) {
        let currency = data.currency.clone().unwrap_or_default();
        let available: Decimal = data
            .available_balance
            .as_ref()
            .and_then(|b| b.value.as_ref())
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let hold: Decimal = data
            .hold
            .as_ref()
            .and_then(|b| b.value.as_ref())
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        (
            currency,
            Balance {
                free: Some(available),
                used: Some(hold),
                total: Some(available + hold),
                debt: None,
            },
        )
    }
}

#[async_trait]
impl Exchange for Coinbase {
    fn id(&self) -> ExchangeId {
        ExchangeId::Coinbase
    }

    fn name(&self) -> &str {
        "Coinbase"
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
        self.markets_by_id.read().ok()?.get(symbol).cloned()
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        self.markets
            .read()
            .ok()?
            .get(market_id)
            .map(|m| m.symbol.clone())
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
        let timestamp = Utc::now().timestamp().to_string();
        let body_str = body.unwrap_or("");

        let message = format!("{timestamp}{method}{path}{body_str}");

        let signature = if let Some(secret) = self.config.secret() {
            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(message.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        } else {
            String::new()
        };

        let mut new_headers = headers.unwrap_or_default();
        if let Some(api_key) = self.config.api_key() {
            new_headers.insert("CB-ACCESS-KEY".to_string(), api_key.to_string());
            new_headers.insert("CB-ACCESS-SIGN".to_string(), signature);
            new_headers.insert("CB-ACCESS-TIMESTAMP".to_string(), timestamp);
        }
        new_headers.insert("Content-Type".to_string(), "application/json".to_string());

        let url = if method == "GET" && !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{}/{}?{}", BASE_URL, path.trim_start_matches('/'), query)
        } else {
            format!("{}/{}", BASE_URL, path.trim_start_matches('/'))
        };

        SignedRequest {
            url,
            method: method.to_string(),
            headers: new_headers,
            body: body.map(|s| s.to_string()),
        }
    }

    async fn load_markets(&self, _reload: bool) -> CcxtResult<HashMap<String, Market>> {
        let response: CoinbaseProductsResponse = self
            .public_get("/api/v3/brokerage/market/products", None)
            .await?;

        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for product in &response.products {
            let market = self.parse_market(product);
            markets_by_id.insert(market.symbol.clone(), market.id.clone());
            markets.insert(market.symbol.clone(), market);
        }

        if let Ok(mut cached) = self.markets.write() {
            *cached = markets.clone();
        }
        if let Ok(mut cached) = self.markets_by_id.write() {
            *cached = markets_by_id;
        }

        Ok(markets)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(false).await?;
        Ok(markets.into_values().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let response: CoinbaseTicker = self
            .public_get(
                &format!("/api/v3/brokerage/market/products/{market_id}"),
                None,
            )
            .await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: CoinbaseProductsResponse = self
            .public_get("/api/v3/brokerage/market/products", None)
            .await?;

        let mut tickers = HashMap::new();
        for product in &response.products {
            let symbol = self.to_symbol(&product.product_id);
            if let Some(syms) = symbols {
                if !syms.contains(&symbol.as_str()) {
                    continue;
                }
            }
            let ticker = Ticker {
                symbol: symbol.clone(),
                timestamp: Some(Utc::now().timestamp_millis()),
                datetime: Some(Utc::now().to_rfc3339()),
                high: product.high_24h.as_ref().and_then(|v| v.parse().ok()),
                low: product.low_24h.as_ref().and_then(|v| v.parse().ok()),
                bid: None,
                bid_volume: None,
                ask: None,
                ask_volume: None,
                vwap: None,
                open: None,
                close: product.price.as_ref().and_then(|v| v.parse().ok()),
                last: product.price.as_ref().and_then(|v| v.parse().ok()),
                previous_close: None,
                change: product
                    .price_percentage_change_24h
                    .as_ref()
                    .and_then(|v| v.parse().ok()),
                percentage: product
                    .price_percentage_change_24h
                    .as_ref()
                    .and_then(|v| v.parse().ok()),
                average: None,
                base_volume: product.volume_24h.as_ref().and_then(|v| v.parse().ok()),
                quote_volume: None,
                index_price: None,
                mark_price: None,
                info: serde_json::to_value(product).unwrap_or_default(),
            };
            tickers.insert(symbol, ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("product_id".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: CoinbaseOrderBookResponse = self
            .public_get("/api/v3/brokerage/market/product_book", Some(params))
            .await?;

        let pricebook = response.pricebook;
        let timestamp = pricebook
            .time
            .as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = pricebook
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: b
                    .price
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default(),
                amount: b
                    .size
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = pricebook
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: a
                    .price
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default(),
                amount: a
                    .size
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default(),
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
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: CoinbaseTradesResponse = self
            .public_get(
                &format!("/api/v3/brokerage/market/products/{market_id}/ticker"),
                Some(params),
            )
            .await?;

        let trades: Vec<Trade> = response
            .trades
            .iter()
            .map(|t| {
                let timestamp = t
                    .time
                    .as_ref()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let price: Decimal = t
                    .price
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
                let amount: Decimal = t
                    .size
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();

                Trade {
                    id: t.trade_id.clone().unwrap_or_default(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: t.side.as_ref().map(|s| s.to_lowercase()),
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
        let market_id = self.to_market_id(symbol);
        let granularity = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("granularity".to_string(), granularity.clone());

        if let Some(s) = since {
            let start = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("start".to_string(), start);
        }

        let response: CoinbaseCandlesResponse = self
            .public_get(
                &format!("/api/v3/brokerage/market/products/{market_id}/candles"),
                Some(params),
            )
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .candles
            .iter()
            .take(limit.unwrap_or(300) as usize)
            .map(|c| OHLCV {
                timestamp: c
                    .start
                    .as_ref()
                    .and_then(|s| s.parse::<i64>().ok())
                    .map(|t| t * 1000)
                    .unwrap_or_default(),
                open: c
                    .open
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default(),
                high: c
                    .high
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default(),
                low: c
                    .low
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default(),
                close: c
                    .close
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default(),
                volume: c
                    .volume
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default(),
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: CoinbaseAccountsResponse = self
            .private_get("/api/v3/brokerage/accounts", HashMap::new())
            .await?;

        let mut balances = Balances {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies: HashMap::new(),
            info: serde_json::to_value(&response).unwrap_or_default(),
        };

        for account in &response.accounts {
            let (currency, balance) = self.parse_balance_data(account);
            balances.currencies.insert(currency, balance);
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
        let market_id = self.to_market_id(symbol);
        let client_order_id = format!("ccxt_{}", Utc::now().timestamp_millis());

        let order_configuration = match order_type {
            OrderType::Market => {
                serde_json::json!({
                    "market_market_ioc": {
                        "base_size": amount.to_string()
                    }
                })
            },
            OrderType::Limit => {
                let p = price.ok_or_else(|| CcxtError::BadRequest {
                    message: "Price required for limit order".into(),
                })?;
                serde_json::json!({
                    "limit_limit_gtc": {
                        "base_size": amount.to_string(),
                        "limit_price": p.to_string(),
                        "post_only": false
                    }
                })
            },
            _ => {
                return Err(CcxtError::BadRequest {
                    message: format!("Unsupported order type: {order_type:?}"),
                });
            },
        };

        let body = serde_json::json!({
            "client_order_id": client_order_id,
            "product_id": market_id,
            "side": match side {
                OrderSide::Buy => "BUY",
                OrderSide::Sell => "SELL",
            },
            "order_configuration": order_configuration
        });

        let response: CoinbaseOrderResponse =
            self.private_post("/api/v3/brokerage/orders", body).await?;

        if let Some(success_response) = response.success_response {
            self.parse_order(&success_response, symbol)
        } else {
            Err(CcxtError::ExchangeError {
                message: response
                    .error_response
                    .and_then(|e| e.message)
                    .unwrap_or_else(|| "Order creation failed".into()),
            })
        }
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let body = serde_json::json!({
            "order_ids": [id]
        });

        let _response: CoinbaseCancelResponse = self
            .private_post("/api/v3/brokerage/orders/batch_cancel", body)
            .await?;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price: None,
            trigger_price: None,
            average: None,
            amount: Decimal::ZERO,
            filled: Decimal::ZERO,
            remaining: None,
            cost: None,
            trades: Vec::new(),
            reduce_only: None,
            post_only: None,
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            fee: None,
            fees: Vec::new(),
            info: serde_json::Value::Null,
        })
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let response: CoinbaseOrderDetailResponse = self
            .private_get(
                &format!("/api/v3/brokerage/orders/historical/{id}"),
                HashMap::new(),
            )
            .await?;

        let symbol = self.to_symbol(&response.order.product_id.clone().unwrap_or_default());
        self.parse_order(&response.order, &symbol)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("order_status".to_string(), "OPEN".to_string());
        if let Some(sym) = symbol {
            params.insert("product_id".to_string(), self.to_market_id(sym));
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: CoinbaseOrdersResponse = self
            .private_get("/api/v3/brokerage/orders/historical", params)
            .await?;

        let mut orders = Vec::new();
        for order_data in &response.orders {
            let sym = symbol.map(|s| s.to_string()).unwrap_or_else(|| {
                self.to_symbol(&order_data.product_id.clone().unwrap_or_default())
            });
            orders.push(self.parse_order(order_data, &sym)?);
        }

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("order_status".to_string(), "FILLED".to_string());
        if let Some(sym) = symbol {
            params.insert("product_id".to_string(), self.to_market_id(sym));
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: CoinbaseOrdersResponse = self
            .private_get("/api/v3/brokerage/orders/historical", params)
            .await?;

        let mut orders = Vec::new();
        for order_data in &response.orders {
            let sym = symbol.map(|s| s.to_string()).unwrap_or_else(|| {
                self.to_symbol(&order_data.product_id.clone().unwrap_or_default())
            });
            orders.push(self.parse_order(order_data, &sym)?);
        }

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();
        if let Some(sym) = symbol {
            params.insert("product_id".to_string(), self.to_market_id(sym));
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: CoinbaseFillsResponse = self
            .private_get("/api/v3/brokerage/orders/historical/fills", params)
            .await?;

        let trades: Vec<Trade> = response
            .fills
            .iter()
            .map(|f| {
                let sym = symbol
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| self.to_symbol(&f.product_id.clone().unwrap_or_default()));
                self.parse_trade_data(f, &sym)
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_deposits(
        &self,
        _code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        Ok(Vec::new())
    }

    async fn fetch_withdrawals(
        &self,
        _code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        Ok(Vec::new())
    }
}

// === Coinbase Response Types ===

#[derive(Debug, Default, Deserialize)]
struct CoinbaseProductsResponse {
    #[serde(default)]
    products: Vec<CoinbaseProduct>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseProduct {
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    base_currency_id: String,
    #[serde(default)]
    quote_currency_id: String,
    #[serde(default)]
    base_increment: Option<String>,
    #[serde(default)]
    quote_increment: Option<String>,
    #[serde(default)]
    base_min_size: Option<String>,
    #[serde(default)]
    base_max_size: Option<String>,
    #[serde(default)]
    min_market_funds: Option<String>,
    #[serde(default)]
    max_market_funds: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    price_percentage_change_24h: Option<String>,
    #[serde(default)]
    volume_24h: Option<String>,
    #[serde(default)]
    high_24h: Option<String>,
    #[serde(default)]
    low_24h: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseTicker {
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    best_bid: Option<String>,
    #[serde(default)]
    best_bid_size: Option<String>,
    #[serde(default)]
    best_ask: Option<String>,
    #[serde(default)]
    best_ask_size: Option<String>,
    #[serde(default)]
    open_24h: Option<String>,
    #[serde(default)]
    high_24h: Option<String>,
    #[serde(default)]
    low_24h: Option<String>,
    #[serde(default)]
    volume_24h: Option<String>,
    #[serde(default)]
    price_percentage_change_24h: Option<String>,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseOrderBookResponse {
    #[serde(default)]
    pricebook: CoinbasePricebook,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbasePricebook {
    #[serde(default)]
    bids: Vec<CoinbaseOrderBookEntry>,
    #[serde(default)]
    asks: Vec<CoinbaseOrderBookEntry>,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseOrderBookEntry {
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseTradesResponse {
    #[serde(default)]
    trades: Vec<CoinbaseTradeData>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseTradeData {
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseCandlesResponse {
    #[serde(default)]
    candles: Vec<CoinbaseCandle>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseCandle {
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    volume: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseAccountsResponse {
    #[serde(default)]
    accounts: Vec<CoinbaseAccount>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseAccount {
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    available_balance: Option<CoinbaseBalanceData>,
    #[serde(default)]
    hold: Option<CoinbaseBalanceData>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseBalanceData {
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    currency: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseOrder {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    product_id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    created_time: Option<String>,
    #[serde(default)]
    filled_size: Option<String>,
    #[serde(default)]
    average_filled_price: Option<String>,
    #[serde(default)]
    total_value_after_fees: Option<String>,
    #[serde(default)]
    total_fees: Option<String>,
    #[serde(default)]
    order_configuration: Option<CoinbaseOrderConfiguration>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseOrderConfiguration {
    #[serde(default)]
    limit_limit_gtc: Option<CoinbaseLimitConfig>,
    #[serde(default)]
    market_market_ioc: Option<CoinbaseMarketConfig>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseLimitConfig {
    #[serde(default)]
    base_size: Option<String>,
    #[serde(default)]
    limit_price: Option<String>,
    #[serde(default)]
    post_only: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseMarketConfig {
    #[serde(default)]
    base_size: Option<String>,
    #[serde(default)]
    quote_size: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseOrderResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    success_response: Option<CoinbaseOrder>,
    #[serde(default)]
    error_response: Option<CoinbaseErrorResponse>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseErrorResponse {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseOrderDetailResponse {
    #[serde(default)]
    order: CoinbaseOrder,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseOrdersResponse {
    #[serde(default)]
    orders: Vec<CoinbaseOrder>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseCancelResponse {
    #[serde(default)]
    results: Vec<CoinbaseCancelResult>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseCancelResult {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    order_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseFillsResponse {
    #[serde(default)]
    fills: Vec<CoinbaseFill>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseFill {
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    product_id: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    commission: Option<String>,
    #[serde(default)]
    liquidity_indicator: Option<String>,
    #[serde(default)]
    trade_time: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::default();
        let coinbase = Coinbase::new(config).unwrap();

        assert_eq!(coinbase.to_market_id("BTC/USD"), "BTC-USD");
        assert_eq!(coinbase.to_symbol("BTC-USD"), "BTC/USD");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let coinbase = Coinbase::new(config).unwrap();

        assert_eq!(coinbase.name(), "Coinbase");
        assert!(coinbase.has().fetch_markets);
        assert!(coinbase.has().fetch_ticker);
        assert!(coinbase.has().create_order);
    }
}
