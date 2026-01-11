//! Kraken Exchange Implementation
//!
//! Kraken 거래소 API 구현

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Fee, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

const BASE_URL: &str = "https://api.kraken.com";
const RATE_LIMIT_MS: u64 = 500;

/// Kraken 거래소 구조체
pub struct Kraken {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Kraken {
    /// 새 Kraken 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), BASE_URL.into());
        api_urls.insert("private".into(), BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27766599-22709304-5ede-11e7-9de1-9f33732e1509.jpg".into()),
            api: api_urls,
            www: Some("https://www.kraken.com".into()),
            doc: vec!["https://docs.kraken.com/rest/".into()],
            fees: Some("https://www.kraken.com/features/fee-schedule".into()),
        };

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: true,
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
            watch_tickers: false,
            watch_order_book: true,
            watch_trades: true,
            watch_ohlcv: true,
            watch_balance: false,
            watch_orders: false,
            watch_my_trades: false,
            ..Default::default()
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".to_string());
        timeframes.insert(Timeframe::Minute5, "5".to_string());
        timeframes.insert(Timeframe::Minute15, "15".to_string());
        timeframes.insert(Timeframe::Minute30, "30".to_string());
        timeframes.insert(Timeframe::Hour1, "60".to_string());
        timeframes.insert(Timeframe::Hour4, "240".to_string());
        timeframes.insert(Timeframe::Day1, "1440".to_string());
        timeframes.insert(Timeframe::Week1, "10080".to_string());

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

    /// Kraken 심볼을 통합 심볼로 변환
    fn to_symbol(&self, kraken_symbol: &str) -> String {
        // Kraken uses special prefixes: X for crypto, Z for fiat
        // Examples: XXBTZUSD = XBT/USD, XETHZEUR = ETH/EUR

        // Known quote currencies with their Kraken formats
        let quote_mappings = [
            ("ZUSD", "USD"),
            ("ZEUR", "EUR"),
            ("ZGBP", "GBP"),
            ("ZJPY", "JPY"),
            ("ZCAD", "CAD"),
            ("ZAUD", "AUD"),
            ("USDT", "USDT"),
            ("USDC", "USDC"),
            ("USD", "USD"),
            ("EUR", "EUR"),
            ("GBP", "GBP"),
            ("JPY", "JPY"),
            ("XXBT", "BTC"),
            ("XETH", "ETH"),
            ("XBT", "BTC"),
            ("ETH", "ETH"),
        ];

        // Try to find a known quote currency at the end
        for (kraken_quote, unified_quote) in &quote_mappings {
            if let Some(base_part) = kraken_symbol.strip_suffix(kraken_quote) {
                // Clean up base: remove X/Z prefix and convert XBT to BTC
                let base = base_part.trim_start_matches('X').trim_start_matches('Z');
                let base = if base == "XBT" || base == "BT" {
                    "BTC"
                } else {
                    base
                };

                // Handle special case where base_part is XXBT -> after trim X -> XBT or BT
                let base = if base_part == "XXBT" { "BTC" } else { base };

                return format!("{base}/{unified_quote}");
            }
        }

        // Fallback: simple split
        let clean = kraken_symbol
            .trim_start_matches('X')
            .trim_start_matches('Z');
        clean.to_string()
    }

