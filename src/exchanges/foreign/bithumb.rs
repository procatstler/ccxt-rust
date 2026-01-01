//! Bithumb Exchange Implementation
//!
//! 빗썸 거래소 API 구현 (한국 2위 암호화폐 거래소)

#![allow(dead_code)]

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::Deserialize;
use sha2::Sha512;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use uuid::Uuid;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, Fee, OHLCV, TakerOrMaker,
};

const BASE_URL: &str = "https://api.bithumb.com";
const RATE_LIMIT_MS: u64 = 500;

/// Bithumb 거래소 구조체
pub struct Bithumb {
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
struct BithumbResponse<T> {
    status: String,
    data: Option<T>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct BithumbTicker {
    opening_price: Option<String>,
    closing_price: Option<String>,
    min_price: Option<String>,
    max_price: Option<String>,
    units_traded: Option<String>,
    acc_trade_value: Option<String>,
    prev_closing_price: Option<String>,
    units_traded_24H: Option<String>,
    acc_trade_value_24H: Option<String>,
    fluctate_24H: Option<String>,
    fluctate_rate_24H: Option<String>,
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BithumbOrderBookItem {
    price: String,
    quantity: String,
}

#[derive(Debug, Deserialize)]
struct BithumbOrderBook {
    timestamp: String,
    order_currency: String,
    payment_currency: String,
    bids: Vec<BithumbOrderBookItem>,
    asks: Vec<BithumbOrderBookItem>,
}

#[derive(Debug, Deserialize)]
struct BithumbTradeItem {
    transaction_date: String,
    #[serde(rename = "type")]
    trade_type: String,
    units_traded: String,
    price: String,
    total: String,
}

#[derive(Debug, Deserialize)]
struct BithumbCandle {
    // [timestamp, open, close, high, low, volume]
    #[serde(rename = "0")]
    timestamp: i64,
    #[serde(rename = "1")]
    open: String,
    #[serde(rename = "2")]
    close: String,
    #[serde(rename = "3")]
    high: String,
    #[serde(rename = "4")]
    low: String,
    #[serde(rename = "5")]
    volume: String,
}

#[derive(Debug, Deserialize)]
struct BithumbBalance {
    total_krw: Option<String>,
    in_use_krw: Option<String>,
    available_krw: Option<String>,
    #[serde(flatten)]
    currencies: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct BithumbOrder {
    order_id: Option<String>,
    order_currency: Option<String>,
    payment_currency: Option<String>,
    order_date: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<String>,
    watch_price: Option<String>,
    units: Option<String>,
    units_remaining: Option<String>,
    price: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BithumbOrderResult {
    order_id: Option<String>,
    status: Option<String>,
}

impl Bithumb {
    /// 새 Bithumb 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), format!("{}/public", BASE_URL));
        api_urls.insert("private".into(), BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/30597177-ea800172-9d5e-11e7-804c-b9d4fa9b56b0.jpg".into()),
            api: api_urls,
            www: Some("https://www.bithumb.com".into()),
            doc: vec!["https://apidocs.bithumb.com/".into()],
            fees: Some("https://www.bithumb.com/customer/fee".into()),
        };

        let features = ExchangeFeatures {
            cors: true,
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
            cancel_all_orders: false,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: false,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
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

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".to_string());
        timeframes.insert(Timeframe::Minute3, "3m".to_string());
        timeframes.insert(Timeframe::Minute5, "5m".to_string());
        timeframes.insert(Timeframe::Minute15, "15m".to_string());
        timeframes.insert(Timeframe::Minute30, "30m".to_string());
        timeframes.insert(Timeframe::Hour1, "1h".to_string());
        timeframes.insert(Timeframe::Hour6, "6h".to_string());
        timeframes.insert(Timeframe::Hour12, "12h".to_string());
        timeframes.insert(Timeframe::Day1, "24h".to_string());

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

    /// 숫자 문자열에서 콤마 제거
    fn parse_number(&self, s: &str) -> Decimal {
        let cleaned = s.replace(",", "");
        Decimal::from_str(&cleaned).unwrap_or(Decimal::ZERO)
    }

    /// 심볼을 Bithumb 마켓 ID로 변환
    fn to_market_parts(&self, symbol: &str) -> (String, String) {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            (symbol.to_string(), "KRW".to_string())
        }
    }

    /// HMAC-SHA512 서명 생성 (null-byte 분리)
    fn create_signature(&self, endpoint: &str, params: &HashMap<String, String>) -> CcxtResult<(String, String, String)> {
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".to_string(),
        })?;

        let nonce = Utc::now().timestamp_millis().to_string();

        // URL encode params with endpoint
        let mut body_params = params.clone();
        body_params.insert("endpoint".to_string(), endpoint.to_string());

        let body = self.url_encode(&body_params);

        // Auth string: endpoint + \0 + body + \0 + nonce
        let auth = format!("{}\x00{}\x00{}", endpoint, body, nonce);

        type HmacSha512 = Hmac<Sha512>;
        let mut mac = HmacSha512::new_from_slice(secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Invalid secret key".to_string(),
            })?;
        mac.update(auth.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        Ok((signature, nonce, body))
    }

    /// URL 인코딩
    fn url_encode(&self, params: &HashMap<String, String>) -> String {
        params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Private API 요청
    async fn private_request(
        &self,
        endpoint: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".to_string(),
        })?;

        let (signature, nonce, body) = self.create_signature(endpoint, &params)?;

        let mut headers = HashMap::new();
        headers.insert("Api-Key".to_string(), api_key.to_string());
        headers.insert("Api-Sign".to_string(), signature);
        headers.insert("Api-Nonce".to_string(), nonce);
        headers.insert("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string());
        headers.insert("Accept".to_string(), "application/json".to_string());

        let url = format!("{}{}", BASE_URL, endpoint);
        let body_json = serde_json::json!(body);
        let response: serde_json::Value = self.client.post(&url, Some(body_json), Some(headers)).await?;

        // Check for errors
        let status = response.get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("0000");

        if status != "0000" {
            let message = response.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");

            // Skip "no trading history" error
            if message.contains("거래 진행중인 내역이 존재하지 않습니다") {
                return Ok(serde_json::json!({"data": []}));
            }

            return Err(match status {
                "5100" => CcxtError::BadRequest { message: message.to_string() },
                "5300" => CcxtError::AuthenticationError { message: message.to_string() },
                _ => CcxtError::ExchangeError { message: format!("{}: {}", status, message) },
            });
        }

        Ok(response)
    }

    /// Public API 요청
    async fn public_request(&self, path: &str) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let url = format!("{}/public{}", BASE_URL, path);
        let response: serde_json::Value = self.client.get(&url, None, None).await?;

        let status = response.get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("0000");

        if status != "0000" {
            let message = response.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            return Err(CcxtError::ExchangeError {
                message: format!("{}: {}", status, message),
            });
        }

        Ok(response)
    }

    /// 주문 상태 파싱
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status.to_lowercase().as_str() {
            "pending" => OrderStatus::Open,
            "completed" => OrderStatus::Closed,
            "cancel" => OrderStatus::Canceled,
            "expired" => OrderStatus::Expired,
            "rejected" => OrderStatus::Rejected,
            _ => OrderStatus::Rejected,
        }
    }
}

#[async_trait]
impl Exchange for Bithumb {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bithumb
    }

