//! Coinone Exchange Implementation
//!
//! 코인원 거래소 API 구현 (한국 주요 암호화폐 거래소)

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

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, Fee, OHLCV, TakerOrMaker,
};

const BASE_URL: &str = "https://api.coinone.co.kr";
const RATE_LIMIT_MS: u64 = 50;

/// Coinone 거래소 구조체
pub struct Coinone {
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
struct CoinoneResponse<T> {
    result: String,
    error_code: Option<String>,
    error_msg: Option<String>,
    #[serde(flatten)]
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct CoinoneTicker {
    quote_currency: Option<String>,
    target_currency: Option<String>,
    timestamp: Option<i64>,
    high: Option<String>,
    low: Option<String>,
    first: Option<String>,
    last: Option<String>,
    quote_volume: Option<String>,
    target_volume: Option<String>,
    best_asks: Option<Vec<CoinonePriceQty>>,
    best_bids: Option<Vec<CoinonePriceQty>>,
}

#[derive(Debug, Deserialize)]
struct CoinonePriceQty {
    price: String,
    qty: String,
}

#[derive(Debug, Deserialize)]
struct CoinoneOrderBookResponse {
    timestamp: Option<i64>,
    id: Option<String>,
    bids: Option<Vec<CoinonePriceQty>>,
    asks: Option<Vec<CoinonePriceQty>>,
}

#[derive(Debug, Deserialize)]
struct CoinoneTradesResponse {
    transactions: Option<Vec<CoinoneTrade>>,
}

#[derive(Debug, Deserialize)]
struct CoinoneTrade {
    id: Option<String>,
    timestamp: Option<i64>,
    price: Option<String>,
    qty: Option<String>,
    is_seller_maker: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct CoinoneBalanceResponse {
    #[serde(flatten)]
    balances: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct CoinoneOrderResponse {
    #[serde(rename = "orderId")]
    order_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoinoneOrder {
    #[serde(rename = "orderId")]
    order_id: Option<String>,
    status: Option<String>,
    #[serde(rename = "orderType")]
    order_type: Option<String>,
    #[serde(rename = "targetCurrency")]
    target_currency: Option<String>,
    #[serde(rename = "quoteCurrency")]
    quote_currency: Option<String>,
    price: Option<String>,
    qty: Option<String>,
    #[serde(rename = "remainQty")]
    remain_qty: Option<String>,
    #[serde(rename = "executedQty")]
    executed_qty: Option<String>,
    fee: Option<String>,
    #[serde(rename = "feeRate")]
    fee_rate: Option<String>,
    #[serde(rename = "averagePrice")]
    average_price: Option<String>,
    timestamp: Option<i64>,
    #[serde(rename = "isAsk")]
    is_ask: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct CoinoneLimitOrdersResponse {
    #[serde(rename = "limitOrders")]
    limit_orders: Option<Vec<CoinoneOrder>>,
}

impl Coinone {
    /// 새 Coinone 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), format!("{}/public/v2", BASE_URL));
        api_urls.insert("private".into(), BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/38003300-adc12fba-322a-11e8-8525-725f53c4a659.jpg".into()),
            api: api_urls,
            www: Some("https://coinone.co.kr".into()),
            doc: vec!["https://doc.coinone.co.kr/".into()],
            fees: Some("https://coinone.co.kr/info/fees".into()),
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
            fetch_ohlcv: false,  // Coinone doesn't have OHLCV endpoint
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: false,  // Only limit orders
            cancel_order: true,
            cancel_all_orders: false,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: false,
            fetch_my_trades: true,
            fetch_deposits: false,
            fetch_withdrawals: false,
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

        // Coinone doesn't support OHLCV
        let timeframes = HashMap::new();

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

    /// 심볼을 파싱
    fn parse_symbol(&self, base: &str, quote: &str) -> String {
        format!("{}/{}", base.to_uppercase(), quote.to_uppercase())
    }

    /// 심볼을 마켓 ID로 변환
    fn to_market_parts(&self, symbol: &str) -> (String, String) {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            (parts[0].to_lowercase(), parts[1].to_lowercase())
        } else {
            (symbol.to_lowercase(), "krw".to_string())
        }
    }

    /// HMAC-SHA512 서명 생성 (uppercase secret)
    fn create_signature(&self, payload: &str) -> CcxtResult<String> {
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".to_string(),
        })?;

        // IMPORTANT: Coinone requires uppercase secret
        let secret_upper = secret.to_uppercase();

        type HmacSha512 = Hmac<Sha512>;
        let mut mac = HmacSha512::new_from_slice(secret_upper.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Invalid secret key".to_string(),
            })?;
        mac.update(payload.as_bytes());

        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Private API 요청
    async fn private_request(
        &self,
        endpoint: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".to_string(),
        })?;

        let nonce = Utc::now().timestamp_millis().to_string();

        // Build payload
        let mut payload_obj = params.clone();
        payload_obj.insert("access_token".to_string(), serde_json::Value::String(api_key.to_string()));
        payload_obj.insert("nonce".to_string(), serde_json::Value::String(nonce));

        let payload_json = serde_json::to_string(&payload_obj)
            .map_err(|e| CcxtError::ParseError { data_type: "json".to_string(), message: e.to_string() })?;

        // Base64 encode payload
        let payload_b64 = BASE64.encode(payload_json.as_bytes());

        // Create signature from Base64 payload
        let signature = self.create_signature(&payload_b64)?;

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        headers.insert("X-COINONE-PAYLOAD".to_string(), payload_b64.clone());
        headers.insert("X-COINONE-SIGNATURE".to_string(), signature);

        // Body is the JSON payload as Value
        let url = format!("{}{}", BASE_URL, endpoint);
        let body_value = serde_json::to_value(&payload_b64)
            .map_err(|e| CcxtError::ParseError { data_type: "json".to_string(), message: e.to_string() })?;
        let response: serde_json::Value = self.client.post(&url, Some(body_value), Some(headers)).await?;

        // Check for errors
        let result = response.get("result")
            .and_then(|r| r.as_str())
            .unwrap_or("success");

        if result == "error" {
            let error_code = response.get("error_code")
                .and_then(|c| c.as_str())
                .unwrap_or("unknown");
            let error_msg = response.get("error_msg")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");

            return Err(match error_code {
                "104" => CcxtError::OrderNotFound { order_id: error_msg.to_string() },
                "107" => CcxtError::BadRequest { message: error_msg.to_string() },
                "108" => CcxtError::BadSymbol { symbol: error_msg.to_string() },
                "405" => CcxtError::ExchangeNotAvailable { message: error_msg.to_string() },
                _ => CcxtError::ExchangeError { message: format!("{}: {}", error_code, error_msg) },
            });
        }

        Ok(response)
    }

