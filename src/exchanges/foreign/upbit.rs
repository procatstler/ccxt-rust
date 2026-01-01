//! Upbit Exchange Implementation
//!
//! 업비트 거래소 API 구현 (한국 최대 암호화폐 거래소)

#![allow(dead_code)]

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize};
use sha2::{Digest, Sha256, Sha512};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use uuid::Uuid;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, Fee, OHLCV, TakerOrMaker,
};

const BASE_URL: &str = "https://api.upbit.com";
const RATE_LIMIT_MS: u64 = 50;

/// Upbit 거래소 구조체
pub struct Upbit {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

// API Response Types
#[derive(Debug, Deserialize)]
struct UpbitMarket {
    market: String,
    korean_name: Option<String>,
    english_name: Option<String>,
    market_warning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpbitTicker {
    market: String,
    trade_date: Option<String>,
    trade_time: Option<String>,
    trade_date_kst: Option<String>,
    trade_time_kst: Option<String>,
    trade_timestamp: Option<i64>,
    opening_price: Option<Decimal>,
    high_price: Option<Decimal>,
    low_price: Option<Decimal>,
    trade_price: Option<Decimal>,
    prev_closing_price: Option<Decimal>,
    change: Option<String>,
    change_price: Option<Decimal>,
    change_rate: Option<Decimal>,
    signed_change_price: Option<Decimal>,
    signed_change_rate: Option<Decimal>,
    trade_volume: Option<Decimal>,
    acc_trade_price: Option<Decimal>,
    acc_trade_price_24h: Option<Decimal>,
    acc_trade_volume: Option<Decimal>,
    acc_trade_volume_24h: Option<Decimal>,
    highest_52_week_price: Option<Decimal>,
    highest_52_week_date: Option<String>,
    lowest_52_week_price: Option<Decimal>,
    lowest_52_week_date: Option<String>,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct UpbitOrderBookUnit {
    ask_price: Decimal,
    bid_price: Decimal,
    ask_size: Decimal,
    bid_size: Decimal,
}

#[derive(Debug, Deserialize)]
struct UpbitOrderBook {
    market: String,
    timestamp: i64,
    total_ask_size: Decimal,
    total_bid_size: Decimal,
    orderbook_units: Vec<UpbitOrderBookUnit>,
}

#[derive(Debug, Deserialize)]
struct UpbitTrade {
    market: String,
    trade_date_utc: String,
    trade_time_utc: String,
    timestamp: i64,
    trade_price: Decimal,
    trade_volume: Decimal,
    prev_closing_price: Option<Decimal>,
    change_price: Option<Decimal>,
    ask_bid: String,
    sequential_id: i64,
}

#[derive(Debug, Deserialize)]
struct UpbitCandle {
    market: String,
    candle_date_time_utc: String,
    candle_date_time_kst: String,
    opening_price: Decimal,
    high_price: Decimal,
    low_price: Decimal,
    trade_price: Decimal,
    timestamp: i64,
    candle_acc_trade_price: Decimal,
    candle_acc_trade_volume: Decimal,
    unit: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct UpbitBalance {
    currency: String,
    balance: String,
    locked: String,
    avg_buy_price: String,
    avg_buy_price_modified: bool,
    unit_currency: String,
}

#[derive(Debug, Deserialize)]
struct UpbitOrder {
    uuid: String,
    side: String,
    ord_type: String,
    price: Option<String>,
    state: String,
    market: String,
    created_at: String,
    volume: Option<String>,
    remaining_volume: Option<String>,
    reserved_fee: Option<String>,
    remaining_fee: Option<String>,
    paid_fee: Option<String>,
    locked: Option<String>,
    executed_volume: Option<String>,
    trades_count: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct UpbitError {
    error: UpbitErrorDetail,
}

#[derive(Debug, Deserialize)]
struct UpbitErrorDetail {
    message: String,
    name: String,
}

impl Upbit {
    /// 새 Upbit 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), format!("{}/v1", BASE_URL));
        api_urls.insert("private".into(), format!("{}/v1", BASE_URL));

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/49245610-eedf3080-f423-11e8-9cba-4b0aed794f26.jpg".into()),
            api: api_urls,
            www: Some("https://upbit.com".into()),
            doc: vec!["https://docs.upbit.com/reference".into()],
            fees: Some("https://upbit.com/service_center/guide".into()),
        };

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
            withdraw: true,
            fetch_deposit_address: true,
            ws: true,
            watch_ticker: true,
            watch_tickers: true,
            watch_order_book: true,
            watch_trades: true,
            watch_ohlcv: false,
            watch_balance: false,
            watch_orders: true,
            watch_my_trades: true,
            ..Default::default()
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Second1, "seconds".to_string());
        timeframes.insert(Timeframe::Minute1, "minutes/1".to_string());
        timeframes.insert(Timeframe::Minute3, "minutes/3".to_string());
        timeframes.insert(Timeframe::Minute5, "minutes/5".to_string());
        timeframes.insert(Timeframe::Minute15, "minutes/15".to_string());
        timeframes.insert(Timeframe::Minute30, "minutes/30".to_string());
        timeframes.insert(Timeframe::Hour1, "minutes/60".to_string());
        timeframes.insert(Timeframe::Hour4, "minutes/240".to_string());
        timeframes.insert(Timeframe::Day1, "days".to_string());
        timeframes.insert(Timeframe::Week1, "weeks".to_string());
        timeframes.insert(Timeframe::Month1, "months".to_string());

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

    /// Upbit 마켓 ID를 통합 심볼로 변환
    /// Upbit format: "QUOTE-BASE" (e.g., "KRW-BTC" -> "BTC/KRW")
    fn parse_market_id(&self, market_id: &str) -> (String, String, String) {
        let parts: Vec<&str> = market_id.split('-').collect();
        if parts.len() == 2 {
            let quote = parts[0].to_string();
            let base = parts[1].to_string();
            let symbol = format!("{}/{}", base, quote);
            (symbol, base, quote)
        } else {
            (market_id.to_string(), market_id.to_string(), "KRW".to_string())
        }
    }

    /// 통합 심볼을 Upbit 마켓 ID로 변환
    fn to_market_id(&self, symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            format!("{}-{}", parts[1], parts[0])
        } else {
            symbol.to_string()
        }
    }

    /// JWT 토큰 생성 (인증용)
    fn create_jwt(&self, query_params: Option<&HashMap<String, String>>) -> CcxtResult<String> {
        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".to_string(),
        })?;
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".to_string(),
        })?;

        let nonce = Uuid::new_v4().to_string();

        // Build JWT payload
        let mut payload = serde_json::json!({
            "access_key": api_key,
            "nonce": nonce,
        });

        // Add query hash if params present
        if let Some(params) = query_params {
            if !params.is_empty() {
                let query_string = self.build_query_string(params);
                let mut hasher = Sha512::new();
                hasher.update(query_string.as_bytes());
                let query_hash = hex::encode(hasher.finalize());

                payload["query_hash"] = serde_json::Value::String(query_hash);
                payload["query_hash_alg"] = serde_json::Value::String("SHA512".to_string());
            }
        }

        // Create JWT header
        let header = serde_json::json!({
            "alg": "HS256",
            "typ": "JWT"
        });

        // Encode header and payload
        let header_b64 = BASE64.encode(header.to_string().as_bytes());
        let payload_b64 = BASE64.encode(payload.to_string().as_bytes());

        // Create signature
        let message = format!("{}.{}", header_b64, payload_b64);

        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Invalid secret key".to_string(),
            })?;
        mac.update(message.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        Ok(format!("{}.{}.{}", header_b64, payload_b64, signature))
    }

    /// 쿼리 문자열 생성
    fn build_query_string(&self, params: &HashMap<String, String>) -> String {
        let mut pairs: Vec<String> = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        pairs.sort();
        pairs.join("&")
    }

    /// Private API 요청
    async fn private_request(
        &self,
        method: &str,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let jwt = self.create_jwt(params.as_ref())?;

        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), format!("Bearer {}", jwt));

        let url = format!("{}/v1{}", BASE_URL, path);

        let response: serde_json::Value = match method {
            "GET" => {
                let query = params.map(|p| self.build_query_string(&p));
                let full_url = if let Some(q) = query {
                    format!("{}?{}", url, q)
                } else {
                    url
                };
                self.client.get(&full_url, Some(headers), None).await?
            }
            "POST" => {
                let body_json = params.map(|p| serde_json::to_value(p).unwrap_or(serde_json::json!({})));
                headers.insert("Content-Type".to_string(), "application/json".to_string());
                self.client.post(&url, body_json, Some(headers)).await?
            }
            "DELETE" => {
                let query = params.map(|p| self.build_query_string(&p));
                let full_url = if let Some(q) = query {
                    format!("{}?{}", url, q)
                } else {
                    url
                };
                self.client.delete(&full_url, None, Some(headers)).await?
            }
            _ => return Err(CcxtError::ExchangeError {
                message: format!("Unsupported method: {}", method),
            }),
        };

        // Check for errors
        if let Some(error) = response.get("error") {
            let message = error.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            let name = error.get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("unknown");

            return Err(match name {
                "invalid_access_key" | "jwt_verification" => CcxtError::AuthenticationError {
                    message: message.to_string(),
                },
                "insufficient_funds" => CcxtError::InsufficientFunds {
                    currency: "".to_string(),
                    required: "".to_string(),
                    available: "".to_string(),
                },
                "order_not_found" => CcxtError::OrderNotFound {
                    order_id: "unknown".to_string(),
                },
                _ => CcxtError::ExchangeError {
                    message: format!("{}: {}", name, message),
                },
            });
        }

        Ok(response)
    }

    /// Public API 요청
    async fn public_request(&self, path: &str, params: Option<HashMap<String, String>>) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let url = format!("{}/v1{}", BASE_URL, path);
        let query = params.map(|p| self.build_query_string(&p));
        let full_url = if let Some(q) = query {
            format!("{}?{}", url, q)
        } else {
            url
        };

        let response: serde_json::Value = self.client.get(&full_url, None, None).await?;

        // Check for errors
        if let Some(error) = response.get("error") {
            let message = error.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            return Err(CcxtError::ExchangeError {
                message: message.to_string(),
            });
        }

        Ok(response)
    }

    /// 주문 상태 파싱
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status {
            "wait" => OrderStatus::Open,
            "watch" => OrderStatus::Open,
            "done" => OrderStatus::Closed,
            "cancel" => OrderStatus::Canceled,
            _ => OrderStatus::Open, // Default to Open for unknown statuses
        }
    }

    /// 주문 파싱
    fn parse_order(&self, order: &UpbitOrder) -> CcxtResult<Order> {
        let (symbol, _, _) = self.parse_market_id(&order.market);

        let side = match order.side.as_str() {
            "bid" => OrderSide::Buy,
            "ask" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match order.ord_type.as_str() {
            "limit" => OrderType::Limit,
            "price" | "market" | "best" => OrderType::Market,
            _ => OrderType::Limit,
        };

        let price = order.price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let amount = order.volume.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let remaining = order.remaining_volume.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let filled = order.executed_volume.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let fee_cost = order.paid_fee.as_ref()
            .and_then(|f| Decimal::from_str(f).ok());

        let timestamp = chrono::DateTime::parse_from_rfc3339(&order.created_at)
            .map(|dt| dt.timestamp_millis())
            .ok();

        Ok(Order {
            id: order.uuid.clone(),
            client_order_id: None,
            timestamp,
            datetime: Some(order.created_at.clone()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: self.parse_order_status(&order.state),
            symbol,
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount: amount.unwrap_or(Decimal::ZERO),
            filled: filled.unwrap_or(Decimal::ZERO),
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: vec![],
            reduce_only: None,
            post_only: None,
            fee: fee_cost.map(|cost| Fee {
                cost: Some(cost),
                currency: None,
                rate: None,
            }),
            fees: vec![],
            info: serde_json::Value::Null,
        })
    }
}

#[async_trait]
impl Exchange for Upbit {
    fn id(&self) -> ExchangeId {
        ExchangeId::Upbit
    }

    fn name(&self) -> &str {
        "Upbit"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["KR"]
    }

    fn rate_limit(&self) -> u64 {
        RATE_LIMIT_MS
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
        let (symbol, _, _) = self.parse_market_id(market_id);
        Some(symbol)
    }

    fn sign(
        &self,
        path: &str,
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut final_headers = headers.unwrap_or_default();

        if api == "private" {
            if let Ok(jwt) = self.create_jwt(Some(params)) {
                final_headers.insert("Authorization".to_string(), format!("Bearer {}", jwt));
            }
        }

        let query = if !params.is_empty() && method == "GET" {
            Some(self.build_query_string(params))
        } else {
            None
        };

        let url = format!("{}/v1{}", BASE_URL, path);
        let full_url = if let Some(q) = &query {
            format!("{}?{}", url, q)
        } else {
            url
        };

        SignedRequest {
            url: full_url,
            method: method.to_string(),
            headers: final_headers,
            body: None,
        }
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        {
            let markets = self.markets.read().unwrap();
            if !reload && !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let fetched_markets = self.fetch_markets().await?;
        let mut markets_map = HashMap::new();
        let mut markets_by_id_map = HashMap::new();

        for market in fetched_markets {
            markets_by_id_map.insert(market.id.clone(), market.symbol.clone());
            markets_map.insert(market.symbol.clone(), market);
        }

        {
            let mut markets = self.markets.write().unwrap();
            *markets = markets_map.clone();
        }
        {
            let mut markets_by_id = self.markets_by_id.write().unwrap();
            *markets_by_id = markets_by_id_map;
        }

        Ok(markets_map)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response = self.public_request("/market/all", None).await?;

        let upbit_markets: Vec<UpbitMarket> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "markets".to_string(),
                message: format!("Failed to parse markets: {}", e),
            })?;

        let mut markets = Vec::new();

        for m in upbit_markets {
            let (symbol, base, quote) = self.parse_market_id(&m.market);

            // Default fees for Upbit
            let (maker, taker) = if quote == "KRW" {
                (Decimal::from_str("0.0005").unwrap(), Decimal::from_str("0.0005").unwrap())
            } else {
                (Decimal::from_str("0.0025").unwrap(), Decimal::from_str("0.0025").unwrap())
            };

            let market = Market {
                id: m.market.clone(),
                lowercase_id: None,
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                active: m.market_warning.as_deref() != Some("CAUTION"),
                market_type: MarketType::Spot,
                spot: true,
                margin: false,
                swap: false,
                future: false,
                option: false,
                index: false,
                contract: false,
                settle: None,
                settle_id: None,
                contract_size: None,
                linear: None,
                inverse: None,
                sub_type: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(8),
                    price: Some(8),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits {
                    amount: MinMax { min: None, max: None },
                    price: MinMax { min: None, max: None },
                    cost: MinMax { min: None, max: None },
                    leverage: MinMax { min: None, max: None },
                },
                margin_modes: None,
                created: None,
                info: serde_json::Value::Null,
                maker: Some(maker),
                taker: Some(taker),
                percentage: true,
                tier_based: false,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, crate::types::Currency>> {
        // Upbit doesn't have a dedicated currencies endpoint
        // Extract from markets
        let markets = self.fetch_markets().await?;
        let mut currencies = HashMap::new();

        for market in markets {
            if !currencies.contains_key(&market.base) {
                currencies.insert(market.base.clone(), crate::types::Currency {
                    id: market.base_id.clone(),
                    code: market.base.clone(),
                    name: None,
                    active: true,
                    deposit: None,
                    withdraw: None,
                    fee: None,
                    precision: Some(8),
                    limits: Some(crate::types::CurrencyLimits {
                        withdraw: MinMax { min: None, max: None },
                        deposit: MinMax { min: None, max: None },
                    }),
                    networks: HashMap::new(),
                    info: serde_json::Value::Null,
                });
            }
            if !currencies.contains_key(&market.quote) {
                currencies.insert(market.quote.clone(), crate::types::Currency {
                    id: market.quote_id.clone(),
                    code: market.quote.clone(),
                    name: None,
                    active: true,
                    deposit: None,
                    withdraw: None,
                    fee: None,
                    precision: Some(8),
                    limits: Some(crate::types::CurrencyLimits {
                        withdraw: MinMax { min: None, max: None },
                        deposit: MinMax { min: None, max: None },
                    }),
                    networks: HashMap::new(),
                    info: serde_json::Value::Null,
                });
            }
        }

        Ok(currencies)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("markets".to_string(), market_id);

        let response = self.public_request("/ticker", Some(params)).await?;

        let tickers: Vec<UpbitTicker> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "ticker".to_string(),
                message: format!("Failed to parse ticker: {}", e),
            })?;

        let ticker = tickers.into_iter().next()
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;

        let (parsed_symbol, _, _) = self.parse_market_id(&ticker.market);

        Ok(Ticker {
            symbol: parsed_symbol,
            timestamp: ticker.trade_timestamp,
            datetime: ticker.trade_timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            high: ticker.high_price,
            low: ticker.low_price,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: ticker.opening_price,
            close: ticker.trade_price,
            last: ticker.trade_price,
            previous_close: ticker.prev_closing_price,
            change: ticker.signed_change_price,
            percentage: ticker.signed_change_rate.map(|r| r * Decimal::from(100)),
            average: None,
            base_volume: ticker.acc_trade_volume_24h,
            quote_volume: ticker.acc_trade_price_24h,
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let market_ids: String = if let Some(syms) = symbols {
            syms.iter()
                .map(|s| self.to_market_id(s))
                .collect::<Vec<_>>()
                .join(",")
        } else {
            // Fetch all markets first
            let markets = self.load_markets(false).await?;
            markets.keys()
                .map(|s| self.to_market_id(s))
                .collect::<Vec<_>>()
                .join(",")
        };

        let mut params = HashMap::new();
        params.insert("markets".to_string(), market_ids);

        let response = self.public_request("/ticker", Some(params)).await?;

        let upbit_tickers: Vec<UpbitTicker> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "tickers".to_string(),
                message: format!("Failed to parse tickers: {}", e),
            })?;

        let mut result = HashMap::new();

        for ticker in upbit_tickers {
            let (symbol, _, _) = self.parse_market_id(&ticker.market);

            result.insert(symbol.clone(), Ticker {
                symbol,
                timestamp: ticker.trade_timestamp,
                datetime: ticker.trade_timestamp.map(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                high: ticker.high_price,
                low: ticker.low_price,
                bid: None,
                bid_volume: None,
                ask: None,
                ask_volume: None,
                vwap: None,
                open: ticker.opening_price,
                close: ticker.trade_price,
                last: ticker.trade_price,
                previous_close: ticker.prev_closing_price,
                change: ticker.signed_change_price,
                percentage: ticker.signed_change_rate.map(|r| r * Decimal::from(100)),
                average: None,
                base_volume: ticker.acc_trade_volume_24h,
                quote_volume: ticker.acc_trade_price_24h,
                index_price: None,
                mark_price: None,
                info: serde_json::Value::Null,
            });
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("markets".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("level".to_string(), l.to_string());
        }

        let response = self.public_request("/orderbook", Some(params)).await?;

        let orderbooks: Vec<UpbitOrderBook> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "orderbook".to_string(),
                message: format!("Failed to parse orderbook: {}", e),
            })?;

        let ob = orderbooks.into_iter().next()
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        for unit in ob.orderbook_units {
            bids.push(OrderBookEntry {
                price: unit.bid_price,
                amount: unit.bid_size,
            });
            asks.push(OrderBookEntry {
                price: unit.ask_price,
                amount: unit.ask_size,
            });
        }

        // Sort: bids descending, asks ascending
        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

        let (parsed_symbol, _, _) = self.parse_market_id(&ob.market);

        Ok(OrderBook {
            symbol: parsed_symbol,
            bids,
            asks,
            timestamp: Some(ob.timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(ob.timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()),
            nonce: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market".to_string(), market_id);

        if let Some(l) = limit {
            params.insert("count".to_string(), l.min(200).to_string());
        }

        let response = self.public_request("/trades/ticks", Some(params)).await?;

        let upbit_trades: Vec<UpbitTrade> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "trades".to_string(),
                message: format!("Failed to parse trades: {}", e),
            })?;

        let mut trades = Vec::new();

        for t in upbit_trades {
            let (parsed_symbol, _, _) = self.parse_market_id(&t.market);

            let side = if t.ask_bid == "ASK" {
                OrderSide::Sell
            } else {
                OrderSide::Buy
            };

            let trade = Trade {
                id: t.sequential_id.to_string(),
                order: None,
                timestamp: Some(t.timestamp),
                datetime: Some(format!("{}T{}Z", t.trade_date_utc, t.trade_time_utc)),
                symbol: parsed_symbol,
                trade_type: None,
                side: Some(match side {
                    OrderSide::Buy => "buy".to_string(),
                    OrderSide::Sell => "sell".to_string(),
                }),
                taker_or_maker: Some(TakerOrMaker::Taker),
                price: t.trade_price,
                amount: t.trade_volume,
                cost: Some(t.trade_price * t.trade_volume),
                fee: None,
                fees: vec![],
                info: serde_json::Value::Null,
            };

            // Filter by since if provided
            if let Some(since_ts) = since {
                if t.timestamp < since_ts {
                    continue;
                }
            }

            trades.push(trade);
        }

        // Reverse to get oldest first
        trades.reverse();

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
        let tf_str = self.timeframes.get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {:?}", timeframe),
            })?;

        let mut params = HashMap::new();
        params.insert("market".to_string(), market_id);

        if let Some(l) = limit {
            params.insert("count".to_string(), l.min(200).to_string());
        }

        // Use 'to' parameter for historical data
        if let Some(since_ts) = since {
            let to_time = chrono::DateTime::from_timestamp_millis(since_ts)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string());
            if let Some(t) = to_time {
                params.insert("to".to_string(), t);
            }
        }

        let path = format!("/candles/{}", tf_str);
        let response = self.public_request(&path, Some(params)).await?;

        let candles: Vec<UpbitCandle> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "OHLCV".to_string(),
                message: format!("Failed to parse OHLCV: {}", e),
            })?;

        let mut ohlcvs: Vec<OHLCV> = candles.iter().map(|c| {
            OHLCV {
                timestamp: c.timestamp,
                open: c.opening_price,
                high: c.high_price,
                low: c.low_price,
                close: c.trade_price,
                volume: c.candle_acc_trade_volume,
            }
        }).collect();

        // Reverse to get oldest first
        ohlcvs.reverse();

        Ok(ohlcvs)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response = self.private_request("GET", "/accounts", None).await?;

        let upbit_balances: Vec<UpbitBalance> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "balance".to_string(),
                message: format!("Failed to parse balance: {}", e),
            })?;

        let mut currencies = HashMap::new();

        for b in upbit_balances {
            let free = Decimal::from_str(&b.balance).unwrap_or(Decimal::ZERO);
            let used = Decimal::from_str(&b.locked).unwrap_or(Decimal::ZERO);
            let total = free + used;

            currencies.insert(b.currency.clone(), Balance {
                free: Some(free),
                used: Some(used),
                total: Some(total),
                debt: None,
            });
        }

        Ok(Balances {
            info: serde_json::Value::Null,
            timestamp: None,
            datetime: None,
            currencies,
        })
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
        params.insert("market".to_string(), market_id);
        params.insert("side".to_string(), match side {
            OrderSide::Buy => "bid".to_string(),
            OrderSide::Sell => "ask".to_string(),
        });

        match order_type {
            OrderType::Limit => {
                let p = price.ok_or_else(|| CcxtError::BadRequest {
                    message: "Price required for limit order".to_string(),
                })?;
                params.insert("ord_type".to_string(), "limit".to_string());
                params.insert("price".to_string(), p.to_string());
                params.insert("volume".to_string(), amount.to_string());
            }
            OrderType::Market => {
                match side {
                    OrderSide::Buy => {
                        // Market buy uses 'price' as total cost
                        let cost = price.ok_or_else(|| CcxtError::BadRequest {
                            message: "Cost (price) required for market buy order".to_string(),
                        })?;
                        params.insert("ord_type".to_string(), "price".to_string());
                        params.insert("price".to_string(), cost.to_string());
                    }
                    OrderSide::Sell => {
                        params.insert("ord_type".to_string(), "market".to_string());
                        params.insert("volume".to_string(), amount.to_string());
                    }
                }
            }
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type {:?}", order_type),
                });
            }
        }

        let response = self.private_request("POST", "/orders", Some(params)).await?;

        let order: UpbitOrder = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "order".to_string(),
                message: format!("Failed to parse order: {}", e),
            })?;

        self.parse_order(&order)
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("uuid".to_string(), id.to_string());

        let response = self.private_request("DELETE", "/order", Some(params)).await?;

        let order: UpbitOrder = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "order".to_string(),
                message: format!("Failed to parse order: {}", e),
            })?;

        self.parse_order(&order)
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("uuid".to_string(), id.to_string());

        let response = self.private_request("GET", "/order", Some(params)).await?;

        let order: UpbitOrder = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "order".to_string(),
                message: format!("Failed to parse order: {}", e),
            })?;

        self.parse_order(&order)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("state".to_string(), "wait".to_string());

        if let Some(s) = symbol {
            params.insert("market".to_string(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response = self.private_request("GET", "/orders", Some(params)).await?;

        let orders: Vec<UpbitOrder> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "orders".to_string(),
                message: format!("Failed to parse orders: {}", e),
            })?;

        orders.iter().map(|o| self.parse_order(o)).collect()
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("state".to_string(), "done".to_string());

        if let Some(s) = symbol {
            params.insert("market".to_string(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response = self.private_request("GET", "/orders", Some(params)).await?;

        let orders: Vec<UpbitOrder> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "orders".to_string(),
                message: format!("Failed to parse orders: {}", e),
            })?;

        orders.iter().map(|o| self.parse_order(o)).collect()
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        // Upbit doesn't have a dedicated my trades endpoint
        // Use orders with trades_count > 0
        let orders = self.fetch_closed_orders(symbol, None, limit).await?;

        let trades: Vec<Trade> = orders.iter()
            .filter_map(|o| {
                if o.filled == Decimal::ZERO {
                    return None;
                }
                Some(Trade {
                    id: o.id.clone(),
                    order: Some(o.id.clone()),
                    timestamp: o.timestamp,
                    datetime: o.datetime.clone(),
                    symbol: o.symbol.clone(),
                    trade_type: None,
                    side: Some(match o.side {
                        OrderSide::Buy => "buy".to_string(),
                        OrderSide::Sell => "sell".to_string(),
                    }),
                    taker_or_maker: Some(TakerOrMaker::Taker),
                    price: o.price.unwrap_or(Decimal::ZERO),
                    amount: o.filled,
                    cost: Some(o.price.unwrap_or(Decimal::ZERO) * o.filled),
                    fee: o.fee.clone(),
                    fees: vec![],
                    info: serde_json::Value::Null,
                })
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("currency".to_string(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let _response = self.private_request("GET", "/deposits", Some(params)).await?;

        // Parse deposits - simplified for now
        Ok(vec![])
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("currency".to_string(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let _response = self.private_request("GET", "/withdraws", Some(params)).await?;

        // Parse withdrawals - simplified for now
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Upbit::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::Upbit);
        assert_eq!(exchange.name(), "Upbit");
        assert_eq!(exchange.version(), "v1");
        assert!(exchange.has().spot);
        assert!(!exchange.has().margin);
        assert!(!exchange.has().swap);
    }

    #[test]
    fn test_market_id_conversion() {
        let config = ExchangeConfig::default();
        let exchange = Upbit::new(config).unwrap();

        // Upbit uses QUOTE-BASE format
        let (symbol, base, quote) = exchange.parse_market_id("KRW-BTC");
        assert_eq!(symbol, "BTC/KRW");
        assert_eq!(base, "BTC");
        assert_eq!(quote, "KRW");

        // Reverse conversion
        assert_eq!(exchange.to_market_id("BTC/KRW"), "KRW-BTC");
        assert_eq!(exchange.to_market_id("ETH/BTC"), "BTC-ETH");
    }

    #[test]
    fn test_rate_limit() {
        let config = ExchangeConfig::default();
        let exchange = Upbit::new(config).unwrap();
        assert_eq!(exchange.rate_limit(), 50);
    }
}
