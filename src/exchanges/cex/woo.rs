//! WOO X Exchange Implementation
//!
//! WOO X 거래소 API 구현

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Fee, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade, OHLCV,
};

const BASE_URL: &str = "https://api.woox.io";
const RATE_LIMIT_MS: u64 = 100;

/// WOO X 거래소 구조체
pub struct Woo {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Woo {
    /// 새 WOO X 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), BASE_URL.into());
        api_urls.insert("private".into(), BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/150730761-1a00e5e0-d28c-480f-9e65-089ce3e6ef3b.jpg".into()),
            api: api_urls,
            www: Some("https://woox.io/".into()),
            doc: vec!["https://docs.woox.io/".into()],
            fees: Some("https://support.woox.io/hc/en-001/articles/4404611795353--Trading-Fees".into()),
        };

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: true,
            swap: true,
            future: false,
            option: false,
            fetch_markets: true,
            fetch_currencies: true,
            fetch_ticker: false,
            fetch_tickers: false,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: false,
            cancel_order: true,
            cancel_all_orders: true,
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
            watch_ticker: false,
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
        timeframes.insert(Timeframe::Minute1, "1m".to_string());
        timeframes.insert(Timeframe::Minute5, "5m".to_string());
        timeframes.insert(Timeframe::Minute15, "15m".to_string());
        timeframes.insert(Timeframe::Minute30, "30m".to_string());
        timeframes.insert(Timeframe::Hour1, "1h".to_string());
        timeframes.insert(Timeframe::Hour4, "4h".to_string());
        timeframes.insert(Timeframe::Hour12, "12h".to_string());
        timeframes.insert(Timeframe::Day1, "1d".to_string());
        timeframes.insert(Timeframe::Week1, "1w".to_string());
        timeframes.insert(Timeframe::Month1, "1mon".to_string());

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

    /// WOO 심볼을 통합 심볼로 변환
    fn to_symbol(&self, woo_symbol: &str) -> String {
        // WOO uses format like "SPOT_BTC_USDT" or "PERP_BTC_USDT"
        let parts: Vec<&str> = woo_symbol.split('_').collect();
        if parts.len() >= 3 {
            let market_type = parts[0];
            let base = parts[1];
            let quote = parts[2];

            if market_type == "PERP" {
                // Perpetual swap format: BTC/USDT:USDT
                format!("{base}/{quote}:{quote}")
            } else {
                // Spot format: BTC/USDT
                format!("{base}/{quote}")
            }
        } else {
            woo_symbol.to_string()
        }
    }

    /// 통합 심볼을 WOO 형식으로 변환
    fn to_market_id(&self, symbol: &str) -> String {
        // Convert BTC/USDT to SPOT_BTC_USDT
        // Convert BTC/USDT:USDT to PERP_BTC_USDT
        if symbol.contains(':') {
            // Swap market
            let parts: Vec<&str> = symbol.split(':').collect();
            if let Some(base_quote) = parts.first() {
                let pair: Vec<&str> = base_quote.split('/').collect();
                if pair.len() == 2 {
                    return format!("PERP_{}_{}", pair[0], pair[1]);
                }
            }
        } else {
            // Spot market
            let pair: Vec<&str> = symbol.split('/').collect();
            if pair.len() == 2 {
                return format!("SPOT_{}_{}", pair[0], pair[1]);
            }
        }
        symbol.to_string()
    }

