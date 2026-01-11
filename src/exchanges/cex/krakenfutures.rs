//! Kraken Futures Exchange Implementation
//!
//! Kraken Futures API implementation (formerly CryptoFacilities)
//! Supports perpetual swaps and futures contracts

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, Position, PositionSide, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

const BASE_URL: &str = "https://futures.kraken.com/derivatives/api";
const RATE_LIMIT_MS: u64 = 600;

/// Kraken Futures exchange structure
pub struct KrakenFutures {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

#[derive(Debug, Deserialize)]
struct KfResponse<T> {
    result: String,
    #[serde(flatten)]
    data: T,
    #[serde(rename = "serverTime")]
    server_time: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InstrumentsData {
    instruments: Vec<KfInstrument>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KfInstrument {
    symbol: String,
    #[serde(rename = "type")]
    instrument_type: String,
    underlying: Option<String>,
    #[serde(rename = "lastTradingTime")]
    last_trading_time: Option<String>,
    #[serde(rename = "tickSize")]
    tick_size: f64,
    #[serde(rename = "contractSize")]
    contract_size: Option<f64>,
    tradeable: bool,
    #[serde(rename = "contractValueTradePrecision")]
    contract_value_trade_precision: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct TickersData {
    tickers: Vec<KfTicker>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KfTicker {
    symbol: String,
    last: Option<f64>,
    bid: Option<f64>,
    ask: Option<f64>,
    #[serde(rename = "bidSize")]
    bid_size: Option<f64>,
    #[serde(rename = "askSize")]
    ask_size: Option<f64>,
    volume: Option<f64>,
    #[serde(rename = "volumeQuote")]
    volume_quote: Option<f64>,
    #[serde(rename = "lastTime")]
    last_time: Option<String>,
    #[serde(rename = "lastSize")]
    last_size: Option<f64>,
    change24h: Option<f64>,
    funding_rate: Option<f64>,
    #[serde(rename = "openInterest")]
    open_interest: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct OrderBookData {
    orderbook: KfOrderBook,
}

#[derive(Debug, Deserialize)]
struct KfOrderBook {
    bids: Vec<(f64, f64)>,
    asks: Vec<(f64, f64)>,
}

#[derive(Debug, Deserialize)]
struct HistoryData {
    history: Vec<KfTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KfTrade {
    time: String,
    price: f64,
    size: f64,
    side: String,
    #[serde(rename = "uid")]
    trade_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AccountsData {
    accounts: HashMap<String, KfAccount>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KfAccount {
    #[serde(rename = "balanceValue")]
    balance_value: Option<f64>,
    #[serde(rename = "availableMargin")]
    available_margin: Option<f64>,
    #[serde(rename = "marginRequirements")]
    margin_requirements: Option<f64>,
    currency: String,
    #[serde(rename = "portfolioValue")]
    portfolio_value: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PositionsData {
    #[serde(rename = "openPositions")]
    open_positions: Vec<KfPosition>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KfPosition {
    symbol: String,
    side: String,
    size: f64,
    #[serde(rename = "fillPrice")]
    fill_price: Option<f64>,
    #[serde(rename = "unrealizedFunding")]
    unrealized_funding: Option<f64>,
    pnl: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OpenOrdersData {
    #[serde(rename = "openOrders")]
    open_orders: Vec<KfOrder>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RecentOrdersData {
    orders: Vec<KfOrder>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KfOrder {
    #[serde(rename = "order_id")]
    order_id: String,
    symbol: String,
    side: String,
    #[serde(rename = "orderType")]
    order_type: String,
    quantity: f64,
    filled: Option<f64>,
    #[serde(rename = "limitPrice")]
    limit_price: Option<f64>,
    #[serde(rename = "stopPrice")]
    stop_price: Option<f64>,
    #[serde(rename = "receivedTime")]
    received_time: Option<String>,
    #[serde(rename = "lastUpdateTime")]
    last_update_time: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct FillsData {
    fills: Vec<KfFill>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KfFill {
    #[serde(rename = "fill_id")]
    fill_id: String,
    symbol: String,
    side: String,
    size: f64,
    price: f64,
    #[serde(rename = "fillTime")]
    fill_time: String,
    #[serde(rename = "order_id")]
    order_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SendOrderData {
    #[serde(rename = "sendStatus")]
    send_status: KfSendStatus,
}

#[derive(Debug, Deserialize, Serialize)]
struct KfSendStatus {
    order_id: Option<String>,
    status: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct CancelOrderData {
    #[serde(rename = "cancelStatus")]
    cancel_status: KfCancelStatus,
}

#[derive(Debug, Deserialize, Serialize)]
struct KfCancelStatus {
    status: String,
    order_id: Option<String>,
}

impl KrakenFutures {
    /// Create a new KrakenFutures instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), format!("{BASE_URL}/v3"));
        api_urls.insert("private".into(), format!("{BASE_URL}/v3"));
        api_urls.insert(
            "charts".into(),
            "https://futures.kraken.com/api/charts/v1".into(),
        );
        api_urls.insert(
            "history".into(),
            "https://futures.kraken.com/api/history/v2".into(),
        );

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/24300605/81436764-b22fd580-9172-11ea-9703-742783e6376d.jpg".into()),
            api: api_urls,
            www: Some("https://futures.kraken.com/".into()),
            doc: vec!["https://docs.kraken.com/api/docs/futures-api/".into()],
            fees: Some("https://support.kraken.com/hc/en-us/articles/360022835771".into()),
        };

        let features = ExchangeFeatures {
            cors: false,
            spot: false,
            margin: false,
            swap: true,
            future: true,
            option: false,
            fetch_markets: true,
            fetch_currencies: false,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: false,
            create_stop_limit_order: true,
            create_stop_market_order: true,
            cancel_order: true,
            cancel_all_orders: true,
            edit_order: true,
            fetch_order: false,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_canceled_orders: true,
            fetch_my_trades: true,
            fetch_positions: true,
            set_leverage: true,
            transfer: true,
            ..Default::default()
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".to_string());
        timeframes.insert(Timeframe::Minute5, "5m".to_string());
        timeframes.insert(Timeframe::Minute15, "15m".to_string());
        timeframes.insert(Timeframe::Minute30, "30m".to_string());
        timeframes.insert(Timeframe::Hour1, "1h".to_string());
        timeframes.insert(Timeframe::Hour4, "4h".to_string());
        timeframes.insert(Timeframe::Hour12, "12h".to_string());
        timeframes.insert(Timeframe::Day1, "1d".to_string());
        timeframes.insert(Timeframe::Week1, "1w".to_string());

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

    /// Parse KrakenFutures symbol to unified symbol
    fn to_symbol(&self, kf_symbol: &str) -> String {
        // KF symbols format: pi_xbtusd (perpetual), fi_xbtusd_230630 (futures)
        let parts: Vec<&str> = kf_symbol.split('_').collect();

        if parts.len() < 2 {
            return kf_symbol.to_uppercase();
        }

        let market_part = parts[1];

        // Extract base and quote (always USD for KF)
        let base = if market_part.len() >= 6 {
            let base_str = &market_part[..market_part.len() - 3];
            if base_str.to_uppercase() == "XBT" {
                "BTC"
            } else {
                base_str
            }
        } else {
            market_part
        };

        let quote = "USD";
        let settle = if market_part.ends_with("usd") {
            "USD"
        } else {
            base
        };

        // Format: BASE/QUOTE:SETTLE or BASE/QUOTE:SETTLE-YYMMDD
        if parts.len() > 2 {
            // Futures with expiry
            format!(
                "{}/{}:{}-{}",
                base.to_uppercase(),
                quote,
                settle.to_uppercase(),
                parts[2]
            )
        } else {
            // Perpetual swap
            format!(
                "{}/{}:{}",
                base.to_uppercase(),
                quote,
                settle.to_uppercase()
            )
        }
    }

    /// Convert unified symbol to KrakenFutures market ID
    fn to_market_id(&self, symbol: &str) -> String {
        let markets = self.markets.read().unwrap();

        if let Some(market) = markets.get(symbol) {
            return market.id.clone();
        }

        // Fallback: try to construct from symbol
        // BTC/USD:USD -> pi_xbtusd
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() != 2 {
            return symbol.to_lowercase();
        }

        let base_lower = parts[0].to_lowercase();
        let base = if parts[0] == "BTC" {
            "xbt".to_string()
        } else {
            base_lower
        };
        let quote_settle: Vec<&str> = parts[1].split(':').collect();

        if quote_settle.len() == 2 {
            // Check if it has expiry date
            let settle_parts: Vec<&str> = quote_settle[1].split('-').collect();
            if settle_parts.len() > 1 {
                format!("fi_{}usd_{}", base, settle_parts[1])
            } else {
                format!("pi_{base}usd")
            }
        } else {
            format!("pi_{base}usd")
        }
    }

    /// Parse ISO8601 timestamp to milliseconds
    fn parse_timestamp(&self, timestamp: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(timestamp)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(0)
    }

    /// URL encode params manually
    fn url_encode_params(params: &HashMap<String, String>) -> String {
        params
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Make a public API request
    async fn public_get(
        &self,
        endpoint: &str,
        params: &HashMap<String, String>,
    ) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let path = format!("/v3/{endpoint}");

        let response: serde_json::Value =
            self.client.get(&path, Some(params.clone()), None).await?;

        // Check for error
        if let Some(error) = response.get("error").and_then(|e| e.as_str()) {
            if !error.is_empty() {
                return Err(CcxtError::ExchangeError {
                    message: error.to_string(),
                });
            }
        }

        Ok(response)
    }

    /// Make a private API request
    async fn private_request(
        &self,
        method: &str,
        endpoint: &str,
        params: &HashMap<String, String>,
    ) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let signed = self.sign(endpoint, "private", method, params, None, None);

        let response: serde_json::Value = if method == "POST" {
            self.client
                .post_form(&signed.url, params, Some(signed.headers))
                .await?
        } else {
            self.client
                .get(&signed.url, Some(params.clone()), Some(signed.headers))
                .await?
        };

        // Check for error
        if let Some(error) = response.get("error").and_then(|e| e.as_str()) {
            if !error.is_empty() {
                return Err(CcxtError::ExchangeError {
                    message: error.to_string(),
                });
            }
        }

        Ok(response)
    }
}

#[async_trait]
impl Exchange for KrakenFutures {
    fn id(&self) -> ExchangeId {
        ExchangeId::KrakenFutures
    }

    fn name(&self) -> &str {
        "Kraken Futures"
    }

    fn version(&self) -> &str {
        "v3"
    }

    fn countries(&self) -> &[&str] {
        &["US"]
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

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        if !reload {
            let markets = self.markets.read().unwrap();
            if !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let markets = self.fetch_markets().await?;
        let mut markets_map = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for market in markets {
            markets_by_id.insert(market.id.clone(), market.symbol.clone());
            markets_map.insert(market.symbol.clone(), market);
        }

        *self.markets.write().unwrap() = markets_map.clone();
        *self.markets_by_id.write().unwrap() = markets_by_id;

        Ok(markets_map)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response = self.public_get("instruments", &HashMap::new()).await?;

        let data: KfResponse<InstrumentsData> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "InstrumentsData".to_string(),
                message: e.to_string(),
            })?;

        let mut markets = Vec::new();

        for instrument in data.data.instruments {
            if !instrument.tradeable {
                continue;
            }

            let id = instrument.symbol.clone();
            let symbol = self.to_symbol(&id);

            let is_index = instrument.instrument_type.contains("index");
            if is_index {
                continue; // Skip index instruments
            }

            let is_inverse = instrument.instrument_type.contains("inverse");
            let has_expiry = instrument.last_trading_time.is_some();

            let market_type = if has_expiry {
                MarketType::Future
            } else {
                MarketType::Swap
            };

            let contract_size = instrument.contract_size.unwrap_or(1.0);
            let tick_size = instrument.tick_size;

            markets.push(Market {
                id,
                symbol: symbol.clone(),
                lowercase_id: None,
                base: if symbol.starts_with("BTC") {
                    "BTC".to_string()
                } else {
                    symbol.split('/').next().unwrap_or("").to_string()
                },
                quote: "USD".to_string(),
                settle: Some(if is_inverse {
                    symbol.split('/').next().unwrap_or("").to_string()
                } else {
                    "USD".to_string()
                }),
                base_id: "".to_string(),
                quote_id: "usd".to_string(),
                settle_id: Some("".to_string()),
                market_type,
                spot: false,
                margin: false,
                swap: !has_expiry,
                future: has_expiry,
                option: false,
                index: false,
                active: instrument.tradeable,
                contract: true,
                linear: Some(!is_inverse),
                inverse: Some(is_inverse),
                sub_type: None,
                taker: None,
                maker: None,
                contract_size: Some(Decimal::from_f64_retain(contract_size).unwrap()),
                expiry: instrument
                    .last_trading_time
                    .as_ref()
                    .map(|t| self.parse_timestamp(t)),
                expiry_datetime: instrument.last_trading_time.clone(),
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(8),
                    price: Some(8),
                    base: Some(8),
                    quote: Some(8),
                    ..Default::default()
                },
                limits: MarketLimits {
                    amount: MinMax {
                        min: Some(Decimal::from_str("0.001").unwrap()),
                        max: None,
                    },
                    price: MinMax {
                        min: Some(Decimal::from_f64_retain(tick_size).unwrap()),
                        max: None,
                    },
                    cost: MinMax {
                        min: None,
                        max: None,
                    },
                    leverage: MinMax {
                        min: Some(Decimal::from(1)),
                        max: Some(Decimal::from(50)),
                    },
                },
                margin_modes: None,
                created: None,
                info: serde_json::json!(instrument),
                tier_based: false,
                percentage: false,
            });
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.load_markets(false).await?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id.clone());

        let response = self.public_get("tickers", &params).await?;

        let data: KfResponse<TickersData> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "TickersData".to_string(),
                message: e.to_string(),
            })?;

        if let Some(ticker) = data.data.tickers.first() {
            let timestamp = ticker
                .last_time
                .as_ref()
                .map(|t| self.parse_timestamp(t))
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            Ok(Ticker {
                symbol: symbol.to_string(),
                timestamp: Some(timestamp),
                datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339()),
                high: None,
                low: None,
                bid: ticker.bid.map(|v| Decimal::from_f64_retain(v).unwrap()),
                bid_volume: ticker
                    .bid_size
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                ask: ticker.ask.map(|v| Decimal::from_f64_retain(v).unwrap()),
                ask_volume: ticker
                    .ask_size
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                vwap: None,
                open: None,
                close: ticker.last.map(|v| Decimal::from_f64_retain(v).unwrap()),
                last: ticker.last.map(|v| Decimal::from_f64_retain(v).unwrap()),
                previous_close: None,
                change: ticker
                    .change24h
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                percentage: ticker
                    .change24h
                    .map(|v| Decimal::from_f64_retain(v * 100.0).unwrap()),
                average: None,
                base_volume: ticker.volume.map(|v| Decimal::from_f64_retain(v).unwrap()),
                quote_volume: ticker
                    .volume_quote
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                index_price: None,
                mark_price: None,
                info: serde_json::json!(ticker),
            })
        } else {
            Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })
        }
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        let response = self.public_get("tickers", &HashMap::new()).await?;

        let data: KfResponse<TickersData> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "TickersData".to_string(),
                message: e.to_string(),
            })?;

        let mut tickers = HashMap::new();

        for ticker_data in data.data.tickers {
            let symbol = self.to_symbol(&ticker_data.symbol);

            // Filter by symbols if provided
            if let Some(syms) = symbols {
                if !syms.contains(&symbol.as_str()) {
                    continue;
                }
            }

            let timestamp = ticker_data
                .last_time
                .as_ref()
                .map(|t| self.parse_timestamp(t))
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            let ticker = Ticker {
                symbol: symbol.clone(),
                timestamp: Some(timestamp),
                datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339()),
                high: None,
                low: None,
                bid: ticker_data
                    .bid
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                bid_volume: ticker_data
                    .bid_size
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                ask: ticker_data
                    .ask
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                ask_volume: ticker_data
                    .ask_size
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                vwap: None,
                open: None,
                close: ticker_data
                    .last
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                last: ticker_data
                    .last
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                previous_close: None,
                change: ticker_data
                    .change24h
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                percentage: ticker_data
                    .change24h
                    .map(|v| Decimal::from_f64_retain(v * 100.0).unwrap()),
                average: None,
                base_volume: ticker_data
                    .volume
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                quote_volume: ticker_data
                    .volume_quote
                    .map(|v| Decimal::from_f64_retain(v).unwrap()),
                index_price: None,
                mark_price: None,
                info: serde_json::json!(ticker_data),
            };

            tickers.insert(symbol, ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        let response = self.public_get("orderbook", &params).await?;

        let data: KfResponse<OrderBookData> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "OrderBookData".to_string(),
                message: e.to_string(),
            })?;

        let ob = &data.data.orderbook;

        let mut bids = Vec::new();
        for (price, amount) in &ob.bids {
            bids.push(OrderBookEntry {
                price: Decimal::from_f64_retain(*price).unwrap(),
                amount: Decimal::from_f64_retain(*amount).unwrap(),
            });
        }

        let mut asks = Vec::new();
        for (price, amount) in &ob.asks {
            asks.push(OrderBookEntry {
                price: Decimal::from_f64_retain(*price).unwrap(),
                amount: Decimal::from_f64_retain(*amount).unwrap(),
            });
        }

        // Apply limit if specified
        if let Some(lim) = limit {
            let lim = lim as usize;
            bids.truncate(lim);
            asks.truncate(lim);
        }

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            checksum: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.load_markets(false).await?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        if let Some(lim) = limit {
            params.insert("limit".to_string(), lim.to_string());
        }

        let response = self.public_get("history", &params).await?;

        let data: KfResponse<HistoryData> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "HistoryData".to_string(),
                message: e.to_string(),
            })?;

        let mut trades = Vec::new();

        for trade_data in data.data.history {
            let timestamp = self.parse_timestamp(&trade_data.time);

            if let Some(s) = since {
                if timestamp < s {
                    continue;
                }
            }

            trades.push(Trade {
                id: trade_data.trade_id.clone().unwrap_or_default(),
                order: None,
                info: serde_json::json!(trade_data),
                timestamp: Some(timestamp),
                datetime: Some(trade_data.time.clone()),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(trade_data.side.clone()),
                taker_or_maker: None,
                price: Decimal::from_f64_retain(trade_data.price).unwrap(),
                amount: Decimal::from_f64_retain(trade_data.size).unwrap(),
                cost: Some(Decimal::from_f64_retain(trade_data.price * trade_data.size).unwrap()),
                fee: None,
                fees: Vec::new(),
            });
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
        self.load_markets(false).await?;

        let market_id = self.to_market_id(symbol);
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: "Unsupported timeframe".to_string(),
            })?;

        // Use charts API
        let url = format!("https://futures.kraken.com/api/charts/v1/trade/{market_id}/{interval}");

        let response: serde_json::Value = self.client.get(&url, None, None).await?;

        // Parse OHLCV data from candles array
        let candles = response
            .get("candles")
            .and_then(|c| c.as_array())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "OHLCV".to_string(),
                message: "Invalid OHLCV response".to_string(),
            })?;

        let mut ohlcv_data = Vec::new();

        for candle in candles {
            let time = candle.get("time").and_then(|v| v.as_i64()).unwrap_or(0);

            if let Some(s) = since {
                if time < s {
                    continue;
                }
            }

            let open = candle.get("open").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let high = candle.get("high").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let low = candle.get("low").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let close = candle.get("close").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let volume = candle.get("volume").and_then(|v| v.as_f64()).unwrap_or(0.0);

            ohlcv_data.push(OHLCV {
                timestamp: time,
                open: Decimal::from_f64_retain(open).unwrap(),
                high: Decimal::from_f64_retain(high).unwrap(),
                low: Decimal::from_f64_retain(low).unwrap(),
                close: Decimal::from_f64_retain(close).unwrap(),
                volume: Decimal::from_f64_retain(volume).unwrap(),
            });
        }

        // Apply limit if specified
        if let Some(lim) = limit {
            ohlcv_data.truncate(lim as usize);
        }

        Ok(ohlcv_data)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response = self
            .private_request("GET", "accounts", &HashMap::new())
            .await?;

        let data: KfResponse<AccountsData> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "AccountsData".to_string(),
                message: e.to_string(),
            })?;

