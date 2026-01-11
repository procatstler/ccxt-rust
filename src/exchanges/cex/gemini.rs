//! Gemini Exchange Implementation
//!
//! Gemini 거래소 API 구현
//! US-based regulated cryptocurrency exchange

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha384;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, OHLCV, Trade,
};

const BASE_URL: &str = "https://api.gemini.com";
const SANDBOX_URL: &str = "https://api.sandbox.gemini.com";
const RATE_LIMIT_MS: u64 = 200; // 5 requests per second

type HmacSha384 = Hmac<Sha384>;

/// Gemini exchange structure
pub struct Gemini {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

// Response wrappers
#[derive(Debug, Deserialize, Serialize)]
struct GeminiSymbol {
    symbol: String,
    base_currency: String,
    quote_currency: String,
    tick_size: f64,
    quote_increment: f64,
    min_order_size: String,
    status: String,
    wrap_enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeminiTicker {
    bid: Option<String>,
    ask: Option<String>,
    last: Option<String>,
    volume: Option<GeminiVolume>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeminiVolume {
    #[serde(flatten)]
    volumes: HashMap<String, String>,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeminiTickerV2 {
    symbol: String,
    open: Option<String>,
    high: Option<String>,
    low: Option<String>,
    close: Option<String>,
    changes: Option<Vec<String>>,
    bid: Option<String>,
    ask: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeminiOrderBook {
    bids: Vec<GeminiOrderBookEntry>,
    asks: Vec<GeminiOrderBookEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeminiOrderBookEntry {
    price: String,
    amount: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeminiTrade {
    timestamp: i64,
    timestampms: i64,
    tid: i64,
    price: String,
    amount: String,
    exchange: String,
    #[serde(rename = "type")]
    side: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeminiCandle(i64, f64, f64, f64, f64, f64);

#[derive(Debug, Deserialize, Serialize)]
struct GeminiBalance {
    currency: String,
    amount: String,
    available: String,
    #[serde(rename = "availableForWithdrawal")]
    available_for_withdrawal: String,
    #[serde(rename = "type")]
    account_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeminiOrder {
    order_id: String,
    id: Option<String>,
    client_order_id: Option<String>,
    symbol: String,
    exchange: Option<String>,
    price: String,
    avg_execution_price: Option<String>,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    timestamp: Option<String>,
    timestampms: Option<i64>,
    is_live: bool,
    is_cancelled: bool,
    is_hidden: Option<bool>,
    was_forced: Option<bool>,
    executed_amount: String,
    remaining_amount: String,
    original_amount: String,
    options: Option<Vec<String>>,
}

impl Gemini {
    /// Create a new Gemini exchange instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let sandbox = config.is_sandbox();
        let base_url = if sandbox { SANDBOX_URL } else { BASE_URL };
        let client = HttpClient::new(base_url, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

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
            fetch_my_trades: true,
            ws: true,
            watch_ticker: true,
            watch_order_book: true,
            watch_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".to_string(), format!("{base_url}/v1"));
        api_urls.insert("private".to_string(), format!("{base_url}/v1"));
        api_urls.insert("v2".to_string(), format!("{base_url}/v2"));

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27816857-ce7be644-6096-11e7-82d6-3c257263229c.jpg".to_string()),
            api: api_urls,
            www: Some("https://gemini.com".to_string()),
            doc: vec!["https://docs.gemini.com/rest-api".to_string()],
            fees: Some("https://gemini.com/fees".to_string()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1hr".into());
        timeframes.insert(Timeframe::Hour6, "6hr".into());
        timeframes.insert(Timeframe::Day1, "1day".into());

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

    /// Generate HMAC-SHA384 signature
    fn sign_payload(&self, payload: &str) -> CcxtResult<String> {
        let secret = self.config.secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".to_string(),
            })?;

        let mut mac = HmacSha384::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("HMAC error: {e}"),
            })?;
        mac.update(payload.as_bytes());
        let result = mac.finalize();
        Ok(hex::encode(result.into_bytes()))
    }

    /// Make authenticated request
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<Value>,
    ) -> CcxtResult<T> {
        let api_key = self.config.api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".to_string(),
            })?;

        self.rate_limiter.throttle(1.0).await;

        let nonce = chrono::Utc::now().timestamp_millis();

        let mut payload = params.unwrap_or(json!({}));
        payload["request"] = json!(path);
        payload["nonce"] = json!(nonce);

        let payload_str = payload.to_string();
        let payload_b64 = general_purpose::STANDARD.encode(payload_str.as_bytes());
        let signature = self.sign_payload(&payload_b64)?;

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "text/plain".to_string());
        headers.insert("Content-Length".to_string(), "0".to_string());
        headers.insert("X-GEMINI-APIKEY".to_string(), api_key.to_string());
        headers.insert("X-GEMINI-PAYLOAD".to_string(), payload_b64);
        headers.insert("X-GEMINI-SIGNATURE".to_string(), signature);
        headers.insert("Cache-Control".to_string(), "no-cache".to_string());

        self.client.post(path, None::<Value>, Some(headers)).await
    }

    /// Make public request
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Parse order status
    fn parse_order_status(order: &GeminiOrder) -> OrderStatus {
        if order.is_cancelled {
            OrderStatus::Canceled
        } else if order.is_live {
            OrderStatus::Open
        } else {
            OrderStatus::Closed
        }
    }

    /// Parse order side
    fn parse_order_side(side: &str) -> OrderSide {
        match side.to_lowercase().as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        }
    }

    /// Parse order type
    fn parse_order_type(order_type: &str) -> OrderType {
        match order_type.to_lowercase().as_str() {
            "exchange limit" | "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            _ => OrderType::Limit,
        }
    }

    /// Convert order to unified format
    fn parse_order(&self, order: &GeminiOrder) -> Order {
        let timestamp = order.timestampms.or_else(|| {
            order.timestamp.as_ref()
                .and_then(|t| t.parse::<f64>().ok())
                .map(|t| (t * 1000.0) as i64)
        });
        let datetime = timestamp.and_then(|t| {
            chrono::DateTime::from_timestamp_millis(t)
                .map(|dt| dt.to_rfc3339())
        });

        let side = Self::parse_order_side(&order.side);
        let status = Self::parse_order_status(order);
        let order_type = Self::parse_order_type(&order.order_type);

        let amount = order.original_amount.parse::<f64>()
            .map(|a| Decimal::from_f64_retain(a).unwrap_or_default())
            .unwrap_or_default();
        let filled = order.executed_amount.parse::<f64>()
            .map(|a| Decimal::from_f64_retain(a).unwrap_or_default())
            .unwrap_or_default();
        let remaining = order.remaining_amount.parse::<f64>()
            .map(|a| Decimal::from_f64_retain(a).unwrap_or_default())
            .unwrap_or_default();
        let price = order.price.parse::<f64>().ok()
            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default());
        let average = order.avg_execution_price.as_ref()
            .and_then(|p| p.parse::<f64>().ok())
            .filter(|p| *p > 0.0)
            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default());

