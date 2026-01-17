//! BitTrade (Japan) Exchange Implementation
//!
//! BitTrade (formerly Huobi Japan) API implementation
//! API Documentation: <https://api-doc.bittrade.co.jp/>

#![allow(dead_code)]

use async_trait::async_trait;
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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// BitTrade (Japan) Exchange
pub struct Bittrade {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    account_id: RwLock<Option<String>>,
}

// API Response structures
#[derive(Debug, Deserialize, Serialize)]
struct BittradeResponse<T> {
    status: String,
    data: Option<T>,
    tick: Option<T>,
    ts: Option<i64>,
    #[serde(rename = "err-code")]
    err_code: Option<String>,
    #[serde(rename = "err-msg")]
    err_msg: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BittradeSymbol {
    symbol: String,
    #[serde(rename = "base-currency")]
    base_currency: String,
    #[serde(rename = "quote-currency")]
    quote_currency: String,
    #[serde(rename = "price-precision")]
    price_precision: i32,
    #[serde(rename = "amount-precision")]
    amount_precision: i32,
    #[serde(rename = "symbol-partition")]
    symbol_partition: Option<String>,
    state: String,
    #[serde(rename = "min-order-amt")]
    min_order_amt: Option<String>,
    #[serde(rename = "max-order-amt")]
    max_order_amt: Option<String>,
    #[serde(rename = "min-order-value")]
    min_order_value: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BittradeTickerWrapper {
    amount: Option<Decimal>,
    open: Option<Decimal>,
    close: Option<Decimal>,
    high: Option<Decimal>,
    low: Option<Decimal>,
    vol: Option<Decimal>,
    bid: Vec<Decimal>,
    ask: Vec<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BittradeAllTicker {
    symbol: String,
    open: Option<Decimal>,
    high: Option<Decimal>,
    low: Option<Decimal>,
    close: Option<Decimal>,
    amount: Option<Decimal>,
    vol: Option<Decimal>,
    bid: Option<Decimal>,
    #[serde(rename = "bidSize")]
    bid_size: Option<Decimal>,
    ask: Option<Decimal>,
    #[serde(rename = "askSize")]
    ask_size: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct BittradeOrderBook {
    bids: Vec<Vec<Decimal>>,
    asks: Vec<Vec<Decimal>>,
    ts: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BittradeTrade {
    id: u64,
    ts: i64,
    #[serde(rename = "trade-id")]
    trade_id: Option<u64>,
    amount: Decimal,
    price: Decimal,
    direction: String,
}

#[derive(Debug, Deserialize)]
struct BittradeTradeWrapper {
    data: Vec<BittradeTrade>,
    ts: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct BittradeKline {
    id: i64,
    open: Decimal,
    close: Decimal,
    high: Decimal,
    low: Decimal,
    amount: Decimal,
    vol: Decimal,
}

#[derive(Debug, Deserialize)]
struct BittradeAccount {
    id: i64,
    #[serde(rename = "type")]
    account_type: String,
    state: String,
}

#[derive(Debug, Deserialize)]
struct BittradeBalanceData {
    id: i64,
    #[serde(rename = "type")]
    account_type: String,
    state: String,
    list: Vec<BittradeBalanceItem>,
}

#[derive(Debug, Deserialize)]
struct BittradeBalanceItem {
    currency: String,
    #[serde(rename = "type")]
    balance_type: String,
    balance: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct BittradeOrderResponse {
    data: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BittradeOrderData {
    id: u64,
    symbol: String,
    #[serde(rename = "account-id")]
    account_id: i64,
    amount: String,
    price: Option<String>,
    #[serde(rename = "created-at")]
    created_at: i64,
    #[serde(rename = "type")]
    order_type: String,
    #[serde(rename = "field-amount")]
    field_amount: Option<String>,
    #[serde(rename = "field-cash-amount")]
    field_cash_amount: Option<String>,
    #[serde(rename = "field-fees")]
    field_fees: Option<String>,
    state: String,
    #[serde(rename = "client-order-id")]
    client_order_id: Option<String>,
}

impl Bittrade {
    const BASE_URL: &'static str = "https://api-cloud.bittrade.co.jp";
    const RATE_LIMIT_MS: u64 = 100;

    /// Create new BitTrade instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

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
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: false,
            fetch_withdrawals: false,
            withdraw: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/100386178-e0aa0a00-3062-11eb-9a87-d75cd7e7fd86.jpg".into()),
            api: api_urls,
            www: Some("https://www.bittrade.co.jp".into()),
            doc: vec!["https://api-doc.bittrade.co.jp/".into()],
            fees: Some("https://www.bittrade.co.jp/ja-jp/fee/".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1min".into());
        timeframes.insert(Timeframe::Minute5, "5min".into());
        timeframes.insert(Timeframe::Minute15, "15min".into());
        timeframes.insert(Timeframe::Minute30, "30min".into());
        timeframes.insert(Timeframe::Hour1, "60min".into());
        timeframes.insert(Timeframe::Hour4, "4hour".into());
        timeframes.insert(Timeframe::Day1, "1day".into());
        timeframes.insert(Timeframe::Week1, "1week".into());
        timeframes.insert(Timeframe::Month1, "1mon".into());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            account_id: RwLock::new(None),
        })
    }

    /// Convert symbol to market ID (BTC/JPY -> btcjpy)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "").to_lowercase()
    }

    /// Convert market ID to symbol (btcjpy -> BTC/JPY)
    fn to_symbol(&self, market_id: &str) -> String {
        let markets_by_id = self.markets_by_id.read().unwrap();
        markets_by_id
            .get(market_id)
            .cloned()
            .unwrap_or_else(|| market_id.to_uppercase())
    }

    /// Public API call
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Private API call with HMAC-SHA256 signature
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
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
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();

        let mut sign_params = HashMap::new();
        sign_params.insert("AccessKeyId".to_string(), api_key.to_string());
        sign_params.insert("SignatureMethod".to_string(), "HmacSHA256".to_string());
        sign_params.insert("SignatureVersion".to_string(), "2".to_string());
        sign_params.insert("Timestamp".to_string(), timestamp);

        // Sort and encode parameters
        let mut sorted_params: Vec<_> = sign_params.iter().collect();
        sorted_params.sort_by(|a, b| a.0.cmp(b.0));

        let query_string: String = sorted_params
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        let host = "api-cloud.bittrade.co.jp";
        let payload = format!("{method}\n{host}\n{path}\n{query_string}");

        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(payload.as_bytes());
        let signature = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            mac.finalize().into_bytes(),
        );

        sign_params.insert("Signature".to_string(), signature);

        let url = format!(
            "{}?{}",
            path,
            sign_params
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&")
        );

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        if method == "GET" {
            self.client.get(&url, None, Some(headers)).await
        } else {
            let body = serde_json::to_value(&params).ok();
            self.client.post(&url, body, Some(headers)).await
        }
    }

    /// Get account ID
    async fn get_account_id(&self) -> CcxtResult<String> {
        {
            let account_id = self.account_id.read().unwrap();
            if let Some(id) = account_id.as_ref() {
                return Ok(id.clone());
            }
        }

        let response: BittradeResponse<Vec<BittradeAccount>> = self
            .private_request("GET", "/v1/account/accounts", HashMap::new())
            .await?;

        let accounts = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to get accounts".into(),
        })?;

        let account = accounts
            .into_iter()
            .find(|a| a.account_type == "spot")
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "Spot account not found".into(),
            })?;