        let mut balances = HashMap::new();

        for account in data.data.accounts.values() {
            let currency = account.currency.to_uppercase();
            let balance_value = account.balance_value.unwrap_or(0.0);
            let available = account.available_margin.unwrap_or(0.0);
            let used = account.margin_requirements.unwrap_or(0.0);

            balances.insert(
                currency.clone(),
                Balance {
                    free: Some(Decimal::from_f64_retain(available).unwrap()),
                    used: Some(Decimal::from_f64_retain(used).unwrap()),
                    total: Some(Decimal::from_f64_retain(balance_value).unwrap()),
                    debt: None,
                },
            );
        }

        Ok(Balances {
            info: serde_json::json!(data.data),
            currencies: balances,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
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
        self.load_markets(false).await?;

        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id.clone());
        params.insert(
            "side".to_string(),
            match side {
                OrderSide::Buy => "buy".to_string(),
                OrderSide::Sell => "sell".to_string(),
            },
        );
        params.insert("size".to_string(), amount.to_string());

        let order_type_str = match order_type {
            OrderType::Limit => {
                if let Some(p) = price {
                    params.insert("limitPrice".to_string(), p.to_string());
                    "lmt"
                } else {
                    return Err(CcxtError::BadRequest {
                        message: "Limit order requires price".to_string(),
                    });
                }
            },
            OrderType::Market => {
                return Err(CcxtError::NotSupported {
                    feature: "Market orders not supported on Kraken Futures".to_string(),
                });
            },
            OrderType::StopLimit => {
                if let Some(p) = price {
                    params.insert("limitPrice".to_string(), p.to_string());
                    "stp"
                } else {
                    return Err(CcxtError::BadRequest {
                        message: "Stop limit order requires price".to_string(),
                    });
                }
            },
            OrderType::StopMarket => "stp",
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type {order_type:?} not supported"),
                })
            },
        };

        params.insert("orderType".to_string(), order_type_str.to_string());

        let response = self.private_request("POST", "sendorder", &params).await?;

        let data: KfResponse<SendOrderData> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "SendOrderData".to_string(),
                message: e.to_string(),
            })?;

        let order_id =
            data.data
                .send_status
                .order_id
                .clone()
                .ok_or_else(|| CcxtError::ExchangeError {
                    message: "Order creation failed".to_string(),
                })?;

        Ok(Order {
            id: order_id,
            client_order_id: None,
            datetime: Some(Utc::now().to_rfc3339()),
            timestamp: Some(Utc::now().timestamp_millis()),
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
            cost: Some(Decimal::ZERO),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::json!(data.data),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("order_id".to_string(), id.to_string());

        let response = self.private_request("POST", "cancelorder", &params).await?;

        let data: KfResponse<CancelOrderData> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "CancelOrderData".to_string(),
                message: e.to_string(),
            })?;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            datetime: Some(Utc::now().to_rfc3339()),
            timestamp: Some(Utc::now().timestamp_millis()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price: None,
            average: None,
            amount: Decimal::ZERO,
            filled: Decimal::ZERO,
            remaining: Some(Decimal::ZERO),
            cost: Some(Decimal::ZERO),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::json!(data.data),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
        })
    }

    async fn fetch_order(&self, _id: &str, _symbol: &str) -> CcxtResult<Order> {
        Err(CcxtError::NotSupported {
            feature: "fetchOrder not supported on Kraken Futures".to_string(),
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let response = self
            .private_request("GET", "openorders", &HashMap::new())
            .await?;

        let data: KfResponse<OpenOrdersData> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "OpenOrdersData".to_string(),
                message: e.to_string(),
            })?;

        let mut orders = Vec::new();

        for order_data in data.data.open_orders {
            let order_symbol = self.to_symbol(&order_data.symbol);

            if let Some(sym) = symbol {
                if order_symbol != sym {
                    continue;
                }
            }

            let timestamp = order_data
                .received_time
                .as_ref()
                .map(|t| self.parse_timestamp(t))
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            if let Some(s) = since {
                if timestamp < s {
                    continue;
                }
            }

            let filled = order_data.filled.unwrap_or(0.0);
            let amount = order_data.quantity;

            orders.push(Order {
                id: order_data.order_id.clone(),
                client_order_id: None,
                datetime: order_data.received_time.clone(),
                timestamp: Some(timestamp),
                last_trade_timestamp: order_data
                    .last_update_time
                    .as_ref()
                    .map(|t| self.parse_timestamp(t)),
                last_update_timestamp: order_data
                    .last_update_time
                    .as_ref()
                    .map(|t| self.parse_timestamp(t)),
                status: OrderStatus::Open,
                symbol: order_symbol,
                order_type: match order_data.order_type.as_str() {
                    "lmt" => OrderType::Limit,
                    "stp" => OrderType::StopLimit,
                    _ => OrderType::Limit,
                },
                time_in_force: None,
                side: if order_data.side == "buy" {
                    OrderSide::Buy
                } else {
                    OrderSide::Sell
                },
                price: order_data
                    .limit_price
                    .map(|p| Decimal::from_f64_retain(p).unwrap()),
                average: None,
                amount: Decimal::from_f64_retain(amount).unwrap(),
                filled: Decimal::from_f64_retain(filled).unwrap(),
                remaining: Some(Decimal::from_f64_retain(amount - filled).unwrap()),
                cost: Some(Decimal::ZERO),
                trades: Vec::new(),
                fee: None,
                fees: Vec::new(),
                reduce_only: None,
                post_only: None,
                info: serde_json::json!(order_data),
                stop_price: order_data
                    .stop_price
                    .map(|p| Decimal::from_f64_retain(p).unwrap()),
                trigger_price: None,
                take_profit_price: None,
                stop_loss_price: None,
            });
        }

        // Apply limit if specified
        if let Some(lim) = limit {
            orders.truncate(lim as usize);
        }

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let response = self
            .private_request("GET", "recentorders", &HashMap::new())
            .await?;

        let data: KfResponse<RecentOrdersData> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "RecentOrdersData".to_string(),
                message: e.to_string(),
            })?;

        let mut orders = Vec::new();

        for order_data in data.data.orders {
            let order_symbol = self.to_symbol(&order_data.symbol);

            if let Some(sym) = symbol {
                if order_symbol != sym {
                    continue;
                }
            }

            let timestamp = order_data
                .received_time
                .as_ref()
                .map(|t| self.parse_timestamp(t))
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            if let Some(s) = since {
                if timestamp < s {
                    continue;
                }
            }

            let filled = order_data.filled.unwrap_or(0.0);
            let amount = order_data.quantity;

            orders.push(Order {
                id: order_data.order_id.clone(),
                client_order_id: None,
                datetime: order_data.received_time.clone(),
                timestamp: Some(timestamp),
                last_trade_timestamp: order_data
                    .last_update_time
                    .as_ref()
                    .map(|t| self.parse_timestamp(t)),
                last_update_timestamp: order_data
                    .last_update_time
                    .as_ref()
                    .map(|t| self.parse_timestamp(t)),
                status: if filled >= amount {
                    OrderStatus::Closed
                } else {
                    OrderStatus::Canceled
                },
                symbol: order_symbol,
                order_type: match order_data.order_type.as_str() {
                    "lmt" => OrderType::Limit,
                    "stp" => OrderType::StopLimit,
                    _ => OrderType::Limit,
                },
                time_in_force: None,
                side: if order_data.side == "buy" {
                    OrderSide::Buy
                } else {
                    OrderSide::Sell
                },
                price: order_data
                    .limit_price
                    .map(|p| Decimal::from_f64_retain(p).unwrap()),
                average: None,
                amount: Decimal::from_f64_retain(amount).unwrap(),
                filled: Decimal::from_f64_retain(filled).unwrap(),
                remaining: Some(Decimal::from_f64_retain(amount - filled).unwrap()),
                cost: Some(Decimal::ZERO),
                trades: Vec::new(),
                fee: None,
                fees: Vec::new(),
                reduce_only: None,
                post_only: None,
                info: serde_json::json!(order_data),
                stop_price: order_data
                    .stop_price
                    .map(|p| Decimal::from_f64_retain(p).unwrap()),
                trigger_price: None,
                take_profit_price: None,
                stop_loss_price: None,
            });
        }

        // Apply limit if specified
        if let Some(lim) = limit {
            orders.truncate(lim as usize);
        }

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let response = self
            .private_request("GET", "fills", &HashMap::new())
            .await?;

        let data: KfResponse<FillsData> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "FillsData".to_string(),
                message: e.to_string(),
            })?;

        let mut trades = Vec::new();

        for fill in data.data.fills {
            let trade_symbol = self.to_symbol(&fill.symbol);

            if let Some(sym) = symbol {
                if trade_symbol != sym {
                    continue;
                }
            }

            let timestamp = self.parse_timestamp(&fill.fill_time);

            if let Some(s) = since {
                if timestamp < s {
                    continue;
                }
            }

            trades.push(Trade {
                id: fill.fill_id.clone(),
                order: fill.order_id.clone(),
                info: serde_json::json!(fill),
                timestamp: Some(timestamp),
                datetime: Some(fill.fill_time.clone()),
                symbol: trade_symbol,
                trade_type: None,
                side: Some(fill.side.clone()),
                taker_or_maker: None,
                price: Decimal::from_f64_retain(fill.price).unwrap(),
                amount: Decimal::from_f64_retain(fill.size).unwrap(),
                cost: Some(Decimal::from_f64_retain(fill.price * fill.size).unwrap()),
                fee: None,
                fees: Vec::new(),
            });
        }

        // Apply limit if specified
        if let Some(lim) = limit {
            trades.truncate(lim as usize);
        }

        Ok(trades)
    }

    async fn fetch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>> {
        let response = self
            .private_request("GET", "openpositions", &HashMap::new())
            .await?;

        let data: KfResponse<PositionsData> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "PositionsData".to_string(),
                message: e.to_string(),
            })?;

        let mut positions = Vec::new();

        for pos in data.data.open_positions {
            let position_symbol = self.to_symbol(&pos.symbol);

            if let Some(syms) = symbols {
                if !syms.contains(&position_symbol.as_str()) {
                    continue;
                }
            }

            positions.push(Position {
                symbol: position_symbol,
                id: None,
                info: serde_json::json!(pos),
                timestamp: Some(Utc::now().timestamp_millis()),
                datetime: Some(Utc::now().to_rfc3339()),
                contracts: Some(Decimal::from_f64_retain(pos.size).unwrap()),
                contract_size: None,
                side: Some(if pos.side == "long" {
                    PositionSide::Long
                } else {
                    PositionSide::Short
                }),
                notional: None,
                leverage: None,
                unrealized_pnl: pos.pnl.map(|p| Decimal::from_f64_retain(p).unwrap()),
                realized_pnl: None,
                collateral: None,
                entry_price: pos.fill_price.map(|p| Decimal::from_f64_retain(p).unwrap()),
                mark_price: None,
                liquidation_price: None,
                margin_mode: None,
                hedged: Some(false),
                maintenance_margin: None,
                maintenance_margin_percentage: None,
                initial_margin: None,
                initial_margin_percentage: None,
                margin_ratio: None,
                last_update_timestamp: None,
                last_price: None,
                stop_loss_price: None,
                take_profit_price: None,
                percentage: None,
            });
        }

        Ok(positions)
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        Some(self.to_market_id(symbol))
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let markets_by_id = self.markets_by_id.read().unwrap();
        markets_by_id
            .get(market_id)
            .cloned()
            .or_else(|| Some(self.to_symbol(market_id)))
    }

    fn sign(
        &self,
        path: &str,
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        let version = "v3";
        let endpoint = format!("{version}/{path}");

        let mut query = endpoint.clone();
        let mut post_data = String::new();

        if !params.is_empty() {
            post_data = Self::url_encode_params(params);
            query = format!("{query}?{post_data}");
        }

        let url = format!("{BASE_URL}/{query}");

        if api == "private" {
            // Kraken Futures authentication:
            // 1. auth_string = postData + "/api/" + endpoint
            // 2. hash = SHA256(auth_string)
            // 3. secret_decoded = base64_decode(secret)
            // 4. signature = HMAC-SHA512(hash, secret_decoded) -> base64

            let mut auth_string = post_data.clone();
            auth_string.push_str("/api/");
            auth_string.push_str(&endpoint);

            // Step 2: SHA256 hash
            let mut hasher = Sha256::new();
            hasher.update(auth_string.as_bytes());
            let hash = hasher.finalize();

            // Step 3: Decode secret from base64
            let secret_decoded = BASE64
                .decode(self.config.secret().unwrap_or_default())
                .unwrap_or_default();

            // Step 4: HMAC-SHA512
            let mut mac = Hmac::<Sha512>::new_from_slice(&secret_decoded).unwrap();
            mac.update(&hash);
            let signature = BASE64.encode(mac.finalize().into_bytes());

            let mut headers_map = HashMap::new();
            headers_map.insert(
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            );
            headers_map.insert("Accept".to_string(), "application/json".to_string());
            headers_map.insert(
                "APIKey".to_string(),
                self.config.api_key().unwrap_or_default().to_string(),
            );
            headers_map.insert("Authent".to_string(), signature);

            SignedRequest {
                url,
                method: method.to_string(),
                headers: headers_map,
                body: if method == "POST" {
                    Some(post_data)
                } else {
                    None
                },
            }
        } else {
            SignedRequest {
                url,
                method: method.to_string(),
                headers: headers.unwrap_or_default(),
                body: body.map(|s| s.to_string()),
            }
        }
    }
}