    /// 마켓 데이터 파싱
    fn parse_market(&self, data: &WooMarket) -> Market {
        let id = data.symbol.clone();
        let parts: Vec<&str> = id.split('_').collect();

        let (market_type, spot, swap, base, quote, settle) = if parts.len() >= 3 {
            let type_str = parts[0];
            let base = parts[1].to_string();
            let quote = parts[2].to_string();

            if type_str == "PERP" {
                (
                    MarketType::Swap,
                    false,
                    true,
                    base.clone(),
                    quote.clone(),
                    Some(quote.clone()),
                )
            } else {
                (MarketType::Spot, true, false, base, quote, None)
            }
        } else {
            (
                MarketType::Spot,
                true,
                false,
                String::new(),
                String::new(),
                None,
            )
        };

        let symbol = if swap {
            format!("{}/{}:{}", base, quote, settle.as_ref().unwrap_or(&quote))
        } else {
            format!("{base}/{quote}")
        };

        let active = data.status.as_deref() == Some("TRADING");

        // Parse precision from tick sizes
        let price_precision =
            self.precision_from_string(data.quote_tick.as_deref().unwrap_or("0.01"));
        let amount_precision =
            self.precision_from_string(data.base_tick.as_deref().unwrap_or("0.0001"));

        Market {
            id,
            lowercase_id: Some(data.symbol.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: base.clone(),
            quote_id: quote.clone(),
            active,
            market_type,
            spot,
            margin: spot,
            swap,
            future: false,
            option: false,
            contract: swap,
            linear: if swap { Some(true) } else { None },
            inverse: if swap { Some(false) } else { None },
            settle: settle.clone(),
            settle_id: settle,
            contract_size: if swap { Some(Decimal::ONE) } else { None },
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            underlying: None,
            underlying_id: None,
            taker: Some(Decimal::from_str("0.0005").unwrap_or_default()),
            maker: Some(Decimal::from_str("0.0002").unwrap_or_default()),
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
                        .base_min
                        .as_ref()
                        .and_then(|v| Decimal::from_str(v).ok()),
                    max: data
                        .base_max
                        .as_ref()
                        .and_then(|v| Decimal::from_str(v).ok()),
                },
                price: MinMax {
                    min: data
                        .quote_min
                        .as_ref()
                        .and_then(|v| Decimal::from_str(v).ok()),
                    max: data
                        .quote_max
                        .as_ref()
                        .and_then(|v| Decimal::from_str(v).ok()),
                },
                cost: MinMax {
                    min: data
                        .min_notional
                        .as_ref()
                        .and_then(|v| Decimal::from_str(v).ok()),
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

    /// Calculate precision from tick size string
    fn precision_from_string(&self, tick_size: &str) -> i32 {
        if let Ok(decimal) = Decimal::from_str(tick_size) {
            let str_val = decimal.to_string();
            if let Some(dot_pos) = str_val.find('.') {
                let decimals = str_val[dot_pos + 1..].trim_end_matches('0');
                return decimals.len() as i32;
            }
        }
        8
    }

    /// 티커 데이터 파싱 (WOO doesn't support ticker endpoint)
    fn parse_ticker(&self, _symbol: &str, _data: &serde_json::Value) -> Ticker {
        let timestamp = Utc::now().timestamp_millis();
        Ticker {
            symbol: _symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: None,
            low: None,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: None,
            close: None,
            last: None,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: None,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        }
    }

    /// OHLCV 데이터 파싱
    fn parse_ohlcv(&self, data: &WooOHLCV) -> OHLCV {
        OHLCV {
            timestamp: data.start_timestamp,
            open: Decimal::from_str(&data.open).unwrap_or_default(),
            high: Decimal::from_str(&data.high).unwrap_or_default(),
            low: Decimal::from_str(&data.low).unwrap_or_default(),
            close: Decimal::from_str(&data.close).unwrap_or_default(),
            volume: Decimal::from_str(&data.volume).unwrap_or_default(),
        }
    }

    /// 체결 데이터 파싱
    fn parse_trade(&self, symbol: &str, data: &WooTrade) -> Trade {
        let price = Decimal::from_str(&data.executed_price).unwrap_or_default();
        let amount = Decimal::from_str(&data.executed_quantity).unwrap_or_default();
        let cost = price * amount;

        let side = data.side.as_ref().map(|s| s.to_lowercase());

        // Handle different timestamp formats
        let timestamp = if data.executed_timestamp.contains('.') {
            // Format: "1752055173.630"
            data.executed_timestamp
                .split('.')
                .next()
                .and_then(|s| s.parse::<i64>().ok())
                .map(|s| s * 1000)
        } else {
            // Already in milliseconds
            data.executed_timestamp.parse::<i64>().ok()
        };

        let datetime = timestamp
            .and_then(|ts| chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339()));

        Trade {
            id: data.id.as_ref().unwrap_or(&data.executed_timestamp).clone(),
            order: data.order_id.clone(),
            timestamp,
            datetime,
            symbol: symbol.to_string(),
            trade_type: None,
            side,
            taker_or_maker: data.is_maker.as_ref().map(|m| {
                if m == "1" {
                    TakerOrMaker::Maker
                } else {
                    TakerOrMaker::Taker
                }
            }),
            price,
            amount,
            cost: Some(cost),
            fee: data.fee.as_ref().and_then(|f| {
                Decimal::from_str(f).ok().map(|fee_cost| Fee {
                    cost: Some(fee_cost),
                    currency: data.fee_asset.clone(),
                    rate: None,
                })
            }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 데이터 파싱
    fn parse_order(&self, data: &WooOrder) -> CcxtResult<Order> {
        let status =
            self.parse_order_status(data.status.as_deref().or(data.algo_status.as_deref()));

        let symbol = data
            .symbol
            .as_ref()
            .map(|s| self.to_symbol(s))
            .unwrap_or_default();

        let side = data
            .side
            .as_ref()
            .map(|s| {
                if s.to_uppercase() == "BUY" {
                    OrderSide::Buy
                } else {
                    OrderSide::Sell
                }
            })
            .unwrap_or(OrderSide::Buy);

        let order_type = data
            .order_type
            .as_ref()
            .map(|t| match t.to_uppercase().as_str() {
                "MARKET" => OrderType::Market,
                "LIMIT" => OrderType::Limit,
                "IOC" => OrderType::Limit,
                "FOK" => OrderType::Limit,
                "POST_ONLY" => OrderType::Limit,
                _ => OrderType::Limit,
            })
            .unwrap_or(OrderType::Limit);

        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let amount = data
            .quantity
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let filled = data
            .executed
            .as_ref()
            .or(data.total_executed_quantity.as_ref())
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let remaining = if amount > filled {
            amount - filled
        } else {
            Decimal::ZERO
        };

        let average = data
            .average_executed_price
            .as_ref()
            .and_then(|p| Decimal::from_str(p).ok());

        let cost = data.amount.as_ref().and_then(|c| Decimal::from_str(c).ok());

        // Parse timestamp
        let timestamp = data
            .created_time
            .as_ref()
            .and_then(|ct| {
                if ct.contains('.') {
                    // Format: "1752049062.496"
                    ct.split('.')
                        .next()
                        .and_then(|s| s.parse::<i64>().ok())
                        .map(|s| s * 1000)
                } else {
                    ct.parse::<i64>().ok()
                }
            })
            .or(data.timestamp);

        let fee_cost = data
            .total_fee
            .as_ref()
            .and_then(|f| Decimal::from_str(f).ok());

        Ok(Order {
            id: data
                .order_id
                .clone()
                .or(data.algo_order_id.clone())
                .unwrap_or_default(),
            client_order_id: data
                .client_order_id
                .clone()
                .or(data.client_algo_order_id.clone()),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: None,
            last_update_timestamp: data.updated_time.as_ref().and_then(|ut| {
                if ut.contains('.') {
                    ut.split('.')
                        .next()
                        .and_then(|s| s.parse::<i64>().ok())
                        .map(|s| s * 1000)
                } else {
                    ut.parse::<i64>().ok()
                }
            }),
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
                currency: data.fee_asset.clone(),
                rate: None,
            }),
            fees: Vec::new(),
            trades: Vec::new(),
            reduce_only: data.reduce_only,
            post_only: None,
            time_in_force: None,
            stop_price: data
                .trigger_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            trigger_price: data
                .trigger_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// Parse order status
    fn parse_order_status(&self, status: Option<&str>) -> OrderStatus {
        match status {
            Some("NEW") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("PARTIAL_FILLED") => OrderStatus::Open,
            Some("CANCELLED") => OrderStatus::Canceled,
            Some("CANCEL_SENT") => OrderStatus::Canceled,
            Some("REJECTED") => OrderStatus::Rejected,
            Some("INCOMPLETE") => OrderStatus::Open,
            Some("COMPLETED") => OrderStatus::Closed,
            _ => OrderStatus::Open,
        }
    }

    /// 잔고 데이터 파싱
    fn parse_balance(&self, data: &WooBalanceData) -> Balances {
        let mut currencies = HashMap::new();

        for balance in &data.holding {
            let currency = balance.token.clone();

            let total = Decimal::from_str(&balance.holding).unwrap_or_default();
            let frozen = Decimal::from_str(&balance.frozen).unwrap_or_default();
            let free = total - frozen;

            currencies.insert(
                currency,
                Balance {
                    free: Some(free),
                    used: Some(frozen),
                    total: Some(total),
                    debt: None,
                },
            );
        }

        Balances {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// API 서명 생성
    fn sign_request(
        &self,
        method: &str,
        path: &str,
        params: &str,
        timestamp: &str,
    ) -> CcxtResult<String> {
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?;

        // Build auth string: timestamp + method + path + params(if POST/PUT)
        let auth = format!("{timestamp}{method}{path}{params}");

        // HMAC-SHA256
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|e| {
            CcxtError::AuthenticationError {
                message: format!("HMAC error: {e}"),
            }
        })?;
        mac.update(auth.as_bytes());
        let signature = mac.finalize().into_bytes();

        Ok(hex::encode(signature))
    }

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned + Default>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<WooResponse<T>> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Private API 요청
    async fn private_request<T: for<'de> Deserialize<'de> + Default>(
        &self,
        method: &str,
        endpoint: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<WooResponse<T>> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let path = format!("/v3/{endpoint}");

        let (body_str, query_str) = if method == "POST" || method == "PUT" {
            let body_json = serde_json::to_string(&params).unwrap_or_else(|_| "{}".to_string());
            (body_json, String::new())
        } else if !params.is_empty() {
            let query = params
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            (String::new(), format!("?{query}"))
        } else {
            (String::new(), String::new())
        };

        let auth_path = format!("{path}{query_str}");
        let signature = self.sign_request(method, &auth_path, &body_str, &timestamp)?;

        let mut headers = HashMap::new();
        headers.insert("x-api-key".to_string(), api_key.to_string());
        headers.insert("x-api-timestamp".to_string(), timestamp);
        headers.insert("x-api-signature".to_string(), signature);

        if method == "POST" || method == "PUT" {
            headers.insert("Content-Type".to_string(), "application/json".to_string());
        }

        let full_path = format!("{path}{query_str}");

        let response: WooResponse<T> = if method == "POST" {
            let json_body: serde_json::Value = serde_json::from_str(&body_str).unwrap_or_default();
            self.client
                .post(&full_path, Some(json_body), Some(headers))
                .await?
        } else if method == "DELETE" {
            self.client.delete(&full_path, None, Some(headers)).await?
        } else {
            self.client.get(&full_path, None, Some(headers)).await?
        };

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: "WOO API error".to_string(),
            });
        }

        Ok(response)
    }
}

#[async_trait]
impl Exchange for Woo {
    fn id(&self) -> ExchangeId {
        ExchangeId::Woo
    }

    fn name(&self) -> &str {
        "WOO X"
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

        let response: WooResponse<WooInstrumentsData> =
            self.public_get("/v3/public/instruments", None).await?;

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No instruments data returned".into(),
        })?;

        let mut markets_map = HashMap::new();
        let mut markets_by_id_map = HashMap::new();

        for market_data in data.rows {
            let market = self.parse_market(&market_data);
            markets_by_id_map.insert(market.id.clone(), market.symbol.clone());
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

    async fn fetch_ticker(&self, _symbol: &str) -> CcxtResult<Ticker> {
        // WOO doesn't support ticker endpoint
        Err(CcxtError::NotSupported {
            feature: "fetchTicker".into(),
        })
    }

    async fn fetch_tickers(
        &self,
        _symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        // WOO doesn't support tickers endpoint
        Err(CcxtError::NotSupported {
            feature: "fetchTickers".into(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let woo_symbol = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), woo_symbol);
        if let Some(l) = limit {
            params.insert("maxLevel".to_string(), l.to_string());
        }

        let response: WooResponse<WooOrderBookData> = self
            .public_get("/v3/public/orderbook", Some(params))
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No order book data returned".into(),
        })?;

        let timestamp = response
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: Decimal::from_str(&b.price).unwrap_or_default(),
                amount: Decimal::from_str(&b.quantity).unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: Decimal::from_str(&a.price).unwrap_or_default(),
                amount: Decimal::from_str(&a.quantity).unwrap_or_default(),
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
        let woo_symbol = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), woo_symbol);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: WooResponse<WooTradesData> = self
            .public_get("/v3/public/marketTrades", Some(params))
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No trades data returned".into(),
        })?;

