//! Coinbase International Exchange Implementation
//!
//! Coinbase International/Institutional API for derivatives and futures trading

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::RwLock;
use base64::{Engine as _, engine::general_purpose};

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade,
    Transaction, Fee, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

const BASE_URL: &str = "https://api.international.coinbase.com/api";
const SANDBOX_URL: &str = "https://api-n5e1.coinbase.com/api";
const RATE_LIMIT_MS: u64 = 100;

/// Coinbase International - Institutional trading platform with derivatives/futures
pub struct CoinbaseInternational {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    portfolio_id: RwLock<Option<String>>,
}

impl CoinbaseInternational {
    /// Create new CoinbaseInternational instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let base_url = if config.is_sandbox() { SANDBOX_URL } else { BASE_URL };
        let client = HttpClient::new(base_url, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: true,
            spot: true,
            margin: true,
            swap: true,
            future: true,
            option: false,
            fetch_markets: true,
            fetch_currencies: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: false,
            fetch_trades: false,
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
            fetch_closed_orders: false,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            fetch_deposit_address: true,
            fetch_positions: true,
            fetch_position: true,
            set_margin: true,
            create_stop_limit_order: true,
            create_stop_market_order: true,
            edit_order: true,
            ws: false,
            watch_ticker: false,
            watch_tickers: false,
            watch_order_book: false,
            watch_trades: false,
            watch_ohlcv: false,
            watch_balance: false,
            watch_orders: false,
            watch_my_trades: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), base_url.into());
        api_urls.insert("private".into(), base_url.into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/ccxt/ccxt/assets/43336371/866ae638-6ab5-4ebf-ab2c-cdcce9545625".into()),
            api: api_urls,
            www: Some("https://international.coinbase.com".into()),
            doc: vec![
                "https://docs.cloud.coinbase.com/intx/docs".into(),
            ],
            fees: Some("https://help.coinbase.com/en/international-exchange/trading-deposits-withdrawals/international-exchange-fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "ONE_MINUTE".into());
        timeframes.insert(Timeframe::Minute5, "FIVE_MINUTE".into());
        timeframes.insert(Timeframe::Minute15, "FIFTEEN_MINUTE".into());
        timeframes.insert(Timeframe::Minute30, "THIRTY_MINUTE".into());
        timeframes.insert(Timeframe::Hour1, "ONE_HOUR".into());
        timeframes.insert(Timeframe::Hour2, "TWO_HOUR".into());
        timeframes.insert(Timeframe::Hour6, "SIX_HOUR".into());
        timeframes.insert(Timeframe::Day1, "ONE_DAY".into());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            portfolio_id: RwLock::new(None),
        })
    }

    /// Convert symbol to market ID (BTC/USD:USD -> BTC-USD)
    fn to_market_id(&self, symbol: &str) -> String {
        // Handle futures contracts like BTC/USD:USD -> BTC-USD
        symbol.split(':').next().unwrap_or(symbol).replace("/", "-")
    }

    /// Convert market ID to symbol (BTC-USD -> BTC/USD)
    fn to_symbol(&self, market_id: &str) -> String {
        market_id.replace("-", "/")
    }

    /// Create HMAC signature for authentication
    fn create_signature(&self, timestamp: &str, method: &str, path: &str, body: &str) -> CcxtResult<String> {
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let message = format!("{timestamp}{method}{path}{body}");
        let secret_bytes = general_purpose::STANDARD
            .decode(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Failed to decode secret: {e}"),
            })?;

        let mut mac = HmacSha256::new_from_slice(&secret_bytes)
            .expect("HMAC can take key of any size");
        mac.update(message.as_bytes());
        Ok(general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
    }

    /// Get or initialize portfolio ID
    async fn ensure_portfolio_id(&self) -> CcxtResult<String> {
        // Check if already cached
        if let Ok(guard) = self.portfolio_id.read() {
            if let Some(id) = guard.as_ref() {
                return Ok(id.clone());
            }
        }

        // Fetch portfolios and find default
        let response: CoinbaseIntlPortfoliosResponse = self
            .private_get("/v1/portfolios", HashMap::new())
            .await?;

        for portfolio in &response.portfolios {
            if portfolio.is_default.unwrap_or(false) {
                let id = portfolio.portfolio_id.clone().unwrap_or_default();
                if let Ok(mut guard) = self.portfolio_id.write() {
                    *guard = Some(id.clone());
                }
                return Ok(id);
            }
        }

        Err(CcxtError::ExchangeError {
            message: "No default portfolio found. Please set portfolio_id in config.".into(),
        })
    }

    /// Public GET request
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Private GET request
    async fn private_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let passphrase = self.config.password().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Passphrase required".into(),
        })?;

        let timestamp = Utc::now().timestamp().to_string();
        let full_path = if params.is_empty() {
            path.to_string()
        } else {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{path}?{query}")
        };

        let signature = self.create_signature(&timestamp, "GET", &full_path, "")?;

        let mut headers = HashMap::new();
        headers.insert("CB-ACCESS-KEY".to_string(), api_key.to_string());
        headers.insert("CB-ACCESS-SIGN".to_string(), signature);
        headers.insert("CB-ACCESS-TIMESTAMP".to_string(), timestamp);
        headers.insert("CB-ACCESS-PASSPHRASE".to_string(), passphrase.to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        self.client.get(&full_path, None, Some(headers)).await
    }

    /// Private POST request
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let passphrase = self.config.password().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Passphrase required".into(),
        })?;

        let timestamp = Utc::now().timestamp().to_string();
        let body_str = body.to_string();
        let signature = self.create_signature(&timestamp, "POST", path, &body_str)?;

        let mut headers = HashMap::new();
        headers.insert("CB-ACCESS-KEY".to_string(), api_key.to_string());
        headers.insert("CB-ACCESS-SIGN".to_string(), signature);
        headers.insert("CB-ACCESS-TIMESTAMP".to_string(), timestamp);
        headers.insert("CB-ACCESS-PASSPHRASE".to_string(), passphrase.to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        self.client.post(path, Some(body), Some(headers)).await
    }

    /// Private PUT request
    async fn private_put<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let passphrase = self.config.password().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Passphrase required".into(),
        })?;

        let timestamp = Utc::now().timestamp().to_string();
        let body_str = body.to_string();
        let signature = self.create_signature(&timestamp, "PUT", path, &body_str)?;

        let mut headers = HashMap::new();
        headers.insert("CB-ACCESS-KEY".to_string(), api_key.to_string());
        headers.insert("CB-ACCESS-SIGN".to_string(), signature);
        headers.insert("CB-ACCESS-TIMESTAMP".to_string(), timestamp);
        headers.insert("CB-ACCESS-PASSPHRASE".to_string(), passphrase.to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        self.client.put_json(path, Some(body), Some(headers)).await
    }

    /// Private DELETE request
    async fn private_delete<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let passphrase = self.config.password().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Passphrase required".into(),
        })?;

        let timestamp = Utc::now().timestamp().to_string();
        let signature = self.create_signature(&timestamp, "DELETE", path, "")?;

        let mut headers = HashMap::new();
        headers.insert("CB-ACCESS-KEY".to_string(), api_key.to_string());
        headers.insert("CB-ACCESS-SIGN".to_string(), signature);
        headers.insert("CB-ACCESS-TIMESTAMP".to_string(), timestamp);
        headers.insert("CB-ACCESS-PASSPHRASE".to_string(), passphrase.to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        self.client.delete(path, None, Some(headers)).await
    }

    /// Parse market data
    fn parse_market(&self, data: &CoinbaseIntlInstrument) -> Market {
        let base = data.base.clone().unwrap_or_default();
        let quote = data.quote.clone().unwrap_or_default();
        let symbol_type = data.instrument_type.as_deref().unwrap_or("SPOT");

        let (market_type, is_spot, is_swap, is_future) = match symbol_type {
            "PERP" => (MarketType::Swap, false, true, false),
            "FUTURE" => (MarketType::Future, false, false, true),
            _ => (MarketType::Spot, true, false, false),
        };

        let settle = if is_swap || is_future {
            Some(quote.clone())
        } else {
            None
        };

        let symbol = if is_swap || is_future {
            format!("{}/{}:{}", base, quote, settle.as_ref().unwrap_or(&quote))
        } else {
            format!("{}/{}", base, quote)
        };

        Market {
            id: data.instrument_id.clone().unwrap_or_default(),
            lowercase_id: data.instrument_id.as_ref().map(|s| s.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: base.to_lowercase(),
            quote_id: quote.to_lowercase(),
            settle,
            settle_id: None,
            market_type,
            spot: is_spot,
            margin: data.margin_enabled.unwrap_or(false),
            swap: is_swap,
            future: is_future,
            option: false,
            index: false,
            active: data.status.as_ref().map(|s| s == "ACTIVE").unwrap_or(true),
            contract: is_swap || is_future,
            linear: Some(true),
            inverse: Some(false),
            sub_type: None,
            taker: Some(Decimal::new(40, 4)), // 0.004 = 0.4%
            maker: Some(Decimal::new(20, 4)), // 0.002 = 0.2%
            contract_size: data.contract_size.as_ref().and_then(|v| v.parse().ok()),
            expiry: data.expiry.as_ref().and_then(|e| {
                chrono::DateTime::parse_from_rfc3339(e).ok().map(|dt| dt.timestamp_millis())
            }),
            expiry_datetime: data.expiry.clone(),
            strike: None,
            option_type: None,
            percentage: true,
            tier_based: true,
            precision: MarketPrecision {
                amount: data.base_increment.as_ref()
                    .and_then(|v| v.parse::<Decimal>().ok())
                    .map(|d| {
                        let s = d.to_string();
                        if let Some(pos) = s.find('.') {
                            (s.len() - pos - 1) as i32
                        } else {
                            0
                        }
                    }),
                price: data.quote_increment.as_ref()
                    .and_then(|v| v.parse::<Decimal>().ok())
                    .map(|d| {
                        let s = d.to_string();
                        if let Some(pos) = s.find('.') {
                            (s.len() - pos - 1) as i32
                        } else {
                            0
                        }
                    }),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: crate::types::MinMax {
                    min: data.base_min_size.as_ref().and_then(|v| v.parse().ok()),
                    max: data.base_max_size.as_ref().and_then(|v| v.parse().ok()),
                },
                price: crate::types::MinMax {
                    min: data.quote_increment.as_ref().and_then(|v| v.parse().ok()),
                    max: None,
                },
                cost: crate::types::MinMax::default(),
                leverage: crate::types::MinMax::default(),
            },
            margin_modes: None,
            created: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse ticker data
    fn parse_ticker(&self, data: &CoinbaseIntlQuote, symbol: &str) -> Ticker {
        let timestamp = data.time.as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: None,
            low: None,
            bid: data.bid_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: None,
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: None,
            quote_volume: None,
            index_price: data.index_price.as_ref().and_then(|v| v.parse().ok()),
            mark_price: data.mark_price.as_ref().and_then(|v| v.parse().ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order data
    fn parse_order(&self, data: &CoinbaseIntlOrder, symbol: &str) -> CcxtResult<Order> {
        let timestamp = data.created_at.as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis());

        let status = match data.status.as_deref() {
            Some("PENDING") | Some("OPEN") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("CANCELLED") | Some("EXPIRED") => OrderStatus::Canceled,
            Some("REJECTED") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let side = match data.side.as_deref() {
            Some("BID") | Some("BUY") => OrderSide::Buy,
            Some("ASK") | Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("MARKET") => OrderType::Market,
            Some("LIMIT") => OrderType::Limit,
            Some("STOP") | Some("STOP_LIMIT") => OrderType::StopLimit,
            _ => OrderType::Limit,
        };

        let price: Option<Decimal> = data.price.as_ref().and_then(|v| v.parse().ok());
        let amount: Decimal = data.size.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data.filled_size.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        Ok(Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: data.time_in_force.as_deref().and_then(|tif| match tif {
                "GTC" | "GTD" => Some(TimeInForce::GTC), // GTD maps to GTC
                "IOC" => Some(TimeInForce::IOC),
                "FOK" => Some(TimeInForce::FOK),
                "PO" => Some(TimeInForce::PO),
                _ => None,
            }),
            side,
            price,
            trigger_price: data.stop_price.as_ref().and_then(|v| v.parse().ok()),
            average: data.avg_price.as_ref().and_then(|v| v.parse().ok()),
            amount,
            filled,
            remaining: Some(amount - filled),
            cost: None,
            trades: Vec::new(),
            reduce_only: data.reduce_only,
            post_only: data.post_only,
            stop_price: data.stop_price.as_ref().and_then(|v| v.parse().ok()),
            take_profit_price: None,
            stop_loss_price: None,
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// Parse trade/fill data
    fn parse_trade_data(&self, data: &CoinbaseIntlFill, symbol: &str) -> Trade {
        let timestamp = data.time.as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let price: Decimal = data.price.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data.size.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: data.order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.side.as_ref().map(|s| s.to_lowercase()),
            taker_or_maker: data.liquidity.as_ref().map(|l| {
                if l == "M" || l == "MAKER" {
                    TakerOrMaker::Maker
                } else {
                    TakerOrMaker::Taker
                }
            }),
            price,
            amount,
            cost: Some(price * amount),
            fee: data.fee.as_ref().and_then(|v| v.parse().ok()).map(|cost| Fee {
                cost: Some(cost),
                currency: data.fee_asset.clone(),
                rate: None,
            }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance data
    fn parse_balance_data(&self, data: &CoinbaseIntlBalance) -> (String, Balance) {
        let currency = data.asset.clone().unwrap_or_default();
        let total: Decimal = data.quantity.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let hold: Decimal = data.hold.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let available = total - hold;

        (
            currency,
            Balance {
                free: Some(available),
                used: Some(hold),
                total: Some(total),
                debt: None,
            },
        )
    }
}

#[async_trait]
impl Exchange for CoinbaseInternational {
    fn id(&self) -> ExchangeId {
        ExchangeId::CoinbaseInternational
    }

    fn name(&self) -> &str {
        "Coinbase International"
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
        self.markets_by_id.read().ok()?.get(symbol).cloned()
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        self.markets.read().ok()?.get(market_id).map(|m| m.symbol.clone())
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
        let timestamp = Utc::now().timestamp().to_string();
        let body_str = body.unwrap_or("");

        let message = format!("{timestamp}{method}{path}{body_str}");

        let signature = if let (Some(secret), Some(_api_key), Some(_passphrase)) =
            (self.config.secret(), self.config.api_key(), self.config.password()) {
            if let Ok(secret_bytes) = general_purpose::STANDARD.decode(secret.as_bytes()) {
                let mut mac = HmacSha256::new_from_slice(&secret_bytes)
                    .expect("HMAC can take key of any size");
                mac.update(message.as_bytes());
                general_purpose::STANDARD.encode(mac.finalize().into_bytes())
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let mut new_headers = headers.unwrap_or_default();
        if let (Some(api_key), Some(passphrase)) = (self.config.api_key(), self.config.password()) {
            new_headers.insert("CB-ACCESS-KEY".to_string(), api_key.to_string());
            new_headers.insert("CB-ACCESS-SIGN".to_string(), signature);
            new_headers.insert("CB-ACCESS-TIMESTAMP".to_string(), timestamp);
            new_headers.insert("CB-ACCESS-PASSPHRASE".to_string(), passphrase.to_string());
        }
        new_headers.insert("Content-Type".to_string(), "application/json".to_string());

        let base_url = if self.config.is_sandbox() { SANDBOX_URL } else { BASE_URL };
        let url = if method == "GET" && !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{}/{}?{}", base_url, path.trim_start_matches('/'), query)
        } else {
            format!("{}/{}", base_url, path.trim_start_matches('/'))
        };

        SignedRequest {
            url,
            method: method.to_string(),
            headers: new_headers,
            body: body.map(|s| s.to_string()),
        }
    }

    async fn load_markets(&self, _reload: bool) -> CcxtResult<HashMap<String, Market>> {
        let response: CoinbaseIntlInstrumentsResponse = self
            .public_get("/v1/instruments", None)
            .await?;

        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for instrument in &response.instruments {
            let market = self.parse_market(instrument);
            markets_by_id.insert(market.symbol.clone(), market.id.clone());
            markets.insert(market.symbol.clone(), market);
        }

        if let Ok(mut cached) = self.markets.write() {
            *cached = markets.clone();
        }
        if let Ok(mut cached) = self.markets_by_id.write() {
            *cached = markets_by_id;
        }

        Ok(markets)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(false).await?;
        Ok(markets.into_values().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let response: CoinbaseIntlQuote = self
            .public_get(&format!("/v1/instruments/{market_id}/quote"), None)
            .await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: CoinbaseIntlInstrumentsResponse = self
            .public_get("/v1/instruments", None)
            .await?;

        let mut tickers = HashMap::new();
        for instrument in &response.instruments {
            let symbol = self.to_symbol(&instrument.instrument_id.clone().unwrap_or_default());
            if let Some(syms) = symbols {
                if !syms.contains(&symbol.as_str()) {
                    continue;
                }
            }

            // Fetch individual ticker for each instrument
            if let Ok(ticker_data) = self.fetch_ticker(&symbol).await {
                tickers.insert(symbol, ticker_data);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, _symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        Err(CcxtError::NotSupported {
            feature: "fetch_order_book not supported by Coinbase International".into(),
        })
    }

    async fn fetch_trades(&self, _symbol: &str, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        Err(CcxtError::NotSupported {
            feature: "fetch_trades not supported by Coinbase International".into(),
        })
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.to_market_id(symbol);
        let granularity = self.timeframes.get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("granularity".to_string(), granularity.clone());

        if let Some(s) = since {
            let start = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("start".to_string(), start);
        }

        let response: CoinbaseIntlCandlesResponse = self
            .public_get(&format!("/v1/instruments/{market_id}/candles"), Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response.aggregations.iter()
            .take(limit.unwrap_or(300) as usize)
            .map(|c| {
                OHLCV {
                    timestamp: c.start.as_ref()
                        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                        .map(|dt| dt.timestamp_millis())
                        .unwrap_or_default(),
                    open: c.open.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
                    high: c.high.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
                    low: c.low.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
                    close: c.close.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
                    volume: c.volume.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
                }
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let portfolio_id = self.ensure_portfolio_id().await?;
        let response: CoinbaseIntlBalancesResponse = self
            .private_get(&format!("/v1/portfolios/{portfolio_id}/balances"), HashMap::new())
            .await?;

        let mut balances = Balances {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies: HashMap::new(),
            info: serde_json::to_value(&response).unwrap_or_default(),
        };

        for balance_data in &response.balances {
            let (currency, balance) = self.parse_balance_data(balance_data);
            balances.currencies.insert(currency, balance);
        }

        Ok(balances)
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let portfolio_id = self.ensure_portfolio_id().await?;
        let market_id = self.to_market_id(symbol);
        let client_order_id = format!("ccxt_{}", Utc::now().timestamp_millis());

        let mut body = serde_json::json!({
            "portfolio_id": portfolio_id,
            "product_id": market_id,
            "client_order_id": client_order_id,
            "side": match side {
                OrderSide::Buy => "BID",
                OrderSide::Sell => "ASK",
            },
        });

        match order_type {
            OrderType::Market => {
                body["order_type"] = serde_json::json!("MARKET");
                body["quote_size"] = serde_json::json!(amount.to_string());
                body["tif"] = serde_json::json!("IOC");
            }
            OrderType::Limit => {
                let p = price.ok_or_else(|| CcxtError::BadRequest {
                    message: "Price required for limit order".into(),
                })?;
                body["order_type"] = serde_json::json!("LIMIT");
                body["base_size"] = serde_json::json!(amount.to_string());
                body["limit_price"] = serde_json::json!(p.to_string());
                body["tif"] = serde_json::json!("GTC");
            }
            _ => {
                return Err(CcxtError::BadRequest {
                    message: format!("Unsupported order type: {order_type:?}"),
                });
            }
        }

        let response: CoinbaseIntlOrder = self
            .private_post("/v1/orders", body)
            .await?;

        self.parse_order(&response, symbol)
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let _response: serde_json::Value = self
            .private_delete(&format!("/v1/orders/{id}"))
            .await?;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: String::new(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price: None,
            trigger_price: None,
            average: None,
            amount: Decimal::ZERO,
            filled: Decimal::ZERO,
            remaining: None,
            cost: None,
            trades: Vec::new(),
            reduce_only: None,
            post_only: None,
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            fee: None,
            fees: Vec::new(),
            info: serde_json::Value::Null,
        })
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let response: CoinbaseIntlOrder = self
            .private_get(&format!("/v1/orders/{id}"), HashMap::new())
            .await?;

        let symbol = response.product_id.clone()
            .map(|id| self.to_symbol(&id))
            .unwrap_or_default();

        self.parse_order(&response, &symbol)
    }

    async fn fetch_open_orders(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let portfolio_id = self.ensure_portfolio_id().await?;
        let mut params = HashMap::new();
        params.insert("portfolio_id".to_string(), portfolio_id);

        if let Some(sym) = symbol {
            params.insert("product_id".to_string(), self.to_market_id(sym));
        }
        if let Some(l) = limit {
            params.insert("result_limit".to_string(), l.to_string());
        }

        let response: CoinbaseIntlOrdersResponse = self
            .private_get("/v1/orders", params)
            .await?;

        let mut orders = Vec::new();
        for order_data in &response.orders {
            let sym = order_data.product_id.clone()
                .map(|id| self.to_symbol(&id))
                .unwrap_or_default();
            orders.push(self.parse_order(order_data, &sym)?);
        }

        Ok(orders)
    }

    async fn fetch_closed_orders(&self, _symbol: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        Err(CcxtError::NotSupported {
            feature: "fetch_closed_orders not supported by Coinbase International".into(),
        })
    }

    async fn fetch_my_trades(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let portfolio_id = self.ensure_portfolio_id().await?;
        let mut params = HashMap::new();

        if let Some(sym) = symbol {
            params.insert("product_id".to_string(), self.to_market_id(sym));
        }
        if let Some(l) = limit {
            params.insert("result_limit".to_string(), l.to_string());
        }

        let response: CoinbaseIntlFillsResponse = self
            .private_get(&format!("/v1/portfolios/{portfolio_id}/fills"), params)
            .await?;

        let trades: Vec<Trade> = response.fills.iter().map(|f| {
            let sym = f.product_id.clone()
                .map(|id| self.to_symbol(&id))
                .unwrap_or_default();
            self.parse_trade_data(f, &sym)
        }).collect();

        Ok(trades)
    }

    async fn fetch_deposits(&self, _code: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Transaction>> {
        Ok(Vec::new())
    }

    async fn fetch_withdrawals(&self, _code: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Transaction>> {
        Ok(Vec::new())
    }
}

// === Coinbase International Response Types ===

#[derive(Debug, Default, Deserialize)]
struct CoinbaseIntlInstrumentsResponse {
    #[serde(default)]
    instruments: Vec<CoinbaseIntlInstrument>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseIntlInstrument {
    #[serde(default)]
    instrument_id: Option<String>,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    quote: Option<String>,
    #[serde(default)]
    instrument_type: Option<String>, // SPOT, PERP, FUTURE
    #[serde(default)]
    base_increment: Option<String>,
    #[serde(default)]
    quote_increment: Option<String>,
    #[serde(default)]
    base_min_size: Option<String>,
    #[serde(default)]
    base_max_size: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    margin_enabled: Option<bool>,
    #[serde(default)]
    contract_size: Option<String>,
    #[serde(default)]
    expiry: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseIntlQuote {
    #[serde(default)]
    bid_price: Option<String>,
    #[serde(default)]
    bid_size: Option<String>,
    #[serde(default)]
    ask_price: Option<String>,
    #[serde(default)]
    ask_size: Option<String>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    index_price: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseIntlCandlesResponse {
    #[serde(default)]
    aggregations: Vec<CoinbaseIntlCandle>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseIntlCandle {
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    volume: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseIntlPortfoliosResponse {
    #[serde(default)]
    portfolios: Vec<CoinbaseIntlPortfolio>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseIntlPortfolio {
    #[serde(default)]
    portfolio_id: Option<String>,
    #[serde(default)]
    portfolio_uuid: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    is_default: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseIntlBalancesResponse {
    #[serde(default)]
    balances: Vec<CoinbaseIntlBalance>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseIntlBalance {
    #[serde(default)]
    asset: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    hold: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseIntlOrdersResponse {
    #[serde(default)]
    orders: Vec<CoinbaseIntlOrder>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseIntlOrder {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    product_id: Option<String>,
    #[serde(default)]
    side: Option<String>, // BID, ASK
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    order_type: Option<String>, // MARKET, LIMIT, STOP
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    filled_size: Option<String>,
    #[serde(default)]
    avg_price: Option<String>,
    #[serde(default)]
    time_in_force: Option<String>, // GTC, IOC, FOK, GTD
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    post_only: Option<bool>,
    #[serde(default)]
    reduce_only: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseIntlFillsResponse {
    #[serde(default)]
    fills: Vec<CoinbaseIntlFill>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseIntlFill {
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    product_id: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    fee_asset: Option<String>,
    #[serde(default)]
    liquidity: Option<String>, // M (maker), T (taker)
    #[serde(default)]
    time: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::default();
        let exchange = CoinbaseInternational::new(config).unwrap();

        assert_eq!(exchange.to_market_id("BTC/USD"), "BTC-USD");
        assert_eq!(exchange.to_market_id("BTC/USD:USD"), "BTC-USD");
        assert_eq!(exchange.to_symbol("BTC-USD"), "BTC/USD");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = CoinbaseInternational::new(config).unwrap();

        assert_eq!(exchange.name(), "Coinbase International");
        assert!(exchange.has().fetch_markets);
        assert!(exchange.has().fetch_ticker);
        assert!(exchange.has().create_order);
        assert!(exchange.has().swap);
        assert!(exchange.has().future);
    }
}
