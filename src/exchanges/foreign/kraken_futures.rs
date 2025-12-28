//! Kraken Futures Exchange Implementation
//!
//! Kraken Futures API implementation

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Sha512};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls,
    Market, MarketLimits, MarketPrecision, MarketType, Order, OrderBook,
    OrderBookEntry, OrderRequest, OrderSide, OrderStatus, OrderType, SignedRequest,
    Ticker, Timeframe, TimeInForce, Trade, FundingRate, FundingRateHistory, Position, PositionSide,
    MarginMode, OHLCV, TakerOrMaker,
};

type HmacSha256 = Hmac<Sha256>;
type HmacSha512 = Hmac<Sha512>;

/// Kraken Futures exchange
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

impl KrakenFutures {
    const BASE_URL: &'static str = "https://futures.kraken.com";
    const RATE_LIMIT_MS: u64 = 600;

    /// Create a new KrakenFutures instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

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
            cancel_order: true,
            cancel_all_orders: true,
            fetch_order: false,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_positions: true,
            fetch_funding_rate: true,
            fetch_funding_rates: true,
            fetch_funding_rate_history: true,
            set_leverage: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), format!("{}/derivatives/api/", Self::BASE_URL));
        api_urls.insert("private".into(), format!("{}/derivatives/api/", Self::BASE_URL));
        api_urls.insert("charts".into(), format!("{}/api/charts/", Self::BASE_URL));
        api_urls.insert("history".into(), format!("{}/api/history/", Self::BASE_URL));

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/24300605/81436764-b22fd580-9172-11ea-9703-742783e6376d.jpg".into()),
            api: api_urls,
            www: Some("https://futures.kraken.com/".into()),
            doc: vec![
                "https://docs.kraken.com/api/docs/futures-api/trading/market-data/".into(),
            ],
            fees: Some("https://support.kraken.com/hc/en-us/articles/360022835771".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Hour12, "12h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());
        timeframes.insert(Timeframe::Week1, "1w".into());

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

    /// Public API call
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

        self.client.get(&url, None, None).await
    }

    /// Private API call
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

        // Build query string
        let query_string = if params.is_empty() {
            String::new()
        } else {
            params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&")
        };

        // Create auth string: postData + '/api/' + endpoint
        let endpoint = path.trim_start_matches('/');
        let mut auth_string = query_string.clone();
        auth_string.push_str("/api/v3/");
        auth_string.push_str(endpoint);

        // Hash the auth string with SHA256
        let mut hasher = HmacSha256::new_from_slice(auth_string.as_bytes())
            .unwrap_or_else(|_| HmacSha256::new_from_slice(b"").unwrap());
        hasher.update(auth_string.as_bytes());
        let hash = hasher.finalize().into_bytes();

        // Decode base64 secret
        let secret_bytes = base64::decode(secret).map_err(|e| CcxtError::AuthenticationError {
            message: format!("Invalid secret: {e}"),
        })?;

        // Sign with HMAC-SHA512
        let mut mac = HmacSha512::new_from_slice(&secret_bytes)
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("HMAC error: {e}"),
            })?;
        mac.update(&hash);
        let signature = base64::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/x-www-form-urlencoded".into());
        headers.insert("Accept".into(), "application/json".into());
        headers.insert("APIKey".into(), api_key.to_string());
        headers.insert("Authent".into(), signature);

        let url = if query_string.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{query_string}")
        };

        match method {
            "GET" => self.client.get(&url, None, Some(headers)).await,
            "POST" => {
                let body = if !params.is_empty() {
                    Some(query_string.into_bytes())
                } else {
                    None
                };
                self.client.post(&url, body, Some(headers)).await
            }
            "PUT" => {
                let body = if !params.is_empty() {
                    Some(query_string.into_bytes())
                } else {
                    None
                };
                self.client.put(&url, body, Some(headers)).await
            }
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// Symbol to market ID conversion
    fn to_market_id(&self, symbol: &str) -> String {
        // BTC/USD:USD -> use market ID from markets
        let markets = self.markets.read().unwrap();
        if let Some(market) = markets.get(symbol) {
            return market.id.clone();
        }
        symbol.to_string()
    }

    /// Market ID to symbol conversion
    fn to_symbol(&self, market_id: &str) -> String {
        let markets_by_id = self.markets_by_id.read().unwrap();
        markets_by_id.get(market_id).cloned().unwrap_or_else(|| market_id.to_string())
    }

    /// Parse ticker response
    fn parse_ticker(&self, data: &KrakenFuturesTicker, symbol: &str) -> Ticker {
        let timestamp = data.last_time.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let open = data.open24h.and_then(|s| s.parse().ok());
        let last = data.last.and_then(|s| s.parse().ok());

        let change = match (last, open) {
            (Some(l), Some(o)) => Some(l - o),
            _ => None,
        };

        let percentage = match (change, open) {
            (Some(c), Some(o)) if o != Decimal::ZERO => Some(c / o * Decimal::from(100)),
            _ => None,
        };

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: None,
            low: None,
            bid: data.bid.and_then(|s| s.parse().ok()),
            bid_volume: data.bid_size.and_then(|s| s.parse().ok()),
            ask: data.ask.and_then(|s| s.parse().ok()),
            ask_volume: data.ask_size.and_then(|s| s.parse().ok()),
            vwap: None,
            open,
            close: last,
            last,
            previous_close: None,
            change,
            percentage,
            average: None,
            base_volume: data.vol24h.and_then(|s| s.parse().ok()),
            quote_volume: None,
            index_price: data.index_price.and_then(|s| s.parse().ok()),
            mark_price: data.mark_price.and_then(|s| s.parse().ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order response
    fn parse_order(&self, data: &KrakenFuturesOrderStatus, symbol: &str) -> Order {
        let status = match data.status.as_str() {
            "placed" | "untouched" => OrderStatus::Open,
            "partiallyFilled" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "cancelled" => OrderStatus::Canceled,
            "rejected" => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("lmt") => OrderType::Limit,
            Some("post") => OrderType::Limit,
            Some("mkt") => OrderType::Market,
            Some("stp") => OrderType::StopLimit,
            Some("take_profit") => OrderType::TakeProfit,
            Some("ioc") => OrderType::Limit,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let timestamp = data.timestamp.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis());

        let price = data.limit_price.as_ref().and_then(|p| p.parse().ok());
        let amount = data.quantity.as_ref().and_then(|a| a.parse().ok()).unwrap_or_default();
        let filled = data.filled.as_ref().and_then(|f| f.parse().ok()).unwrap_or_default();
        let remaining = Some(amount - filled);

        Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.cli_ord_id.clone(),
            timestamp,
            datetime: timestamp.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: None,
            last_update_timestamp: data.last_update_timestamp.as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis()),
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled,
            remaining,
            stop_price: data.stop_price.as_ref().and_then(|p| p.parse().ok()),
            trigger_price: data.trigger_price.as_ref().and_then(|p| p.parse().ok()),
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: data.reduce_only,
            post_only: data.order_type.as_deref() == Some("post"),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance response
    fn parse_balance(&self, accounts: &HashMap<String, KrakenFuturesAccount>) -> Balances {
        let mut result = Balances::new();

        for (account_name, account) in accounts {
            if let Some(balances) = &account.balances {
                for (currency, amount_str) in balances {
                    let amount: Option<Decimal> = amount_str.parse().ok();
                    if let Some(amt) = amount {
                        let balance = Balance {
                            free: Some(amt),
                            used: None,
                            total: Some(amt),
                            debt: None,
                        };
                        result.add(&currency.to_uppercase(), balance);
                    }
                }
            }
        }

        result
    }

    /// Parse position response
    fn parse_position(&self, data: &KrakenFuturesPosition) -> Position {
        let side = match data.side.as_str() {
            "long" => Some(PositionSide::Long),
            "short" => Some(PositionSide::Short),
            _ => None,
        };

        let symbol = self.to_symbol(&data.symbol);
        let timestamp = data.fill_time.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis());

        let contracts = data.size.parse().ok();
        let entry_price = data.price.parse().ok();

        Position {
            symbol,
            id: None,
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
            }),
            contracts,
            contract_size: Some(Decimal::ONE),
            side,
            notional: None,
            leverage: None,
            unrealized_pnl: data.unrealized_funding.as_ref().and_then(|s| s.parse().ok()),
            realized_pnl: None,
            collateral: None,
            entry_price,
            mark_price: None,
            liquidation_price: None,
            margin_mode: Some(MarginMode::Cross),
            hedged: None,
            maintenance_margin: None,
            maintenance_margin_percentage: None,
            initial_margin: None,
            initial_margin_percentage: None,
            margin_ratio: None,
            last_update_timestamp: timestamp,
            last_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            percentage: None,
        }
    }

    /// Parse trade response
    fn parse_trade(&self, data: &KrakenFuturesTrade, symbol: &str) -> Trade {
        let timestamp = data.fill_time.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis());

        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.size.parse().unwrap_or_default();

        Trade {
            id: data.fill_id.clone().unwrap_or_default(),
            order: data.order_id.clone(),
            timestamp,
            datetime: timestamp.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
            }),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.side.clone(),
            taker_or_maker: data.fill_type.as_ref().and_then(|ft| match ft.as_str() {
                "maker" => Some(TakerOrMaker::Maker),
                "taker" => Some(TakerOrMaker::Taker),
                _ => None,
            }),
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
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
        let response: KrakenFuturesInstrumentsResponse = self
            .public_get("/derivatives/api/v3/instruments", None)
            .await?;

        let mut markets = Vec::new();

        for instrument in response.instruments {
            let id = instrument.symbol.clone();
            let market_type = instrument.instrument_type.as_deref().unwrap_or("unknown");

            // Skip index markets
            if market_type.contains("index") {
                continue;
            }

            // Only process tradeable instruments
            if !instrument.tradeable.unwrap_or(false) {
                continue;
            }

            let linear = market_type.contains("vanilla");
            let inverse = !linear && (market_type.contains("inverse") || market_type == "futures_inverse");

            // Parse symbol parts: fi_xbtusd_210625 -> base: xbt, quote: usd
            let parts: Vec<&str> = id.split('_').collect();
            let base_quote = if parts.len() > 1 {
                parts[1]
            } else {
                &id
            };

            let base_id = if base_quote.len() >= 6 {
                &base_quote[0..base_quote.len() - 3]
            } else {
                "btc"
            };
            let quote_id = "usd";

            let base = base_id.to_uppercase();
            let quote = quote_id.to_uppercase();

            let settle = if inverse {
                base.clone()
            } else {
                quote.clone()
            };
            let settle_id = if inverse {
                base_id.to_string()
            } else {
                quote_id.to_string()
            };

            let expiry = instrument.last_trading_time.as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis());

            let swap = expiry.is_none();
            let future = expiry.is_some();

            let symbol = if swap {
                format!("{base}/{quote}:{settle}")
            } else {
                let expiry_date = expiry.and_then(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.format("%y%m%d").to_string())
                }).unwrap_or_default();
                format!("{base}/{quote}:{settle}-{expiry_date}")
            };

            let tick_size = instrument.tick_size.unwrap_or(0.01);
            let price_precision = (-tick_size.log10().floor()) as i32;

            markets.push(Market {
                id,
                lowercase_id: Some(instrument.symbol.to_lowercase()),
                symbol,
                base: base.clone(),
                quote: quote.clone(),
                base_id: base_id.to_string(),
                quote_id: quote_id.to_string(),
                settle: Some(settle),
                settle_id: Some(settle_id),
                active: instrument.tradeable.unwrap_or(false),
                market_type: if swap { MarketType::Swap } else { MarketType::Future },
                spot: false,
                margin: false,
                swap,
                future,
                option: false,
                index: false,
                contract: true,
                linear: Some(linear),
                inverse: Some(inverse),
                sub_type: if linear { Some("linear".into()) } else { Some("inverse".into()) },
                taker: Some(Decimal::new(5, 4)), // 0.0005
                maker: Some(Decimal::new(2, 4)), // 0.0002
                contract_size: instrument.contract_size,
                expiry,
                expiry_datetime: expiry.and_then(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                }),
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(8),
                    price: Some(price_precision),
                    cost: None,
                    base: Some(8),
                    quote: Some(8),
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: instrument.opening_date.as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis()),
                info: serde_json::to_value(&instrument).unwrap_or_default(),
                tier_based: true,
                percentage: true,
            });
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let tickers = self.fetch_tickers(Some(&[symbol])).await?;
        tickers.get(symbol).cloned().ok_or_else(|| CcxtError::ExchangeError {
            message: format!("Ticker not found for symbol: {symbol}"),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: KrakenFuturesTickersResponse = self
            .public_get("/derivatives/api/v3/tickers", None)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for ticker_data in response.tickers {
            if let Some(symbol) = markets_by_id.get(&ticker_data.symbol) {
                if let Some(filter) = symbols {
                    if !filter.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let ticker = self.parse_ticker(&ticker_data, symbol);
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: KrakenFuturesOrderBookResponse = self
            .public_get("/derivatives/api/v3/orderbook", Some(params))
            .await?;

        let timestamp = response.server_time.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let orderbook = response.orderbook;
        let bids: Vec<OrderBookEntry> = orderbook.bids
            .iter()
            .map(|b| OrderBookEntry {
                price: b[0].parse().unwrap_or_default(),
                amount: b[1].parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = orderbook.asks
            .iter()
            .map(|a| OrderBookEntry {
                price: a[0].parse().unwrap_or_default(),
                amount: a[1].parse().unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
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
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("count".into(), l.to_string());
        }

        let response: KrakenFuturesHistoryResponse = self
            .public_get("/derivatives/api/v3/history", Some(params))
            .await?;

        let trades: Vec<Trade> = response.history
            .iter()
            .map(|t| {
                let timestamp = t.time;
                let price: Decimal = t.price.parse().unwrap_or_default();
                let amount: Decimal = t.size.parse().unwrap_or_default();

                Trade {
                    id: t.uid.clone().unwrap_or_default(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: t.side.clone(),
                    taker_or_maker: t.trade_type.as_ref().and_then(|tt| match tt.as_str() {
                        "maker" => Some(TakerOrMaker::Maker),
                        "taker" => Some(TakerOrMaker::Taker),
                        _ => None,
                    }),
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
        let interval = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {timeframe:?}"),
        })?;

        let price_type = "trade";
        let path = format!("/api/charts/v1/{price_type}/{market_id}/{interval}");

        let mut params = HashMap::new();

        if let Some(s) = since {
            params.insert("from".into(), (s / 1000).to_string());
        }

        if let Some(l) = limit {
            params.insert("to".into(), (Utc::now().timestamp() + (l as i64) * 60).to_string());
        }

        let response: KrakenFuturesOHLCVResponse = self
            .public_get(&path, Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response.candles
            .iter()
            .map(|c| OHLCV {
                timestamp: c.time,
                open: c.open.parse().unwrap_or_default(),
                high: c.high.parse().unwrap_or_default(),
                low: c.low.parse().unwrap_or_default(),
                close: c.close.parse().unwrap_or_default(),
                volume: c.volume.parse().unwrap_or_default(),
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: KrakenFuturesAccountsResponse = self
            .private_request("GET", "/derivatives/api/v3/accounts", params)
            .await?;

        Ok(self.parse_balance(&response.accounts))
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
        params.insert("symbol".into(), market_id);
        params.insert("side".into(), match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }.to_string());
        params.insert("size".into(), amount.to_string());

        let order_type_str = match order_type {
            OrderType::Limit => "lmt",
            OrderType::Market => return Err(CcxtError::NotSupported {
                feature: "Market orders not supported by Kraken Futures".into(),
            }),
            OrderType::StopLimit => "stp",
            OrderType::TakeProfit => "take_profit",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {order_type:?}"),
            }),
        };
        params.insert("orderType".into(), order_type_str.to_string());

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("limitPrice".into(), price_val.to_string());
        }

        let response: KrakenFuturesOrderResponse = self
            .private_request("POST", "/derivatives/api/v3/sendorder", params)
            .await?;

        if let Some(status) = response.send_status {
            Ok(self.parse_order(&status, symbol))
        } else {
            Err(CcxtError::ExchangeError {
                message: "Invalid order response".into(),
            })
        }
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());

        let response: KrakenFuturesCancelOrderResponse = self
            .private_request("POST", "/derivatives/api/v3/cancelorder", params)
            .await?;

        if let Some(status) = response.cancel_status {
            Ok(self.parse_order(&status, symbol))
        } else {
            Err(CcxtError::ExchangeError {
                message: "Invalid cancel response".into(),
            })
        }
    }

    async fn fetch_order(&self, _id: &str, _symbol: Option<&str>) -> CcxtResult<Order> {
        Err(CcxtError::NotSupported {
            feature: "fetchOrder not supported by Kraken Futures".into(),
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let params = HashMap::new();

        let response: KrakenFuturesOpenOrdersResponse = self
            .private_request("GET", "/derivatives/api/v3/openorders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut orders = Vec::new();

        for order_data in response.open_orders {
            if let Some(market_id) = &order_data.symbol {
                if let Some(sym) = markets_by_id.get(market_id) {
                    if let Some(filter) = symbol {
                        if sym != filter {
                            continue;
                        }
                    }
                    orders.push(self.parse_order(&order_data, sym));
                }
            }
        }

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let params = HashMap::new();

        let response: KrakenFuturesFillsResponse = self
            .private_request("GET", "/derivatives/api/v3/fills", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut trades = Vec::new();

        for fill_data in response.fills {
            if let Some(market_id) = &fill_data.symbol {
                if let Some(sym) = markets_by_id.get(market_id) {
                    if let Some(filter) = symbol {
                        if sym != filter {
                            continue;
                        }
                    }
                    trades.push(self.parse_trade(&fill_data, sym));
                }
            }
        }

        Ok(trades)
    }

    async fn fetch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>> {
        let params = HashMap::new();

        let response: KrakenFuturesPositionsResponse = self
            .private_request("GET", "/derivatives/api/v3/openpositions", params)
            .await?;

        let mut positions = Vec::new();

        for position_data in response.open_positions {
            let symbol = self.to_symbol(&position_data.symbol);

            if let Some(filter) = symbols {
                if !filter.contains(&symbol.as_str()) {
                    continue;
                }
            }

            positions.push(self.parse_position(&position_data));
        }

        Ok(positions)
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> CcxtResult<FundingRate> {
        let rates = self.fetch_funding_rates(Some(&[symbol])).await?;
        rates.get(symbol).cloned().ok_or_else(|| CcxtError::ExchangeError {
            message: format!("Funding rate not found for symbol: {symbol}"),
        })
    }

    async fn fetch_funding_rates(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, FundingRate>> {
        let tickers = self.fetch_tickers(symbols).await?;
        let mut rates = HashMap::new();

        for (symbol, ticker) in tickers {
            let funding_rate = FundingRate {
                symbol: symbol.clone(),
                info: ticker.info.clone(),
                timestamp: ticker.timestamp,
                datetime: ticker.datetime.clone(),
                funding_rate: None,
                mark_price: ticker.mark_price,
                index_price: ticker.index_price,
                interest_rate: None,
                estimated_settle_price: None,
                funding_timestamp: None,
                funding_datetime: None,
                next_funding_timestamp: None,
                next_funding_datetime: None,
                next_funding_rate: None,
                previous_funding_timestamp: None,
                previous_funding_datetime: None,
                previous_funding_rate: None,
                interval: Some("8h".to_string()),
            };
            rates.insert(symbol, funding_rate);
        }

        Ok(rates)
    }

    async fn fetch_funding_rate_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<FundingRateHistory>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: KrakenFuturesFundingRateHistoryResponse = self
            .public_get("/derivatives/api/v4/historicalfundingrates", Some(params))
            .await?;

        let mut rates: Vec<FundingRateHistory> = response.rates
            .iter()
            .filter_map(|r| {
                let timestamp = r.timestamp.as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())?;
                let funding_rate: Decimal = r.relative_funding_rate.as_ref()?.parse().ok()?;

                Some(FundingRateHistory {
                    info: serde_json::to_value(r).unwrap_or_default(),
                    symbol: symbol.to_string(),
                    funding_rate,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                })
            })
            .collect();

        // Filter by since and limit
        if let Some(s) = since {
            rates.retain(|r| r.timestamp.unwrap_or(0) >= s);
        }

        if let Some(l) = limit {
            rates.truncate(l as usize);
        }

        Ok(rates)
    }

    fn sign(&self, path: &str, api: &str, method: &str, params: HashMap<String, String>) -> CcxtResult<SignedRequest> {
        let mut url = if api == "charts" {
            format!("{}{}", self.urls.api.get("charts").unwrap(), path)
        } else if api == "history" {
            format!("{}{}", self.urls.api.get("history").unwrap(), path)
        } else if api == "public" {
            format!("{}{}", self.urls.api.get("public").unwrap(), path)
        } else {
            format!("{}{}", self.urls.api.get("private").unwrap(), path)
        };

        let mut headers = HashMap::new();

        if api == "private" {
            let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
            let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

            let query_string = if params.is_empty() {
                String::new()
            } else {
                params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&")
            };

            let endpoint = path.trim_start_matches('/');
            let mut auth_string = query_string.clone();
            auth_string.push_str("/api/v3/");
            auth_string.push_str(endpoint);

            // Hash with SHA256
            let hash = sha2::Sha256::digest(auth_string.as_bytes());

            // Decode base64 secret
            let secret_bytes = base64::decode(secret).map_err(|e| CcxtError::AuthenticationError {
                message: format!("Invalid secret: {e}"),
            })?;

            // Sign with HMAC-SHA512
            let mut mac = HmacSha512::new_from_slice(&secret_bytes)
                .map_err(|e| CcxtError::AuthenticationError {
                    message: format!("HMAC error: {e}"),
                })?;
            mac.update(&hash);
            let signature = base64::encode(mac.finalize().into_bytes());

            headers.insert("Content-Type".into(), "application/x-www-form-urlencoded".into());
            headers.insert("Accept".into(), "application/json".into());
            headers.insert("APIKey".into(), api_key.to_string());
            headers.insert("Authent".into(), signature);

            if !query_string.is_empty() {
                url = format!("{url}?{query_string}");
            }
        } else if !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}?{query}");
        }

        Ok(SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        })
    }
}

// === Kraken Futures API Response Types ===

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesInstrumentsResponse {
    instruments: Vec<KrakenFuturesInstrument>,
    #[serde(default)]
    result: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesInstrument {
    symbol: String,
    #[serde(rename = "type")]
    instrument_type: Option<String>,
    #[serde(default)]
    tradeable: Option<bool>,
    #[serde(default)]
    last_trading_time: Option<String>,
    #[serde(default)]
    tick_size: Option<f64>,
    #[serde(default)]
    contract_size: Option<Decimal>,
    #[serde(default)]
    opening_date: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesTickersResponse {
    tickers: Vec<KrakenFuturesTicker>,
    #[serde(default)]
    server_time: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesTicker {
    symbol: String,
    #[serde(default)]
    bid: Option<String>,
    #[serde(default)]
    bid_size: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    ask_size: Option<String>,
    #[serde(default)]
    vol24h: Option<String>,
    #[serde(default)]
    open24h: Option<String>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    last_time: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    index_price: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesOrderBookResponse {
    #[serde(rename = "orderBook")]
    orderbook: KrakenFuturesOrderBookData,
    #[serde(default)]
    server_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KrakenFuturesOrderBookData {
    bids: Vec<Vec<serde_json::Value>>,
    asks: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesHistoryResponse {
    history: Vec<KrakenFuturesHistoryItem>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesHistoryItem {
    time: i64,
    #[serde(default)]
    uid: Option<String>,
    price: String,
    size: String,
    #[serde(default)]
    side: Option<String>,
    #[serde(rename = "type")]
    #[serde(default)]
    trade_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesOHLCVResponse {
    candles: Vec<KrakenFuturesCandle>,
}

#[derive(Debug, Deserialize)]
struct KrakenFuturesCandle {
    time: i64,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesOrderStatus {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    cli_ord_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    limit_price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    filled: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    last_update_timestamp: Option<String>,
    status: String,
    #[serde(default)]
    reduce_only: Option<bool>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    trigger_price: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesOrderResponse {
    #[serde(rename = "sendStatus")]
    send_status: Option<KrakenFuturesOrderStatus>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesCancelOrderResponse {
    #[serde(rename = "cancelStatus")]
    cancel_status: Option<KrakenFuturesOrderStatus>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesOpenOrdersResponse {
    #[serde(rename = "openOrders")]
    open_orders: Vec<KrakenFuturesOrderStatus>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesAccountsResponse {
    accounts: HashMap<String, KrakenFuturesAccount>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesAccount {
    #[serde(default)]
    balances: Option<HashMap<String, String>>,
    #[serde(rename = "type")]
    #[serde(default)]
    account_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesPosition {
    symbol: String,
    side: String,
    price: String,
    size: String,
    #[serde(default)]
    fill_time: Option<String>,
    #[serde(default)]
    unrealized_funding: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesPositionsResponse {
    #[serde(rename = "openPositions")]
    open_positions: Vec<KrakenFuturesPosition>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesTrade {
    #[serde(default)]
    fill_id: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    price: String,
    size: String,
    #[serde(default)]
    fill_time: Option<String>,
    #[serde(default)]
    fill_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesFillsResponse {
    fills: Vec<KrakenFuturesTrade>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesFundingRateHistoryResponse {
    rates: Vec<KrakenFuturesFundingRateItem>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KrakenFuturesFundingRateItem {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    funding_rate: Option<String>,
    #[serde(default)]
    relative_funding_rate: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_creation() {
        let config = ExchangeConfig::new();
        let exchange = KrakenFutures::new(config);
        assert!(exchange.is_ok());

        let exchange = exchange.unwrap();
        assert_eq!(exchange.name(), "Kraken Futures");
        assert_eq!(exchange.version(), "v3");
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = KrakenFutures::new(config).unwrap();

        // Test with empty markets (should return input)
        assert_eq!(exchange.to_market_id("BTC/USD:USD"), "BTC/USD:USD");
    }

    #[test]
    fn test_timeframes() {
        let config = ExchangeConfig::new();
        let exchange = KrakenFutures::new(config).unwrap();

        let timeframes = exchange.timeframes();
        assert!(timeframes.contains_key(&Timeframe::Minute1));
        assert_eq!(timeframes.get(&Timeframe::Minute1).unwrap(), "1m");
        assert!(timeframes.contains_key(&Timeframe::Hour1));
        assert_eq!(timeframes.get(&Timeframe::Hour1).unwrap(), "1h");
    }

    #[test]
    fn test_features() {
        let config = ExchangeConfig::new();
        let exchange = KrakenFutures::new(config).unwrap();

        let features = exchange.has();
        assert!(features.swap);
        assert!(features.future);
        assert!(!features.spot);
        assert!(features.fetch_markets);
        assert!(features.fetch_ticker);
        assert!(features.fetch_positions);
        assert!(features.fetch_funding_rate);
    }

    #[tokio::test]
    async fn test_parse_ticker() {
        let config = ExchangeConfig::new();
        let exchange = KrakenFutures::new(config).unwrap();

        let ticker_data = KrakenFuturesTicker {
            symbol: "pf_xbtusd".to_string(),
            bid: Some("50000".to_string()),
            bid_size: Some("100".to_string()),
            ask: Some("50100".to_string()),
            ask_size: Some("200".to_string()),
            vol24h: Some("1000".to_string()),
            open24h: Some("49000".to_string()),
            last: Some("50050".to_string()),
            last_time: Some("2024-01-01T00:00:00.000Z".to_string()),
            mark_price: Some("50060".to_string()),
            index_price: Some("50040".to_string()),
        };

        let ticker = exchange.parse_ticker(&ticker_data, "BTC/USD:USD");
        assert_eq!(ticker.symbol, "BTC/USD:USD");
        assert!(ticker.bid.is_some());
        assert!(ticker.ask.is_some());
    }

    #[tokio::test]
    async fn test_parse_position() {
        let config = ExchangeConfig::new();
        let exchange = KrakenFutures::new(config).unwrap();

        let position_data = KrakenFuturesPosition {
            symbol: "pf_xbtusd".to_string(),
            side: "long".to_string(),
            price: "50000".to_string(),
            size: "0.5".to_string(),
            fill_time: Some("2024-01-01T00:00:00.000Z".to_string()),
            unrealized_funding: Some("-0.001".to_string()),
        };

        let position = exchange.parse_position(&position_data);
        assert_eq!(position.side, Some(PositionSide::Long));
        assert!(position.contracts.is_some());
        assert!(position.entry_price.is_some());
    }
}
