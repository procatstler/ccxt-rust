//! Defx Exchange Implementation
//!
//! Defx DEX exchange implementation for derivatives trading
//! API Documentation: <https://api-docs.defx.com/>

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use sha2::Sha256;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketType,
    Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, SignedRequest, Ticker,
    Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

// Response structures
#[derive(Debug, serde::Deserialize)]
struct DefxResponse<T> {
    data: Option<T>,
    success: Option<bool>,
    err: Option<DefxError>,
}

#[derive(Debug, serde::Deserialize)]
struct DefxError {
    msg: Option<String>,
    code: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DefxMarket {
    market: Option<String>,
    filters: Option<Vec<DefxFilter>>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DefxFilter {
    filter_type: Option<String>,
    min_qty: Option<String>,
    max_qty: Option<String>,
    step_size: Option<String>,
    min_price: Option<String>,
    max_price: Option<String>,
    tick_size: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DefxTicker {
    symbol: Option<String>,
    price_change: Option<String>,
    price_change_percent: Option<String>,
    last_price: Option<String>,
    last_qty: Option<String>,
    best_bid_price: Option<String>,
    best_bid_qty: Option<String>,
    best_ask_price: Option<String>,
    best_ask_qty: Option<String>,
    open_price: Option<String>,
    high_price: Option<String>,
    low_price: Option<String>,
    volume: Option<String>,
    quote_volume: Option<String>,
    open_time: Option<i64>,
    close_time: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefxOrderBookData {
    bids: Option<Vec<Vec<String>>>,
    asks: Option<Vec<Vec<String>>>,
    last_update_id: Option<i64>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DefxTrade {
    id: Option<String>,
    price: Option<String>,
    qty: Option<String>,
    quote_qty: Option<String>,
    side: Option<String>,
    time: Option<i64>,
    is_buyer_maker: Option<bool>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefxOHLC {
    open_time: Option<i64>,
    open: Option<String>,
    high: Option<String>,
    low: Option<String>,
    close: Option<String>,
    volume: Option<String>,
    close_time: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefxBalance {
    asset: Option<String>,
    free: Option<String>,
    locked: Option<String>,
    total: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefxBalanceResponse {
    balances: Option<Vec<DefxBalance>>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DefxOrder {
    order_id: Option<String>,
    client_order_id: Option<String>,
    symbol: Option<String>,
    side: Option<String>,
    order_type: Option<String>,
    price: Option<String>,
    quantity: Option<String>,
    filled_quantity: Option<String>,
    remaining_quantity: Option<String>,
    status: Option<String>,
    created_at: Option<i64>,
    updated_at: Option<i64>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DefxMyTrade {
    trade_id: Option<String>,
    order_id: Option<String>,
    symbol: Option<String>,
    side: Option<String>,
    price: Option<String>,
    qty: Option<String>,
    commission: Option<String>,
    commission_asset: Option<String>,
    time: Option<i64>,
    is_maker: Option<bool>,
}

pub struct Defx {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Defx {
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".to_string());
        timeframes.insert(Timeframe::Minute3, "3m".to_string());
        timeframes.insert(Timeframe::Minute5, "5m".to_string());
        timeframes.insert(Timeframe::Minute15, "15m".to_string());
        timeframes.insert(Timeframe::Minute30, "30m".to_string());
        timeframes.insert(Timeframe::Hour1, "1h".to_string());
        timeframes.insert(Timeframe::Hour2, "2h".to_string());
        timeframes.insert(Timeframe::Hour4, "4h".to_string());
        timeframes.insert(Timeframe::Hour12, "12h".to_string());
        timeframes.insert(Timeframe::Day1, "1d".to_string());
        timeframes.insert(Timeframe::Week1, "1w".to_string());
        timeframes.insert(Timeframe::Month1, "1M".to_string());

        let features = ExchangeFeatures {
            swap: true,
            fetch_markets: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            cancel_order: true,
            cancel_all_orders: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_positions: true,
            set_leverage: true,
            withdraw: true,
            ..Default::default()
        };

        let urls = ExchangeUrls {
            logo: Some(
                "https://github.com/user-attachments/assets/4e92bace-d7a9-45ea-92be-122168dc87e4"
                    .to_string(),
            ),
            api: HashMap::from([
                ("public".to_string(), "https://api.defx.com".to_string()),
                ("private".to_string(), "https://api.defx.com".to_string()),
            ]),
            www: Some("https://defx.com".to_string()),
            doc: vec!["https://api-docs.defx.com/".to_string()],
            fees: Some("https://defx.com/fees".to_string()),
        };

        let client = HttpClient::new("https://api.defx.com", &config)?;
        let rate_limiter = RateLimiter::new(100);

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

    fn generate_signature(&self, payload: &str) -> String {
        if let Some(secret) = self.config.secret() {
            if let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) {
                mac.update(payload.as_bytes());
                let result = mac.finalize();
                return hex::encode(result.into_bytes());
            }
        }
        String::new()
    }

    fn safe_symbol(&self, market_id: &str) -> String {
        // Convert DOGE_USDC to DOGE/USDC
        market_id.replace('_', "/")
    }

    fn safe_market_id(&self, symbol: &str) -> String {
        // Convert DOGE/USDC to DOGE_USDC
        symbol.replace('/', "_")
    }

    fn decimal_places(value: &Decimal) -> i32 {
        let s = value.to_string();
        if let Some(pos) = s.find('.') {
            (s.len() - pos - 1) as i32
        } else {
            0
        }
    }

    fn parse_ticker(&self, ticker_data: &DefxTicker) -> Ticker {
        let symbol = ticker_data
            .symbol
            .as_ref()
            .map(|s| self.safe_symbol(s))
            .unwrap_or_default();

        let last = ticker_data
            .last_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let open = ticker_data
            .open_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let high = ticker_data
            .high_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let low = ticker_data
            .low_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let base_volume = ticker_data
            .volume
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let quote_volume = ticker_data
            .quote_volume
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let change = ticker_data
            .price_change
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let percentage = ticker_data
            .price_change_percent
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let bid = ticker_data
            .best_bid_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let bid_volume = ticker_data
            .best_bid_qty
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let ask = ticker_data
            .best_ask_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let ask_volume = ticker_data
            .best_ask_qty
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let timestamp = ticker_data
            .close_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high,
            low,
            bid,
            bid_volume,
            ask,
            ask_volume,
            vwap: None,
            open,
            close: last,
            last,
            previous_close: None,
            change,
            percentage,
            average: None,
            base_volume,
            quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(ticker_data).unwrap_or_default(),
        }
    }

    fn parse_order(&self, order_data: &DefxOrder) -> Order {
        let symbol = order_data
            .symbol
            .as_ref()
            .map(|s| self.safe_symbol(s))
            .unwrap_or_default();

        let side = match order_data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match order_data.order_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let status = match order_data.status.as_deref() {
            Some("NEW") | Some("OPEN") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("CANCELED") | Some("CANCELLED") => OrderStatus::Canceled,
            Some("EXPIRED") => OrderStatus::Expired,
            Some("REJECTED") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let amount = order_data
            .quantity
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let price = order_data
            .price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let filled = order_data
            .filled_quantity
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let remaining = order_data
            .remaining_quantity
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let cost = price.map(|p| filled * p);

        let timestamp = order_data.created_at;

        Order {
            id: order_data.order_id.clone().unwrap_or_default(),
            client_order_id: order_data.client_order_id.clone(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: order_data.updated_at,
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
            cost,
            trades: vec![],
            fee: None,
            fees: vec![],
            info: serde_json::to_value(order_data).unwrap_or_default(),
            reduce_only: None,
            post_only: None,
            take_profit_price: None,
            stop_loss_price: None,
        }
    }

    fn parse_trade(&self, trade_data: &DefxTrade, symbol: &str) -> Trade {
        let price = trade_data
            .price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let amount = trade_data
            .qty
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);

        let side_str = trade_data.side.as_deref().unwrap_or("");
        let timestamp = trade_data.time;

        Trade {
            id: trade_data.id.clone().unwrap_or_default(),
            info: serde_json::to_value(trade_data).unwrap_or_default(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            symbol: symbol.to_string(),
            order: None,
            trade_type: None,
            side: if side_str.is_empty() {
                None
            } else {
                Some(side_str.to_lowercase())
            },
            taker_or_maker: trade_data.is_buyer_maker.map(|is_maker| {
                if is_maker {
                    crate::types::TakerOrMaker::Maker
                } else {
                    crate::types::TakerOrMaker::Taker
                }
            }),
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: vec![],
        }
    }

    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        let full_path = format!("/v1/open/{path}");
        self.client.get(&full_path, params, None).await
    }

    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key not configured".to_string(),
            })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let full_path = format!("/v1/auth/{path}");

        let mut headers = HashMap::new();
        headers.insert("X-DEFX-SOURCE".to_string(), "ccxt-rust".to_string());
        headers.insert("X-DEFX-APIKEY".to_string(), api_key.to_string());
        headers.insert("X-DEFX-TIMESTAMP".to_string(), timestamp.clone());

        if method == "GET" {
            // For GET requests, payload is timestamp + query string
            let query_string = params
                .as_ref()
                .map(|p| {
                    let mut pairs: Vec<_> = p.iter().collect();
                    pairs.sort_by_key(|(k, _)| *k);
                    pairs
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join("&")
                })
                .unwrap_or_default();
            let payload = format!("{timestamp}{query_string}");
            let signature = self.generate_signature(&payload);
            headers.insert("X-DEFX-SIGNATURE".to_string(), signature);
            self.client.get(&full_path, params, Some(headers)).await
        } else {
            // For POST/DELETE requests, payload is timestamp + body
            let body_json = params
                .as_ref()
                .map(|p| serde_json::to_value(p).unwrap_or(serde_json::json!({})));
            let body_str = body_json
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or_default();
            let payload = format!("{timestamp}{body_str}");
            let signature = self.generate_signature(&payload);
            headers.insert("X-DEFX-SIGNATURE".to_string(), signature);
            headers.insert("Content-Type".to_string(), "application/json".to_string());

            match method {
                "POST" => self.client.post(&full_path, body_json, Some(headers)).await,
                "DELETE" => self.client.delete(&full_path, None, Some(headers)).await,
                _ => Err(CcxtError::NotSupported {
                    feature: format!("HTTP method {method}"),
                }),
            }
        }
    }
}

#[async_trait]
impl Exchange for Defx {
    fn id(&self) -> ExchangeId {
        ExchangeId::Defx
    }

    fn name(&self) -> &'static str {
        "Defx"
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
        Some(self.safe_market_id(symbol))
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        Some(self.safe_symbol(market_id))
    }

    fn sign(
        &self,
        path: &str,
        _api: &str,
        method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let url = format!("https://api.defx.com{path}");
        let mut headers = HashMap::new();

        if let Some(api_key) = self.config.api_key() {
            let timestamp = Utc::now().timestamp_millis().to_string();
            let query_string = params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            let payload = format!("{timestamp}{query_string}");
            let signature = self.generate_signature(&payload);

            headers.insert("X-DEFX-SOURCE".to_string(), "ccxt-rust".to_string());
            headers.insert("X-DEFX-APIKEY".to_string(), api_key.to_string());
            headers.insert("X-DEFX-TIMESTAMP".to_string(), timestamp);
            headers.insert("X-DEFX-SIGNATURE".to_string(), signature);
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        }
    }

    async fn load_markets(&self, _reload: bool) -> CcxtResult<HashMap<String, Market>> {
        let response: DefxResponse<Vec<DefxMarket>> =
            self.public_get("c/markets?type=perps", None).await?;

        let markets_data = response.data.unwrap_or_default();
        let mut markets = HashMap::new();

        for market_data in markets_data {
            let id = market_data.market.clone().unwrap_or_default();
            let symbol = self.safe_symbol(&id);
            let parts: Vec<&str> = symbol.split('/').collect();
            let (base, quote) = if parts.len() >= 2 {
                (parts[0].to_string(), parts[1].to_string())
            } else {
                (symbol.clone(), "USDC".to_string())
            };

            let mut step_size = Decimal::new(1, 8);
            let mut tick_size = Decimal::new(1, 8);
            let mut min_amount: Option<Decimal> = None;
            let mut max_amount: Option<Decimal> = None;

            if let Some(filters) = &market_data.filters {
                for filter in filters {
                    match filter.filter_type.as_deref() {
                        Some("LOT_SIZE") => {
                            if let Some(step) = filter
                                .step_size
                                .as_ref()
                                .and_then(|s| Decimal::from_str(s).ok())
                            {
                                step_size = step;
                            }
                            min_amount = filter
                                .min_qty
                                .as_ref()
                                .and_then(|s| Decimal::from_str(s).ok());
                            max_amount = filter
                                .max_qty
                                .as_ref()
                                .and_then(|s| Decimal::from_str(s).ok());
                        },
                        Some("PRICE_FILTER") => {
                            if let Some(tick) = filter
                                .tick_size
                                .as_ref()
                                .and_then(|s| Decimal::from_str(s).ok())
                            {
                                tick_size = tick;
                            }
                        },
                        _ => {},
                    }
                }
            }

            let price_precision = Self::decimal_places(&tick_size);
            let amount_precision = Self::decimal_places(&step_size);

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                settle: Some(quote.clone()),
                settle_id: Some(quote.clone()),
                active: true,
                market_type: MarketType::Swap,
                spot: false,
                margin: false,
                swap: true,
                future: false,
                option: false,
                index: false,
                contract: true,
                linear: Some(true),
                inverse: Some(false),
                sub_type: Some("linear".into()),
                contract_size: Some(Decimal::ONE),
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                taker: Some(Decimal::new(5, 4)), // 0.05%
                maker: Some(Decimal::new(2, 4)), // 0.02%
                percentage: true,
                tier_based: false,
                precision: crate::types::MarketPrecision {
                    price: Some(price_precision),
                    amount: Some(amount_precision),
                    cost: None,
                    base: Some(amount_precision),
                    quote: Some(price_precision),
                },
                limits: crate::types::MarketLimits {
                    amount: crate::types::MinMax {
                        min: min_amount,
                        max: max_amount,
                    },
                    price: crate::types::MinMax {
                        min: Some(tick_size),
                        max: None,
                    },
                    cost: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                    leverage: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&market_data).unwrap_or_default(),
            };

            markets.insert(symbol.clone(), market);
        }

        // Update internal markets cache
        if let Ok(mut markets_guard) = self.markets.write() {
            *markets_guard = markets.clone();
        }

        if let Ok(mut markets_by_id_guard) = self.markets_by_id.write() {
            for (symbol, market) in &markets {
                markets_by_id_guard.insert(market.id.clone(), symbol.clone());
            }
        }

        Ok(markets)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(false).await?;
        Ok(markets.into_values().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.safe_market_id(symbol);
        let path = format!("symbols/{market_id}/ticker/24hr");
        let response: DefxTicker = self.public_get(&path, None).await?;
        Ok(self.parse_ticker(&response))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<DefxTicker> = self.public_get("ticker/24HrAgg", None).await?;

        let mut tickers = HashMap::new();
        for ticker_data in response {
            let ticker = self.parse_ticker(&ticker_data);
            if symbols.is_none()
                || symbols
                    .map(|s| s.contains(&ticker.symbol.as_str()))
                    .unwrap_or(false)
            {
                tickers.insert(ticker.symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.safe_market_id(symbol);
        let level = limit
            .map(|l| {
                if l <= 5 {
                    "5"
                } else if l <= 10 {
                    "10"
                } else {
                    "20"
                }
            })
            .unwrap_or("20");
        let path = format!("symbols/{market_id}/depth/{level}/0.01");
        let response: DefxOrderBookData = self.public_get(&path, None).await?;

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        if let Some(bid_data) = response.bids {
            for entry in bid_data {
                if entry.len() >= 2 {
                    if let (Ok(price), Ok(amount)) =
                        (Decimal::from_str(&entry[0]), Decimal::from_str(&entry[1]))
                    {
                        bids.push(OrderBookEntry { price, amount });
                    }
                }
            }
        }

        if let Some(ask_data) = response.asks {
            for entry in ask_data {
                if entry.len() >= 2 {
                    if let (Ok(price), Ok(amount)) =
                        (Decimal::from_str(&entry[0]), Decimal::from_str(&entry[1]))
                    {
                        asks.push(OrderBookEntry { price, amount });
                    }
                }
            }
        }

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            checksum: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: response.last_update_id,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.safe_market_id(symbol);
        let path = format!("symbols/{market_id}/trades");
        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<DefxTrade> = self.public_get(&path, Some(params)).await?;
        Ok(response
            .iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect())
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.safe_market_id(symbol);
        let tf_str = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::NotSupported {
                feature: format!("Timeframe {timeframe:?}"),
            })?;

        let path = format!("symbols/{market_id}/ohlc");
        let mut params = HashMap::new();
        params.insert("interval".to_string(), tf_str.clone());
        if let Some(s) = since {
            params.insert("startTime".to_string(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<DefxOHLC> = self.public_get(&path, Some(params)).await?;

        Ok(response
            .iter()
            .map(|c| OHLCV {
                timestamp: c.open_time.unwrap_or(0),
                open: c
                    .open
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO),
                high: c
                    .high
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO),
                low: c
                    .low
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO),
                close: c
                    .close
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO),
                volume: c
                    .volume
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO),
            })
            .collect())
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: DefxBalanceResponse = self
            .private_request("GET", "api/wallet/balance", None)
            .await?;

        let mut currencies = HashMap::new();

        if let Some(balances) = response.balances {
            for balance_data in balances {
                if let Some(asset) = &balance_data.asset {
                    let free = balance_data
                        .free
                        .as_ref()
                        .and_then(|s| Decimal::from_str(s).ok())
                        .unwrap_or(Decimal::ZERO);
                    let locked = balance_data
                        .locked
                        .as_ref()
                        .and_then(|s| Decimal::from_str(s).ok())
                        .unwrap_or(Decimal::ZERO);
                    let total = balance_data
                        .total
                        .as_ref()
                        .and_then(|s| Decimal::from_str(s).ok())
                        .unwrap_or(free + locked);

                    currencies.insert(
                        asset.clone(),
                        Balance {
                            free: Some(free),
                            used: Some(locked),
                            total: Some(total),
                            debt: None,
                        },
                    );
                }
            }
        }

        Ok(Balances {
            currencies,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
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
        let market_id = self.safe_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert(
            "side".to_string(),
            match side {
                OrderSide::Buy => "BUY".to_string(),
                OrderSide::Sell => "SELL".to_string(),
            },
        );
        params.insert(
            "type".to_string(),
            match order_type {
                OrderType::Limit => "LIMIT".to_string(),
                OrderType::Market => "MARKET".to_string(),
                _ => "LIMIT".to_string(),
            },
        );
        params.insert("quantity".to_string(), amount.to_string());

        if let Some(p) = price {
            params.insert("price".to_string(), p.to_string());
        }

        let response: DefxOrder = self
            .private_request("POST", "api/order", Some(params))
            .await?;
        Ok(self.parse_order(&response))
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("api/order/{id}");
        let response: DefxOrder = self.private_request("DELETE", &path, None).await?;
        Ok(self.parse_order(&response))
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("api/order/{id}");
        let response: DefxOrder = self.private_request("GET", &path, None).await?;
        Ok(self.parse_order(&response))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".to_string(), "OPEN".to_string());
        if let Some(s) = symbol {
            params.insert("symbol".to_string(), self.safe_market_id(s));
        }

        let response: DefxResponse<Vec<DefxOrder>> = self
            .private_request("GET", "api/orders", Some(params))
            .await?;
        let orders = response.data.unwrap_or_default();
        Ok(orders.iter().map(|o| self.parse_order(o)).collect())
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("symbol".to_string(), self.safe_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: DefxResponse<Vec<DefxMyTrade>> = self
            .private_request("GET", "api/trades", Some(params))
            .await?;
        let trades_data = response.data.unwrap_or_default();

        Ok(trades_data
            .iter()
            .map(|t| {
                let sym = t
                    .symbol
                    .as_ref()
                    .map(|s| self.safe_symbol(s))
                    .unwrap_or_default();
                let price = t
                    .price
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                let amount = t
                    .qty
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);

                Trade {
                    id: t.trade_id.clone().unwrap_or_default(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                    timestamp: t.time,
                    datetime: t.time.map(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
                    symbol: sym,
                    order: t.order_id.clone(),
                    trade_type: None,
                    side: t.side.as_ref().map(|s| s.to_lowercase()),
                    taker_or_maker: t.is_maker.map(|is_maker| {
                        if is_maker {
                            crate::types::TakerOrMaker::Maker
                        } else {
                            crate::types::TakerOrMaker::Taker
                        }
                    }),
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: t.commission.as_ref().and_then(|c| {
                        Decimal::from_str(c).ok().map(|cost| crate::types::Fee {
                            currency: t.commission_asset.clone(),
                            cost: Some(cost),
                            rate: None,
                        })
                    }),
                    fees: vec![],
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_exchange() -> Defx {
        let config = ExchangeConfig::default()
            .with_api_key("test_key")
            .with_api_secret("test_secret");
        Defx::new(config).unwrap()
    }

    #[test]
    fn test_defx_creation() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.id(), ExchangeId::Defx);
        assert_eq!(exchange.name(), "Defx");
    }

    #[test]
    fn test_has() {
        let exchange = create_test_exchange();
        let features = exchange.has();
        assert!(features.swap);
        assert!(features.fetch_ticker);
        assert!(features.fetch_order_book);
        assert!(features.fetch_trades);
        assert!(features.fetch_ohlcv);
        assert!(features.fetch_balance);
        assert!(features.create_order);
        assert!(features.cancel_order);
    }

    #[test]
    fn test_timeframes() {
        let exchange = create_test_exchange();
        let timeframes = exchange.timeframes();
        assert!(timeframes.contains_key(&Timeframe::Minute1));
        assert!(timeframes.contains_key(&Timeframe::Hour1));
        assert!(timeframes.contains_key(&Timeframe::Day1));
        assert_eq!(timeframes.get(&Timeframe::Minute1), Some(&"1m".to_string()));
    }

    #[test]
    fn test_safe_symbol() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.safe_symbol("BTC_USDC"), "BTC/USDC");
        assert_eq!(exchange.safe_symbol("ETH_USDC"), "ETH/USDC");
    }

    #[test]
    fn test_safe_market_id() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.safe_market_id("BTC/USDC"), "BTC_USDC");
        assert_eq!(exchange.safe_market_id("ETH/USDC"), "ETH_USDC");
    }

    #[test]
    fn test_urls() {
        let exchange = create_test_exchange();
        let urls = exchange.urls();
        assert!(urls.api.contains_key("public"));
        assert!(urls.api.contains_key("private"));
        assert_eq!(urls.www, Some("https://defx.com".to_string()));
    }

    #[test]
    fn test_signature_generation() {
        let exchange = create_test_exchange();
        let signature = exchange.generate_signature("test_payload");
        assert!(!signature.is_empty());
        // HMAC-SHA256 hex output is 64 characters
        assert_eq!(signature.len(), 64);
    }
}