        let cost = average.map(|avg| avg * filled);

        let post_only = order.options.as_ref()
            .map(|opts| opts.iter().any(|o| o == "maker-or-cancel"))
            .unwrap_or(false);

        Order {
            id: order.order_id.clone(),
            client_order_id: order.client_order_id.clone(),
            timestamp,
            datetime,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: order.symbol.to_uppercase(),
            order_type,
            time_in_force: None,
            side,
            price,
            average,
            amount,
            filled,
            remaining: Some(remaining),
            cost,
            reduce_only: None,
            post_only: Some(post_only),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(order).unwrap_or(Value::Null),
        }
    }
}

#[async_trait]
impl Exchange for Gemini {
    fn id(&self) -> ExchangeId {
        ExchangeId::Gemini
    }

    fn name(&self) -> &'static str {
        "Gemini"
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        {
            let markets = self.markets.read().unwrap();
            if !reload && !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        // Get symbol list
        let symbols: Vec<String> = self.public_get("/v1/symbols", None).await?;

        // Get detailed info for each symbol
        let details: Vec<GeminiSymbol> = self.public_get("/v1/symbols/details", None).await?;

        let details_map: HashMap<String, GeminiSymbol> = details.into_iter()
            .map(|d| (d.symbol.to_lowercase(), d))
            .collect();

        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for symbol_id in symbols {
            let detail = details_map.get(&symbol_id.to_lowercase());

            let (base, quote) = if let Some(d) = detail {
                (d.base_currency.clone(), d.quote_currency.clone())
            } else {
                // Infer from symbol (e.g., btcusd -> BTC/USD)
                if symbol_id.len() >= 6 {
                    let mid = symbol_id.len() - 3;
                    (symbol_id[..mid].to_uppercase(), symbol_id[mid..].to_uppercase())
                } else {
                    continue;
                }
            };

            let symbol = format!("{base}/{quote}");

            let (tick_size, min_amount) = if let Some(d) = detail {
                (d.quote_increment, d.min_order_size.parse::<f64>().unwrap_or(0.001))
            } else {
                (0.01, 0.001)
            };

            let precision = MarketPrecision {
                price: Some((1.0 / tick_size).log10().ceil() as i32),
                amount: Some(8),
                cost: None,
                base: None,
                quote: None,
            };

            let limits = MarketLimits {
                amount: MinMax {
                    min: Some(Decimal::from_f64_retain(min_amount).unwrap_or_default()),
                    max: None,
                },
                price: MinMax {
                    min: Some(Decimal::from_f64_retain(tick_size).unwrap_or_default()),
                    max: None,
                },
                cost: MinMax { min: None, max: None },
                leverage: MinMax { min: None, max: None },
            };

            let active = detail.map(|d| d.status == "open").unwrap_or(true);

            let market = Market {
                id: symbol_id.clone(),
                lowercase_id: Some(symbol_id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.to_lowercase(),
                quote_id: quote.to_lowercase(),
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
                settle: None,
                contract_size: None,
                linear: None,
                inverse: None,
                sub_type: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                taker: Some(Decimal::from_f64_retain(0.004).unwrap_or_default()),
                maker: Some(Decimal::from_f64_retain(0.002).unwrap_or_default()),
                percentage: true,
                tier_based: true,
                precision,
                limits,
                margin_modes: None,
                created: None,
                info: serde_json::to_value(detail).unwrap_or(Value::Null),
            };

            markets_by_id.insert(symbol_id, symbol.clone());
            markets.insert(symbol, market);
        }

        *self.markets.write().unwrap() = markets.clone();
        *self.markets_by_id.write().unwrap() = markets_by_id;

        Ok(markets)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(true).await?;
        Ok(markets.values().cloned().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let path = format!("/v1/pubticker/{market_id}");
        let ticker: GeminiTicker = self.public_get(&path, None).await?;

        let timestamp = ticker.volume.as_ref()
            .and_then(|v| v.timestamp);
        let datetime = timestamp.and_then(|t| {
            chrono::DateTime::from_timestamp_millis(t)
                .map(|dt| dt.to_rfc3339())
        });

        let (base_volume, quote_volume) = if let Some(vol) = &ticker.volume {
            let base_key = symbol.split('/').next().unwrap_or("");
            let quote_key = symbol.split('/').nth(1).unwrap_or("");
            (
                vol.volumes.get(base_key)
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(|v| Decimal::from_f64_retain(v).unwrap_or_default()),
                vol.volumes.get(quote_key)
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(|v| Decimal::from_f64_retain(v).unwrap_or_default()),
            )
        } else {
            (None, None)
        };

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime,
            high: None,
            low: None,
            bid: ticker.bid.as_ref()
                .and_then(|p| p.parse::<f64>().ok())
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            bid_volume: None,
            ask: ticker.ask.as_ref()
                .and_then(|p| p.parse::<f64>().ok())
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: ticker.last.as_ref()
                .and_then(|p| p.parse::<f64>().ok())
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            last: ticker.last.as_ref()
                .and_then(|p| p.parse::<f64>().ok())
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume,
            quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&ticker).unwrap_or(Value::Null),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        let tickers: Vec<GeminiTickerV2> = self.public_get("/v2/ticker", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut result = HashMap::new();

        for ticker in tickers {
            if let Some(symbol) = markets_by_id.get(&ticker.symbol.to_lowercase()) {
                if let Some(filter_symbols) = symbols {
                    if !filter_symbols.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let timestamp = Some(chrono::Utc::now().timestamp_millis());
                let datetime = Some(chrono::Utc::now().to_rfc3339());

                let t = Ticker {
                    symbol: symbol.clone(),
                    timestamp,
                    datetime,
                    high: ticker.high.as_ref()
                        .and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    low: ticker.low.as_ref()
                        .and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    bid: ticker.bid.as_ref()
                        .and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    bid_volume: None,
                    ask: ticker.ask.as_ref()
                        .and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    ask_volume: None,
                    vwap: None,
                    open: ticker.open.as_ref()
                        .and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    close: ticker.close.as_ref()
                        .and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    last: ticker.close.as_ref()
                        .and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    previous_close: None,
                    change: None,
                    percentage: ticker.changes.as_ref()
                        .and_then(|c| c.first())
                        .and_then(|c| c.parse::<f64>().ok())
                        .map(|c| Decimal::from_f64_retain(c * 100.0).unwrap_or_default()),
                    average: None,
                    base_volume: None,
                    quote_volume: None,
                    index_price: None,
                    mark_price: None,
                    info: serde_json::to_value(&ticker).unwrap_or(Value::Null),
                };

                result.insert(symbol.clone(), t);
            }
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit_bids".to_string(), l.to_string());
            params.insert("limit_asks".to_string(), l.to_string());
        }

        let path = format!("/v1/book/{market_id}");
        let book: GeminiOrderBook = self.public_get(&path, Some(params)).await?;

        let bids: Vec<OrderBookEntry> = book.bids.iter()
            .map(|b| {
                let price = b.price.parse::<f64>()
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or_default())
                    .unwrap_or_default();
                let amount = b.amount.parse::<f64>()
                    .map(|a| Decimal::from_f64_retain(a).unwrap_or_default())
                    .unwrap_or_default();
                OrderBookEntry { price, amount }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = book.asks.iter()
            .map(|a| {
                let price = a.price.parse::<f64>()
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or_default())
                    .unwrap_or_default();
                let amount = a.amount.parse::<f64>()
                    .map(|a| Decimal::from_f64_retain(a).unwrap_or_default())
                    .unwrap_or_default();
                OrderBookEntry { price, amount }
            })
            .collect();

        let timestamp = chrono::Utc::now().timestamp_millis();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::Utc::now().to_rfc3339()),
            nonce: None,
            bids,
            asks,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit_trades".to_string(), l.to_string());
        }
        if let Some(s) = since {
            params.insert("timestamp".to_string(), (s / 1000).to_string());
        }

        let path = format!("/v1/trades/{market_id}");
        let trades: Vec<GeminiTrade> = self.public_get(&path, Some(params)).await?;

        let result: Vec<Trade> = trades.iter()
            .map(|t| {
                let timestamp = Some(t.timestampms);
                let datetime = chrono::DateTime::from_timestamp_millis(t.timestampms)
                    .map(|dt| dt.to_rfc3339());

                let price = t.price.parse::<f64>()
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or_default())
                    .unwrap_or_default();
                let amount = t.amount.parse::<f64>()
                    .map(|a| Decimal::from_f64_retain(a).unwrap_or_default())
                    .unwrap_or_default();

                Trade {
                    id: t.tid.to_string(),
                    timestamp,
                    datetime,
                    symbol: symbol.to_string(),
                    order: None,
                    trade_type: None,
                    side: Some(t.side.to_lowercase()),
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or(Value::Null),
                }
            })
            .collect();

        Ok(result)
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let interval = self.timeframes.get(&timeframe)
            .cloned()
            .unwrap_or_else(|| "1hr".to_string());

        let path = format!("/v2/candles/{market_id}/{interval}");
        let candles: Vec<GeminiCandle> = self.public_get(&path, None).await?;

        let result: Vec<OHLCV> = candles.iter()
            .map(|c| OHLCV {
                timestamp: c.0,
                open: Decimal::from_f64_retain(c.1).unwrap_or_default(),
                high: Decimal::from_f64_retain(c.2).unwrap_or_default(),
                low: Decimal::from_f64_retain(c.3).unwrap_or_default(),
                close: Decimal::from_f64_retain(c.4).unwrap_or_default(),
                volume: Decimal::from_f64_retain(c.5).unwrap_or_default(),
            })
            .collect();

        Ok(result)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let balances: Vec<GeminiBalance> = self.private_request("/v1/balances", None).await?;

        let mut result = Balances::new();

        for balance in balances {
            let total = balance.amount.parse::<f64>()
                .map(|b| Decimal::from_f64_retain(b).unwrap_or_default())
                .unwrap_or_default();
            let free = balance.available.parse::<f64>()
                .map(|a| Decimal::from_f64_retain(a).unwrap_or_default())
                .unwrap_or_default();
            let used = total - free;

            result.currencies.insert(balance.currency.to_uppercase(), Balance {
                free: Some(free),
                used: Some(used),
                total: Some(total),
                debt: None,
            });
        }

        Ok(result)
    }