    /// Public API 요청
    async fn public_request(&self, path: &str) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let url = format!("{}/public/v2{}", BASE_URL, path);
        let response: serde_json::Value = self.client.get(&url, None, None).await?;

        let result = response.get("result")
            .and_then(|r| r.as_str())
            .unwrap_or("success");

        if result == "error" {
            let error_code = response.get("error_code")
                .and_then(|c| c.as_str())
                .unwrap_or("unknown");
            let error_msg = response.get("error_msg")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            return Err(CcxtError::ExchangeError {
                message: format!("{}: {}", error_code, error_msg),
            });
        }

        Ok(response)
    }

    /// 주문 상태 파싱
    fn parse_order_status(&self, status: &str, remain_qty: Option<Decimal>, original_qty: Option<Decimal>) -> OrderStatus {
        match status.to_lowercase().as_str() {
            "live" => {
                // Check if partially filled and status changed
                if let (Some(remain), Some(orig)) = (remain_qty, original_qty) {
                    if remain < orig && remain > Decimal::ZERO {
                        return OrderStatus::Open;
                    }
                }
                OrderStatus::Open
            }
            "partially_filled" => OrderStatus::Open,
            "partially_canceled" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "canceled" => OrderStatus::Canceled,
            _ => OrderStatus::Open, // Default to Open for unknown statuses
        }
    }

