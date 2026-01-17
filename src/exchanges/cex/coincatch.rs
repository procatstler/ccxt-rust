//! CoinCatch Exchange Implementation
//!
//! CoinCatch is a cryptocurrency derivatives exchange.
//! API Documentation: <https://coincatch.github.io/github.io/en/spot/>

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Fee, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// CoinCatch exchange implementation
pub struct CoinCatch {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, Market>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl CoinCatch {
    /// Create a new CoinCatch instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let base_url = "https://api.coincatch.com";
        let client = HttpClient::new(base_url, &config)?;
        let rate_limiter = RateLimiter::new(100); // 100ms between requests

        let mut urls = ExchangeUrls::default();
        urls.api.insert("public".to_string(), base_url.to_string());
        urls.api.insert("private".to_string(), base_url.to_string());
        urls.www = Some("https://www.coincatch.com".to_string());
        urls.doc = vec!["https://coincatch.github.io/github.io/en/spot/".to_string()];

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1min".to_string());
        timeframes.insert(Timeframe::Minute5, "5min".to_string());
        timeframes.insert(Timeframe::Minute15, "15min".to_string());
        timeframes.insert(Timeframe::Minute30, "30min".to_string());
        timeframes.insert(Timeframe::Hour1, "1h".to_string());
        timeframes.insert(Timeframe::Hour4, "4h".to_string());
        timeframes.insert(Timeframe::Hour6, "6h".to_string());
        timeframes.insert(Timeframe::Hour12, "12h".to_string());
        timeframes.insert(Timeframe::Day1, "1day".to_string());
        timeframes.insert(Timeframe::Day3, "3day".to_string());
        timeframes.insert(Timeframe::Week1, "1week".to_string());
        timeframes.insert(Timeframe::Month1, "1M".to_string());