    fn name(&self) -> &str {
        "Bithumb"
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
        let (base, _) = self.to_market_parts(symbol);
        Some(base)
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        // Bithumb market IDs are just base currency
        // Default to KRW quote
        Some(format!("{}/KRW", market_id))
    }

    fn sign(
        &self,
        path: &str,
        api: &str,
        _method: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut final_headers = headers.unwrap_or_default();

        if api == "private" {
            if let Ok((signature, nonce, _body)) = self.create_signature(path, params) {
                if let Some(api_key) = self.config.api_key() {
                    final_headers.insert("Api-Key".to_string(), api_key.to_string());
                    final_headers.insert("Api-Sign".to_string(), signature);
                    final_headers.insert("Api-Nonce".to_string(), nonce);
                    final_headers.insert("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string());
                }
            }
        }

        SignedRequest {
            url: format!("{}{}", BASE_URL, path),
            method: if api == "private" { "POST".to_string() } else { "GET".to_string() },
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
        let mut all_markets = Vec::new();

        // Fetch tickers for each quote currency (KRW, BTC, USDT)
        for quote in &["KRW", "BTC", "USDT"] {
            let response = self.public_request(&format!("/ticker/ALL_{}", quote)).await;

            if let Ok(resp) = response {
                if let Some(data) = resp.get("data") {
                    if let Some(obj) = data.as_object() {
                        for (base, _ticker_data) in obj {
                            if base == "date" {
                                continue;
                            }

                            let symbol = format!("{}/{}", base, quote);
                            let market_id = base.to_string();

                            // Set limits based on quote currency
                            let (min_cost, max_cost) = match *quote {
                                "KRW" => (Some(Decimal::from(500)), Some(Decimal::from(5_000_000_000i64))),
                                "BTC" => (Some(Decimal::from_str("0.0002").unwrap()), Some(Decimal::from(100))),
                                _ => (None, None),
                            };

                            let market = Market {
                                id: market_id,
                                lowercase_id: None,
                                symbol: symbol.clone(),
                                base: base.to_string(),
                                quote: quote.to_string(),
                                settle: None,
                                base_id: base.to_string(),
                                quote_id: quote.to_string(),
                                settle_id: None,
                                market_type: MarketType::Spot,
                                spot: true,
                                margin: false,
                                swap: false,
                                future: false,
                                option: false,
                                index: false,
                                active: true,
                                contract: false,
                                linear: None,
                                inverse: None,
                                sub_type: None,
                                taker: Some(Decimal::from_str("0.0025").unwrap()),
                                maker: Some(Decimal::from_str("0.0025").unwrap()),
                                contract_size: None,
                                expiry: None,
                                expiry_datetime: None,
                                strike: None,
                                option_type: None,
                                precision: MarketPrecision {
                                    amount: Some(4),
                                    price: Some(4),
                                    cost: None,
                                    base: None,
                                    quote: None,
                                },
                                limits: MarketLimits {
                                    amount: MinMax { min: None, max: None },
                                    price: MinMax { min: None, max: None },
                                    cost: MinMax { min: min_cost, max: max_cost },
                                    leverage: MinMax { min: None, max: None },
                                },
                                margin_modes: None,
                                created: None,
                                info: serde_json::Value::Null,
                                tier_based: false,
                                percentage: true,
                            };

                            all_markets.push(market);
                        }
                    }
                }
            }
        }

        Ok(all_markets)
    }

    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, crate::types::Currency>> {
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
        }

        // Add fiat
        currencies.insert("KRW".to_string(), crate::types::Currency {
            id: "KRW".to_string(),
            code: "KRW".to_string(),
            name: Some("South Korean Won".to_string()),
            active: true,
            deposit: None,
            withdraw: None,
            fee: None,
            precision: Some(0),
            limits: Some(crate::types::CurrencyLimits {
                withdraw: MinMax { min: None, max: None },
                deposit: MinMax { min: None, max: None },
            }),
            networks: HashMap::new(),
            info: serde_json::Value::Null,
        });

        Ok(currencies)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let (base, quote) = self.to_market_parts(symbol);
        let path = format!("/ticker/{}_{}", base, quote);

        let response = self.public_request(&path).await?;

        let data = response.get("data")
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "ticker".to_string(),
                message: "Missing data field".to_string(),
            })?;