    /// 주문 파싱
    fn parse_order(&self, order: &CoinoneOrder, symbol: &str) -> Order {
        let price = order.price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let amount = order.qty.as_ref()
            .and_then(|q| Decimal::from_str(q).ok());
        let remaining = order.remain_qty.as_ref()
            .and_then(|r| Decimal::from_str(r).ok());
        let filled = order.executed_qty.as_ref()
            .and_then(|e| Decimal::from_str(e).ok());
        let average = order.average_price.as_ref()
            .and_then(|a| Decimal::from_str(a).ok());
        let fee_cost = order.fee.as_ref()
            .and_then(|f| Decimal::from_str(f).ok())
            .map(|f| f.abs());

        let side = if order.is_ask.unwrap_or(false) {
            OrderSide::Sell
        } else {
            OrderSide::Buy
        };

        let status = self.parse_order_status(
            order.status.as_deref().unwrap_or(""),
            remaining,
            amount,
        );

        Order {
            id: order.order_id.clone().unwrap_or_default(),
            client_order_id: None,
            timestamp: order.timestamp.map(|t| t * 1000),
            datetime: order.timestamp.map(|t| {
                chrono::DateTime::from_timestamp(t, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side,
            price,
            average,
            amount: amount.unwrap_or(Decimal::ZERO),
            filled: filled.unwrap_or(Decimal::ZERO),
            remaining,
            stop_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            trigger_price: None,
            cost: filled.zip(average).map(|(f, a)| f * a),
            trades: vec![],
            fees: vec![],
            reduce_only: None,
            post_only: None,
            fee: fee_cost.map(|cost| Fee {
                cost: Some(cost),
                currency: if side == OrderSide::Buy {
                    Some(symbol.split('/').next().unwrap_or("").to_string())
                } else {
                    Some(symbol.split('/').nth(1).unwrap_or("").to_string())
                },
                rate: order.fee_rate.as_ref().and_then(|r| Decimal::from_str(r).ok()),
            }),
            info: serde_json::Value::Null,
        }
    }
}

#[async_trait]
impl Exchange for Coinone {
    fn id(&self) -> ExchangeId {
        ExchangeId::Coinone
    }

    fn name(&self) -> &str {
        "Coinone"
    }

    fn version(&self) -> &str {
        "v2"
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
        Some(format!("{}/KRW", market_id.to_uppercase()))
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
            if let Some(api_key) = self.config.api_key() {
                let nonce = Utc::now().timestamp_millis().to_string();

                let mut payload_obj: HashMap<String, serde_json::Value> = params.iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect();
                payload_obj.insert("access_token".to_string(), serde_json::Value::String(api_key.to_string()));
                payload_obj.insert("nonce".to_string(), serde_json::Value::String(nonce));

                if let Ok(payload_json) = serde_json::to_string(&payload_obj) {
                    let payload_b64 = BASE64.encode(payload_json.as_bytes());
                    if let Ok(signature) = self.create_signature(&payload_b64) {
                        final_headers.insert("Content-Type".to_string(), "application/json".to_string());
                        final_headers.insert("X-COINONE-PAYLOAD".to_string(), payload_b64);
                        final_headers.insert("X-COINONE-SIGNATURE".to_string(), signature);
                    }
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
        // Fetch tickers to get market list
        let response = self.public_request("/ticker_new/KRW").await?;

        let tickers = response.get("tickers")
            .and_then(|t| t.as_array())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "tickers".to_string(),
                message: "Missing tickers array".to_string(),
            })?;

        let mut markets = Vec::new();

        for ticker in tickers {
            let base = ticker.get("target_currency")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            let quote = ticker.get("quote_currency")
                .and_then(|c| c.as_str())
                .unwrap_or("KRW");

            if base.is_empty() {
                continue;
            }

            let symbol = self.parse_symbol(base, quote);

            let market = Market {
                id: base.to_lowercase(),
                lowercase_id: Some(base.to_lowercase()),
                symbol: symbol.clone(),
                base: base.to_uppercase(),
                quote: quote.to_uppercase(),
                base_id: base.to_lowercase(),
                quote_id: quote.to_lowercase(),
                active: true,
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
                    amount: Some(4),
                    price: Some(4),
                    cost: Some(8),
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
                maker: Some(Decimal::from_str("0.002").unwrap()),
                taker: Some(Decimal::from_str("0.002").unwrap()),
                percentage: true,
                tier_based: false,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, crate::types::Currency>> {
        let response = self.public_request("/currencies").await?;

        let currencies_arr = response.get("currencies")
            .and_then(|c| c.as_array())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "currencies".to_string(),
                message: "Missing currencies array".to_string(),
            })?;

        let mut currencies = HashMap::new();

        for item in currencies_arr {
            let code = item.get("currency")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            let name = item.get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string());

            if code.is_empty() {
                continue;
            }

            let code_upper = code.to_uppercase();

            currencies.insert(code_upper.clone(), crate::types::Currency {
                id: code.to_lowercase(),
                code: code_upper,
                name,
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

        // Add KRW
        currencies.insert("KRW".to_string(), crate::types::Currency {
            id: "krw".to_string(),
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
        let path = format!("/ticker_new/{}/{}", quote.to_uppercase(), base);

        let response = self.public_request(&path).await?;

        let timestamp = response.get("timestamp")
            .and_then(|t| t.as_i64());

        let high = response.get("high")
            .and_then(|h| h.as_str())
            .and_then(|s| Decimal::from_str(s).ok());
        let low = response.get("low")
            .and_then(|l| l.as_str())
            .and_then(|s| Decimal::from_str(s).ok());
        let open = response.get("first")
            .and_then(|f| f.as_str())
            .and_then(|s| Decimal::from_str(s).ok());
        let close = response.get("last")
            .and_then(|l| l.as_str())
            .and_then(|s| Decimal::from_str(s).ok());
        let base_volume = response.get("target_volume")
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok());
        let quote_volume = response.get("quote_volume")
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok());

        // Parse best bid/ask
        let bid = response.get("best_bids")
            .and_then(|b| b.as_array())
            .and_then(|arr| arr.first())
            .and_then(|b| b.get("price"))
            .and_then(|p| p.as_str())
            .and_then(|s| Decimal::from_str(s).ok());
        let ask = response.get("best_asks")
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|a| a.get("price"))
            .and_then(|p| p.as_str())
            .and_then(|s| Decimal::from_str(s).ok());

        let change = open.zip(close).map(|(o, c)| c - o);
        let percentage = open.zip(close).map(|(o, c)| {
            if o > Decimal::ZERO {
                ((c - o) / o) * Decimal::from(100)
            } else {
                Decimal::ZERO
            }
        });

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: timestamp.map(|t| t * 1000),
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp(t, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            high,
            low,
            bid,
            bid_volume: None,
            ask,
            ask_volume: None,
            vwap: None,
            open,
            close,
            last: close,
            previous_close: None,
            change,
            percentage,
            average: None,
            base_volume,
            quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response = self.public_request("/ticker_new/KRW").await?;

        let tickers = response.get("tickers")
            .and_then(|t| t.as_array())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "tickers".to_string(),
                message: "Missing tickers array".to_string(),
            })?;

        let timestamp = response.get("timestamp")
            .and_then(|t| t.as_i64());

        let mut result = HashMap::new();

        for ticker in tickers {
            let base = ticker.get("target_currency")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            let quote = ticker.get("quote_currency")
                .and_then(|c| c.as_str())
                .unwrap_or("KRW");

            if base.is_empty() {
                continue;
            }

            let symbol = self.parse_symbol(base, quote);

            // Filter by requested symbols
            if let Some(syms) = symbols {
                if !syms.contains(&symbol.as_str()) {
                    continue;
                }
            }

            let high = ticker.get("high")
                .and_then(|h| h.as_str())
                .and_then(|s| Decimal::from_str(s).ok());
            let low = ticker.get("low")
                .and_then(|l| l.as_str())
                .and_then(|s| Decimal::from_str(s).ok());
            let open = ticker.get("first")
                .and_then(|f| f.as_str())
                .and_then(|s| Decimal::from_str(s).ok());
            let close = ticker.get("last")
                .and_then(|l| l.as_str())
                .and_then(|s| Decimal::from_str(s).ok());
            let base_volume = ticker.get("target_volume")
                .and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str(s).ok());
            let quote_volume = ticker.get("quote_volume")
                .and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str(s).ok());

            result.insert(symbol.clone(), Ticker {
                symbol,
                timestamp: timestamp.map(|t| t * 1000),
                datetime: timestamp.map(|t| {
                    chrono::DateTime::from_timestamp(t, 0)
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
                base_volume,
                quote_volume,
                index_price: None,
                mark_price: None,
                info: serde_json::Value::Null,
            });
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let (base, quote) = self.to_market_parts(symbol);
        let size = limit.unwrap_or(15).min(16);
        let path = format!("/orderbook/{}/{}?size={}", quote.to_uppercase(), base, size);

        let response = self.public_request(&path).await?;

        let timestamp = response.get("timestamp")
            .and_then(|t| t.as_i64());

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        if let Some(bids_arr) = response.get("bids").and_then(|b| b.as_array()) {
            for bid in bids_arr {
                let price = bid.get("price")
                    .and_then(|p| p.as_str())
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                let amount = bid.get("qty")
                    .and_then(|q| q.as_str())
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                bids.push(OrderBookEntry { price, amount });
            }
        }

        if let Some(asks_arr) = response.get("asks").and_then(|a| a.as_array()) {
            for ask in asks_arr {
                let price = ask.get("price")
                    .and_then(|p| p.as_str())
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                let amount = ask.get("qty")
                    .and_then(|q| q.as_str())
                    .and_then(|s| Decimal::from_str(s).ok())
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
            timestamp: timestamp.map(|t| t * 1000),
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp(t, 0)
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
        let size = limit.unwrap_or(200).min(200);
        let path = format!("/trades/{}/{}?size={}", quote.to_uppercase(), base, size);

        let response = self.public_request(&path).await?;

        let transactions = response.get("transactions")
            .and_then(|t| t.as_array())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "transactions".to_string(),
                message: "Missing transactions array".to_string(),
            })?;

        let mut trades = Vec::new();

        for item in transactions {
            let id = item.get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("");
            let timestamp = item.get("timestamp")
                .and_then(|t| t.as_i64());
            let price = item.get("price")
                .and_then(|p| p.as_str())
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);
            let amount = item.get("qty")
                .and_then(|q| q.as_str())
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);

            let is_seller_maker = item.get("is_seller_maker")
                .and_then(|m| m.as_bool())
                .unwrap_or(false);

            let side = if is_seller_maker {
                OrderSide::Sell
            } else {
                OrderSide::Buy
            };

            trades.push(Trade {
                id: id.to_string(),
                order: None,
                timestamp: timestamp.map(|t| t * 1000),
                datetime: timestamp.map(|t| {
                    chrono::DateTime::from_timestamp(t, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(if side == OrderSide::Buy { "buy".to_string() } else { "sell".to_string() }),
                taker_or_maker: Some(if is_seller_maker { TakerOrMaker::Maker } else { TakerOrMaker::Taker }),
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: vec![],
                info: serde_json::Value::Null,
            });
        }

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        Err(CcxtError::NotSupported {
            feature: "fetchOHLCV".to_string(),
        })
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response = self.private_request("/v2/account/balance", HashMap::new()).await?;

        let mut balances = HashMap::new();
        let mut total_free = Decimal::ZERO;
        let mut total_used = Decimal::ZERO;

        if let Some(obj) = response.as_object() {
            for (key, value) in obj {
                if key == "result" || key == "error_code" {
                    continue;
                }

                if let Some(balance_obj) = value.as_object() {
                    let avail = balance_obj.get("avail")
                        .and_then(|a| a.as_str())
                        .and_then(|s| Decimal::from_str(s).ok())
                        .unwrap_or(Decimal::ZERO);
                    let total = balance_obj.get("balance")
                        .and_then(|b| b.as_str())
                        .and_then(|s| Decimal::from_str(s).ok())
                        .unwrap_or(Decimal::ZERO);
                    let used = total - avail;

                    if total > Decimal::ZERO || avail > Decimal::ZERO {
                        total_free += avail;
                        total_used += used;

                        let currency = key.to_uppercase();
                        balances.insert(currency.clone(), Balance {
                            free: Some(avail),
                            used: Some(used),
                            total: Some(total),
                            debt: None,
                        });
                    }
                }
            }
        }

        Ok(Balances {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies: balances,
            info: response,
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
        // Coinone only supports limit orders
        if order_type != OrderType::Limit {
            return Err(CcxtError::NotSupported {
                feature: format!("Order type {:?} - Coinone only supports limit orders", order_type),
            });
        }

        let p = price.ok_or_else(|| CcxtError::BadRequest {
            message: "Price required for limit order".to_string(),
        })?;

        let (base, _) = self.to_market_parts(symbol);

        let endpoint = match side {
            OrderSide::Buy => "/order/limit_buy",
            OrderSide::Sell => "/order/limit_sell",
        };

        let mut params = HashMap::new();
        params.insert("price".to_string(), serde_json::Value::String(p.to_string()));
        params.insert("qty".to_string(), serde_json::Value::String(amount.to_string()));
        params.insert("currency".to_string(), serde_json::Value::String(base));

        let response = self.private_request(endpoint, params).await?;

        let order_id = response.get("orderId")
            .and_then(|id| id.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        Ok(Order {
            id: order_id,
            client_order_id: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side,
            price: Some(p),
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            stop_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            trigger_price: None,
            cost: None,
            trades: vec![],
            fees: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            info: response,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        // Coinone requires original order details for cancellation
        // First fetch the order
        let order = self.fetch_order(id, symbol).await?;

        let (base, _) = self.to_market_parts(symbol);

        let mut params = HashMap::new();
        params.insert("order_id".to_string(), serde_json::Value::String(id.to_string()));
        params.insert("currency".to_string(), serde_json::Value::String(base));
        params.insert("price".to_string(), serde_json::Value::String(order.price.unwrap_or(Decimal::ZERO).to_string()));
        params.insert("qty".to_string(), serde_json::Value::String(order.amount.to_string()));
        params.insert("is_ask".to_string(), serde_json::Value::Bool(order.side == OrderSide::Sell));

        let response = self.private_request("/v2/order/cancel", params).await?;

        Ok(Order {
            id: id.to_string(),
            status: OrderStatus::Canceled,
            info: response,
            ..order
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let (base, _) = self.to_market_parts(symbol);

        let mut params = HashMap::new();
        params.insert("order_id".to_string(), serde_json::Value::String(id.to_string()));
        params.insert("currency".to_string(), serde_json::Value::String(base));

        let response = self.private_request("/v2/order/query_order", params).await?;

        let price = response.get("price")
            .and_then(|p| p.as_str())
            .and_then(|s| Decimal::from_str(s).ok());
        let qty = response.get("qty")
            .and_then(|q| q.as_str())
            .and_then(|s| Decimal::from_str(s).ok());
        let remain_qty = response.get("remainQty")
            .and_then(|r| r.as_str())
            .and_then(|s| Decimal::from_str(s).ok());
        let executed_qty = response.get("executedQty")
            .and_then(|e| e.as_str())
            .and_then(|s| Decimal::from_str(s).ok());
        let average_price = response.get("averagePrice")
            .and_then(|a| a.as_str())
            .and_then(|s| Decimal::from_str(s).ok());
        let status = response.get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let is_ask = response.get("isAsk")
            .and_then(|a| a.as_bool())
            .unwrap_or(false);

        let order_status = self.parse_order_status(status, remain_qty, qty);

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: None,
            datetime: None,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: order_status,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: if is_ask { OrderSide::Sell } else { OrderSide::Buy },
            price,
            average: average_price,
            amount: qty.unwrap_or(Decimal::ZERO),
            filled: executed_qty.unwrap_or(Decimal::ZERO),
            remaining: remain_qty,
            stop_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            trigger_price: None,
            cost: executed_qty.zip(average_price).map(|(e, a)| e * a),
            trades: vec![],
            fees: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            info: response,
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::BadRequest {
            message: "Symbol required for Coinone".to_string(),
        })?;

        let (base, _) = self.to_market_parts(symbol);

        let mut params = HashMap::new();
        params.insert("currency".to_string(), serde_json::Value::String(base));

        let response = self.private_request("/order/limit_orders", params).await?;

        let limit_orders = response.get("limitOrders")
            .and_then(|o| o.as_array())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "limitOrders".to_string(),
                message: "Missing limitOrders array".to_string(),
            })?;

        let mut orders = Vec::new();

        for item in limit_orders {
            let order: CoinoneOrder = serde_json::from_value(item.clone())
                .map_err(|e| CcxtError::ParseError { data_type: "CoinoneOrder".to_string(), message: e.to_string() })?;
            orders.push(self.parse_order(&order, symbol));
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
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol = symbol.ok_or_else(|| CcxtError::BadRequest {
            message: "Symbol required for Coinone".to_string(),
        })?;

        let (base, _) = self.to_market_parts(symbol);

        let mut params = HashMap::new();
        params.insert("currency".to_string(), serde_json::Value::String(base));

        let response = self.private_request("/v2/order/complete_orders", params).await?;

        let complete_orders = response.get("completeOrders")
            .and_then(|o| o.as_array())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "completeOrders".to_string(),
                message: "Missing completeOrders array".to_string(),
            })?;

        let mut trades = Vec::new();

        for item in complete_orders {
            let order_id = item.get("orderId")
                .and_then(|id| id.as_str())
                .unwrap_or("");
            let price = item.get("price")
                .and_then(|p| p.as_str())
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);
            let qty = item.get("qty")
                .and_then(|q| q.as_str())
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);
            let timestamp = item.get("timestamp")
                .and_then(|t| t.as_i64());
            let is_ask = item.get("type")
                .and_then(|t| t.as_str())
                .map(|t| t == "ask")
                .unwrap_or(false);
            let fee_cost = item.get("fee")
                .and_then(|f| f.as_str())
                .and_then(|s| Decimal::from_str(s).ok())
                .map(|f| f.abs());

            let side = if is_ask { OrderSide::Sell } else { OrderSide::Buy };

            trades.push(Trade {
                id: format!("{}-{}", order_id, timestamp.unwrap_or(0)),
                order: Some(order_id.to_string()),
                timestamp: timestamp.map(|t| t * 1000),
                datetime: timestamp.map(|t| {
                    chrono::DateTime::from_timestamp(t, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(if side == OrderSide::Buy { "buy".to_string() } else { "sell".to_string() }),
                taker_or_maker: Some(TakerOrMaker::Taker),
                price,
                amount: qty,
                cost: Some(price * qty),
                fee: fee_cost.map(|cost| Fee {
                    cost: Some(cost),
                    currency: Some(if is_ask {
                        symbol.split('/').nth(1).unwrap_or("").to_string()
                    } else {
                        symbol.split('/').next().unwrap_or("").to_string()
                    }),
                    rate: None,
                }),
                fees: vec![],
                info: serde_json::Value::Null,
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
        Err(CcxtError::NotSupported {
            feature: "fetchDeposits".to_string(),
        })
    }

    async fn fetch_withdrawals(
        &self,
        _code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        Err(CcxtError::NotSupported {
            feature: "fetchWithdrawals".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Coinone::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::Coinone);
        assert_eq!(exchange.name(), "Coinone");
        assert_eq!(exchange.version(), "v2");
        assert!(exchange.has().spot);
        assert!(!exchange.has().margin);
        assert!(!exchange.has().swap);
        assert!(!exchange.has().create_market_order);  // Only limit orders
        assert!(!exchange.has().fetch_ohlcv);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::default();
        let exchange = Coinone::new(config).unwrap();

        let (base, quote) = exchange.to_market_parts("BTC/KRW");
        assert_eq!(base, "btc");
        assert_eq!(quote, "krw");

        let symbol = exchange.parse_symbol("btc", "krw");
        assert_eq!(symbol, "BTC/KRW");
    }

    #[test]
    fn test_rate_limit() {
        let config = ExchangeConfig::default();
        let exchange = Coinone::new(config).unwrap();
        assert_eq!(exchange.rate_limit(), 50);
    }
}