        let features = ExchangeFeatures {
            spot: true,
            fetch_markets: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            cancel_order: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

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

    /// Convert symbol to market ID
    fn to_market_id(&self, symbol: &str) -> String {
        // Convert BTC/USDT to BTCUSDT_SPBL
        let market_id = symbol.replace("/", "");
        format!("{market_id}_SPBL")
    }

    /// Convert market ID to symbol
    fn to_symbol(&self, market_id: &str) -> String {
        // Convert BTCUSDT_SPBL to BTC/USDT
        let id = market_id.replace("_SPBL", "");
        if id.ends_with("USDT") {
            let base = id.trim_end_matches("USDT");
            format!("{base}/USDT")
        } else if id.ends_with("USDC") {
            let base = id.trim_end_matches("USDC");
            format!("{base}/USDC")
        } else if id.ends_with("BTC") && id != "BTC" {
            let base = id.trim_end_matches("BTC");
            format!("{base}/BTC")
        } else if id.ends_with("ETH") && id != "ETH" {
            let base = id.trim_end_matches("ETH");
            format!("{base}/ETH")
        } else {
            id
        }
    }

    /// Send public GET request
    async fn public_get(
        &self,
        path: &str,
        params: Option<&HashMap<String, String>>,
    ) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let params_owned = params.cloned();
        let data: CoinCatchResponse<serde_json::Value> =
            self.client.get(path, params_owned, None).await?;

        if data.code != "00000" {
            return Err(CcxtError::ExchangeError {
                message: format!(
                    "CoinCatch API error: {} - {}",
                    data.code,
                    data.msg.unwrap_or_default()
                ),
            });
        }

        data.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".to_string(),
        })
    }

    /// Send private request with authentication
    async fn private_request(
        &self,
        method: &str,
        path: &str,
        params: Option<&HashMap<String, String>>,
        body: Option<&serde_json::Value>,
    ) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".to_string(),
            })?;
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".to_string(),
            })?;
        let passphrase = self
            .config
            .password()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Passphrase required".to_string(),
            })?;

        let timestamp = chrono::Utc::now().timestamp_millis().to_string();

        let mut request_path = path.to_string();
        if let Some(p) = params {
            if !p.is_empty() {
                let query: Vec<String> = p.iter().map(|(k, v)| format!("{k}={v}")).collect();
                request_path = format!("{}?{}", path, query.join("&"));
            }
        }

        let body_str = if let Some(b) = body {
            serde_json::to_string(b)?
        } else {
            String::new()
        };

        // Signature: timestamp + method + requestPath + body
        let sign_str = format!(
            "{}{}{}{}",
            timestamp,
            method.to_uppercase(),
            request_path,
            body_str
        );
        let signature = self.sign_message(&sign_str, secret)?;

        let mut headers = HashMap::new();
        headers.insert("ACCESS-KEY".to_string(), api_key.to_string());
        headers.insert("ACCESS-SIGN".to_string(), signature);
        headers.insert("ACCESS-TIMESTAMP".to_string(), timestamp);
        headers.insert("ACCESS-PASSPHRASE".to_string(), passphrase.to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let data: CoinCatchResponse<serde_json::Value> = match method.to_uppercase().as_str() {
            "GET" => {
                self.client
                    .get(&request_path, params.cloned(), Some(headers))
                    .await?
            },
            "POST" => {
                self.client
                    .post(&request_path, body.cloned(), Some(headers))
                    .await?
            },
            _ => {
                return Err(CcxtError::ExchangeError {
                    message: format!("Unsupported method: {method}"),
                })
            },
        };

        if data.code != "00000" {
            return Err(CcxtError::ExchangeError {
                message: format!(
                    "CoinCatch API error: {} - {}",
                    data.code,
                    data.msg.unwrap_or_default()
                ),
            });
        }

        data.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".to_string(),
        })
    }

    /// Create HMAC-SHA256 signature
    fn sign_message(&self, message: &str, secret: &str) -> CcxtResult<String> {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| {
            CcxtError::ExchangeError {
                message: format!("HMAC error: {e}"),
            }
        })?;
        mac.update(message.as_bytes());
        let result = mac.finalize();
        Ok(BASE64.encode(result.into_bytes()))
    }

    /// Parse ticker from API response
    fn parse_ticker(&self, data: &CoinCatchTicker, symbol: &str) -> Ticker {
        let timestamp = data.ts.parse::<i64>().unwrap_or(0);
        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            low: data.low.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            bid: data
                .buy_one
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok()),
            bid_volume: None,
            ask: data
                .sell_one
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok()),
            ask_volume: None,
            vwap: None,
            open: data.open.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            close: data.close.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            last: data.close.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            previous_close: None,
            change: data.change.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            percentage: data
                .change_percentage
                .as_ref()
                .and_then(|s| Decimal::from_str(s.trim_end_matches('%')).ok()),
            average: None,
            base_volume: data
                .base_vol
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok()),
            quote_volume: data
                .quote_vol
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order from API response
    fn parse_order(&self, data: &CoinCatchOrder) -> Order {
        let timestamp = data
            .c_time
            .as_ref()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);

        let status = match data.status.as_deref() {
            Some("new") => OrderStatus::Open,
            Some("partial_fill") => OrderStatus::Open,
            Some("full_fill") => OrderStatus::Closed,
            Some("cancelled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let symbol = data
            .symbol
            .as_ref()
            .map(|s| self.to_symbol(s))
            .unwrap_or_default();

        let price = data.price.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let amount = data
            .quantity
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let filled = data
            .fill_quantity
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let remaining = amount - filled;
        let cost = data
            .fill_total_amount
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price,
            average: data
                .fill_price
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok()),
            amount,
            filled,
            remaining: Some(remaining),
            cost,
            trades: vec![],
            fee: None,
            fees: vec![],
            info: serde_json::to_value(data).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        }
    }

    /// Parse balance from API response
    fn parse_balance(&self, data: &[CoinCatchAsset]) -> Balances {
        let mut currencies = HashMap::new();

        for asset in data {
            let currency = asset.coin_name.clone().unwrap_or_default();
            let free = asset
                .available
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);
            let used = asset
                .frozen
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO)
                + asset
                    .lock
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
            let total = free + used;

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

        Balances {
            currencies,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            datetime: Some(chrono::Utc::now().to_rfc3339()),
            info: serde_json::Value::Null,
        }
    }
}