    /// 통합 심볼을 Kraken 형식으로 변환
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// 마켓 데이터 파싱
    fn parse_market(&self, id: &str, data: &KrakenAssetPair) -> Market {
        let base = data
            .base
            .as_ref()
            .map(|b| b.trim_start_matches('X').trim_start_matches('Z'))
            .map(|b| if b == "XBT" { "BTC" } else { b })
            .unwrap_or("")
            .to_string();
        let quote = data
            .quote
            .as_ref()
            .map(|q| q.trim_start_matches('X').trim_start_matches('Z'))
            .map(|q| if q == "XBT" { "BTC" } else { q })
            .unwrap_or("")
            .to_string();
        let symbol = format!("{base}/{quote}");

        let price_precision = data.pair_decimals.unwrap_or(8) as i32;
        let amount_precision = data.lot_decimals.unwrap_or(8) as i32;

        Market {
            id: id.to_string(),
            lowercase_id: Some(id.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: data.base.clone().unwrap_or_default(),
            quote_id: data.quote.clone().unwrap_or_default(),
            active: data.status.as_deref() == Some("online"),
            market_type: MarketType::Spot,
            spot: true,
            margin: data.margin_call.is_some(),
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
            taker: data
                .fees
                .as_ref()
                .and_then(|f| f.first())
                .and_then(|f| f.get(1))
                .copied()
                .map(|f| {
                    Decimal::from_str(&f.to_string()).unwrap_or_default() / Decimal::from(100)
                }),
            maker: data
                .fees_maker
                .as_ref()
                .and_then(|f| f.first())
                .and_then(|f| f.get(1))
                .copied()
                .map(|f| {
                    Decimal::from_str(&f.to_string()).unwrap_or_default() / Decimal::from(100)
                }),
            precision: MarketPrecision {
                amount: Some(amount_precision),
                price: Some(price_precision),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: MinMax {
                    min: data
                        .ordermin
                        .and_then(|v| Decimal::from_str(&v.to_string()).ok()),
                    max: None,
                },
                price: MinMax::default(),
                cost: MinMax {
                    min: data
                        .costmin
                        .and_then(|v| Decimal::from_str(&v.to_string()).ok()),
                    max: None,
                },
                leverage: MinMax::default(),
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
    fn parse_ticker(&self, symbol: &str, data: &KrakenTickerData) -> Ticker {
        let timestamp = Utc::now().timestamp_millis();

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: data
                .h
                .as_ref()
                .and_then(|h| h.get(1))
                .and_then(|v| Decimal::from_str(v).ok()),
            low: data
                .l
                .as_ref()
                .and_then(|l| l.get(1))
                .and_then(|v| Decimal::from_str(v).ok()),
            bid: data
                .b
                .as_ref()
                .and_then(|b| b.first())
                .and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: data
                .b
                .as_ref()
                .and_then(|b| b.get(2))
                .and_then(|v| Decimal::from_str(v).ok()),
            ask: data
                .a
                .as_ref()
                .and_then(|a| a.first())
                .and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: data
                .a
                .as_ref()
                .and_then(|a| a.get(2))
                .and_then(|v| Decimal::from_str(v).ok()),
            vwap: data
                .p
                .as_ref()
                .and_then(|p| p.get(1))
                .and_then(|v| Decimal::from_str(v).ok()),
            open: data.o.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            close: data
                .c
                .as_ref()
                .and_then(|c| c.first())
                .and_then(|v| Decimal::from_str(v).ok()),
            last: data
                .c
                .as_ref()
                .and_then(|c| c.first())
                .and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data
                .v
                .as_ref()
                .and_then(|v| v.get(1))
                .and_then(|val| Decimal::from_str(val).ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// OHLCV 데이터 파싱
    fn parse_ohlcv(&self, data: &[serde_json::Value]) -> Option<OHLCV> {
        if data.len() < 7 {
            return None;
        }

        Some(OHLCV {
            timestamp: data[0].as_i64()? * 1000,
            open: Decimal::from_str(data[1].as_str()?).ok()?,
            high: Decimal::from_str(data[2].as_str()?).ok()?,
            low: Decimal::from_str(data[3].as_str()?).ok()?,
            close: Decimal::from_str(data[4].as_str()?).ok()?,
            volume: Decimal::from_str(data[6].as_str().unwrap_or("0"))
                .ok()
                .unwrap_or_default(),
        })
    }

    /// 체결 데이터 파싱
    fn parse_trade(&self, symbol: &str, data: &[serde_json::Value]) -> Option<Trade> {
        if data.len() < 6 {
            return None;
        }

        let price = Decimal::from_str(data[0].as_str()?).ok()?;
        let amount = Decimal::from_str(data[1].as_str()?).ok()?;
        let timestamp = (data[2].as_f64()? * 1000.0) as i64;
        let side = match data[3].as_str() {
            Some("b") => Some("buy".to_string()),
            Some("s") => Some("sell".to_string()),
            _ => None,
        };

        Some(Trade {
            id: data[2].to_string(),
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
            info: serde_json::Value::Array(data.to_vec()),
        })
    }

    /// 주문 데이터 파싱
    fn parse_order(&self, id: &str, data: &KrakenOrderInfo) -> CcxtResult<Order> {
        let status = match data.status.as_deref() {
            Some("pending") => OrderStatus::Open,
            Some("open") => OrderStatus::Open,
            Some("closed") => OrderStatus::Closed,
            Some("canceled") => OrderStatus::Canceled,
            Some("expired") => OrderStatus::Expired,
            Some("rejected") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let descr = data.descr.as_ref();
        let symbol = descr
            .and_then(|d| d.pair.as_ref())
            .map(|p| self.to_symbol(p))
            .unwrap_or_default();

        let side = descr
            .and_then(|d| d.order_type.as_ref())
            .map(|t| {
                if t.contains("buy") {
                    OrderSide::Buy
                } else {
                    OrderSide::Sell
                }
            })
            .unwrap_or(OrderSide::Buy);

        let order_type = descr
            .and_then(|d| d.ordertype.as_ref())
            .map(|t| match t.as_str() {
                "market" => OrderType::Market,
                "limit" => OrderType::Limit,
                "stop-loss" => OrderType::StopLoss,
                "take-profit" => OrderType::TakeProfit,
                "stop-loss-limit" => OrderType::StopLossLimit,
                "take-profit-limit" => OrderType::TakeProfitLimit,
                _ => OrderType::Limit,
            })
            .unwrap_or(OrderType::Limit);

        let price = descr
            .and_then(|d| d.price.as_ref())
            .and_then(|p| Decimal::from_str(p).ok());

        let amount = data
            .vol
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let filled = data
            .vol_exec
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let remaining = amount - filled;
        let cost = data.cost.as_ref().and_then(|c| Decimal::from_str(c).ok());
        let average = if filled > Decimal::ZERO {
            cost.map(|c| c / filled)
        } else {
            None
        };

        let timestamp = data.opentm.map(|t| (t * 1000.0) as i64);
        let fee_cost = data.fee.as_ref().and_then(|f| Decimal::from_str(f).ok());

        Ok(Order {
            id: id.to_string(),
            client_order_id: data.userref.map(|r| r.to_string()),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: data.closetm.map(|t| (t * 1000.0) as i64),
            last_update_timestamp: None,
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
            fee: fee_cost.map(|c| Fee {
                cost: Some(c),
                currency: None,
                rate: None,
            }),
            fees: Vec::new(),
            trades: Vec::new(),
            reduce_only: None,
            post_only: None,
            time_in_force: None,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// 잔고 데이터 파싱
    fn parse_balance(&self, data: &HashMap<String, String>) -> Balances {
        let mut currencies = HashMap::new();

        for (currency, amount_str) in data {
            let clean_currency = currency
                .trim_start_matches('X')
                .trim_start_matches('Z')
                .to_string();
            let clean_currency = if clean_currency == "XBT" {
                "BTC".to_string()
            } else {
                clean_currency
            };

            if let Ok(amount) = Decimal::from_str(amount_str) {
                currencies.insert(
                    clean_currency,
                    Balance {
                        free: Some(amount),
                        used: Some(Decimal::ZERO),
                        total: Some(amount),
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
    fn sign_request(&self, path: &str, nonce: u64, body: &str) -> CcxtResult<String> {
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?;

        // SHA256(nonce + body)
        let mut sha256 = Sha256::new();
        sha256.update(format!("{nonce}{body}"));
        let sha256_digest = sha256.finalize();

        // path + sha256_digest
        let mut message = path.as_bytes().to_vec();
        message.extend_from_slice(&sha256_digest);

        // Decode secret from base64
        let decoded_secret = BASE64
            .decode(secret)
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Failed to decode secret: {e}"),
            })?;

        // HMAC-SHA512
        let mut mac = Hmac::<Sha512>::new_from_slice(&decoded_secret).map_err(|e| {
            CcxtError::AuthenticationError {
                message: format!("HMAC error: {e}"),
            }
        })?;
        mac.update(&message);
        let signature = mac.finalize().into_bytes();

        Ok(BASE64.encode(signature))
    }

    /// URL encode params manually
    fn url_encode_params(params: &HashMap<String, String>) -> String {
        params
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned + Default>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<KrakenResponse<T>> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Private API 요청
    async fn private_request<T: for<'de> Deserialize<'de> + Default>(
        &self,
        endpoint: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<KrakenResponse<T>> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;

        let nonce = Utc::now().timestamp_millis() as u64 * 1000;
        let mut body_params = params;
        body_params.insert("nonce".to_string(), nonce.to_string());

        let body = Self::url_encode_params(&body_params);

        let path = format!("/0/private/{endpoint}");
        let signature = self.sign_request(&path, nonce, &body)?;

        let mut headers = HashMap::new();
        headers.insert("API-Key".to_string(), api_key.to_string());
        headers.insert("API-Sign".to_string(), signature);
        headers.insert(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        );

        let json_body: serde_json::Value = serde_json::json!({
            "_form_encoded": body
        });
        let response: KrakenResponse<T> = self
            .client
            .post(&path, Some(json_body), Some(headers))
            .await?;

        if let Some(errors) = &response.error {
            if !errors.is_empty() {
                return Err(CcxtError::ExchangeError {
                    message: errors.join(", "),
                });
            }
        }

        Ok(response)
    }
}

#[async_trait]
impl Exchange for Kraken {
    fn id(&self) -> ExchangeId {
        ExchangeId::Kraken
    }

    fn name(&self) -> &str {
        "Kraken"
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

        let response: KrakenResponse<HashMap<String, KrakenAssetPair>> =
            self.public_get("/0/public/AssetPairs", None).await?;

        if let Some(errors) = &response.error {
            if !errors.is_empty() {
                return Err(CcxtError::ExchangeError {
                    message: errors.join(", "),
                });
            }
        }

        let result = response.result.unwrap_or_default();
        let mut markets_map = HashMap::new();
        let mut markets_by_id_map = HashMap::new();

        for (id, data) in result {
            // Skip darkpool pairs
            if id.ends_with(".d") {
                continue;
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
        let kraken_symbol = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".to_string(), kraken_symbol.clone());

        let response: KrakenResponse<HashMap<String, KrakenTickerData>> =
            self.public_get("/0/public/Ticker", Some(params)).await?;

        if let Some(errors) = &response.error {
            if !errors.is_empty() {
                return Err(CcxtError::ExchangeError {
                    message: errors.join(", "),
                });
            }
        }

        let result = response.result.unwrap_or_default();
        result
            .into_iter()
            .next()
            .map(|(_, data)| self.parse_ticker(symbol, &data))
            .ok_or_else(|| CcxtError::BadResponse {
                message: "No ticker data returned".into(),
            })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let pairs = if let Some(syms) = symbols {
            syms.iter()
                .map(|s| self.to_market_id(s))
                .collect::<Vec<_>>()
                .join(",")
        } else {
            return Err(CcxtError::BadRequest {
                message: "Kraken requires symbols parameter for fetch_tickers".into(),
            });
        };

        let mut params = HashMap::new();
        params.insert("pair".to_string(), pairs);

        let response: KrakenResponse<HashMap<String, KrakenTickerData>> =
            self.public_get("/0/public/Ticker", Some(params)).await?;

        if let Some(errors) = &response.error {
            if !errors.is_empty() {
                return Err(CcxtError::ExchangeError {
                    message: errors.join(", "),
                });
            }
        }

        let result = response.result.unwrap_or_default();
        let mut tickers = HashMap::new();

        for (id, data) in result {
            let sym = self.to_symbol(&id);
            let ticker = self.parse_ticker(&sym, &data);
            tickers.insert(sym, ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let kraken_symbol = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".to_string(), kraken_symbol);
        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }

        let response: KrakenResponse<HashMap<String, KrakenOrderBookData>> =
            self.public_get("/0/public/Depth", Some(params)).await?;

        if let Some(errors) = &response.error {
            if !errors.is_empty() {
                return Err(CcxtError::ExchangeError {
                    message: errors.join(", "),
                });
            }
        }

        let result = response.result.unwrap_or_default();
        let (_, data) = result
            .into_iter()
            .next()
            .ok_or_else(|| CcxtError::BadResponse {
                message: "No order book data returned".into(),
            })?;

        let timestamp = Utc::now().timestamp_millis();

        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|b| {
                if b.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(b[0].as_str()?).ok()?,
                        amount: Decimal::from_str(b[1].as_str()?).ok()?,
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
                        price: Decimal::from_str(a[0].as_str()?).ok()?,
                        amount: Decimal::from_str(a[1].as_str()?).ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
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
        let kraken_symbol = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".to_string(), kraken_symbol);
        if let Some(s) = since {
            params.insert("since".to_string(), (s * 1_000_000).to_string());
        }
        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }

        let response: KrakenResponse<HashMap<String, serde_json::Value>> =
            self.public_get("/0/public/Trades", Some(params)).await?;

        if let Some(errors) = &response.error {
            if !errors.is_empty() {
                return Err(CcxtError::ExchangeError {
                    message: errors.join(", "),
                });
            }
        }

        let result = response.result.unwrap_or_default();
        let mut trades = Vec::new();

        for (key, value) in result {
            if key == "last" {
                continue;
            }
            if let Some(trade_array) = value.as_array() {
                for trade_data in trade_array {
                    if let Some(arr) = trade_data.as_array() {
                        if let Some(trade) = self.parse_trade(symbol, arr) {
                            trades.push(trade);
                        }
                    }
                }
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
        let kraken_symbol = self.to_market_id(symbol);
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("pair".to_string(), kraken_symbol);
        params.insert("interval".to_string(), interval.clone());
        if let Some(s) = since {
            params.insert("since".to_string(), (s / 1000).to_string());
        }

        let response: KrakenResponse<HashMap<String, serde_json::Value>> =
            self.public_get("/0/public/OHLC", Some(params)).await?;

        if let Some(errors) = &response.error {
            if !errors.is_empty() {
                return Err(CcxtError::ExchangeError {
                    message: errors.join(", "),
                });
            }
        }

        let result = response.result.unwrap_or_default();
        let mut ohlcv_list = Vec::new();

        for (key, value) in result {
            if key == "last" {
                continue;
            }
            if let Some(candles) = value.as_array() {
                for candle in candles {
                    if let Some(arr) = candle.as_array() {
                        if let Some(ohlcv) = self.parse_ohlcv(arr) {
                            ohlcv_list.push(ohlcv);
                        }
                    }
                }
            }
        }

        // Apply limit if specified
        if let Some(l) = limit {
            ohlcv_list.truncate(l as usize);
        }

        Ok(ohlcv_list)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: KrakenResponse<HashMap<String, String>> =
            self.private_request("Balance", HashMap::new()).await?;

        let result = response.result.unwrap_or_default();
        Ok(self.parse_balance(&result))
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let kraken_symbol = self.to_market_id(symbol);

        let type_str = match order_type {
            OrderType::Market => "market",
            OrderType::Limit => "limit",
            OrderType::StopLoss => "stop-loss",
            OrderType::TakeProfit => "take-profit",
            OrderType::StopLossLimit => "stop-loss-limit",
            OrderType::TakeProfitLimit => "take-profit-limit",
            _ => {
                return Err(CcxtError::BadRequest {
                    message: format!("Unsupported order type: {order_type:?}"),
                })
            },
        };

        let side_str = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };

        let mut request_params = HashMap::new();
        request_params.insert("pair".to_string(), kraken_symbol);
        request_params.insert("type".to_string(), side_str.to_string());
        request_params.insert("ordertype".to_string(), type_str.to_string());
        request_params.insert("volume".to_string(), amount.to_string());

        if let Some(p) = price {
            request_params.insert("price".to_string(), p.to_string());
        }

        let response: KrakenResponse<KrakenAddOrderResponse> =
            self.private_request("AddOrder", request_params).await?;

        let result = response.result.ok_or_else(|| CcxtError::BadResponse {
            message: "No order response".into(),
        })?;

        let order_id = result.txid.first().ok_or_else(|| CcxtError::BadResponse {
            message: "No order ID returned".into(),
        })?;

        Ok(Order {
            id: order_id.clone(),
            client_order_id: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            symbol: symbol.to_string(),
            order_type,
            side,
            price,
            amount,
            cost: None,
            average: None,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            status: OrderStatus::Open,
            fee: None,
            fees: Vec::new(),
            trades: Vec::new(),
            reduce_only: None,
            post_only: None,
            time_in_force: None,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::to_value(&result).unwrap_or_default(),
        })
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("txid".to_string(), id.to_string());

        let _response: KrakenResponse<KrakenCancelOrderResponse> =
            self.private_request("CancelOrder", params).await?;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: None,
            datetime: None,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            symbol: _symbol.to_string(),
            order_type: OrderType::Limit,
            side: OrderSide::Buy,
            price: None,
            amount: Decimal::ZERO,
            cost: None,
            average: None,
            filled: Decimal::ZERO,
            remaining: None,
            status: OrderStatus::Canceled,
            fee: None,
            fees: Vec::new(),
            trades: Vec::new(),
            reduce_only: None,
            post_only: None,
            time_in_force: None,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::json!({}),
        })
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("txid".to_string(), id.to_string());

        let response: KrakenResponse<HashMap<String, KrakenOrderInfo>> =
            self.private_request("QueryOrders", params).await?;

        let result = response.result.unwrap_or_default();
        result
            .into_iter()
            .next()
            .map(|(order_id, data)| self.parse_order(&order_id, &data))
            .transpose()?
            .ok_or_else(|| CcxtError::OrderNotFound {
                order_id: id.to_string(),
            })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let response: KrakenResponse<KrakenOpenOrdersResponse> =
            self.private_request("OpenOrders", HashMap::new()).await?;

        let result = response.result.ok_or_else(|| CcxtError::BadResponse {
            message: "No open orders response".into(),
        })?;

        let mut orders = Vec::new();
        for (order_id, data) in result.open {
            if let Some(sym) = symbol {
                let order_symbol = data
                    .descr
                    .as_ref()
                    .and_then(|d| d.pair.as_ref())
                    .map(|p| self.to_symbol(p));
                if order_symbol.as_deref() != Some(sym) {
                    continue;
                }
            }
            orders.push(self.parse_order(&order_id, &data)?);
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
        if let Some(s) = since {
            params.insert("start".to_string(), (s / 1000).to_string());
        }

        let response: KrakenResponse<KrakenClosedOrdersResponse> =
            self.private_request("ClosedOrders", params).await?;

        let result = response.result.ok_or_else(|| CcxtError::BadResponse {
            message: "No closed orders response".into(),
        })?;

        let mut orders = Vec::new();
        for (order_id, data) in result.closed {
            if let Some(sym) = symbol {
                let order_symbol = data
                    .descr
                    .as_ref()
                    .and_then(|d| d.pair.as_ref())
                    .map(|p| self.to_symbol(p));
                if order_symbol.as_deref() != Some(sym) {
                    continue;
                }
            }
            orders.push(self.parse_order(&order_id, &data)?);
        }

        // Apply limit
        if let Some(l) = limit {
            orders.truncate(l as usize);
        }

        Ok(orders)
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

        if method == "POST" && self.config.api_key().is_some() {
            let nonce = Utc::now().timestamp_millis() as u64 * 1000;
            let body_str = body.unwrap_or("");

            if let Ok(signature) = self.sign_request(path, nonce, body_str) {
                if let Some(api_key) = self.config.api_key() {
                    result_headers.insert("API-Key".to_string(), api_key.to_string());
                    result_headers.insert("API-Sign".to_string(), signature);
                    result_headers.insert(
                        "Content-Type".to_string(),
                        "application/x-www-form-urlencoded".to_string(),
                    );
                }
            }
        }

        let url = if params.is_empty() {
            format!("{BASE_URL}{path}")
        } else {
            let query = Self::url_encode_params(params);
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

// === Kraken Response Types ===

#[derive(Debug, Default, Deserialize)]
struct KrakenResponse<T: Default> {
    #[serde(default)]
    error: Option<Vec<String>>,
    #[serde(default)]
    result: Option<T>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KrakenAssetPair {
    #[serde(default)]
    altname: Option<String>,
    #[serde(default)]
    wsname: Option<String>,
    #[serde(default)]
    aclass_base: Option<String>,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    aclass_quote: Option<String>,
    #[serde(default)]
    quote: Option<String>,
    #[serde(default)]
    pair_decimals: Option<u32>,
    #[serde(default)]
    lot_decimals: Option<u32>,
    #[serde(default)]
    lot_multiplier: Option<u32>,
    #[serde(default)]
    fees: Option<Vec<Vec<f64>>>,
    #[serde(default)]
    fees_maker: Option<Vec<Vec<f64>>>,
    #[serde(default)]
    margin_call: Option<u32>,
    #[serde(default)]
    margin_stop: Option<u32>,
    #[serde(default)]
    ordermin: Option<f64>,
    #[serde(default)]
    costmin: Option<f64>,
    #[serde(default)]
    tick_size: Option<f64>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KrakenTickerData {
    #[serde(default)]
    a: Option<Vec<String>>, // ask [price, whole lot volume, lot volume]
    #[serde(default)]
    b: Option<Vec<String>>, // bid [price, whole lot volume, lot volume]
    #[serde(default)]
    c: Option<Vec<String>>, // last trade closed [price, lot volume]
    #[serde(default)]
    v: Option<Vec<String>>, // volume [today, last 24 hours]
    #[serde(default)]
    p: Option<Vec<String>>, // vwap [today, last 24 hours]
    #[serde(default)]
    t: Option<Vec<i64>>, // number of trades [today, last 24 hours]
    #[serde(default)]
    l: Option<Vec<String>>, // low [today, last 24 hours]
    #[serde(default)]
    h: Option<Vec<String>>, // high [today, last 24 hours]
    #[serde(default)]
    o: Option<String>, // today's opening price
}

#[derive(Debug, Default, Deserialize)]
struct KrakenOrderBookData {
    #[serde(default)]
    bids: Vec<Vec<serde_json::Value>>,
    #[serde(default)]
    asks: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KrakenOrderDescr {
    #[serde(default)]
    pair: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    ordertype: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    price2: Option<String>,
    #[serde(default)]
    leverage: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    close: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KrakenOrderInfo {
    #[serde(default)]
    refid: Option<String>,
    #[serde(default)]
    userref: Option<i64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    opentm: Option<f64>,
    #[serde(default)]
    starttm: Option<f64>,
    #[serde(default)]
    expiretm: Option<f64>,
    #[serde(default)]
    closetm: Option<f64>,
    #[serde(default)]
    descr: Option<KrakenOrderDescr>,
    #[serde(default)]
    vol: Option<String>,
    #[serde(default)]
    vol_exec: Option<String>,
    #[serde(default)]
    cost: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    stopprice: Option<String>,
    #[serde(default)]
    limitprice: Option<String>,
    #[serde(default)]
    misc: Option<String>,
    #[serde(default)]
    oflags: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KrakenAddOrderResponse {
    #[serde(default)]
    descr: Option<KrakenOrderDescr>,
    #[serde(default)]
    txid: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct KrakenCancelOrderResponse {
    #[serde(default)]
    count: Option<i32>,
    #[serde(default)]
    pending: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct KrakenOpenOrdersResponse {
    #[serde(default)]
    open: HashMap<String, KrakenOrderInfo>,
}

#[derive(Debug, Default, Deserialize)]
struct KrakenClosedOrdersResponse {
    #[serde(default)]
    closed: HashMap<String, KrakenOrderInfo>,
    #[serde(default)]
    count: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_kraken() -> Kraken {
        Kraken::new(ExchangeConfig::default()).unwrap()
    }

    #[test]
    fn test_to_symbol() {
        let kraken = create_test_kraken();
        assert_eq!(kraken.to_symbol("XXBTZUSD"), "BTC/USD");
        assert_eq!(kraken.to_symbol("XETHZEUR"), "ETH/EUR");
        assert_eq!(kraken.to_symbol("BTCUSDT"), "BTC/USDT");
    }

    #[test]
    fn test_to_market_id() {
        let kraken = create_test_kraken();
        assert_eq!(kraken.to_market_id("BTC/USD"), "BTCUSD");
        assert_eq!(kraken.to_market_id("ETH/EUR"), "ETHEUR");
    }

    #[test]
    fn test_exchange_id() {
        let kraken = create_test_kraken();
        assert_eq!(kraken.id(), ExchangeId::Kraken);
        assert_eq!(kraken.name(), "Kraken");
    }

    #[test]
    fn test_has_features() {
        let kraken = create_test_kraken();
        let features = kraken.has();
        assert!(features.fetch_markets);
        assert!(features.fetch_ticker);
        assert!(features.fetch_order_book);
        assert!(features.fetch_balance);
        assert!(features.create_order);
    }
}
