//! AscendEX Exchange Implementation
//!
//! AscendEX (formerly BitMax) exchange API implementation

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
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, Fee, OHLCV,
};

const BASE_URL: &str = "https://ascendex.com";
const RATE_LIMIT_MS: u64 = 400;

/// AscendEX exchange structure
pub struct Ascendex {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    account_group: RwLock<Option<String>>,
}

impl Ascendex {
    /// Create new AscendEX instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), BASE_URL.into());
        api_urls.insert("private".into(), BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/user-attachments/assets/55bab6b9-d4ca-42a8-a0e6-fac81ae557f1".into()),
            api: api_urls,
            www: Some("https://ascendex.com".into()),
            doc: vec!["https://ascendex.github.io/ascendex-pro-api/#ascendex-pro-api-documentation".into()],
            fees: Some("https://ascendex.com/en/feerate/transactionfee-traderate".into()),
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
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: false,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: false,
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
        timeframes.insert(Timeframe::Hour2, "120".to_string());
        timeframes.insert(Timeframe::Hour4, "240".to_string());
        timeframes.insert(Timeframe::Hour6, "360".to_string());
        timeframes.insert(Timeframe::Hour12, "720".to_string());
        timeframes.insert(Timeframe::Day1, "1d".to_string());
        timeframes.insert(Timeframe::Week1, "1w".to_string());
        timeframes.insert(Timeframe::Month1, "1m".to_string());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            account_group: RwLock::new(None),
        })
    }

    /// Get account group ID
    async fn get_account_group(&self) -> CcxtResult<String> {
        {
            let ag = self.account_group.read().map_err(|_| CcxtError::ExchangeError {
                message: "Failed to acquire read lock".into(),
            })?;
            if let Some(group) = ag.as_ref() {
                return Ok(group.clone());
            }
        }

        // Fetch account info
        let response: AscendexResponse<AscendexAccountInfo> = self
            .private_get("/api/pro/v1/info", HashMap::new())
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No account info returned".into(),
        })?;

        let group_id = data.account_group.to_string();

        {
            let mut ag = self.account_group.write().map_err(|_| CcxtError::ExchangeError {
                message: "Failed to acquire write lock".into(),
            })?;
            *ag = Some(group_id.clone());
        }

        Ok(group_id)
    }

    /// Parse market data
    fn parse_market(&self, data: &AscendexMarket, market_type: MarketType) -> Market {
        let (id, symbol, base, quote, base_id, quote_id, settle, settle_id) = if market_type == MarketType::Spot {
            let id = data.symbol.clone().unwrap_or_default();
            let parts: Vec<&str> = id.split('/').collect();
            let base_id = parts.get(0).unwrap_or(&"").to_string();
            let quote_id = parts.get(1).unwrap_or(&"").to_string();
            let base = base_id.clone();
            let quote = quote_id.clone();
            let symbol = format!("{}/{}", base, quote);
            (id, symbol, base, quote, base_id, quote_id, None, None)
        } else {
            let id = data.symbol.clone().unwrap_or_default();
            let underlying = data.underlying.as_ref().unwrap_or(&id);
            let parts: Vec<&str> = underlying.split('/').collect();
            let base_id = parts.get(0).unwrap_or(&"").to_string();
            let quote_id = parts.get(1).unwrap_or(&"").to_string();
            let settle_id = data.settlement_asset.clone().unwrap_or_default();
            let base = base_id.clone();
            let quote = quote_id.clone();
            let settle = settle_id.clone();
            let symbol = format!("{}{}:{}", base, quote, settle);
            (id, symbol, base, quote, base_id, quote_id, Some(settle), Some(settle_id))
        };

        let active = data.status.as_deref() == Some("Normal");

        let (price_precision, amount_precision, min_price, max_price, min_amount, max_amount, min_cost, max_cost) =
            if let Some(price_filter) = &data.price_filter {
                let tick_size = price_filter.tick_size.and_then(|v| Decimal::from_str(&v.to_string()).ok());
                let min_price = price_filter.min_price.and_then(|v| Decimal::from_str(&v.to_string()).ok());
                let max_price = price_filter.max_price.and_then(|v| Decimal::from_str(&v.to_string()).ok());

                let lot_filter = data.lot_size_filter.as_ref();
                let lot_size = lot_filter.and_then(|f| f.lot_size).and_then(|v| Decimal::from_str(&v.to_string()).ok());
                let min_qty = lot_filter.and_then(|f| f.min_qty).and_then(|v| Decimal::from_str(&v.to_string()).ok());
                let max_qty = lot_filter.and_then(|f| f.max_qty).and_then(|v| Decimal::from_str(&v.to_string()).ok());

                let min_notional = data.min_notional.and_then(|v| Decimal::from_str(&v.to_string()).ok());
                let max_notional = data.max_notional.and_then(|v| Decimal::from_str(&v.to_string()).ok());

                (tick_size, lot_size, min_price, max_price, min_qty, max_qty, min_notional, max_notional)
            } else {
                let tick_size = data.tick_size.and_then(|v| Decimal::from_str(&v.to_string()).ok());
                let lot_size = data.lot_size.and_then(|v| Decimal::from_str(&v.to_string()).ok());
                let min_qty = data.min_qty.and_then(|v| Decimal::from_str(&v.to_string()).ok());
                let max_qty = data.max_qty.and_then(|v| Decimal::from_str(&v.to_string()).ok());
                let min_notional = data.min_notional.and_then(|v| Decimal::from_str(&v.to_string()).ok());
                let max_notional = data.max_notional.and_then(|v| Decimal::from_str(&v.to_string()).ok());

                (tick_size, lot_size, None, None, min_qty, max_qty, min_notional, max_notional)
            };

        let fee = data.commission_reserve_rate
            .and_then(|v| Decimal::from_str(&v.to_string()).ok());

        Market {
            id,
            lowercase_id: None,
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id,
            quote_id,
            active,
            market_type,
            spot: market_type == MarketType::Spot,
            margin: data.margin_tradable.unwrap_or(false),
            swap: market_type == MarketType::Swap,
            future: false,
            option: false,
            contract: market_type == MarketType::Swap,
            linear: settle.as_ref().map(|s| s == &quote),
            inverse: settle.as_ref().map(|s| s == &base),
            settle,
            settle_id,
            contract_size: if market_type == MarketType::Swap {
                Some(Decimal::from(1))
            } else {
                None
            },
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            taker: fee,
            maker: fee,
            precision: MarketPrecision {
                amount: amount_precision.and_then(|p| {
                    // Convert decimal to precision (number of decimal places)
                    let s = p.to_string();
                    if let Some(dot_pos) = s.find('.') {
                        Some((s.len() - dot_pos - 1) as i32)
                    } else {
                        Some(0)
                    }
                }),
                price: price_precision.and_then(|p| {
                    let s = p.to_string();
                    if let Some(dot_pos) = s.find('.') {
                        Some((s.len() - dot_pos - 1) as i32)
                    } else {
                        Some(0)
                    }
                }),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: MinMax {
                    min: min_amount,
                    max: max_amount,
                },
                price: MinMax {
                    min: min_price,
                    max: max_price,
                },
                cost: MinMax {
                    min: min_cost,
                    max: max_cost,
                },
                leverage: MinMax::default(),
            },
            index: false,
            sub_type: None,
            margin_modes: None,
            created: data.trading_start_time,
            tier_based: true,
            percentage: true,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse ticker data
    fn parse_ticker(&self, data: &AscendexTicker, symbol: &str) -> Ticker {
        let timestamp = Utc::now().timestamp_millis();

        let bid_data = data.bid.as_ref().and_then(|b| b.as_array());
        let ask_data = data.ask.as_ref().and_then(|a| a.as_array());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: data.high.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: data.low.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: bid_data.and_then(|b| b.get(0)).and_then(|v| v.as_str()).and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: bid_data.and_then(|b| b.get(1)).and_then(|v| v.as_str()).and_then(|v| Decimal::from_str(v).ok()),
            ask: ask_data.and_then(|a| a.get(0)).and_then(|v| v.as_str()).and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: ask_data.and_then(|a| a.get(1)).and_then(|v| v.as_str()).and_then(|v| Decimal::from_str(v).ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            close: data.close.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            last: data.close.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse OHLCV data
    fn parse_ohlcv(&self, data: &AscendexOHLCV) -> Option<OHLCV> {
        let ohlcv_data = data.data.as_ref()?;

        Some(OHLCV {
            timestamp: ohlcv_data.ts?,
            open: Decimal::from_str(ohlcv_data.o.as_ref()?).ok()?,
            high: Decimal::from_str(ohlcv_data.h.as_ref()?).ok()?,
            low: Decimal::from_str(ohlcv_data.l.as_ref()?).ok()?,
            close: Decimal::from_str(ohlcv_data.c.as_ref()?).ok()?,
            volume: Decimal::from_str(ohlcv_data.v.as_ref()?).ok()?,
        })
    }

    /// Parse trade data
    fn parse_trade(&self, symbol: &str, data: &AscendexTrade) -> Option<Trade> {
        let price = Decimal::from_str(data.p.as_ref()?).ok()?;
        let amount = Decimal::from_str(data.q.as_ref()?).ok()?;
        let timestamp = data.ts?;

        // bm = buyer is maker, if true then this is a sell, otherwise buy
        let side = if data.bm.unwrap_or(false) {
            Some("sell".to_string())
        } else {
            Some("buy".to_string())
        };

        Some(Trade {
            id: data.seqnum.map(|s| s.to_string()).unwrap_or_default(),
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
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// Parse order data
    fn parse_order(&self, data: &AscendexOrder) -> CcxtResult<Order> {
        let status = match data.status.as_deref() {
            Some("PendingNew") | Some("New") | Some("PartiallyFilled") => OrderStatus::Open,
            Some("Filled") => OrderStatus::Closed,
            Some("Canceled") => OrderStatus::Canceled,
            Some("Rejected") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("Market") => OrderType::Market,
            Some("Limit") => OrderType::Limit,
            Some("StopMarket") => OrderType::StopMarket,
            Some("StopLimit") => OrderType::StopLimit,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("Buy") => OrderSide::Buy,
            Some("Sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let symbol = data.symbol.clone().unwrap_or_default();
        let price = data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let amount = data.order_qty.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();
        let filled = data.cum_filled_qty.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();
        let remaining = amount - filled;
        let average = data.avg_filled_px.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let cost = if filled > Decimal::ZERO {
            average.map(|avg| avg * filled)
        } else {
            None
        };
        let fee_cost = data.cum_fee.as_ref().and_then(|f| Decimal::from_str(f).ok());

        Ok(Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: None,
            timestamp: data.time,
            datetime: data.time.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: data.last_exec_time,
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
                currency: data.fee_asset.clone(),
                rate: None,
            }),
            fees: Vec::new(),
            trades: Vec::new(),
            reduce_only: data.exec_inst.as_ref().map(|i| i.contains("ReduceOnly")),
            post_only: None,
            time_in_force: None,
            stop_price: data.stop_price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// Parse balance data
    fn parse_balance(&self, data: &[AscendexBalanceEntry]) -> Balances {
        let mut currencies = HashMap::new();

        for entry in data {
            let code = entry.asset.clone().unwrap_or_default();
            let total = entry.total_balance.as_ref()
                .and_then(|v| Decimal::from_str(v).ok())
                .unwrap_or_default();
            let free = entry.available_balance.as_ref()
                .and_then(|v| Decimal::from_str(v).ok())
                .unwrap_or_default();
            let used = total - free;

            currencies.insert(code, Balance {
                free: Some(free),
                used: Some(used),
                total: Some(total),
                debt: None,
            });
        }

        Balances {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Sign request with HMAC SHA256
    fn sign_request(&self, timestamp: &str, path: &str) -> CcxtResult<String> {
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;

        let message = format!("{}+{}", timestamp, path);

        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("HMAC error: {}", e),
            })?;
        mac.update(message.as_bytes());
        let signature = mac.finalize().into_bytes();

        Ok(BASE64.encode(signature))
    }

    /// Public GET request
    async fn public_get<T: serde::de::DeserializeOwned + Default>(
        &self,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<AscendexResponse<T>> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, Some(params), None).await
    }

    /// Private GET request
    async fn private_get<T: for<'de> Deserialize<'de> + Default>(
        &self,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<AscendexResponse<T>> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let signature = self.sign_request(&timestamp, path)?;

        let mut headers = HashMap::new();
        headers.insert("x-auth-key".to_string(), api_key.to_string());
        headers.insert("x-auth-timestamp".to_string(), timestamp);
        headers.insert("x-auth-signature".to_string(), signature);

        let response: AscendexResponse<T> = self.client.get(path, Some(params), Some(headers)).await?;

        if response.code != Some(0) {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_else(|| "Unknown error".to_string()),
            });
        }

        Ok(response)
    }

    /// Private POST request
    async fn private_post<T: for<'de> Deserialize<'de> + Default>(
        &self,
        path: &str,
        params: serde_json::Value,
    ) -> CcxtResult<AscendexResponse<T>> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let signature = self.sign_request(&timestamp, path)?;

        let mut headers = HashMap::new();
        headers.insert("x-auth-key".to_string(), api_key.to_string());
        headers.insert("x-auth-timestamp".to_string(), timestamp);
        headers.insert("x-auth-signature".to_string(), signature);
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let response: AscendexResponse<T> = self.client.post(path, Some(params), Some(headers)).await?;

        if response.code != Some(0) {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_else(|| "Unknown error".to_string()),
            });
        }

        Ok(response)
    }

    /// Private DELETE request
    async fn private_delete<T: for<'de> Deserialize<'de> + Default>(
        &self,
        path: &str,
        params: serde_json::Value,
    ) -> CcxtResult<AscendexResponse<T>> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let signature = self.sign_request(&timestamp, path)?;

        let mut headers = HashMap::new();
        headers.insert("x-auth-key".to_string(), api_key.to_string());
        headers.insert("x-auth-timestamp".to_string(), timestamp);
        headers.insert("x-auth-signature".to_string(), signature);
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let response: AscendexResponse<T> = self.client.delete_json(path, Some(serde_json::to_value(&params).unwrap_or_default()), Some(headers)).await?;

        if response.code != Some(0) {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_else(|| "Unknown error".to_string()),
            });
        }

        Ok(response)
    }
}