    async fn fetch_open_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let orders: Vec<GeminiOrder> = self.private_request("/v1/orders", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let result: Vec<Order> = orders.iter()
            .map(|o| {
                let mut order = self.parse_order(o);
                if let Some(s) = markets_by_id.get(&o.symbol.to_lowercase()) {
                    order.symbol = s.clone();
                }
                order
            })
            .collect();

        Ok(result)
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let params = json!({ "order_id": id });
        let order: GeminiOrder = self.private_request("/v1/order/status", Some(params)).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut result = self.parse_order(&order);
        if let Some(s) = markets_by_id.get(&order.symbol.to_lowercase()) {
            result.symbol = s.clone();
        }

        Ok(result)
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

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let side_str = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };

        let type_str = match order_type {
            OrderType::Market => "exchange market",
            OrderType::Limit => "exchange limit",
            _ => "exchange limit",
        };

        let mut params = json!({
            "symbol": market_id,
            "amount": amount.to_string(),
            "side": side_str,
            "type": type_str
        });

        if let Some(p) = price {
            params["price"] = json!(p.to_string());
        }

        let order: GeminiOrder = self.private_request("/v1/order/new", Some(params)).await?;

        let mut result = self.parse_order(&order);
        result.symbol = symbol.to_string();

        Ok(result)
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let params = json!({ "order_id": id });
        let order: GeminiOrder = self.private_request("/v1/order/cancel", Some(params)).await?;

        Ok(self.parse_order(&order))
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
        let markets = self.markets.read().unwrap();
        markets.get(symbol).map(|m| m.id.clone())
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let markets_by_id = self.markets_by_id.read().unwrap();
        markets_by_id.get(market_id).cloned()
    }

    fn sign(
        &self,
        _path: &str,
        _api: &str,
        _method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        SignedRequest {
            url: String::new(),
            method: String::new(),
            headers: HashMap::new(),
            body: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_creation() {
        let config = ExchangeConfig::new();
        let exchange = Gemini::new(config);
        assert!(exchange.is_ok());
    }

    #[test]
    fn test_order_side_parsing() {
        assert!(matches!(Gemini::parse_order_side("buy"), OrderSide::Buy));
        assert!(matches!(Gemini::parse_order_side("sell"), OrderSide::Sell));
    }

    #[test]
    fn test_order_type_parsing() {
        assert!(matches!(Gemini::parse_order_type("exchange limit"), OrderType::Limit));
        assert!(matches!(Gemini::parse_order_type("market"), OrderType::Market));
    }
}