        let timestamp = data.get("date")
            .and_then(|d| d.as_str())
            .and_then(|d| d.parse::<i64>().ok());

        let open = data.get("opening_price")
            .and_then(|v| v.as_str())
            .map(|s| self.parse_number(s));
        let close = data.get("closing_price")
            .and_then(|v| v.as_str())
            .map(|s| self.parse_number(s));
        let high = data.get("max_price")
            .and_then(|v| v.as_str())
            .map(|s| self.parse_number(s));
        let low = data.get("min_price")
            .and_then(|v| v.as_str())
            .map(|s| self.parse_number(s));
        let volume = data.get("units_traded_24H")
            .and_then(|v| v.as_str())
            .map(|s| self.parse_number(s));
        let quote_volume = data.get("acc_trade_value_24H")
            .and_then(|v| v.as_str())
            .map(|s| self.parse_number(s));
        let change = data.get("fluctate_24H")
            .and_then(|v| v.as_str())
            .map(|s| self.parse_number(s));
        let percentage = data.get("fluctate_rate_24H")
            .and_then(|v| v.as_str())
            .map(|s| self.parse_number(s));
        let prev_close = data.get("prev_closing_price")
            .and_then(|v| v.as_str())
            .map(|s| self.parse_number(s));

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime: timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            high,
            low,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open,
            close,
            last: close,
            previous_close: prev_close,
            change,
            percentage,
            average: None,
            base_volume: volume,
            quote_volume,
            index_price: None,
            mark_price: None,
            info: data.clone(),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let mut result = HashMap::new();