#[async_trait]
impl Exchange for Ascendex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Ascendex
    }

    fn name(&self) -> &str {
        "AscendEX"
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
        Some(symbol.replace("/", ""))
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let markets_by_id = self.markets_by_id.read().ok()?;
        markets_by_id.get(market_id).cloned()
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

        // Fetch spot markets
        let spot_response: AscendexResponse<Vec<AscendexMarket>> = self
            .public_get("/api/pro/v1/products", HashMap::new())
            .await?;
        let spot_data = spot_response.data.unwrap_or_default();

        // Fetch contract markets
        let contract_response: AscendexResponse<Vec<AscendexMarket>> = self
            .public_get("/api/pro/v2/futures/contract", HashMap::new())
            .await?;
        let contract_data = contract_response.data.unwrap_or_default();

        let mut markets_map = HashMap::new();
        let mut markets_by_id_map = HashMap::new();

        // Parse spot markets
        for market_data in spot_data {
            if market_data.symbol.as_ref().map(|s| s.contains("-PERP")).unwrap_or(false) {
                continue; // Skip perpetuals in spot list
            }
            let market = self.parse_market(&market_data, MarketType::Spot);
            markets_by_id_map.insert(market.id.clone(), market.symbol.clone());
            markets_map.insert(market.symbol.clone(), market);
        }

        // Parse contract markets
        for market_data in contract_data {
            let market = self.parse_market(&market_data, MarketType::Swap);
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
            let mut m = self.markets_by_id.write().map_err(|_| CcxtError::ExchangeError {
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
        self.load_markets(false).await?;
        let market_id = {
            let markets = self.markets.read().map_err(|_| CcxtError::ExchangeError {
                message: "Failed to acquire read lock".into(),
            })?;
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        let response: AscendexResponse<AscendexTicker> = self
            .public_get("/api/pro/v1/ticker", params)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No ticker data returned".into(),
        })?;

        Ok(self.parse_ticker(&data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        if let Some(syms) = symbols {
            let market_ids = {
                let markets = self.markets.read().map_err(|_| CcxtError::ExchangeError {
                    message: "Failed to acquire read lock".into(),
                })?;
                syms.iter()
                    .filter_map(|s| markets.get(*s).map(|m| m.id.clone()))
                    .collect::<Vec<String>>()
            };
            params.insert("symbol".to_string(), market_ids.join(","));
        }

        let response: AscendexResponse<Vec<AscendexTicker>> = self
            .public_get("/api/pro/v1/ticker", params)
            .await?;

        let data = response.data.unwrap_or_default();
        let mut tickers = HashMap::new();

        for ticker_data in data {
            if let Some(market_id) = &ticker_data.symbol {
                if let Some(symbol) = self.symbol(market_id) {
                    let ticker = self.parse_ticker(&ticker_data, &symbol);
                    tickers.insert(symbol, ticker);
                }
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;
        let market_id = {
            let markets = self.markets.read().map_err(|_| CcxtError::ExchangeError {
                message: "Failed to acquire read lock".into(),
            })?;
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        let response: AscendexResponse<AscendexOrderBookWrapper> = self
            .public_get("/api/pro/v1/depth", params)
            .await?;

        let wrapper = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No order book data returned".into(),
        })?;

        let orderbook_data = wrapper.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No order book data returned".into(),
        })?;

        let timestamp = orderbook_data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = orderbook_data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: Decimal::from_str(b[0].as_str()?).ok()?,
                    amount: Decimal::from_str(b[1].as_str()?).ok()?,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = orderbook_data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: Decimal::from_str(a[0].as_str()?).ok()?,
                    amount: Decimal::from_str(a[1].as_str()?).ok()?,
                })
            } else {
                None
            }
        }).collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: orderbook_data.seqnum,
            bids,
            asks,
        })
    }

    async fn fetch_trades(&self, symbol: &str, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        self.load_markets(false).await?;
        let market_id = {
            let markets = self.markets.read().map_err(|_| CcxtError::ExchangeError {
                message: "Failed to acquire read lock".into(),
            })?;
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("n".to_string(), l.to_string());
        }

        let response: AscendexResponse<AscendexTradesWrapper> = self
            .public_get("/api/pro/v1/trades", params)
            .await?;

        let wrapper = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No trades data returned".into(),
        })?;

        let trades_data = wrapper.data.unwrap_or_default();
        let mut trades = Vec::new();

        for trade_data in trades_data {
            if let Some(trade) = self.parse_trade(symbol, &trade_data) {
                if let Some(since_ts) = since {
                    if let Some(ts) = trade.timestamp {
                        if ts < since_ts {
                            continue;
                        }
                    }
                }
                trades.push(trade);
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
        self.load_markets(false).await?;
        let market_id = {
            let markets = self.markets.read().map_err(|_| CcxtError::ExchangeError {
                message: "Failed to acquire read lock".into(),
            })?;
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let interval = self.timeframes.get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {:?}", timeframe),
            })?;

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("interval".to_string(), interval.clone());

        if let Some(s) = since {
            params.insert("from".to_string(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("n".to_string(), l.to_string());
        }

        let response: AscendexResponse<Vec<AscendexOHLCV>> = self
            .public_get("/api/pro/v1/barhist", params)
            .await?;

        let data = response.data.unwrap_or_default();
        let mut ohlcv_list = Vec::new();

        for candle in data {
            if let Some(ohlcv) = self.parse_ohlcv(&candle) {
                ohlcv_list.push(ohlcv);
            }
        }

        Ok(ohlcv_list)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        self.load_markets(false).await?;
        let account_group = self.get_account_group().await?;

        let path = format!("/api/pro/v1/{}/spot/balance", account_group);
        let response: AscendexResponse<Vec<AscendexBalanceEntry>> = self
            .private_get(&path, HashMap::new())
            .await?;

        let data = response.data.unwrap_or_default();
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
        self.load_markets(false).await?;
        let market_id = {
            let markets = self.markets.read().map_err(|_| CcxtError::ExchangeError {
                message: "Failed to acquire read lock".into(),
            })?;
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };
        let account_group = self.get_account_group().await?;

        let type_str = match order_type {
            OrderType::Market => "market",
            OrderType::Limit => "limit",
            OrderType::StopMarket => "stop_market",
            OrderType::StopLimit => "stop_limit",
            _ => return Err(CcxtError::BadRequest {
                message: format!("Unsupported order type: {:?}", order_type),
            }),
        };

        let side_str = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };

        let mut request = serde_json::json!({
            "symbol": market_id,
            "orderType": type_str,
            "side": side_str,
            "orderQty": amount.to_string(),
        });

        if let Some(p) = price {
            request["orderPrice"] = serde_json::json!(p.to_string());
        }

        let path = format!("/api/pro/v1/{}/cash/order", account_group);
        let response: AscendexResponse<AscendexOrderResponse> = self
            .private_post(&path, request)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No order response".into(),
        })?;

        let info = serde_json::to_value(&data).unwrap_or_default();
        Ok(Order {
            id: data.order_id.unwrap_or_default(),
            client_order_id: None,
            timestamp: data.timestamp,
            datetime: data.timestamp.and_then(|ts| {
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
            info,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.load_markets(false).await?;
        let market_id = {
            let markets = self.markets.read().map_err(|_| CcxtError::ExchangeError {
                message: "Failed to acquire read lock".into(),
            })?;
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };
        let account_group = self.get_account_group().await?;

        let request = serde_json::json!({
            "orderId": id,
            "symbol": market_id,
        });

        let path = format!("/api/pro/v1/{}/cash/order", account_group);
        let _response: AscendexResponse<AscendexCancelResponse> = self
            .private_delete(&path, request)
            .await?;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: None,
            datetime: None,
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
        self.load_markets(false).await?;
        let account_group = self.get_account_group().await?;

        let mut params = HashMap::new();
        params.insert("orderId".to_string(), id.to_string());

        let path = format!("/api/pro/v1/{}/cash/order/status", account_group);
        let response: AscendexResponse<AscendexOrder> = self
            .private_get(&path, params)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })?;

        self.parse_order(&data)
    }

    async fn fetch_open_orders(&self, symbol: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;
        let account_group = self.get_account_group().await?;

        let path = format!("/api/pro/v1/{}/cash/order/open", account_group);
        let response: AscendexResponse<Vec<AscendexOrder>> = self
            .private_get(&path, HashMap::new())
            .await?;

        let data = response.data.unwrap_or_default();
        let mut orders = Vec::new();

        for order_data in data {
            if let Some(sym) = symbol {
                if order_data.symbol.as_deref() != Some(sym) {
                    continue;
                }
            }
            orders.push(self.parse_order(&order_data)?);
        }

        Ok(orders)
    }

    async fn fetch_closed_orders(&self, symbol: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;
        let account_group = self.get_account_group().await?;

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let path = format!("/{}/api/pro/v2/data/order/hist", account_group);
        let response: AscendexResponse<Vec<AscendexOrder>> = self
            .private_get(&path, params)
            .await?;

        let data = response.data.unwrap_or_default();
        let mut orders = Vec::new();

        for order_data in data {
            if let Some(sym) = symbol {
                if order_data.symbol.as_deref() != Some(sym) {
                    continue;
                }
            }

            if let Some(since_ts) = since {
                if let Some(ts) = order_data.time {
                    if ts < since_ts {
                        continue;
                    }
                }
            }

            orders.push(self.parse_order(&order_data)?);
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
        let mut url = format!("{}{}", BASE_URL, path);

        if method == "GET" && !params.is_empty() {
            let query: Vec<String> = params
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect();
            url.push_str("?");
            url.push_str(&query.join("&"));
        }

        if self.config.api_key().is_some() && self.config.secret().is_some() {
            let timestamp = Utc::now().timestamp_millis().to_string();
            if let Ok(signature) = self.sign_request(&timestamp, path) {
                if let Some(api_key) = self.config.api_key() {
                    result_headers.insert("x-auth-key".to_string(), api_key.to_string());
                    result_headers.insert("x-auth-timestamp".to_string(), timestamp);
                    result_headers.insert("x-auth-signature".to_string(), signature);

                    if method == "POST" || method == "DELETE" {
                        result_headers.insert("Content-Type".to_string(), "application/json".to_string());
                    }
                }
            }
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers: result_headers,
            body: body.map(|s| s.to_string()),
        }
    }
}

// === AscendEX Response Types ===

#[derive(Debug, Default, Deserialize)]
struct AscendexResponse<T: Default> {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AscendexAccountInfo {
    #[serde(default, rename = "accountGroup")]
    account_group: i64,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AscendexMarket {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "baseAsset")]
    base_asset: Option<String>,
    #[serde(default, rename = "quoteAsset")]
    quote_asset: Option<String>,
    #[serde(default)]
    underlying: Option<String>,
    #[serde(default, rename = "settlementAsset")]
    settlement_asset: Option<String>,
    #[serde(default, rename = "marginTradable")]
    margin_tradable: Option<bool>,
    #[serde(default, rename = "tickSize")]
    tick_size: Option<f64>,
    #[serde(default, rename = "lotSize")]
    lot_size: Option<f64>,
    #[serde(default, rename = "minQty")]
    min_qty: Option<f64>,
    #[serde(default, rename = "maxQty")]
    max_qty: Option<f64>,
    #[serde(default, rename = "minNotional")]
    min_notional: Option<f64>,
    #[serde(default, rename = "maxNotional")]
    max_notional: Option<f64>,
    #[serde(default, rename = "commissionReserveRate")]
    commission_reserve_rate: Option<f64>,
    #[serde(default, rename = "tradingStartTime")]
    trading_start_time: Option<i64>,
    #[serde(default, rename = "priceFilter")]
    price_filter: Option<PriceFilter>,
    #[serde(default, rename = "lotSizeFilter")]
    lot_size_filter: Option<LotSizeFilter>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PriceFilter {
    #[serde(default, rename = "minPrice")]
    min_price: Option<f64>,
    #[serde(default, rename = "maxPrice")]
    max_price: Option<f64>,
    #[serde(default, rename = "tickSize")]
    tick_size: Option<f64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct LotSizeFilter {
    #[serde(default, rename = "minQty")]
    min_qty: Option<f64>,
    #[serde(default, rename = "maxQty")]
    max_qty: Option<f64>,
    #[serde(default, rename = "lotSize")]
    lot_size: Option<f64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AscendexTicker {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    ask: Option<serde_json::Value>,
    #[serde(default)]
    bid: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct AscendexOrderBookWrapper {
    #[serde(default)]
    data: Option<AscendexOrderBookData>,
}

#[derive(Debug, Default, Deserialize)]
struct AscendexOrderBookData {
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    seqnum: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<serde_json::Value>>,
    #[serde(default)]
    asks: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AscendexOHLCV {
    #[serde(default)]
    data: Option<AscendexOHLCVData>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AscendexOHLCVData {
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    o: Option<String>,
    #[serde(default)]
    h: Option<String>,
    #[serde(default)]
    l: Option<String>,
    #[serde(default)]
    c: Option<String>,
    #[serde(default)]
    v: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AscendexTradesWrapper {
    #[serde(default)]
    data: Option<Vec<AscendexTrade>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AscendexTrade {
    #[serde(default)]
    p: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    bm: Option<bool>,
    #[serde(default)]
    seqnum: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AscendexOrder {
    #[serde(default, rename = "orderId")]
    order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default, rename = "orderQty")]
    order_qty: Option<String>,
    #[serde(default, rename = "orderType")]
    order_type: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "avgFilledPx")]
    avg_filled_px: Option<String>,
    #[serde(default, rename = "cumFilledQty")]
    cum_filled_qty: Option<String>,
    #[serde(default, rename = "cumFee")]
    cum_fee: Option<String>,
    #[serde(default, rename = "feeAsset")]
    fee_asset: Option<String>,
    #[serde(default, rename = "stopPrice")]
    stop_price: Option<String>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default, rename = "lastExecTime")]
    last_exec_time: Option<i64>,
    #[serde(default, rename = "execInst")]
    exec_inst: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AscendexOrderResponse {
    #[serde(default, rename = "orderId")]
    order_id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct AscendexCancelResponse {
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AscendexBalanceEntry {
    #[serde(default)]
    asset: Option<String>,
    #[serde(default, rename = "totalBalance")]
    total_balance: Option<String>,
    #[serde(default, rename = "availableBalance")]
    available_balance: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_ascendex() -> Ascendex {
        Ascendex::new(ExchangeConfig::default()).unwrap()
    }

    #[test]
    fn test_exchange_id() {
        let ascendex = create_test_ascendex();
        assert_eq!(ascendex.name(), "AscendEX");
    }

    #[test]
    fn test_has_features() {
        let ascendex = create_test_ascendex();
        let features = ascendex.has();
        assert!(features.fetch_markets);
        assert!(features.fetch_ticker);
        assert!(features.fetch_order_book);
        assert!(features.fetch_balance);
        assert!(features.create_order);
    }

    #[test]
    fn test_timeframes() {
        let ascendex = create_test_ascendex();
        let timeframes = ascendex.timeframes();
        assert_eq!(timeframes.get(&Timeframe::Minute1), Some(&"1".to_string()));
        assert_eq!(timeframes.get(&Timeframe::Hour1), Some(&"60".to_string()));
        assert_eq!(timeframes.get(&Timeframe::Day1), Some(&"1d".to_string()));
    }

    #[test]
    fn test_market_id() {
        let ascendex = create_test_ascendex();
        assert_eq!(ascendex.market_id("BTC/USDT"), Some("BTCUSDT".to_string()));
        assert_eq!(ascendex.market_id("ETH/USDT"), Some("ETHUSDT".to_string()));
    }
}