#[async_trait]
impl Exchange for CoinCatch {
    fn id(&self) -> ExchangeId {
        ExchangeId::CoinCatch
    }

    fn name(&self) -> &str {
        "CoinCatch"
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

        let markets = self.fetch_markets().await?;
        let mut markets_map = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for market in markets {
            markets_map.insert(market.symbol.clone(), market.clone());
            markets_by_id.insert(market.id.clone(), market);
        }

        {
            let mut m = self.markets.write().unwrap();
            *m = markets_map.clone();
        }
        {
            let mut m = self.markets_by_id.write().unwrap();
            *m = markets_by_id;
        }

        Ok(markets_map)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        // Fetch tickers to get all trading pairs
        let data = self.public_get("/api/spot/v1/market/tickers", None).await?;
        let tickers: Vec<CoinCatchTicker> = serde_json::from_value(data)?;

        let mut markets = Vec::new();

        for ticker in tickers {
            if let Some(symbol_id) = &ticker.symbol {
                let symbol = self.to_symbol(symbol_id);
                let parts: Vec<&str> = symbol.split('/').collect();
                if parts.len() != 2 {
                    continue;
                }

                let base = parts[0].to_string();
                let quote = parts[1].to_string();

                markets.push(Market {
                    id: symbol_id.clone(),
                    lowercase_id: Some(symbol_id.to_lowercase()),
                    symbol: symbol.clone(),
                    base: base.clone(),
                    quote: quote.clone(),
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
            underlying: None,
            underlying_id: None,
                    taker: Some(Decimal::from_str("0.001").unwrap()),
                    maker: Some(Decimal::from_str("0.001").unwrap()),
                    percentage: true,
                    tier_based: false,
                    precision: MarketPrecision {
                        amount: Some(8),
                        price: Some(8),
                        cost: None,
                        base: None,
                        quote: None,
                    },
                    limits: MarketLimits {
                        amount: MinMax {
                            min: Some(Decimal::from_str("0.00001").unwrap()),
                            max: None,
                        },
                        price: MinMax {
                            min: None,
                            max: None,
                        },
                        cost: MinMax {
                            min: Some(Decimal::from_str("1").unwrap()),
                            max: None,
                        },
                        leverage: MinMax {
                            min: None,
                            max: None,
                        },
                    },
                    margin_modes: None,
                    created: None,
                    info: serde_json::to_value(&ticker).unwrap_or_default(),
                });
            }
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        let data = self
            .public_get("/api/spot/v1/market/ticker", Some(&params))
            .await?;
        let ticker: CoinCatchTicker = serde_json::from_value(data)?;

        Ok(self.parse_ticker(&ticker, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let data = self.public_get("/api/spot/v1/market/tickers", None).await?;
        let tickers: Vec<CoinCatchTicker> = serde_json::from_value(data)?;

        let mut result = HashMap::new();
        for ticker in tickers {
            if let Some(symbol_id) = &ticker.symbol {
                let symbol = self.to_symbol(symbol_id);

                if let Some(syms) = symbols {
                    if !syms.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let parsed_ticker = self.parse_ticker(&ticker, &symbol);
                result.insert(symbol, parsed_ticker);
            }
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("type".to_string(), "step0".to_string());
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let data = self
            .public_get("/api/spot/v1/market/depth", Some(&params))
            .await?;
        let book: CoinCatchOrderBook = serde_json::from_value(data)?;

        let timestamp = book
            .timestamp
            .as_ref()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);

        let parse_entries = |entries: &[Vec<String>]| -> Vec<OrderBookEntry> {
            entries
                .iter()
                .filter_map(|e| {
                    if e.len() >= 2 {
                        Some(OrderBookEntry {
                            price: Decimal::from_str(&e[0]).ok()?,
                            amount: Decimal::from_str(&e[1]).ok()?,
                        })
                    } else {
                        None
                    }
                })
                .collect()
        };

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
            checksum: None,
            bids: parse_entries(&book.bids),
            asks: parse_entries(&book.asks),
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
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let data = self
            .public_get("/api/spot/v1/market/fills", Some(&params))
            .await?;
        let trades: Vec<CoinCatchTrade> = serde_json::from_value(data)?;

        let result: Vec<Trade> = trades
            .iter()
            .map(|t| {
                let timestamp =
                    t.ts.as_ref()
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0);

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
                    side: t.side.clone(),
                    taker_or_maker: None,
                    price: t
                        .price
                        .as_ref()
                        .and_then(|s| Decimal::from_str(s).ok())
                        .unwrap_or(Decimal::ZERO),
                    amount: t
                        .size
                        .as_ref()
                        .and_then(|s| Decimal::from_str(s).ok())
                        .unwrap_or(Decimal::ZERO),
                    cost: None,
                    fee: None,
                    fees: vec![],
                    info: serde_json::to_value(t).unwrap_or_default(),
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
        let market_id = self.to_market_id(symbol);
        let period = self
            .timeframes
            .get(&timeframe)
            .cloned()
            .unwrap_or_else(|| "1min".to_string());

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("period".to_string(), period);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }
        if let Some(s) = since {
            params.insert("after".to_string(), s.to_string());
        }

        let data = self
            .public_get("/api/spot/v1/market/candles", Some(&params))
            .await?;
        let candles: Vec<Vec<String>> = serde_json::from_value(data)?;

        let result: Vec<OHLCV> = candles
            .iter()
            .filter_map(|c| {
                if c.len() >= 6 {
                    Some(OHLCV {
                        timestamp: c[0].parse().ok()?,
                        open: Decimal::from_str(&c[1]).ok()?,
                        high: Decimal::from_str(&c[2]).ok()?,
                        low: Decimal::from_str(&c[3]).ok()?,
                        close: Decimal::from_str(&c[4]).ok()?,
                        volume: Decimal::from_str(&c[5]).ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(result)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let data = self
            .private_request("GET", "/api/spot/v1/account/assets", None, None)
            .await?;
        let assets: Vec<CoinCatchAsset> = serde_json::from_value(data)?;
        Ok(self.parse_balance(&assets))
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

        let order_type_str = match order_type {
            OrderType::Limit => "limit",
            OrderType::Market => "market",
            _ => {
                return Err(CcxtError::ExchangeError {
                    message: format!("Unsupported order type: {order_type:?}"),
                })
            },
        };

        let side_str = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };

        let mut body = serde_json::json!({
            "symbol": market_id,
            "side": side_str,
            "orderType": order_type_str,
            "quantity": amount.to_string(),
            "force": "normal"
        });

        if let Some(p) = price {
            body["price"] = serde_json::Value::String(p.to_string());
        }

        let data = self
            .private_request("POST", "/api/spot/v1/trade/orders", None, Some(&body))
            .await?;
        let result: CoinCatchOrderResult = serde_json::from_value(data)?;

        let timestamp = chrono::Utc::now().timestamp_millis();
        Ok(Order {
            id: result.order_id.unwrap_or_default(),
            client_order_id: result.client_order_id,
            timestamp: Some(timestamp),
            datetime: Some(chrono::Utc::now().to_rfc3339()),
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
            trades: vec![],
            fee: None,
            fees: vec![],
            info: serde_json::Value::Null,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let body = serde_json::json!({
            "symbol": market_id,
            "orderId": id
        });

        self.private_request("POST", "/api/spot/v1/trade/cancel-order", None, Some(&body))
            .await?;

        let timestamp = chrono::Utc::now().timestamp_millis();
        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(chrono::Utc::now().to_rfc3339()),
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
            remaining: None,
            cost: None,
            trades: vec![],
            fee: None,
            fees: vec![],
            info: serde_json::Value::Null,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let body = serde_json::json!({
            "symbol": market_id,
            "orderId": id
        });

        let data = self
            .private_request("POST", "/api/spot/v1/trade/orderInfo", None, Some(&body))
            .await?;
        let order: CoinCatchOrder = serde_json::from_value(data)?;

        Ok(self.parse_order(&order))
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let sym = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Symbol required".to_string(),
        })?;
        let market_id = self.to_market_id(sym);

        let mut body = serde_json::json!({
            "symbol": market_id
        });

        if let Some(l) = limit {
            body["limit"] = serde_json::Value::String(l.to_string());
        }

        let data = self
            .private_request("POST", "/api/spot/v1/trade/history", None, Some(&body))
            .await?;
        let orders: Vec<CoinCatchOrder> = serde_json::from_value(data)?;

        Ok(orders.iter().map(|o| self.parse_order(o)).collect())
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let sym = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Symbol required".to_string(),
        })?;
        let market_id = self.to_market_id(sym);

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let data = self
            .private_request(
                "POST",
                "/api/spot/v1/trade/open-orders",
                Some(&params),
                None,
            )
            .await?;
        let orders: Vec<CoinCatchOrder> = serde_json::from_value(data)?;

        Ok(orders.iter().map(|o| self.parse_order(o)).collect())
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let orders = self.fetch_orders(symbol, since, limit).await?;
        Ok(orders
            .into_iter()
            .filter(|o| matches!(o.status, OrderStatus::Closed | OrderStatus::Canceled))
            .collect())
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let sym = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Symbol required".to_string(),
        })?;
        let market_id = self.to_market_id(sym);

        let mut body = serde_json::json!({
            "symbol": market_id
        });

        if let Some(l) = limit {
            body["limit"] = serde_json::Value::String(l.to_string());
        }

        let data = self
            .private_request("POST", "/api/spot/v1/trade/fills", None, Some(&body))
            .await?;
        let fills: Vec<CoinCatchFill> = serde_json::from_value(data)?;

        let result: Vec<Trade> = fills
            .iter()
            .map(|f| {
                let timestamp = f
                    .c_time
                    .as_ref()
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0);

                let fee = f
                    .fee
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .map(|cost| Fee {
                        cost: Some(cost),
                        currency: f.fee_currency.clone(),
                        rate: None,
                    });

                Trade {
                    id: f.trade_id.clone().unwrap_or_default(),
                    order: f.order_id.clone(),
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: sym.to_string(),
                    trade_type: None,
                    side: f.side.clone(),
                    taker_or_maker: f.trade_scope.as_ref().and_then(|s| match s.as_str() {
                        "taker" => Some(TakerOrMaker::Taker),
                        "maker" => Some(TakerOrMaker::Maker),
                        _ => None,
                    }),
                    price: f
                        .price
                        .as_ref()
                        .and_then(|s| Decimal::from_str(s).ok())
                        .unwrap_or(Decimal::ZERO),
                    amount: f
                        .size
                        .as_ref()
                        .and_then(|s| Decimal::from_str(s).ok())
                        .unwrap_or(Decimal::ZERO),
                    cost: None,
                    fee,
                    fees: vec![],
                    info: serde_json::to_value(f).unwrap_or_default(),
                }
            })
            .collect();

        Ok(result)
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        Some(self.to_market_id(symbol))
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        Some(self.to_symbol(market_id))
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
        let timestamp = chrono::Utc::now().timestamp_millis().to_string();

        let mut request_path = path.to_string();
        if !params.is_empty() {
            let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
            request_path = format!("{}?{}", path, query.join("&"));
        }

        let body_str = body.unwrap_or("");
        let sign_str = format!(
            "{}{}{}{}",
            timestamp,
            method.to_uppercase(),
            request_path,
            body_str
        );

        let mut result_headers = headers.unwrap_or_default();

        if let (Some(api_key), Some(secret), Some(passphrase)) = (
            self.config.api_key(),
            self.config.secret(),
            self.config.password(),
        ) {
            if let Ok(signature) = self.sign_message(&sign_str, secret) {
                result_headers.insert("ACCESS-KEY".to_string(), api_key.to_string());
                result_headers.insert("ACCESS-SIGN".to_string(), signature);
                result_headers.insert("ACCESS-TIMESTAMP".to_string(), timestamp);
                result_headers.insert("ACCESS-PASSPHRASE".to_string(), passphrase.to_string());
                result_headers.insert("Content-Type".to_string(), "application/json".to_string());
            }
        }

        SignedRequest {
            url: format!("{}{}", self.urls.api.get("private").unwrap(), request_path),
            method: method.to_string(),
            headers: result_headers,
            body: body.map(|s| s.to_string()),
        }
    }
}

// Response structures
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoinCatchResponse<T> {
    code: String,
    msg: Option<String>,
    data: Option<T>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoinCatchTicker {
    symbol: Option<String>,
    ts: String,
    #[serde(rename = "buyOne")]
    buy_one: Option<String>,
    #[serde(rename = "sellOne")]
    sell_one: Option<String>,
    high: Option<String>,
    low: Option<String>,
    open: Option<String>,
    close: Option<String>,
    change: Option<String>,
    #[serde(rename = "changePercentage")]
    change_percentage: Option<String>,
    #[serde(rename = "baseVol")]
    base_vol: Option<String>,
    #[serde(rename = "quoteVol")]
    quote_vol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoinCatchOrderBook {
    timestamp: Option<String>,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoinCatchTrade {
    #[serde(rename = "tradeId")]
    trade_id: Option<String>,
    ts: Option<String>,
    side: Option<String>,
    price: Option<String>,
    size: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoinCatchOrder {
    #[serde(rename = "orderId")]
    order_id: Option<String>,
    #[serde(rename = "clientOrderId")]
    client_order_id: Option<String>,
    symbol: Option<String>,
    #[serde(rename = "cTime")]
    c_time: Option<String>,
    side: Option<String>,
    #[serde(rename = "orderType")]
    order_type: Option<String>,
    price: Option<String>,
    quantity: Option<String>,
    status: Option<String>,
    #[serde(rename = "fillPrice")]
    fill_price: Option<String>,
    #[serde(rename = "fillQuantity")]
    fill_quantity: Option<String>,
    #[serde(rename = "fillTotalAmount")]
    fill_total_amount: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoinCatchOrderResult {
    #[serde(rename = "orderId")]
    order_id: Option<String>,
    #[serde(rename = "clientOrderId")]
    client_order_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoinCatchAsset {
    #[serde(rename = "coinId")]
    coin_id: Option<String>,
    #[serde(rename = "coinName")]
    coin_name: Option<String>,
    available: Option<String>,
    frozen: Option<String>,
    lock: Option<String>,
    #[serde(rename = "uTime")]
    u_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoinCatchFill {
    #[serde(rename = "tradeId")]
    trade_id: Option<String>,
    #[serde(rename = "orderId")]
    order_id: Option<String>,
    symbol: Option<String>,
    #[serde(rename = "cTime")]
    c_time: Option<String>,
    side: Option<String>,
    price: Option<String>,
    size: Option<String>,
    fee: Option<String>,
    #[serde(rename = "feeCurrency")]
    fee_currency: Option<String>,
    #[serde(rename = "tradeScope")]
    trade_scope: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_exchange() -> CoinCatch {
        let config = ExchangeConfig::default()
            .with_api_key("test_api_key")
            .with_api_secret("test_secret")
            .with_password("test_passphrase");
        CoinCatch::new(config).unwrap()
    }

    #[test]
    fn test_exchange_info() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.id(), ExchangeId::CoinCatch);
        assert_eq!(exchange.name(), "CoinCatch");
    }

    #[test]
    fn test_market_id_conversion() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.to_market_id("BTC/USDT"), "BTCUSDT_SPBL");
        assert_eq!(exchange.to_market_id("ETH/BTC"), "ETHBTC_SPBL");
    }

    #[test]
    fn test_symbol_conversion() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.to_symbol("BTCUSDT_SPBL"), "BTC/USDT");
        assert_eq!(exchange.to_symbol("ETHBTC_SPBL"), "ETH/BTC");
    }

    #[test]
    fn test_has_features() {
        let exchange = create_test_exchange();
        let features = exchange.has();
        assert!(features.fetch_markets);
        assert!(features.fetch_ticker);
        assert!(features.fetch_tickers);
        assert!(features.fetch_order_book);
        assert!(features.fetch_trades);
        assert!(features.fetch_ohlcv);
        assert!(features.fetch_balance);
        assert!(features.create_order);
        assert!(features.cancel_order);
        assert!(features.fetch_order);
        assert!(features.fetch_orders);
        assert!(features.fetch_open_orders);
        assert!(features.fetch_closed_orders);
        assert!(features.fetch_my_trades);
    }

    #[test]
    fn test_timeframes() {
        let exchange = create_test_exchange();
        let timeframes = exchange.timeframes();
        assert_eq!(
            timeframes.get(&Timeframe::Minute1),
            Some(&"1min".to_string())
        );
        assert_eq!(timeframes.get(&Timeframe::Hour1), Some(&"1h".to_string()));
        assert_eq!(timeframes.get(&Timeframe::Day1), Some(&"1day".to_string()));
    }

    #[test]
    fn test_signature() {
        let exchange = create_test_exchange();
        let result = exchange.sign_message("test_message", "test_secret");
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_ticker() {
        let exchange = create_test_exchange();
        let ticker_data = CoinCatchTicker {
            symbol: Some("BTCUSDT_SPBL".to_string()),
            ts: "1704067200000".to_string(),
            buy_one: Some("42000.00".to_string()),
            sell_one: Some("42001.00".to_string()),
            high: Some("42500.00".to_string()),
            low: Some("41500.00".to_string()),
            open: Some("41800.00".to_string()),
            close: Some("42000.00".to_string()),
            change: Some("200.00".to_string()),
            change_percentage: Some("0.48%".to_string()),
            base_vol: Some("1000.00".to_string()),
            quote_vol: Some("42000000.00".to_string()),
        };

        let ticker = exchange.parse_ticker(&ticker_data, "BTC/USDT");
        assert_eq!(ticker.symbol, "BTC/USDT");
        assert_eq!(ticker.bid, Some(Decimal::from_str("42000.00").unwrap()));
        assert_eq!(ticker.ask, Some(Decimal::from_str("42001.00").unwrap()));
    }

    #[test]
    fn test_parse_order() {
        let exchange = create_test_exchange();
        let order_data = CoinCatchOrder {
            order_id: Some("123456".to_string()),
            client_order_id: Some("client123".to_string()),
            symbol: Some("BTCUSDT_SPBL".to_string()),
            c_time: Some("1704067200000".to_string()),
            side: Some("buy".to_string()),
            order_type: Some("limit".to_string()),
            price: Some("42000.00".to_string()),
            quantity: Some("0.1".to_string()),
            status: Some("new".to_string()),
            fill_price: None,
            fill_quantity: Some("0".to_string()),
            fill_total_amount: Some("0".to_string()),
        };

        let order = exchange.parse_order(&order_data);
        assert_eq!(order.id, "123456");
        assert_eq!(order.client_order_id, Some("client123".to_string()));
        assert_eq!(order.symbol, "BTC/USDT");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.status, OrderStatus::Open);
    }

    #[test]
    fn test_parse_balance() {
        let exchange = create_test_exchange();
        let assets = vec![
            CoinCatchAsset {
                coin_id: Some("1".to_string()),
                coin_name: Some("BTC".to_string()),
                available: Some("1.5".to_string()),
                frozen: Some("0.5".to_string()),
                lock: Some("0".to_string()),
                u_time: None,
            },
            CoinCatchAsset {
                coin_id: Some("2".to_string()),
                coin_name: Some("USDT".to_string()),
                available: Some("10000".to_string()),
                frozen: Some("0".to_string()),
                lock: Some("1000".to_string()),
                u_time: None,
            },
        ];

        let balances = exchange.parse_balance(&assets);
        let btc = balances.currencies.get("BTC").unwrap();
        assert_eq!(btc.free, Some(Decimal::from_str("1.5").unwrap()));
        assert_eq!(btc.used, Some(Decimal::from_str("0.5").unwrap()));
        assert_eq!(btc.total, Some(Decimal::from_str("2.0").unwrap()));

        let usdt = balances.currencies.get("USDT").unwrap();
        assert_eq!(usdt.free, Some(Decimal::from_str("10000").unwrap()));
        assert_eq!(usdt.used, Some(Decimal::from_str("1000").unwrap()));
    }
}