        let trades: Vec<Trade> = data
            .rows
            .iter()
            .map(|t| self.parse_trade(symbol, t))
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
        let woo_symbol = self.to_market_id(symbol);
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), woo_symbol);
        params.insert("type".to_string(), interval.clone());
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string().min("1000".to_string()));
        }
        if let Some(s) = since {
            params.insert("after".to_string(), s.to_string());
        }

        let response: WooResponse<WooOHLCVData> = self
            .public_get("/v3/public/klineHistory", Some(params))
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No OHLCV data returned".into(),
        })?;

        let ohlcv_list: Vec<OHLCV> = data.rows.iter().map(|c| self.parse_ohlcv(c)).collect();

        Ok(ohlcv_list)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: WooResponse<WooBalanceData> = self
            .private_request("GET", "asset/balances", HashMap::new())
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No balance data returned".into(),
        })?;

        Ok(self.parse_balance(&data))
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let woo_symbol = self.to_market_id(symbol);

        let type_str = match order_type {
            OrderType::Market => "MARKET",
            OrderType::Limit => "LIMIT",
            _ => {
                return Err(CcxtError::BadRequest {
                    message: format!("Unsupported order type: {order_type:?}"),
                })
            },
        };

        let side_str = match side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };

        let mut request_params = HashMap::new();
        request_params.insert("symbol".to_string(), woo_symbol);
        request_params.insert("side".to_string(), side_str.to_string());
        request_params.insert("type".to_string(), type_str.to_string());
        request_params.insert("quantity".to_string(), amount.to_string());

        if let Some(p) = price {
            request_params.insert("price".to_string(), p.to_string());
        }

        let response: WooResponse<WooOrderResponse> = self
            .private_request("POST", "trade/order", request_params)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No order response".into(),
        })?;

        let order_id = data.order_id.clone().unwrap_or_default();

        Ok(Order {
            id: order_id,
            client_order_id: data.client_order_id.clone(),
            timestamp: response.timestamp,
            datetime: response.timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
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
            info: serde_json::to_value(&data).unwrap_or_default(),
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let woo_symbol = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), woo_symbol);
        params.insert("orderId".to_string(), id.to_string());

        let response: WooResponse<WooCancelOrderResponse> = self
            .private_request("DELETE", "trade/order", params)
            .await?;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: response.timestamp,
            datetime: response.timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            symbol: symbol.to_string(),
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
        params.insert("orderId".to_string(), id.to_string());

        let response: WooResponse<WooOrder> =
            self.private_request("GET", "trade/order", params).await?;

        let data = response.data.ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })?;

        self.parse_order(&data)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".to_string(), "INCOMPLETE".to_string());

        if let Some(sym) = symbol {
            let woo_symbol = self.to_market_id(sym);
            params.insert("symbol".to_string(), woo_symbol);
        }
        if let Some(l) = limit {
            params.insert("size".to_string(), l.to_string().min("500".to_string()));
        }

        let response: WooResponse<WooOrdersData> =
            self.private_request("GET", "trade/orders", params).await?;

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No orders data returned".into(),
        })?;

        let orders: Vec<Order> = data
            .rows
            .iter()
            .filter_map(|o| self.parse_order(o).ok())
            .collect();

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".to_string(), "COMPLETED".to_string());

        if let Some(sym) = symbol {
            let woo_symbol = self.to_market_id(sym);
            params.insert("symbol".to_string(), woo_symbol);
        }
        if let Some(l) = limit {
            params.insert("size".to_string(), l.to_string().min("500".to_string()));
        }

        let response: WooResponse<WooOrdersData> =
            self.private_request("GET", "trade/orders", params).await?;

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No orders data returned".into(),
        })?;

        let orders: Vec<Order> = data
            .rows
            .iter()
            .filter_map(|o| self.parse_order(o).ok())
            .collect();

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

        if self.config.api_key().is_some() && self.config.secret().is_some() {
            let timestamp = Utc::now().timestamp_millis().to_string();

            let (body_str, query_str) = if method == "POST" || method == "PUT" {
                (body.unwrap_or("{}").to_string(), String::new())
            } else if !params.is_empty() {
                let query = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");
                (String::new(), format!("?{query}"))
            } else {
                (String::new(), String::new())
            };

            let auth_path = format!("{path}{query_str}");

            if let Ok(signature) = self.sign_request(method, &auth_path, &body_str, &timestamp) {
                if let Some(api_key) = self.config.api_key() {
                    result_headers.insert("x-api-key".to_string(), api_key.to_string());
                    result_headers.insert("x-api-timestamp".to_string(), timestamp);
                    result_headers.insert("x-api-signature".to_string(), signature);
                    if method == "POST" || method == "PUT" {
                        result_headers
                            .insert("Content-Type".to_string(), "application/json".to_string());
                    }
                }
            }
        }

        let url = if params.is_empty() || method == "POST" || method == "PUT" {
            format!("{BASE_URL}{path}")
        } else {
            let query = params
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
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

// === WOO Response Types ===

#[derive(Debug, Default, Deserialize)]
struct WooResponse<T> {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    data: Option<T>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WooInstrumentsData {
    #[serde(default)]
    rows: Vec<WooMarket>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WooMarket {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "baseAsset")]
    base_asset: Option<String>,
    #[serde(default, rename = "quoteAsset")]
    quote_asset: Option<String>,
    #[serde(default, rename = "baseMin")]
    base_min: Option<String>,
    #[serde(default, rename = "baseMax")]
    base_max: Option<String>,
    #[serde(default, rename = "baseTick")]
    base_tick: Option<String>,
    #[serde(default, rename = "quoteMin")]
    quote_min: Option<String>,
    #[serde(default, rename = "quoteMax")]
    quote_max: Option<String>,
    #[serde(default, rename = "quoteTick")]
    quote_tick: Option<String>,
    #[serde(default, rename = "minNotional")]
    min_notional: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WooOrderBookData {
    #[serde(default)]
    bids: Vec<WooOrderBookEntry>,
    #[serde(default)]
    asks: Vec<WooOrderBookEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct WooOrderBookEntry {
    #[serde(default)]
    price: String,
    #[serde(default)]
    quantity: String,
}

#[derive(Debug, Default, Deserialize)]
struct WooTradesData {
    #[serde(default)]
    rows: Vec<WooTrade>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WooTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "executedPrice")]
    executed_price: String,
    #[serde(default, rename = "executedQuantity")]
    executed_quantity: String,
    #[serde(default, rename = "executedTimestamp")]
    executed_timestamp: String,
    #[serde(default, rename = "orderId")]
    order_id: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default, rename = "feeAsset")]
    fee_asset: Option<String>,
    #[serde(default, rename = "isMaker")]
    is_maker: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WooOHLCVData {
    #[serde(default)]
    rows: Vec<WooOHLCV>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WooOHLCV {
    #[serde(default, rename = "startTimestamp")]
    start_timestamp: i64,
    #[serde(default)]
    open: String,
    #[serde(default)]
    high: String,
    #[serde(default)]
    low: String,
    #[serde(default)]
    close: String,
    #[serde(default)]
    volume: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WooOrder {
    #[serde(default, rename = "orderId")]
    order_id: Option<String>,
    #[serde(default, rename = "algoOrderId")]
    algo_order_id: Option<String>,
    #[serde(default, rename = "clientOrderId")]
    client_order_id: Option<String>,
    #[serde(default, rename = "clientAlgoOrderId")]
    client_algo_order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "algoStatus")]
    algo_status: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    executed: Option<String>,
    #[serde(default, rename = "totalExecutedQuantity")]
    total_executed_quantity: Option<String>,
    #[serde(default, rename = "averageExecutedPrice")]
    average_executed_price: Option<String>,
    #[serde(default, rename = "totalFee")]
    total_fee: Option<String>,
    #[serde(default, rename = "feeAsset")]
    fee_asset: Option<String>,
    #[serde(default, rename = "reduceOnly")]
    reduce_only: Option<bool>,
    #[serde(default, rename = "createdTime")]
    created_time: Option<String>,
    #[serde(default, rename = "updatedTime")]
    updated_time: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, rename = "triggerPrice")]
    trigger_price: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WooOrdersData {
    #[serde(default)]
    rows: Vec<WooOrder>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WooOrderResponse {
    #[serde(default, rename = "orderId")]
    order_id: Option<String>,
    #[serde(default, rename = "clientOrderId")]
    client_order_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WooCancelOrderResponse {
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WooBalanceData {
    #[serde(default)]
    holding: Vec<WooBalance>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WooBalance {
    #[serde(default)]
    token: String,
    #[serde(default)]
    holding: String,
    #[serde(default)]
    frozen: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_woo() -> Woo {
        Woo::new(ExchangeConfig::default()).unwrap()
    }

    #[test]
    fn test_to_symbol() {
        let woo = create_test_woo();
        assert_eq!(woo.to_symbol("SPOT_BTC_USDT"), "BTC/USDT");
        assert_eq!(woo.to_symbol("SPOT_ETH_USDT"), "ETH/USDT");
        assert_eq!(woo.to_symbol("PERP_BTC_USDT"), "BTC/USDT:USDT");
    }

    #[test]
    fn test_to_market_id() {
        let woo = create_test_woo();
        assert_eq!(woo.to_market_id("BTC/USDT"), "SPOT_BTC_USDT");
        assert_eq!(woo.to_market_id("ETH/USDT"), "SPOT_ETH_USDT");
        assert_eq!(woo.to_market_id("BTC/USDT:USDT"), "PERP_BTC_USDT");
    }

    #[test]
    fn test_exchange_id() {
        let woo = create_test_woo();
        assert_eq!(woo.id(), ExchangeId::Woo);
        assert_eq!(woo.name(), "WOO X");
    }

    #[test]
    fn test_has_features() {
        let woo = create_test_woo();
        let features = woo.has();
        assert!(features.fetch_markets);
        assert!(features.fetch_order_book);
        assert!(features.fetch_balance);
        assert!(features.create_order);
        assert!(!features.fetch_ticker);
    }

    #[test]
    fn test_parse_order_status() {
        let woo = create_test_woo();
        assert_eq!(woo.parse_order_status(Some("NEW")), OrderStatus::Open);
        assert_eq!(woo.parse_order_status(Some("FILLED")), OrderStatus::Closed);
        assert_eq!(
            woo.parse_order_status(Some("CANCELLED")),
            OrderStatus::Canceled
        );
        assert_eq!(
            woo.parse_order_status(Some("REJECTED")),
            OrderStatus::Rejected
        );
    }
}
