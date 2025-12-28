//! Crypto.com Exchange Implementation
//!
//! Crypto.com 거래소 API 구현
//! Major cryptocurrency exchange with spot, margin, and derivatives

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, TimeInForce, OHLCV,
    Trade,
};

const BASE_URL: &str = "https://api.crypto.com/exchange/v1";
const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

type HmacSha256 = Hmac<Sha256>;

/// Crypto.com exchange structure
pub struct CryptoCom {
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
struct CryptoComResponse<T> {
    code: i32,
    method: Option<String>,
    result: Option<T>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComInstrument {
    symbol: String,
    inst_type: String,
    display_name: Option<String>,
    base_ccy: String,
    quote_ccy: String,
    quote_decimals: i32,
    quantity_decimals: i32,
    price_tick_size: String,
    qty_tick_size: String,
    max_leverage: Option<String>,
    tradable: bool,
    expiry_timestamp_ms: Option<i64>,
    beta_product: Option<bool>,
    underlying_symbol: Option<String>,
    put_call: Option<String>,
    strike: Option<String>,
    contract_size: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComTicker {
    i: String,  // instrument name
    h: Option<String>,  // high
    l: Option<String>,  // low
    a: Option<String>,  // last trade price
    v: Option<String>,  // 24h volume
    vv: Option<String>,  // 24h volume in quote
    c: Option<String>,  // 24h price change
    b: Option<String>,  // best bid
    k: Option<String>,  // best ask
    t: i64,  // timestamp
    oi: Option<String>,  // open interest
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComOrderBookData {
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
    t: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComTrade {
    d: String,  // trade id
    s: String,  // side
    p: String,  // price
    q: String,  // quantity
    t: i64,     // timestamp
    i: String,  // instrument name
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComCandle {
    t: i64,     // timestamp
    o: String,  // open
    h: String,  // high
    l: String,  // low
    c: String,  // close
    v: String,  // volume
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComBalance {
    currency: String,
    balance: String,
    available: String,
    order: String,
    stake: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComOrder {
    order_id: String,
    client_oid: Option<String>,
    account_id: Option<String>,
    instrument_name: String,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    price: Option<String>,
    quantity: String,
    cumulative_quantity: Option<String>,
    cumulative_value: Option<String>,
    avg_price: Option<String>,
    status: String,
    time_in_force: Option<String>,
    create_time: i64,
    update_time: Option<i64>,
    exec_inst: Option<Vec<String>>,
    trigger_price: Option<String>,
    fee_currency: Option<String>,
    fee_amount: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComInstrumentsResult {
    data: Vec<CryptoComInstrument>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComTickerResult {
    data: Vec<CryptoComTicker>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComOrderBookResult {
    data: Vec<CryptoComOrderBookData>,
    instrument_name: String,
    depth: i32,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComTradesResult {
    data: Vec<CryptoComTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComCandlesResult {
    data: Vec<CryptoComCandle>,
    instrument_name: String,
    interval: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComBalanceResult {
    data: Vec<CryptoComBalance>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComOrderResult {
    order_info: CryptoComOrder,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptoComOrdersResult {
    data: Vec<CryptoComOrder>,
}

impl CryptoCom {
    /// Create a new Crypto.com exchange instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: true,
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
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            ws: true,
            watch_ticker: true,
            watch_order_book: true,
            watch_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".to_string(), BASE_URL.to_string());
        api_urls.insert("private".to_string(), BASE_URL.to_string());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/147792121-38ed5e36-c229-48d6-b49a-48d05fc19ed4.jpeg".to_string()),
            api: api_urls,
            www: Some("https://crypto.com".to_string()),
            doc: vec!["https://exchange-docs.crypto.com/exchange/v1/rest-ws/index.html".to_string()],
            fees: Some("https://crypto.com/exchange/document/fees-limits".to_string()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Hour6, "6h".into());
        timeframes.insert(Timeframe::Hour12, "12h".into());
        timeframes.insert(Timeframe::Day1, "1D".into());
        timeframes.insert(Timeframe::Week1, "7D".into());
        timeframes.insert(Timeframe::Month1, "1M".into());

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

    /// Generate HMAC-SHA256 signature
    fn sign_message(&self, message: &str) -> CcxtResult<String> {
        let secret = self.config.secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".to_string(),
            })?;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("HMAC error: {e}"),
            })?;
        mac.update(message.as_bytes());
        let result = mac.finalize();
        Ok(hex::encode(result.into_bytes()))
    }

    /// Make authenticated request
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> CcxtResult<T> {
        let api_key = self.config.api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".to_string(),
            })?;

        self.rate_limiter.throttle(1.0).await;

        let nonce = chrono::Utc::now().timestamp_millis();
        let id = nonce;

        let params_value = params.unwrap_or(json!({}));

        // Build signature string
        let params_str = if params_value.is_object() {
            let obj = params_value.as_object().unwrap();
            let mut sorted_keys: Vec<_> = obj.keys().collect();
            sorted_keys.sort();
            sorted_keys.iter()
                .map(|k| format!("{}{}", k, obj[*k]))
                .collect::<Vec<_>>()
                .join("")
        } else {
            String::new()
        };

        let sig_payload = format!("{method}{id}{api_key}{params_str}", );
        let signature = self.sign_message(&sig_payload)?;

        let body = json!({
            "id": id,
            "method": method,
            "api_key": api_key,
            "params": params_value,
            "sig": signature,
            "nonce": nonce
        });

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let response: CryptoComResponse<T> = self.client.post("/private", Some(body), Some(headers)).await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: format!("API error code: {}", response.code),
            });
        }

        response.result.ok_or_else(|| CcxtError::ExchangeError {
            message: "No result in response".to_string(),
        })
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
    fn parse_order_status(status: &str) -> OrderStatus {
        match status.to_uppercase().as_str() {
            "NEW" | "PENDING" | "ACTIVE" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" | "CANCELLED" | "EXPIRED" | "REJECTED" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        }
    }

    /// Parse order side
    fn parse_order_side(side: &str) -> OrderSide {
        match side.to_uppercase().as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        }
    }

    /// Parse order type
    fn parse_order_type(order_type: &str) -> OrderType {
        match order_type.to_uppercase().as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            "STOP_LIMIT" => OrderType::StopLimit,
            "STOP_MARKET" | "STOP_LOSS" => OrderType::StopMarket,
            "TAKE_PROFIT" => OrderType::TakeProfit,
            "TAKE_PROFIT_LIMIT" => OrderType::TakeProfitLimit,
            _ => OrderType::Limit,
        }
    }

    /// Convert order to unified format
    fn parse_order(&self, order: &CryptoComOrder) -> Order {
        let timestamp = Some(order.create_time);
        let datetime = chrono::DateTime::from_timestamp_millis(order.create_time)
            .map(|dt| dt.to_rfc3339());

        let side = Self::parse_order_side(&order.side);
        let status = Self::parse_order_status(&order.status);
        let order_type = Self::parse_order_type(&order.order_type);

        let amount = order.quantity.parse::<f64>()
            .map(|q| Decimal::from_f64_retain(q).unwrap_or_default())
            .unwrap_or_default();
        let filled = order.cumulative_quantity.as_ref()
            .and_then(|q| q.parse::<f64>().ok())
            .map(|q| Decimal::from_f64_retain(q).unwrap_or_default())
            .unwrap_or_default();
        let remaining = amount - filled;
        let price = order.price.as_ref()
            .and_then(|p| p.parse::<f64>().ok())
            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default());
        let average = order.avg_price.as_ref()
            .and_then(|p| p.parse::<f64>().ok())
            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default());
        let stop_price = order.trigger_price.as_ref()
            .and_then(|p| p.parse::<f64>().ok())
            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default());

        let cost = order.cumulative_value.as_ref()
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| Decimal::from_f64_retain(v).unwrap_or_default());

        let time_in_force = order.time_in_force.as_ref().map(|tif| {
            match tif.to_uppercase().as_str() {
                "GTC" | "GOOD_TIL_CANCEL" => TimeInForce::GTC,
                "IOC" | "IMMEDIATE_OR_CANCEL" => TimeInForce::IOC,
                "FOK" | "FILL_OR_KILL" => TimeInForce::FOK,
                _ => TimeInForce::GTC,
            }
        });

        let post_only = order.exec_inst.as_ref()
            .map(|insts| insts.iter().any(|i| i == "POST_ONLY"))
            .unwrap_or(false);
        let reduce_only = order.exec_inst.as_ref()
            .map(|insts| insts.iter().any(|i| i == "REDUCE_ONLY"))
            .unwrap_or(false);

        let fee = if let (Some(currency), Some(amount)) = (&order.fee_currency, &order.fee_amount) {
            Some(crate::types::Fee {
                currency: Some(currency.clone()),
                cost: amount.parse::<f64>().ok()
                    .map(|a| Decimal::from_f64_retain(a).unwrap_or_default()),
                rate: None,
            })
        } else {
            None
        };

        Order {
            id: order.order_id.clone(),
            client_order_id: order.client_oid.clone(),
            timestamp,
            datetime,
            last_trade_timestamp: order.update_time,
            last_update_timestamp: order.update_time,
            status,
            symbol: order.instrument_name.clone(),
            order_type,
            time_in_force,
            side,
            price,
            average,
            amount,
            filled,
            remaining: Some(remaining),
            cost,
            reduce_only: Some(reduce_only),
            post_only: Some(post_only),
            stop_price,
            trigger_price: stop_price,
            take_profit_price: None,
            stop_loss_price: None,
            trades: Vec::new(),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(order).unwrap_or(Value::Null),
        }
    }
}

#[async_trait]
impl Exchange for CryptoCom {
    fn id(&self) -> ExchangeId {
        ExchangeId::CryptoCom
    }

    fn name(&self) -> &'static str {
        "Crypto.com"
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        {
            let markets = self.markets.read().unwrap();
            if !reload && !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let response: CryptoComResponse<CryptoComInstrumentsResult> = self.public_get("/public/get-instruments", None).await?;

        let instruments = response.result
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "No instruments in response".to_string(),
            })?;

        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for inst in instruments.data {
            let is_spot = inst.inst_type == "SPOT";
            let is_future = inst.inst_type == "PERPETUAL" || inst.inst_type == "FUTURE";
            let is_option = inst.inst_type == "OPTION";
            let is_perpetual = inst.inst_type == "PERPETUAL";

            let symbol = if is_spot {
                format!("{}/{}", inst.base_ccy, inst.quote_ccy)
            } else {
                format!("{}/{}:{}", inst.base_ccy, inst.quote_ccy, inst.quote_ccy)
            };

            let market_type = if is_option {
                MarketType::Option
            } else if is_perpetual {
                MarketType::Swap
            } else if is_future {
                MarketType::Future
            } else {
                MarketType::Spot
            };

            let tick_size = inst.price_tick_size.parse::<f64>().unwrap_or(0.01);
            let qty_tick = inst.qty_tick_size.parse::<f64>().unwrap_or(0.001);

            let precision = MarketPrecision {
                price: Some(inst.quote_decimals),
                amount: Some(inst.quantity_decimals),
                cost: None,
                base: None,
                quote: None,
            };

            let contract_size = inst.contract_size.as_ref()
                .and_then(|s| s.parse::<f64>().ok())
                .map(|s| Decimal::from_f64_retain(s).unwrap_or(Decimal::ONE))
                .unwrap_or(Decimal::ONE);

            let limits = MarketLimits {
                amount: MinMax {
                    min: Some(Decimal::from_f64_retain(qty_tick).unwrap_or_default()),
                    max: None,
                },
                price: MinMax {
                    min: Some(Decimal::from_f64_retain(tick_size).unwrap_or_default()),
                    max: None,
                },
                cost: MinMax { min: None, max: None },
                leverage: MinMax {
                    min: Some(Decimal::ONE),
                    max: inst.max_leverage.as_ref()
                        .and_then(|l| l.parse::<i32>().ok())
                        .map(Decimal::from),
                },
            };

            let market = Market {
                id: inst.symbol.clone(),
                lowercase_id: Some(inst.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: inst.base_ccy.clone(),
                quote: inst.quote_ccy.clone(),
                base_id: inst.base_ccy.clone(),
                quote_id: inst.quote_ccy.clone(),
                settle_id: if is_future { Some(inst.quote_ccy.clone()) } else { None },
                active: inst.tradable,
                market_type,
                spot: is_spot,
                margin: !is_spot,
                swap: is_perpetual,
                future: is_future && !is_perpetual,
                option: is_option,
                index: false,
                contract: !is_spot,
                settle: if is_future { Some(inst.quote_ccy.clone()) } else { None },
                contract_size: if is_future { Some(contract_size) } else { None },
                linear: Some(!is_spot),
                inverse: Some(false),
                sub_type: None,
                expiry: inst.expiry_timestamp_ms,
                expiry_datetime: inst.expiry_timestamp_ms.map(|e| {
                    chrono::DateTime::from_timestamp_millis(e)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                strike: inst.strike.as_ref()
                    .and_then(|s| s.parse::<f64>().ok())
                    .map(|s| Decimal::from_f64_retain(s).unwrap_or_default()),
                option_type: inst.put_call.clone(),
                taker: Some(Decimal::from_f64_retain(0.004).unwrap_or_default()),
                maker: Some(Decimal::from_f64_retain(0.004).unwrap_or_default()),
                percentage: true,
                tier_based: true,
                precision,
                limits,
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&inst).unwrap_or(Value::Null),
            };

            markets_by_id.insert(inst.symbol.clone(), symbol.clone());
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

        let mut params = HashMap::new();
        params.insert("instrument_name".to_string(), market_id);

        let response: CryptoComResponse<CryptoComTickerResult> = self.public_get("/public/get-ticker", Some(params)).await?;

        let tickers = response.result
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "No ticker data".to_string(),
            })?;

        let ticker = tickers.data.first()
            .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?;

        let timestamp = Some(ticker.t);
        let datetime = chrono::DateTime::from_timestamp_millis(ticker.t)
            .map(|dt| dt.to_rfc3339());

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime,
            high: ticker.h.as_ref().and_then(|p| p.parse::<f64>().ok())
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            low: ticker.l.as_ref().and_then(|p| p.parse::<f64>().ok())
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            bid: ticker.b.as_ref().and_then(|p| p.parse::<f64>().ok())
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            bid_volume: None,
            ask: ticker.k.as_ref().and_then(|p| p.parse::<f64>().ok())
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: ticker.a.as_ref().and_then(|p| p.parse::<f64>().ok())
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            last: ticker.a.as_ref().and_then(|p| p.parse::<f64>().ok())
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            previous_close: None,
            change: None,
            percentage: ticker.c.as_ref().and_then(|p| p.parse::<f64>().ok())
                .map(|p| Decimal::from_f64_retain(p * 100.0).unwrap_or_default()),
            average: None,
            base_volume: ticker.v.as_ref().and_then(|v| v.parse::<f64>().ok())
                .map(|v| Decimal::from_f64_retain(v).unwrap_or_default()),
            quote_volume: ticker.vv.as_ref().and_then(|v| v.parse::<f64>().ok())
                .map(|v| Decimal::from_f64_retain(v).unwrap_or_default()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(ticker).unwrap_or(Value::Null),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        let response: CryptoComResponse<CryptoComTickerResult> = self.public_get("/public/get-ticker", None).await?;

        let tickers = response.result
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "No ticker data".to_string(),
            })?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut result = HashMap::new();

        for ticker in tickers.data {
            if let Some(symbol) = markets_by_id.get(&ticker.i) {
                if let Some(filter_symbols) = symbols {
                    if !filter_symbols.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let timestamp = Some(ticker.t);
                let datetime = chrono::DateTime::from_timestamp_millis(ticker.t)
                    .map(|dt| dt.to_rfc3339());

                let t = Ticker {
                    symbol: symbol.clone(),
                    timestamp,
                    datetime,
                    high: ticker.h.as_ref().and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    low: ticker.l.as_ref().and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    bid: ticker.b.as_ref().and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    bid_volume: None,
                    ask: ticker.k.as_ref().and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    ask_volume: None,
                    vwap: None,
                    open: None,
                    close: ticker.a.as_ref().and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    last: ticker.a.as_ref().and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    previous_close: None,
                    change: None,
                    percentage: ticker.c.as_ref().and_then(|p| p.parse::<f64>().ok())
                        .map(|p| Decimal::from_f64_retain(p * 100.0).unwrap_or_default()),
                    average: None,
                    base_volume: ticker.v.as_ref().and_then(|v| v.parse::<f64>().ok())
                        .map(|v| Decimal::from_f64_retain(v).unwrap_or_default()),
                    quote_volume: ticker.vv.as_ref().and_then(|v| v.parse::<f64>().ok())
                        .map(|v| Decimal::from_f64_retain(v).unwrap_or_default()),
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
        params.insert("instrument_name".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("depth".to_string(), l.to_string());
        }

        let response: CryptoComResponse<CryptoComOrderBookResult> = self.public_get("/public/get-book", Some(params)).await?;

        let book = response.result
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "No order book data".to_string(),
            })?;

        let book_data = book.data.first()
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "Empty order book".to_string(),
            })?;

        let bids: Vec<OrderBookEntry> = book_data.bids.iter()
            .filter_map(|b| {
                if b.len() >= 2 {
                    let price = b[0].parse::<f64>().ok()?;
                    let amount = b[1].parse::<f64>().ok()?;
                    Some(OrderBookEntry {
                        price: Decimal::from_f64_retain(price).unwrap_or_default(),
                        amount: Decimal::from_f64_retain(amount).unwrap_or_default(),
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = book_data.asks.iter()
            .filter_map(|a| {
                if a.len() >= 2 {
                    let price = a[0].parse::<f64>().ok()?;
                    let amount = a[1].parse::<f64>().ok()?;
                    Some(OrderBookEntry {
                        price: Decimal::from_f64_retain(price).unwrap_or_default(),
                        amount: Decimal::from_f64_retain(amount).unwrap_or_default(),
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(book_data.t),
            datetime: chrono::DateTime::from_timestamp_millis(book_data.t)
                .map(|dt| dt.to_rfc3339()),
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
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let mut params = HashMap::new();
        params.insert("instrument_name".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }

        let response: CryptoComResponse<CryptoComTradesResult> = self.public_get("/public/get-trades", Some(params)).await?;

        let trades = response.result
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "No trades data".to_string(),
            })?;

        let result: Vec<Trade> = trades.data.iter()
            .map(|t| {
                let timestamp = Some(t.t);
                let datetime = chrono::DateTime::from_timestamp_millis(t.t)
                    .map(|dt| dt.to_rfc3339());

                let price = t.p.parse::<f64>()
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or_default())
                    .unwrap_or_default();
                let amount = t.q.parse::<f64>()
                    .map(|q| Decimal::from_f64_retain(q).unwrap_or_default())
                    .unwrap_or_default();

                Trade {
                    id: t.d.clone(),
                    timestamp,
                    datetime,
                    symbol: symbol.to_string(),
                    order: None,
                    trade_type: None,
                    side: Some(t.s.to_lowercase()),
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
        since: Option<i64>,
        limit: Option<u32>,
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
            .unwrap_or_else(|| "1h".to_string());

        let mut params = HashMap::new();
        params.insert("instrument_name".to_string(), market_id);
        params.insert("timeframe".to_string(), interval);
        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }
        if let Some(s) = since {
            params.insert("start_ts".to_string(), s.to_string());
        }

        let response: CryptoComResponse<CryptoComCandlesResult> = self.public_get("/public/get-candlestick", Some(params)).await?;

        let candles = response.result
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "No OHLCV data".to_string(),
            })?;

        let result: Vec<OHLCV> = candles.data.iter()
            .map(|c| OHLCV {
                timestamp: c.t,
                open: c.o.parse::<f64>()
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or_default())
                    .unwrap_or_default(),
                high: c.h.parse::<f64>()
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or_default())
                    .unwrap_or_default(),
                low: c.l.parse::<f64>()
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or_default())
                    .unwrap_or_default(),
                close: c.c.parse::<f64>()
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or_default())
                    .unwrap_or_default(),
                volume: c.v.parse::<f64>()
                    .map(|v| Decimal::from_f64_retain(v).unwrap_or_default())
                    .unwrap_or_default(),
            })
            .collect();

        Ok(result)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let result_data: CryptoComBalanceResult = self.private_request(
            "private/user-balance",
            None,
        ).await?;

        let mut result = Balances::new();

        for balance in result_data.data {
            let total = balance.balance.parse::<f64>()
                .map(|b| Decimal::from_f64_retain(b).unwrap_or_default())
                .unwrap_or_default();
            let free = balance.available.parse::<f64>()
                .map(|a| Decimal::from_f64_retain(a).unwrap_or_default())
                .unwrap_or_default();
            let used = total - free;

            result.currencies.insert(balance.currency.clone(), Balance {
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
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let params = if let Some(sym) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                markets.get(sym)
                    .map(|m| m.id.clone())
                    .ok_or_else(|| CcxtError::BadSymbol { symbol: sym.to_string() })?
            };
            json!({ "instrument_name": market_id })
        } else {
            json!({})
        };

        let orders: CryptoComOrdersResult = self.private_request(
            "private/get-open-orders",
            Some(params),
        ).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let result: Vec<Order> = orders.data.iter()
            .map(|o| {
                let mut order = self.parse_order(o);
                if let Some(s) = markets_by_id.get(&o.instrument_name) {
                    order.symbol = s.clone();
                }
                order
            })
            .collect();

        Ok(result)
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let params = json!({ "order_id": id });
        let result: CryptoComOrderResult = self.private_request(
            "private/get-order-detail",
            Some(params),
        ).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut order = self.parse_order(&result.order_info);
        if let Some(s) = markets_by_id.get(&result.order_info.instrument_name) {
            order.symbol = s.clone();
        }

        Ok(order)
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
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };

        let type_str = match order_type {
            OrderType::Market => "MARKET",
            OrderType::Limit => "LIMIT",
            OrderType::StopMarket => "STOP_LOSS",
            OrderType::StopLimit => "STOP_LIMIT",
            _ => "LIMIT",
        };

        let mut params = json!({
            "instrument_name": market_id,
            "side": side_str,
            "type": type_str,
            "quantity": amount.to_string()
        });

        if let Some(p) = price {
            params["price"] = json!(p.to_string());
        }

        let result: CryptoComOrderResult = self.private_request(
            "private/create-order",
            Some(params),
        ).await?;

        let mut order = self.parse_order(&result.order_info);
        order.symbol = symbol.to_string();

        Ok(order)
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let params = json!({
            "order_id": id,
            "instrument_name": market_id
        });

        let result: CryptoComOrderResult = self.private_request(
            "private/cancel-order",
            Some(params),
        ).await?;

        let mut order = self.parse_order(&result.order_info);
        order.symbol = symbol.to_string();

        Ok(order)
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
    fn test_cryptocom_creation() {
        let config = ExchangeConfig::new();
        let exchange = CryptoCom::new(config);
        assert!(exchange.is_ok());
    }

    #[test]
    fn test_order_status_parsing() {
        assert!(matches!(CryptoCom::parse_order_status("NEW"), OrderStatus::Open));
        assert!(matches!(CryptoCom::parse_order_status("FILLED"), OrderStatus::Closed));
        assert!(matches!(CryptoCom::parse_order_status("CANCELED"), OrderStatus::Canceled));
    }

    #[test]
    fn test_order_side_parsing() {
        assert!(matches!(CryptoCom::parse_order_side("BUY"), OrderSide::Buy));
        assert!(matches!(CryptoCom::parse_order_side("SELL"), OrderSide::Sell));
    }
}