        let id = account.id.to_string();
        {
            let mut account_id = self.account_id.write().unwrap();
            *account_id = Some(id.clone());
        }
        Ok(id)
    }

    /// Parse order status
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status {
            "created" | "submitted" => OrderStatus::Open,
            "partial-filled" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "partial-canceled" | "canceled" => OrderStatus::Canceled,
            "canceling" => OrderStatus::Open,
            _ => OrderStatus::Open,
        }
    }

    /// Parse order side
    fn parse_order_side(&self, order_type: &str) -> OrderSide {
        if order_type.contains("buy") {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        }
    }

    /// Parse order type
    fn parse_order_type(&self, order_type: &str) -> OrderType {
        if order_type.contains("market") {
            OrderType::Market
        } else {
            OrderType::Limit
        }
    }
}

#[async_trait]
impl Exchange for Bittrade {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bittrade
    }

    fn name(&self) -> &str {
        "BitTrade"
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
        let markets_by_id = self.markets_by_id.read().unwrap();
        markets_by_id.get(market_id).cloned()
    }

    fn sign(
        &self,
        path: &str,
        _api: &str,
        method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        let url = format!("{}{}", Self::BASE_URL, path);
        SignedRequest {
            url,
            method: method.to_string(),
            headers: HashMap::new(),
            body: body.map(|s| s.to_string()),
        }
    }

    async fn load_markets(&self, _reload: bool) -> CcxtResult<HashMap<String, Market>> {
        let response: BittradeResponse<Vec<BittradeSymbol>> =
            self.public_get("/v1/common/symbols", None).await?;

        let symbols = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to load markets".into(),
        })?;

        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for symbol_data in symbols {
            if symbol_data.state != "online" {
                continue;
            }

            let base = symbol_data.base_currency.to_uppercase();
            let quote = symbol_data.quote_currency.to_uppercase();
            let symbol = format!("{base}/{quote}");
            let market_id = symbol_data.symbol.clone();

            let market = Market {
                id: market_id.clone(),
                lowercase_id: Some(market_id.clone()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: symbol_data.base_currency.clone(),
                quote_id: symbol_data.quote_currency.clone(),
                settle: None,
                settle_id: None,
                active: true,
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
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
            underlying: None,
            underlying_id: None,
                taker: Some(Decimal::new(2, 3)), // 0.2%
                maker: Some(Decimal::new(2, 3)), // 0.2%
                percentage: true,
                tier_based: false,
                precision: MarketPrecision {
                    amount: Some(symbol_data.amount_precision),
                    price: Some(symbol_data.price_precision),
                    cost: None,
                    base: Some(symbol_data.amount_precision),
                    quote: Some(symbol_data.price_precision),
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax {
                        min: symbol_data
                            .min_order_amt
                            .as_ref()
                            .and_then(|v| v.parse().ok()),
                        max: symbol_data
                            .max_order_amt
                            .as_ref()
                            .and_then(|v| v.parse().ok()),
                    },
                    price: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                    cost: crate::types::MinMax {
                        min: symbol_data
                            .min_order_value
                            .as_ref()
                            .and_then(|v| v.parse().ok()),
                        max: None,
                    },
                    leverage: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&symbol_data).unwrap_or_default(),
            };

            markets_by_id.insert(market_id, symbol.clone());
            markets.insert(symbol, market);
        }

        *self.markets.write().unwrap() = markets.clone();
        *self.markets_by_id.write().unwrap() = markets_by_id;

        Ok(markets)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(false).await?;
        Ok(markets.into_values().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        let response: BittradeResponse<BittradeTickerWrapper> = self
            .public_get("/market/detail/merged", Some(params))
            .await?;

        let ticker_data = response.tick.ok_or_else(|| CcxtError::ExchangeError {
            message: "Ticker data not found".into(),
        })?;

        let timestamp = response.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: ticker_data.high,
            low: ticker_data.low,
            bid: ticker_data.bid.first().copied(),
            bid_volume: ticker_data.bid.get(1).copied(),
            ask: ticker_data.ask.first().copied(),
            ask_volume: ticker_data.ask.get(1).copied(),
            vwap: None,
            open: ticker_data.open,
            close: ticker_data.close,
            last: ticker_data.close,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: ticker_data.amount,
            quote_volume: ticker_data.vol,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&ticker_data).unwrap_or_default(),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: BittradeResponse<Vec<BittradeAllTicker>> =
            self.public_get("/market/tickers", None).await?;

        let tickers_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Tickers data not found".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();
        let mut tickers = HashMap::new();

        for ticker_data in tickers_data {
            let symbol = self.to_symbol(&ticker_data.symbol);

            if let Some(filter_symbols) = symbols {
                if !filter_symbols.contains(&symbol.as_str()) {
                    continue;
                }
            }

            let ticker = Ticker {
                symbol: symbol.clone(),
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                high: ticker_data.high,
                low: ticker_data.low,
                bid: ticker_data.bid,
                bid_volume: ticker_data.bid_size,
                ask: ticker_data.ask,
                ask_volume: ticker_data.ask_size,
                vwap: None,
                open: ticker_data.open,
                close: ticker_data.close,
                last: ticker_data.close,
                previous_close: None,
                change: None,
                percentage: None,
                average: None,
                base_volume: ticker_data.amount,
                quote_volume: ticker_data.vol,
                index_price: None,
                mark_price: None,
                info: serde_json::to_value(&ticker_data).unwrap_or_default(),
            };

            tickers.insert(symbol, ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let depth = limit.unwrap_or(20).min(150);
        let depth_type = if depth <= 5 {
            "step0"
        } else if depth <= 20 {
            "step1"
        } else {
            "step2"
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("type".to_string(), depth_type.to_string());

        let response: BittradeResponse<BittradeOrderBook> =
            self.public_get("/market/depth", Some(params)).await?;

        let ob_data = response.tick.ok_or_else(|| CcxtError::ExchangeError {
            message: "Order book data not found".into(),
        })?;

        let timestamp = response.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = ob_data
            .bids
            .iter()
            .filter_map(|b| {
                if b.len() >= 2 {
                    Some(OrderBookEntry {
                        price: b[0],
                        amount: b[1],
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = ob_data
            .asks
            .iter()
            .filter_map(|a| {
                if a.len() >= 2 {
                    Some(OrderBookEntry {
                        price: a[0],
                        amount: a[1],
                    })
                } else {
                    None
                }
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
        let size = limit.unwrap_or(100).min(2000);

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("size".to_string(), size.to_string());

        let response: BittradeResponse<Vec<BittradeTradeWrapper>> = self
            .public_get("/market/history/trade", Some(params))
            .await?;

        let trade_wrappers = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Trade data not found".into(),
        })?;

        let mut trades = Vec::new();

        for wrapper in trade_wrappers {
            for trade_data in wrapper.data {
                let side = if trade_data.direction == "buy" {
                    OrderSide::Buy
                } else {
                    OrderSide::Sell
                };

                let price = trade_data.price;
                let amount = trade_data.amount;

                trades.push(Trade {
                    id: trade_data.trade_id.unwrap_or(trade_data.id).to_string(),
                    order: None,
                    timestamp: Some(trade_data.ts),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(trade_data.ts)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(
                        match side {
                            OrderSide::Buy => "buy",
                            OrderSide::Sell => "sell",
                        }
                        .to_string(),
                    ),
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: vec![],
                    info: serde_json::to_value(&trade_data).unwrap_or_default(),
                });
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
        let market_id = self.to_market_id(symbol);
        let period = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;
        let size = limit.unwrap_or(300).min(2000);

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("period".to_string(), period.clone());
        params.insert("size".to_string(), size.to_string());

        let response: BittradeResponse<Vec<BittradeKline>> = self
            .public_get("/market/history/kline", Some(params))
            .await?;

        let klines = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Kline data not found".into(),
        })?;

        let mut ohlcv_list: Vec<OHLCV> = klines
            .iter()
            .map(|k| OHLCV {
                timestamp: k.id * 1000,
                open: k.open,
                high: k.high,
                low: k.low,
                close: k.close,
                volume: k.amount,
            })
            .collect();

        // Sort by timestamp
        ohlcv_list.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        // Filter by since
        if let Some(since_ts) = since {
            ohlcv_list.retain(|o| o.timestamp >= since_ts);
        }

        Ok(ohlcv_list)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let account_id = self.get_account_id().await?;
        let path = format!("/v1/account/accounts/{account_id}/balance");

        let response: BittradeResponse<BittradeBalanceData> =
            self.private_request("GET", &path, HashMap::new()).await?;

        let balance_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Balance data not found".into(),
        })?;

        let mut currencies = HashMap::new();

        // Group by currency
        let mut currency_balances: HashMap<String, (Decimal, Decimal)> = HashMap::new();

        for item in balance_data.list {
            let currency = item.currency.to_uppercase();
            let balance = Decimal::from_str(&item.balance).unwrap_or(Decimal::ZERO);

            let entry = currency_balances
                .entry(currency)
                .or_insert((Decimal::ZERO, Decimal::ZERO));
            if item.balance_type == "trade" {
                entry.0 = balance; // free
            } else if item.balance_type == "frozen" {
                entry.1 = balance; // used
            }
        }

        for (currency, (free, used)) in currency_balances {
            let total = free + used;
            if total > Decimal::ZERO {
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

        let timestamp = Utc::now().timestamp_millis();

        Ok(Balances {
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            currencies,
            info: serde_json::json!({}),
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
        let account_id = self.get_account_id().await?;
        let market_id = self.to_market_id(symbol);

        let type_str = match (order_type, side) {
            (OrderType::Market, OrderSide::Buy) => "buy-market",
            (OrderType::Market, OrderSide::Sell) => "sell-market",
            (OrderType::Limit, OrderSide::Buy) => "buy-limit",
            (OrderType::Limit, OrderSide::Sell) => "sell-limit",
            _ => {
                return Err(CcxtError::BadRequest {
                    message: "Unsupported order type".into(),
                })
            },
        };

        let mut params = HashMap::new();
        params.insert("account-id".to_string(), account_id);
        params.insert("symbol".to_string(), market_id);
        params.insert("type".to_string(), type_str.to_string());
        params.insert("amount".to_string(), amount.to_string());

        if let Some(p) = price {
            params.insert("price".to_string(), p.to_string());
        }

        let response: BittradeResponse<String> = self
            .private_request("POST", "/v1/order/orders/place", params)
            .await?;

        let order_id = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Order creation failed".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Order {
            id: order_id,
            client_order_id: None,
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            timestamp: Some(timestamp),
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
            cost: Some(Decimal::ZERO),
            trades: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            fees: vec![],
            info: serde_json::json!({}),
        })
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("/v1/order/orders/{id}/submitcancel");

        let response: BittradeResponse<String> =
            self.private_request("POST", &path, HashMap::new()).await?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            timestamp: Some(timestamp),
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
            filled: Decimal::ZERO,
            remaining: None,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            fees: vec![],
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("/v1/order/orders/{id}");

        let response: BittradeResponse<BittradeOrderData> =
            self.private_request("GET", &path, HashMap::new()).await?;

        let order_data = response.data.ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })?;

        let symbol = self.to_symbol(&order_data.symbol);
        let status = self.parse_order_status(&order_data.state);
        let side = self.parse_order_side(&order_data.order_type);
        let order_type = self.parse_order_type(&order_data.order_type);

        let amount = Decimal::from_str(&order_data.amount).unwrap_or(Decimal::ZERO);
        let price = order_data
            .price
            .as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let filled = order_data
            .field_amount
            .as_ref()
            .and_then(|f| Decimal::from_str(f).ok())
            .unwrap_or(Decimal::ZERO);
        let cost = order_data
            .field_cash_amount
            .as_ref()
            .and_then(|c| Decimal::from_str(c).ok());
        let remaining = if amount > filled {
            Some(amount - filled)
        } else {
            None
        };
        let average = if filled > Decimal::ZERO {
            cost.map(|c| c / filled)
        } else {
            None
        };

        Ok(Order {
            id: order_data.id.to_string(),
            client_order_id: order_data.client_order_id.clone(),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(order_data.created_at)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            timestamp: Some(order_data.created_at),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price,
            average,
            amount,
            filled,
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost,
            trades: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            fees: vec![],
            info: serde_json::to_value(&order_data).unwrap_or_default(),
        })
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert(
            "states".to_string(),
            "submitted,partial-filled,filled,partial-canceled,canceled".to_string(),
        );

        if let Some(sym) = symbol {
            params.insert("symbol".to_string(), self.to_market_id(sym));
        }

        if let Some(lim) = limit {
            params.insert("size".to_string(), lim.to_string());
        }

        let response: BittradeResponse<Vec<BittradeOrderData>> = self
            .private_request("GET", "/v1/order/orders", params)
            .await?;

        let orders_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Orders data not found".into(),
        })?;

        let mut orders = Vec::new();

        for order_data in orders_data {
            let symbol = self.to_symbol(&order_data.symbol);
            let status = self.parse_order_status(&order_data.state);
            let side = self.parse_order_side(&order_data.order_type);
            let order_type = self.parse_order_type(&order_data.order_type);

            let amount = Decimal::from_str(&order_data.amount).unwrap_or(Decimal::ZERO);
            let price = order_data
                .price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok());
            let filled = order_data
                .field_amount
                .as_ref()
                .and_then(|f| Decimal::from_str(f).ok())
                .unwrap_or(Decimal::ZERO);
            let cost = order_data
                .field_cash_amount
                .as_ref()
                .and_then(|c| Decimal::from_str(c).ok());
            let remaining = if amount > filled {
                Some(amount - filled)
            } else {
                None
            };

            orders.push(Order {
                id: order_data.id.to_string(),
                client_order_id: order_data.client_order_id.clone(),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(order_data.created_at)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                timestamp: Some(order_data.created_at),
                last_trade_timestamp: None,
                last_update_timestamp: None,
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
                stop_price: None,
                trigger_price: None,
                take_profit_price: None,
                stop_loss_price: None,
                cost,
                trades: vec![],
                reduce_only: None,
                post_only: None,
                fee: None,
                fees: vec![],
                info: serde_json::to_value(&order_data).unwrap_or_default(),
            });
        }

        Ok(orders)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(sym) = symbol {
            params.insert("symbol".to_string(), self.to_market_id(sym));
        }

        if let Some(lim) = limit {
            params.insert("size".to_string(), lim.to_string());
        }

        let response: BittradeResponse<Vec<BittradeOrderData>> = self
            .private_request("GET", "/v1/order/openOrders", params)
            .await?;

        let orders_data = response.data.unwrap_or_default();

        let mut orders = Vec::new();

        for order_data in orders_data {
            let symbol = self.to_symbol(&order_data.symbol);
            let side = self.parse_order_side(&order_data.order_type);
            let order_type = self.parse_order_type(&order_data.order_type);

            let amount = Decimal::from_str(&order_data.amount).unwrap_or(Decimal::ZERO);
            let price = order_data
                .price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok());
            let filled = order_data
                .field_amount
                .as_ref()
                .and_then(|f| Decimal::from_str(f).ok())
                .unwrap_or(Decimal::ZERO);
            let remaining = if amount > filled {
                Some(amount - filled)
            } else {
                None
            };

            orders.push(Order {
                id: order_data.id.to_string(),
                client_order_id: order_data.client_order_id.clone(),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(order_data.created_at)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                timestamp: Some(order_data.created_at),
                last_trade_timestamp: None,
                last_update_timestamp: None,
                status: OrderStatus::Open,
                symbol,
                order_type,
                time_in_force: None,
                side,
                price,
                average: None,
                amount,
                filled,
                remaining,
                stop_price: None,
                trigger_price: None,
                take_profit_price: None,
                stop_loss_price: None,
                cost: None,
                trades: vec![],
                reduce_only: None,
                post_only: None,
                fee: None,
                fees: vec![],
                info: serde_json::to_value(&order_data).unwrap_or_default(),
            });
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
            params.insert("symbol".to_string(), self.to_market_id(sym));
        }

        if let Some(lim) = limit {
            params.insert("size".to_string(), lim.to_string());
        }

        let response: BittradeResponse<Vec<BittradeMyTrade>> = self
            .private_request("GET", "/v1/order/matchresults", params)
            .await?;

        let trades_data = response.data.unwrap_or_default();

        let mut trades = Vec::new();

        for trade_data in trades_data {
            let symbol = self.to_symbol(&trade_data.symbol);
            let side = self.parse_order_side(&trade_data.trade_type);

            let price = Decimal::from_str(&trade_data.price).unwrap_or(Decimal::ZERO);
            let amount = Decimal::from_str(&trade_data.filled_amount).unwrap_or(Decimal::ZERO);

            trades.push(Trade {
                id: trade_data.id.to_string(),
                order: Some(trade_data.order_id.to_string()),
                timestamp: Some(trade_data.created_at),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(trade_data.created_at)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol,
                trade_type: None,
                side: Some(
                    match side {
                        OrderSide::Buy => "buy",
                        OrderSide::Sell => "sell",
                    }
                    .to_string(),
                ),
                taker_or_maker: Some(if trade_data.role == "taker" {
                    crate::types::TakerOrMaker::Taker
                } else {
                    crate::types::TakerOrMaker::Maker
                }),
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: vec![],
                info: serde_json::to_value(&trade_data).unwrap_or_default(),
            });
        }

        Ok(trades)
    }
}

// Additional structures for fetch_my_trades
#[derive(Debug, Deserialize, Serialize)]
struct BittradeMyTrade {
    id: u64,
    #[serde(rename = "order-id")]
    order_id: u64,
    symbol: String,
    #[serde(rename = "type")]
    trade_type: String,
    price: String,
    #[serde(rename = "filled-amount")]
    filled_amount: String,
    #[serde(rename = "filled-fees")]
    filled_fees: String,
    role: String,
    #[serde(rename = "created-at")]
    created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bittrade_creation() {
        let config = ExchangeConfig::default();
        let exchange = Bittrade::new(config);
        assert!(exchange.is_ok());
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Bittrade::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Bittrade);
        assert_eq!(exchange.name(), "BitTrade");
    }

    #[test]
    fn test_to_market_id() {
        let config = ExchangeConfig::default();
        let exchange = Bittrade::new(config).unwrap();
        assert_eq!(exchange.to_market_id("BTC/JPY"), "btcjpy");
        assert_eq!(exchange.to_market_id("ETH/JPY"), "ethjpy");
    }

    #[test]
    fn test_urls() {
        let config = ExchangeConfig::default();
        let exchange = Bittrade::new(config).unwrap();
        let urls = exchange.urls();
        assert!(urls.www.is_some());
        assert!(!urls.doc.is_empty());
    }

    #[test]
    fn test_timeframes() {
        let config = ExchangeConfig::default();
        let exchange = Bittrade::new(config).unwrap();
        let timeframes = exchange.timeframes();
        assert!(timeframes.contains_key(&Timeframe::Minute1));
        assert!(timeframes.contains_key(&Timeframe::Hour1));
        assert!(timeframes.contains_key(&Timeframe::Day1));
    }

    #[test]
    fn test_parse_order_status() {
        let config = ExchangeConfig::default();
        let exchange = Bittrade::new(config).unwrap();
        assert_eq!(exchange.parse_order_status("submitted"), OrderStatus::Open);
        assert_eq!(exchange.parse_order_status("filled"), OrderStatus::Closed);
        assert_eq!(
            exchange.parse_order_status("canceled"),
            OrderStatus::Canceled
        );
    }

    #[test]
    fn test_parse_order_side() {
        let config = ExchangeConfig::default();
        let exchange = Bittrade::new(config).unwrap();
        assert_eq!(exchange.parse_order_side("buy-limit"), OrderSide::Buy);
        assert_eq!(exchange.parse_order_side("sell-market"), OrderSide::Sell);
    }

    #[test]
    fn test_parse_order_type() {
        let config = ExchangeConfig::default();
        let exchange = Bittrade::new(config).unwrap();
        assert_eq!(exchange.parse_order_type("buy-limit"), OrderType::Limit);
        assert_eq!(exchange.parse_order_type("sell-market"), OrderType::Market);
    }
}