        // Fetch all tickers for each quote currency
        for quote in &["KRW", "BTC", "USDT"] {
            let response = self.public_request(&format!("/ticker/ALL_{}", quote)).await;

            if let Ok(resp) = response {
                if let Some(data) = resp.get("data") {
                    let timestamp = data.get("date")
                        .and_then(|d| d.as_str())
                        .and_then(|d| d.parse::<i64>().ok());

                    if let Some(obj) = data.as_object() {
                        for (base, ticker_data) in obj {
                            if base == "date" {
                                continue;
                            }

                            let symbol = format!("{}/{}", base, quote);

                            // Filter by requested symbols
                            if let Some(syms) = symbols {
                                if !syms.contains(&symbol.as_str()) {
                                    continue;
                                }
                            }

                            let open = ticker_data.get("opening_price")
                                .and_then(|v| v.as_str())
                                .map(|s| self.parse_number(s));
                            let close = ticker_data.get("closing_price")
                                .and_then(|v| v.as_str())
                                .map(|s| self.parse_number(s));
                            let high = ticker_data.get("max_price")
                                .and_then(|v| v.as_str())
                                .map(|s| self.parse_number(s));
                            let low = ticker_data.get("min_price")
                                .and_then(|v| v.as_str())
                                .map(|s| self.parse_number(s));
                            let volume = ticker_data.get("units_traded_24H")
                                .and_then(|v| v.as_str())
                                .map(|s| self.parse_number(s));
                            let quote_volume = ticker_data.get("acc_trade_value_24H")
                                .and_then(|v| v.as_str())
                                .map(|s| self.parse_number(s));

                            result.insert(symbol.clone(), Ticker {
                                symbol,
                                timestamp,
                                datetime: timestamp.map(|ts| {
                                    chrono::DateTime::from_timestamp_millis(ts)
                                        .map(|dt| dt.to_rfc3339())
                                        .unwrap_or_default()
                                }),
                                high,
                                low,
                                bid: None,
                                bid_volume: None,
                                ask: None,
                                ask_volume: None,
                                vwap: None,
                                open,
                                close,
                                last: close,
                                previous_close: None,
                                change: None,
                                percentage: None,
                                average: None,
                                base_volume: volume,
                                quote_volume,
                                index_price: None,
                                mark_price: None,
                                info: serde_json::Value::Null,
                            });
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let (base, quote) = self.to_market_parts(symbol);
        let count = limit.unwrap_or(30).min(30);
        let path = format!("/orderbook/{}_{}", base, quote);

        let response = self.public_request(&path).await?;

        let data = response.get("data")
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "orderbook".to_string(),
                message: "Missing data field".to_string(),
            })?;

        let timestamp = data.get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(|t| t.parse::<i64>().ok());

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        if let Some(bids_arr) = data.get("bids").and_then(|b| b.as_array()) {
            for bid in bids_arr.iter().take(count as usize) {
                let price = bid.get("price")
                    .and_then(|p| p.as_str())
                    .map(|s| self.parse_number(s))
                    .unwrap_or(Decimal::ZERO);
                let amount = bid.get("quantity")
                    .and_then(|q| q.as_str())
                    .map(|s| self.parse_number(s))
                    .unwrap_or(Decimal::ZERO);
                bids.push(OrderBookEntry { price, amount });
            }
        }

        if let Some(asks_arr) = data.get("asks").and_then(|a| a.as_array()) {
            for ask in asks_arr.iter().take(count as usize) {
                let price = ask.get("price")
                    .and_then(|p| p.as_str())
                    .map(|s| self.parse_number(s))
                    .unwrap_or(Decimal::ZERO);
                let amount = ask.get("quantity")
                    .and_then(|q| q.as_str())
                    .map(|s| self.parse_number(s))
                    .unwrap_or(Decimal::ZERO);
                asks.push(OrderBookEntry { price, amount });
            }
        }

        // Sort: bids descending, asks ascending
        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp,
            datetime: timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            nonce: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let (base, quote) = self.to_market_parts(symbol);
        let count = limit.unwrap_or(20).min(100);
        let path = format!("/transaction_history/{}_{}", base, quote);

        let response = self.public_request(&path).await?;

        let data = response.get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "trades".to_string(),
                message: "Missing data array".to_string(),
            })?;

        let mut trades = Vec::new();

        for (idx, item) in data.iter().take(count as usize).enumerate() {
            let price = item.get("price")
                .and_then(|p| p.as_str())
                .map(|s| self.parse_number(s))
                .unwrap_or(Decimal::ZERO);
            let amount = item.get("units_traded")
                .and_then(|u| u.as_str())
                .map(|s| self.parse_number(s))
                .unwrap_or(Decimal::ZERO);
            let cost = item.get("total")
                .and_then(|t| t.as_str())
                .map(|s| self.parse_number(s))
                .unwrap_or(price * amount);

            let trade_type = item.get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("bid");

            let side = if trade_type == "ask" {
                "sell"
            } else {
                "buy"
            };

            // Parse timestamp from transaction_date (Korean timezone - subtract 9 hours)
            let datetime = item.get("transaction_date")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string());

            let timestamp = datetime.as_ref().and_then(|dt| {
                chrono::NaiveDateTime::parse_from_str(dt, "%Y-%m-%d %H:%M:%S")
                    .ok()
                    .map(|ndt| ndt.and_utc().timestamp_millis() - 9 * 3600 * 1000)
            });

            trades.push(Trade {
                id: format!("{}-{}", timestamp.unwrap_or(0), idx),
                order: None,
                timestamp,
                datetime,
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(side.to_string()),
                taker_or_maker: Some(TakerOrMaker::Taker),
                price,
                amount,
                cost: Some(cost),
                fee: None,
                fees: vec![],
                info: item.clone(),
            });
        }

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let (base, quote) = self.to_market_parts(symbol);
        let tf_str = self.timeframes.get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {:?}", timeframe),
            })?;

        let path = format!("/candlestick/{}_{}/{}", base, quote, tf_str);

        let response = self.public_request(&path).await?;

        let data = response.get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "ohlcv".to_string(),
                message: "Missing data array".to_string(),
            })?;

        let mut ohlcvs = Vec::new();
        let count = limit.unwrap_or(200) as usize;

        for item in data.iter().take(count) {
            if let Some(arr) = item.as_array() {
                if arr.len() >= 6 {
                    let timestamp = arr[0].as_i64().unwrap_or(0);
                    let open = arr[1].as_str().map(|s| self.parse_number(s)).unwrap_or(Decimal::ZERO);
                    let close = arr[2].as_str().map(|s| self.parse_number(s)).unwrap_or(Decimal::ZERO);
                    let high = arr[3].as_str().map(|s| self.parse_number(s)).unwrap_or(Decimal::ZERO);
                    let low = arr[4].as_str().map(|s| self.parse_number(s)).unwrap_or(Decimal::ZERO);
                    let volume = arr[5].as_str().map(|s| self.parse_number(s)).unwrap_or(Decimal::ZERO);

                    ohlcvs.push(OHLCV {
                        timestamp,
                        open,
                        high,
                        low,
                        close,
                        volume,
                    });
                }
            }
        }

        Ok(ohlcvs)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let mut params = HashMap::new();
        params.insert("currency".to_string(), "ALL".to_string());

        let response = self.private_request("/info/balance", params).await?;

        let data = response.get("data")
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "balance".to_string(),
                message: "Missing data field".to_string(),
            })?;

        let mut currencies = HashMap::new();

        if let Some(obj) = data.as_object() {
            // Extract currency balances
            let mut currency_list = Vec::new();
            for key in obj.keys() {
                if key.starts_with("total_") {
                    let currency = key.strip_prefix("total_").unwrap().to_uppercase();
                    currency_list.push(currency);
                }
            }

            for currency in currency_list {
                let total_key = format!("total_{}", currency.to_lowercase());
                let available_key = format!("available_{}", currency.to_lowercase());
                let in_use_key = format!("in_use_{}", currency.to_lowercase());

                let total = obj.get(&total_key)
                    .and_then(|v| v.as_str())
                    .map(|s| self.parse_number(s))
                    .unwrap_or(Decimal::ZERO);
                let free = obj.get(&available_key)
                    .and_then(|v| v.as_str())
                    .map(|s| self.parse_number(s))
                    .unwrap_or(Decimal::ZERO);
                let used = obj.get(&in_use_key)
                    .and_then(|v| v.as_str())
                    .map(|s| self.parse_number(s))
                    .unwrap_or(Decimal::ZERO);

                if total > Decimal::ZERO || free > Decimal::ZERO || used > Decimal::ZERO {
                    currencies.insert(currency, Balance {
                        free: Some(free),
                        used: Some(used),
                        total: Some(total),
                        debt: None,
                    });
                }
            }
        }

        Ok(Balances {
            timestamp: None,
            datetime: None,
            currencies,
            info: data.clone(),
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
        let (base, quote) = self.to_market_parts(symbol);

        let endpoint = match (&order_type, &side) {
            (OrderType::Limit, _) => "/trade/place",
            (OrderType::Market, OrderSide::Buy) => "/trade/market_buy",
            (OrderType::Market, OrderSide::Sell) => "/trade/market_sell",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type {:?}", order_type),
            }),
        };

        let mut params = HashMap::new();
        params.insert("order_currency".to_string(), base);
        params.insert("payment_currency".to_string(), quote);
        params.insert("units".to_string(), amount.to_string());

        match order_type {
            OrderType::Limit => {
                let p = price.ok_or_else(|| CcxtError::BadRequest {
                    message: "Price required for limit order".to_string(),
                })?;
                params.insert("price".to_string(), p.to_string());
                params.insert("type".to_string(), match side {
                    OrderSide::Buy => "bid".to_string(),
                    OrderSide::Sell => "ask".to_string(),
                });
            }
            OrderType::Market => {
                // Market orders don't need price
            }
            _ => {}
        }

        let response = self.private_request(endpoint, params).await?;

        let order_id = response.get("order_id")
            .or_else(|| response.get("data").and_then(|d| d.get("order_id")))
            .and_then(|id| id.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        Ok(Order {
            id: order_id,
            client_order_id: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: vec![],
            fee: None,
            fees: vec![],
            reduce_only: None,
            post_only: None,
            info: response,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let (base, quote) = self.to_market_parts(symbol);

        // Bithumb requires side for cancellation, try both
        for order_type in &["bid", "ask"] {
            let mut params = HashMap::new();
            params.insert("order_id".to_string(), id.to_string());
            params.insert("order_currency".to_string(), base.clone());
            params.insert("payment_currency".to_string(), quote.clone());
            params.insert("type".to_string(), order_type.to_string());

            if let Ok(response) = self.private_request("/trade/cancel", params).await {
                return Ok(Order {
                    id: id.to_string(),
                    client_order_id: None,
                    timestamp: None,
                    datetime: None,
                    last_trade_timestamp: None,
                    last_update_timestamp: None,
                    status: OrderStatus::Canceled,
                    symbol: symbol.to_string(),
                    order_type: OrderType::Limit,
                    time_in_force: None,
                    side: if *order_type == "bid" { OrderSide::Buy } else { OrderSide::Sell },
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
                    trades: vec![],
                    fee: None,
                    fees: vec![],
                    reduce_only: None,
                    post_only: None,
                    info: response,
                });
            }
        }

        Err(CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let (base, quote) = self.to_market_parts(symbol);

        let mut params = HashMap::new();
        params.insert("order_id".to_string(), id.to_string());
        params.insert("order_currency".to_string(), base);
        params.insert("payment_currency".to_string(), quote);

        let response = self.private_request("/info/order_detail", params).await?;

        let data = response.get("data")
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "order".to_string(),
                message: "Missing data field".to_string(),
            })?;

        let status = data.get("order_status")
            .and_then(|s| s.as_str())
            .map(|s| self.parse_order_status(s))
            .unwrap_or(OrderStatus::Rejected);

        let side = data.get("type")
            .and_then(|t| t.as_str())
            .map(|t| if t == "bid" { OrderSide::Buy } else { OrderSide::Sell })
            .unwrap_or(OrderSide::Buy);

        let price = data.get("order_price")
            .and_then(|p| p.as_str())
            .map(|s| self.parse_number(s));
        let amount = data.get("order_qty")
            .and_then(|q| q.as_str())
            .map(|s| self.parse_number(s));

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: None,
            datetime: None,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side,
            price,
            average: None,
            amount: amount.unwrap_or(Decimal::ZERO),
            filled: Decimal::ZERO,
            remaining: None,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: vec![],
            fee: None,
            fees: vec![],
            reduce_only: None,
            post_only: None,
            info: data.clone(),
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::BadRequest {
            message: "Symbol required for Bithumb".to_string(),
        })?;

        let (base, quote) = self.to_market_parts(symbol);

        let mut params = HashMap::new();
        params.insert("order_currency".to_string(), base);
        params.insert("payment_currency".to_string(), quote);
        if let Some(l) = limit {
            params.insert("count".to_string(), l.min(1000).to_string());
        }

        let response = self.private_request("/info/orders", params).await?;

        let data = response.get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "orders".to_string(),
                message: "Missing data array".to_string(),
            })?;

        let mut orders = Vec::new();

        for item in data {
            let order_id = item.get("order_id")
                .and_then(|id| id.as_str())
                .unwrap_or("");

            let side = item.get("type")
                .and_then(|t| t.as_str())
                .map(|t| if t == "bid" { OrderSide::Buy } else { OrderSide::Sell })
                .unwrap_or(OrderSide::Buy);

            let price = item.get("price")
                .and_then(|p| p.as_str())
                .map(|s| self.parse_number(s));
            let amount = item.get("units")
                .and_then(|u| u.as_str())
                .map(|s| self.parse_number(s));
            let remaining = item.get("units_remaining")
                .and_then(|u| u.as_str())
                .map(|s| self.parse_number(s));

            let filled = amount.zip(remaining).map(|(a, r)| a - r).unwrap_or(Decimal::ZERO);

            orders.push(Order {
                id: order_id.to_string(),
                client_order_id: None,
                timestamp: None,
                datetime: None,
                last_trade_timestamp: None,
                last_update_timestamp: None,
                status: OrderStatus::Open,
                symbol: symbol.to_string(),
                order_type: OrderType::Limit,
                time_in_force: None,
                side,
                price,
                average: None,
                amount: amount.unwrap_or(Decimal::ZERO),
                filled,
                remaining,
                stop_price: None,
                trigger_price: None,
                take_profit_price: None,
                stop_loss_price: None,
                cost: None,
                trades: vec![],
                fee: None,
                fees: vec![],
                reduce_only: None,
                post_only: None,
                info: item.clone(),
            });
        }

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        Err(CcxtError::NotSupported {
            feature: "fetchClosedOrders".to_string(),
        })
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol = symbol.ok_or_else(|| CcxtError::BadRequest {
            message: "Symbol required for Bithumb".to_string(),
        })?;

        let (base, quote) = self.to_market_parts(symbol);

        let mut params = HashMap::new();
        params.insert("order_currency".to_string(), base);
        params.insert("payment_currency".to_string(), quote);
        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }

        let response = self.private_request("/info/user_transactions", params).await?;

        let data = response.get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "trades".to_string(),
                message: "Missing data array".to_string(),
            })?;

        let mut trades = Vec::new();

        for (idx, item) in data.iter().enumerate() {
            let search_type = item.get("search")
                .and_then(|s| s.as_str())
                .unwrap_or("");

            // Only include buy/sell transactions
            if search_type != "1" && search_type != "2" {
                continue;
            }

            let side_str = if search_type == "1" { "buy" } else { "sell" };

            let price = item.get("price")
                .and_then(|p| p.as_str())
                .map(|s| self.parse_number(s))
                .unwrap_or(Decimal::ZERO);
            let amount = item.get("units")
                .and_then(|u| u.as_str())
                .map(|s| self.parse_number(s))
                .unwrap_or(Decimal::ZERO);
            let fee_cost = item.get("fee")
                .and_then(|f| f.as_str())
                .map(|s| self.parse_number(s).abs());

            let timestamp = item.get("transfer_date")
                .and_then(|d| d.as_str())
                .and_then(|s| s.parse::<i64>().ok());

            trades.push(Trade {
                id: format!("{}-{}", timestamp.unwrap_or(0), idx),
                order: None,
                timestamp,
                datetime: timestamp.map(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(side_str.to_string()),
                taker_or_maker: Some(TakerOrMaker::Taker),
                price,
                amount,
                cost: Some(price * amount),
                fee: fee_cost.map(|c| Fee {
                    cost: Some(c),
                    currency: Some(if side_str == "buy" {
                        symbol.split('/').next().unwrap_or("").to_string()
                    } else {
                        symbol.split('/').nth(1).unwrap_or("").to_string()
                    }),
                    rate: None,
                }),
                fees: vec![],
                info: item.clone(),
            });
        }

        Ok(trades)
    }

    async fn fetch_deposits(
        &self,
        _code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        Ok(vec![])
    }

    async fn fetch_withdrawals(
        &self,
        _code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Bithumb::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::Bithumb);
        assert_eq!(exchange.name(), "Bithumb");
        assert_eq!(exchange.version(), "v1");
        assert!(exchange.has().spot);
        assert!(!exchange.has().margin);
        assert!(!exchange.has().swap);
    }

    #[test]
    fn test_parse_number() {
        let config = ExchangeConfig::default();
        let exchange = Bithumb::new(config).unwrap();

        assert_eq!(exchange.parse_number("1,234.56"), Decimal::from_str("1234.56").unwrap());
        assert_eq!(exchange.parse_number("1000"), Decimal::from(1000));
    }

    #[test]
    fn test_market_parts() {
        let config = ExchangeConfig::default();
        let exchange = Bithumb::new(config).unwrap();

        let (base, quote) = exchange.to_market_parts("BTC/KRW");
        assert_eq!(base, "BTC");
        assert_eq!(quote, "KRW");
    }

    #[test]
    fn test_rate_limit() {
        let config = ExchangeConfig::default();
        let exchange = Bithumb::new(config).unwrap();
        assert_eq!(exchange.rate_limit(), 500);
    }
}
