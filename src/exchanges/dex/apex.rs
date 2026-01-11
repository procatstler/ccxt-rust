//! ApeX Pro (DEX) Exchange Implementation
//!
//! ApeX Pro is a decentralized exchange using StarkEx Layer 2
//! API Documentation: <https://api-docs.pro.apex.exchange/>

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls,
    Market, MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// ApeX Pro (DEX) Exchange
pub struct Apex {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

// API Response structures
#[derive(Debug, Deserialize, Serialize)]
struct ApexResponse<T> {
    #[serde(default)]
    code: Option<String>,
    data: Option<T>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(rename = "timeCost")]
    time_cost: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexSymbolsData {
    #[serde(rename = "contractConfig")]
    contract_config: Option<ApexContractConfig>,
    #[serde(rename = "spotConfig")]
    spot_config: Option<ApexSpotConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexContractConfig {
    #[serde(rename = "perpetualContract")]
    perpetual_contract: Option<Vec<ApexContract>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexSpotConfig {
    #[serde(rename = "spotSymbols")]
    spot_symbols: Option<Vec<ApexSpotSymbol>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexContract {
    symbol: String,
    #[serde(rename = "symbolDisplayName")]
    symbol_display_name: Option<String>,
    #[serde(rename = "baseCurrency")]
    base_currency: String,
    #[serde(rename = "quoteCurrency")]
    quote_currency: String,
    #[serde(rename = "tickSize")]
    tick_size: String,
    #[serde(rename = "stepSize")]
    step_size: String,
    #[serde(rename = "minOrderSize")]
    min_order_size: String,
    #[serde(rename = "maxOrderSize")]
    max_order_size: String,
    #[serde(rename = "makerFeeRate")]
    maker_fee_rate: Option<String>,
    #[serde(rename = "takerFeeRate")]
    taker_fee_rate: Option<String>,
    status: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexSpotSymbol {
    symbol: String,
    #[serde(rename = "baseCurrency")]
    base_currency: String,
    #[serde(rename = "quoteCurrency")]
    quote_currency: String,
    #[serde(rename = "tickSize")]
    tick_size: String,
    #[serde(rename = "stepSize")]
    step_size: String,
    #[serde(rename = "minOrderSize")]
    min_order_size: String,
    status: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexTicker {
    symbol: String,
    #[serde(rename = "lastPrice")]
    last_price: Option<String>,
    #[serde(rename = "markPrice")]
    mark_price: Option<String>,
    #[serde(rename = "indexPrice")]
    index_price: Option<String>,
    #[serde(rename = "high24h")]
    high_24h: Option<String>,
    #[serde(rename = "low24h")]
    low_24h: Option<String>,
    #[serde(rename = "volume24h")]
    volume_24h: Option<String>,
    #[serde(rename = "turnover24h")]
    turnover_24h: Option<String>,
    #[serde(rename = "openPrice24h")]
    open_price_24h: Option<String>,
    #[serde(rename = "priceChange24h")]
    price_change_24h: Option<String>,
    #[serde(rename = "nextFundingRate")]
    next_funding_rate: Option<String>,
    #[serde(rename = "nextFundingTime")]
    next_funding_time: Option<i64>,
    #[serde(rename = "oraclePrice")]
    oracle_price: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexOrderBook {
    #[serde(rename = "a")]
    asks: Vec<Vec<String>>,
    #[serde(rename = "b")]
    bids: Vec<Vec<String>>,
    #[serde(rename = "u")]
    update_id: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexTrade {
    #[serde(rename = "i")]
    id: String,
    #[serde(rename = "T")]
    timestamp: i64,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "v")]
    amount: String,
    #[serde(rename = "S")]
    side: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexKline {
    #[serde(rename = "t")]
    timestamp: i64,
    #[serde(rename = "o")]
    open: String,
    #[serde(rename = "h")]
    high: String,
    #[serde(rename = "l")]
    low: String,
    #[serde(rename = "c")]
    close: String,
    #[serde(rename = "v")]
    volume: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexBalance {
    #[serde(rename = "totalEquityValue")]
    total_equity: Option<String>,
    #[serde(rename = "availableBalance")]
    available_balance: Option<String>,
    #[serde(rename = "initialMargin")]
    initial_margin: Option<String>,
    #[serde(rename = "maintenanceMargin")]
    maintenance_margin: Option<String>,
    #[serde(rename = "spotBalances")]
    spot_balances: Option<Vec<ApexSpotBalance>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexSpotBalance {
    currency: String,
    #[serde(rename = "availableBalance")]
    available_balance: String,
    #[serde(rename = "frozenBalance")]
    frozen_balance: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexOrder {
    id: String,
    #[serde(rename = "clientOrderId")]
    client_order_id: Option<String>,
    symbol: String,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    size: String,
    price: Option<String>,
    #[serde(rename = "filledSize")]
    filled_size: Option<String>,
    #[serde(rename = "filledValue")]
    filled_value: Option<String>,
    status: String,
    #[serde(rename = "createdAt")]
    created_at: i64,
    #[serde(rename = "updatedAt")]
    updated_at: Option<i64>,
}

impl Apex {
    const BASE_URL: &'static str = "https://omni.apex.exchange/api";
    const RATE_LIMIT_MS: u64 = 100;

    /// Create new Apex instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: true,
            spot: true,
            margin: false,
            swap: true,
            future: false,
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
            logo: Some("https://apex.exchange/favicon.ico".into()),
            api: api_urls,
            www: Some("https://apex.exchange".into()),
            doc: vec!["https://api-docs.pro.apex.exchange/".into()],
            fees: Some("https://apex.exchange/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".into());
        timeframes.insert(Timeframe::Minute5, "5".into());
        timeframes.insert(Timeframe::Minute15, "15".into());
        timeframes.insert(Timeframe::Minute30, "30".into());
        timeframes.insert(Timeframe::Hour1, "60".into());
        timeframes.insert(Timeframe::Hour2, "120".into());
        timeframes.insert(Timeframe::Hour4, "240".into());
        timeframes.insert(Timeframe::Hour6, "360".into());
        timeframes.insert(Timeframe::Hour12, "720".into());
        timeframes.insert(Timeframe::Day1, "D".into());
        timeframes.insert(Timeframe::Week1, "W".into());
        timeframes.insert(Timeframe::Month1, "M".into());

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
        // BTC/USDT -> BTCUSDT, BTC/USDT:USDT -> BTC-USDT
        if symbol.contains(":") {
            let parts: Vec<&str> = symbol.split(':').collect();
            let base_quote = parts[0];
            base_quote.replace("/", "-")
        } else {
            symbol.replace("/", "")
        }
    }

    /// Convert market ID to symbol
    fn to_symbol(&self, market_id: &str) -> String {
        let markets_by_id = self.markets_by_id.read().unwrap();
        markets_by_id.get(market_id).cloned().unwrap_or_else(|| market_id.to_string())
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

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;
        let passphrase = self.config.password().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Passphrase required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();

        // Build query string for GET or body for POST
        let data_string = if params.is_empty() {
            String::new()
        } else {
            let mut sorted: Vec<_> = params.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            sorted.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        };

        // Signature payload: timestamp + method + path + dataString
        let sign_payload = format!("{}{}{}{}", timestamp, method.to_uppercase(), path, data_string);

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(sign_payload.as_bytes());
        let signature = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            mac.finalize().into_bytes(),
        );

        let mut headers = HashMap::new();
        headers.insert("APEX-API-KEY".to_string(), api_key.to_string());
        headers.insert("APEX-SIGNATURE".to_string(), signature);
        headers.insert("APEX-TIMESTAMP".to_string(), timestamp);
        headers.insert("APEX-PASSPHRASE".to_string(), passphrase.to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        if method == "GET" {
            let url = if data_string.is_empty() {
                path.to_string()
            } else {
                format!("{path}?{data_string}")
            };
            self.client.get(&url, None, Some(headers)).await
        } else {
            let body = serde_json::to_value(&params).ok();
            self.client.post(path, body, Some(headers)).await
        }
    }

    /// Parse order status
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status.to_uppercase().as_str() {
            "NEW" | "PENDING" | "OPEN" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" | "CANCELLED" => OrderStatus::Canceled,
            "EXPIRED" => OrderStatus::Expired,
            "REJECTED" => OrderStatus::Rejected,
            "PARTIALLY_FILLED" => OrderStatus::Open,
            _ => OrderStatus::Open,
        }
    }
}

#[async_trait]
impl Exchange for Apex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Apex
    }

    fn name(&self) -> &str {
        "Apex"
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
        let response: ApexResponse<ApexSymbolsData> = self.public_get("/v3/symbols", None).await?;

        let symbols_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to load markets".into(),
        })?;

        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        // Parse perpetual contracts
        if let Some(contract_config) = symbols_data.contract_config {
            if let Some(contracts) = contract_config.perpetual_contract {
                for contract in contracts {
                    if contract.status.to_uppercase() != "ONLINE" {
                        continue;
                    }

                    let base = contract.base_currency.to_uppercase();
                    let quote = contract.quote_currency.to_uppercase();
                    let symbol = format!("{base}/{quote}");
                    let market_id = contract.symbol.clone();

                    let tick_size = Decimal::from_str(&contract.tick_size).unwrap_or(Decimal::ZERO);
                    let step_size = Decimal::from_str(&contract.step_size).unwrap_or(Decimal::ZERO);

                    let price_precision = if tick_size > Decimal::ZERO {
                        tick_size.to_string().split('.').nth(1).map(|s| s.trim_end_matches('0').len() as i32).unwrap_or(0)
                    } else { 8 };

                    let amount_precision = if step_size > Decimal::ZERO {
                        step_size.to_string().split('.').nth(1).map(|s| s.trim_end_matches('0').len() as i32).unwrap_or(0)
                    } else { 8 };

                    let taker_fee = contract.taker_fee_rate.as_ref()
                        .and_then(|f| Decimal::from_str(f).ok());
                    let maker_fee = contract.maker_fee_rate.as_ref()
                        .and_then(|f| Decimal::from_str(f).ok());

                    let market = Market {
                        id: market_id.clone(),
                        lowercase_id: Some(market_id.to_lowercase()),
                        symbol: symbol.clone(),
                        base: base.clone(),
                        quote: quote.clone(),
                        base_id: contract.base_currency.clone(),
                        quote_id: contract.quote_currency.clone(),
                        settle: Some(quote.clone()),
                        settle_id: Some(contract.quote_currency.clone()),
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
                        sub_type: Some("perpetual".into()),
                        contract_size: Some(Decimal::ONE),
                        expiry: None,
                        expiry_datetime: None,
                        strike: None,
                        option_type: None,
                        taker: taker_fee,
                        maker: maker_fee,
                        percentage: true,
                        tier_based: false,
                        precision: MarketPrecision {
                            amount: Some(amount_precision),
                            price: Some(price_precision),
                            cost: None,
                            base: Some(amount_precision),
                            quote: Some(price_precision),
                        },
                        limits: MarketLimits {
                            amount: crate::types::MinMax {
                                min: Decimal::from_str(&contract.min_order_size).ok(),
                                max: Decimal::from_str(&contract.max_order_size).ok(),
                            },
                            price: crate::types::MinMax { min: None, max: None },
                            cost: crate::types::MinMax { min: None, max: None },
                            leverage: crate::types::MinMax { min: None, max: None },
                        },
                        margin_modes: None,
                        created: None,
                        info: serde_json::to_value(&contract).unwrap_or_default(),
                    };

                    markets_by_id.insert(market_id, symbol.clone());
                    markets.insert(symbol, market);
                }
            }
        }

        // Parse spot symbols
        if let Some(spot_config) = symbols_data.spot_config {
            if let Some(spots) = spot_config.spot_symbols {
                for spot in spots {
                    if spot.status.to_uppercase() != "ONLINE" {
                        continue;
                    }

                    let base = spot.base_currency.to_uppercase();
                    let quote = spot.quote_currency.to_uppercase();
                    let symbol = format!("{base}/{quote}");
                    let market_id = spot.symbol.clone();

                    if markets.contains_key(&symbol) {
                        continue;
                    }

                    let tick_size = Decimal::from_str(&spot.tick_size).unwrap_or(Decimal::ZERO);
                    let step_size = Decimal::from_str(&spot.step_size).unwrap_or(Decimal::ZERO);

                    let price_precision = if tick_size > Decimal::ZERO {
                        tick_size.to_string().split('.').nth(1).map(|s| s.trim_end_matches('0').len() as i32).unwrap_or(0)
                    } else { 8 };

                    let amount_precision = if step_size > Decimal::ZERO {
                        step_size.to_string().split('.').nth(1).map(|s| s.trim_end_matches('0').len() as i32).unwrap_or(0)
                    } else { 8 };

                    let market = Market {
                        id: market_id.clone(),
                        lowercase_id: Some(market_id.to_lowercase()),
                        symbol: symbol.clone(),
                        base: base.clone(),
                        quote: quote.clone(),
                        base_id: spot.base_currency.clone(),
                        quote_id: spot.quote_currency.clone(),
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
                        taker: None,
                        maker: None,
                        percentage: true,
                        tier_based: false,
                        precision: MarketPrecision {
                            amount: Some(amount_precision),
                            price: Some(price_precision),
                            cost: None,
                            base: Some(amount_precision),
                            quote: Some(price_precision),
                        },
                        limits: MarketLimits {
                            amount: crate::types::MinMax {
                                min: Decimal::from_str(&spot.min_order_size).ok(),
                                max: None,
                            },
                            price: crate::types::MinMax { min: None, max: None },
                            cost: crate::types::MinMax { min: None, max: None },
                            leverage: crate::types::MinMax { min: None, max: None },
                        },
                        margin_modes: None,
                        created: None,
                        info: serde_json::to_value(&spot).unwrap_or_default(),
                    };

                    markets_by_id.insert(market_id, symbol.clone());
                    markets.insert(symbol, market);
                }
            }
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

        let response: ApexResponse<ApexTicker> = self.public_get("/v3/ticker", Some(params)).await?;

        let ticker_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Ticker data not found".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: ticker_data.high_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: ticker_data.low_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: ticker_data.open_price_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            close: ticker_data.last_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            last: ticker_data.last_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: ticker_data.price_change_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            percentage: None,
            average: None,
            base_volume: ticker_data.volume_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: ticker_data.turnover_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            index_price: ticker_data.index_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            mark_price: ticker_data.mark_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            info: serde_json::to_value(&ticker_data).unwrap_or_default(),
        })
    }

    async fn fetch_tickers(&self, _symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: ApexResponse<Vec<ApexTicker>> = self.public_get("/v3/tickers", None).await?;

        let tickers_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Tickers data not found".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();
        let mut tickers = HashMap::new();

        for ticker_data in tickers_data {
            let symbol = self.to_symbol(&ticker_data.symbol);

            let ticker = Ticker {
                symbol: symbol.clone(),
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                high: ticker_data.high_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                low: ticker_data.low_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                bid: None,
                bid_volume: None,
                ask: None,
                ask_volume: None,
                vwap: None,
                open: ticker_data.open_price_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                close: ticker_data.last_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                last: ticker_data.last_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                previous_close: None,
                change: ticker_data.price_change_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                percentage: None,
                average: None,
                base_volume: ticker_data.volume_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                quote_volume: ticker_data.turnover_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                index_price: ticker_data.index_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                mark_price: ticker_data.mark_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                info: serde_json::to_value(&ticker_data).unwrap_or_default(),
            };

            tickers.insert(symbol, ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        if let Some(lim) = limit {
            params.insert("limit".to_string(), lim.to_string());
        }

        let response: ApexResponse<ApexOrderBook> = self.public_get("/v3/depth", Some(params)).await?;

        let ob_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Order book data not found".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        let bids: Vec<OrderBookEntry> = ob_data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: Decimal::from_str(&b[0]).unwrap_or(Decimal::ZERO),
                    amount: Decimal::from_str(&b[1]).unwrap_or(Decimal::ZERO),
                })
            } else { None }
        }).collect();

        let asks: Vec<OrderBookEntry> = ob_data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: Decimal::from_str(&a[0]).unwrap_or(Decimal::ZERO),
                    amount: Decimal::from_str(&a[1]).unwrap_or(Decimal::ZERO),
                })
            } else { None }
        }).collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: ob_data.update_id,
            bids,
            asks,
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        if let Some(lim) = limit {
            params.insert("limit".to_string(), lim.to_string());
        }

        let response: ApexResponse<Vec<ApexTrade>> = self.public_get("/v3/trades", Some(params)).await?;

        let trades_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Trades data not found".into(),
        })?;

        let mut trades = Vec::new();

        for trade_data in trades_data {
            let price = Decimal::from_str(&trade_data.price).unwrap_or(Decimal::ZERO);
            let amount = Decimal::from_str(&trade_data.amount).unwrap_or(Decimal::ZERO);

            trades.push(Trade {
                id: trade_data.id.clone(),
                order: None,
                timestamp: Some(trade_data.timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(trade_data.timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(trade_data.side.to_lowercase()),
                taker_or_maker: None,
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

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.to_market_id(symbol);
        let interval = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {timeframe:?}"),
        })?;

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("interval".to_string(), interval.clone());
        if let Some(lim) = limit {
            params.insert("limit".to_string(), lim.to_string());
        }

        let response: ApexResponse<Vec<ApexKline>> = self.public_get("/v3/klines", Some(params)).await?;

        let klines = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Kline data not found".into(),
        })?;

        let ohlcv_list: Vec<OHLCV> = klines.iter().map(|k| {
            OHLCV {
                timestamp: k.timestamp,
                open: Decimal::from_str(&k.open).unwrap_or(Decimal::ZERO),
                high: Decimal::from_str(&k.high).unwrap_or(Decimal::ZERO),
                low: Decimal::from_str(&k.low).unwrap_or(Decimal::ZERO),
                close: Decimal::from_str(&k.close).unwrap_or(Decimal::ZERO),
                volume: Decimal::from_str(&k.volume).unwrap_or(Decimal::ZERO),
            }
        }).collect();

        Ok(ohlcv_list)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: ApexResponse<ApexBalance> = self.private_request(
            "GET",
            "/v3/account-balance",
            HashMap::new(),
        ).await?;

        let balance_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Balance data not found".into(),
        })?;

        let mut currencies = HashMap::new();

        // Add USDT balance from total equity
        if let Some(equity) = &balance_data.total_equity {
            if let Ok(total) = Decimal::from_str(equity) {
                let available = balance_data.available_balance.as_ref()
                    .and_then(|v| Decimal::from_str(v).ok())
                    .unwrap_or(Decimal::ZERO);
                let used = total - available;
                currencies.insert("USDT".to_string(), Balance {
                    free: Some(available),
                    used: Some(used),
                    total: Some(total),
                    debt: None,
                });
            }
        }

        // Add spot balances
        if let Some(ref spot_balances) = balance_data.spot_balances {
            for spot in spot_balances {
                let currency = spot.currency.to_uppercase();
                let free = Decimal::from_str(&spot.available_balance).unwrap_or(Decimal::ZERO);
                let used = Decimal::from_str(&spot.frozen_balance).unwrap_or(Decimal::ZERO);
                let total = free + used;
                if total > Decimal::ZERO {
                    currencies.insert(currency, Balance {
                        free: Some(free),
                        used: Some(used),
                        total: Some(total),
                        debt: None,
                    });
                }
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
            info: serde_json::to_value(&balance_data).unwrap_or_default(),
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
        let market_id = self.to_market_id(symbol);

        let type_str = match order_type {
            OrderType::Limit => "LIMIT",
            OrderType::Market => "MARKET",
            _ => return Err(CcxtError::BadRequest {
                message: "Unsupported order type".into(),
            }),
        };

        let side_str = match side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };

        let client_order_id = format!("ccxt-{}", Utc::now().timestamp_millis());

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("side".to_string(), side_str.to_string());
        params.insert("type".to_string(), type_str.to_string());
        params.insert("size".to_string(), amount.to_string());
        params.insert("clientOrderId".to_string(), client_order_id.clone());

        if let Some(p) = price {
            params.insert("price".to_string(), p.to_string());
        }

        let response: ApexResponse<ApexOrder> = self.private_request(
            "POST",
            "/v3/order",
            params,
        ).await?;

        let order_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Order creation failed".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Order {
            id: order_data.id.clone(),
            client_order_id: Some(client_order_id),
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
            info: serde_json::to_value(&order_data).unwrap_or_default(),
        })
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("id".to_string(), id.to_string());

        let response: ApexResponse<ApexOrder> = self.private_request(
            "POST",
            "/v3/delete-order",
            params,
        ).await?;

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
        let mut params = HashMap::new();
        params.insert("id".to_string(), id.to_string());

        let response: ApexResponse<ApexOrder> = self.private_request(
            "GET",
            "/v3/order",
            params,
        ).await?;

        let order_data = response.data.ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })?;

        let symbol = self.to_symbol(&order_data.symbol);
        let status = self.parse_order_status(&order_data.status);

        let amount = Decimal::from_str(&order_data.size).unwrap_or(Decimal::ZERO);
        let price = order_data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let filled = order_data.filled_size.as_ref().and_then(|f| Decimal::from_str(f).ok()).unwrap_or(Decimal::ZERO);
        let cost = order_data.filled_value.as_ref().and_then(|c| Decimal::from_str(c).ok());
        let remaining = if amount > filled { Some(amount - filled) } else { None };

        let side = match order_data.side.to_uppercase().as_str() {
            "BUY" => OrderSide::Buy,
            _ => OrderSide::Sell,
        };

        let order_type = match order_data.order_type.to_uppercase().as_str() {
            "MARKET" => OrderType::Market,
            _ => OrderType::Limit,
        };

        Ok(Order {
            id: order_data.id.clone(),
            client_order_id: order_data.client_order_id.clone(),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(order_data.created_at)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            timestamp: Some(order_data.created_at),
            last_trade_timestamp: None,
            last_update_timestamp: order_data.updated_at,
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
        })
    }

    async fn fetch_orders(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(sym) = symbol {
            params.insert("symbol".to_string(), self.to_market_id(sym));
        }
        if let Some(lim) = limit {
            params.insert("limit".to_string(), lim.to_string());
        }

        let response: ApexResponse<Vec<ApexOrder>> = self.private_request(
            "GET",
            "/v3/orders",
            params,
        ).await?;

        let orders_data = response.data.unwrap_or_default();
        self.parse_orders(orders_data)
    }

    async fn fetch_open_orders(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".to_string(), "OPEN".to_string());

        if let Some(sym) = symbol {
            params.insert("symbol".to_string(), self.to_market_id(sym));
        }
        if let Some(lim) = limit {
            params.insert("limit".to_string(), lim.to_string());
        }

        let response: ApexResponse<Vec<ApexOrder>> = self.private_request(
            "GET",
            "/v3/open-orders",
            params,
        ).await?;

        let orders_data = response.data.unwrap_or_default();
        self.parse_orders(orders_data)
    }

    async fn fetch_my_trades(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();

        if let Some(sym) = symbol {
            params.insert("symbol".to_string(), self.to_market_id(sym));
        }
        if let Some(lim) = limit {
            params.insert("limit".to_string(), lim.to_string());
        }

        let response: ApexResponse<Vec<ApexMyTrade>> = self.private_request(
            "GET",
            "/v3/fills",
            params,
        ).await?;

        let trades_data = response.data.unwrap_or_default();
        let mut trades = Vec::new();

        for trade_data in trades_data {
            let symbol = self.to_symbol(&trade_data.symbol);
            let price = Decimal::from_str(&trade_data.price).unwrap_or(Decimal::ZERO);
            let amount = Decimal::from_str(&trade_data.size).unwrap_or(Decimal::ZERO);

            trades.push(Trade {
                id: trade_data.id.clone(),
                order: Some(trade_data.order_id.clone()),
                timestamp: Some(trade_data.created_at),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(trade_data.created_at)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol,
                trade_type: None,
                side: Some(trade_data.side.to_lowercase()),
                taker_or_maker: Some(if trade_data.is_maker {
                    crate::types::TakerOrMaker::Maker
                } else {
                    crate::types::TakerOrMaker::Taker
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

impl Apex {
    fn parse_orders(&self, orders_data: Vec<ApexOrder>) -> CcxtResult<Vec<Order>> {
        let mut orders = Vec::new();

        for order_data in orders_data {
            let symbol = self.to_symbol(&order_data.symbol);
            let status = self.parse_order_status(&order_data.status);

            let amount = Decimal::from_str(&order_data.size).unwrap_or(Decimal::ZERO);
            let price = order_data.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
            let filled = order_data.filled_size.as_ref().and_then(|f| Decimal::from_str(f).ok()).unwrap_or(Decimal::ZERO);
            let cost = order_data.filled_value.as_ref().and_then(|c| Decimal::from_str(c).ok());
            let remaining = if amount > filled { Some(amount - filled) } else { None };

            let side = match order_data.side.to_uppercase().as_str() {
                "BUY" => OrderSide::Buy,
                _ => OrderSide::Sell,
            };

            let order_type = match order_data.order_type.to_uppercase().as_str() {
                "MARKET" => OrderType::Market,
                _ => OrderType::Limit,
            };

            orders.push(Order {
                id: order_data.id.clone(),
                client_order_id: order_data.client_order_id.clone(),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(order_data.created_at)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                timestamp: Some(order_data.created_at),
                last_trade_timestamp: None,
                last_update_timestamp: order_data.updated_at,
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
}

#[derive(Debug, Deserialize, Serialize)]
struct ApexMyTrade {
    id: String,
    #[serde(rename = "orderId")]
    order_id: String,
    symbol: String,
    side: String,
    price: String,
    size: String,
    #[serde(rename = "isMaker")]
    is_maker: bool,
    #[serde(rename = "createdAt")]
    created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apex_creation() {
        let config = ExchangeConfig::default();
        let exchange = Apex::new(config);
        assert!(exchange.is_ok());
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Apex::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Apex);
        assert_eq!(exchange.name(), "Apex");
    }

    #[test]
    fn test_to_market_id() {
        let config = ExchangeConfig::default();
        let exchange = Apex::new(config).unwrap();
        assert_eq!(exchange.to_market_id("BTC/USDT"), "BTCUSDT");
        assert_eq!(exchange.to_market_id("BTC/USDT:USDT"), "BTC-USDT");
    }

    #[test]
    fn test_urls() {
        let config = ExchangeConfig::default();
        let exchange = Apex::new(config).unwrap();
        let urls = exchange.urls();
        assert!(urls.www.is_some());
        assert!(!urls.doc.is_empty());
    }

    #[test]
    fn test_timeframes() {
        let config = ExchangeConfig::default();
        let exchange = Apex::new(config).unwrap();
        let timeframes = exchange.timeframes();
        assert!(timeframes.contains_key(&Timeframe::Minute1));
        assert!(timeframes.contains_key(&Timeframe::Hour1));
        assert!(timeframes.contains_key(&Timeframe::Day1));
    }

    #[test]
    fn test_parse_order_status() {
        let config = ExchangeConfig::default();
        let exchange = Apex::new(config).unwrap();
        assert_eq!(exchange.parse_order_status("NEW"), OrderStatus::Open);
        assert_eq!(exchange.parse_order_status("FILLED"), OrderStatus::Closed);
        assert_eq!(exchange.parse_order_status("CANCELED"), OrderStatus::Canceled);
        assert_eq!(exchange.parse_order_status("EXPIRED"), OrderStatus::Expired);
    }

    #[test]
    fn test_features() {
        let config = ExchangeConfig::default();
        let exchange = Apex::new(config).unwrap();
        let features = exchange.has();
        assert!(features.fetch_markets);
        assert!(features.fetch_ticker);
        assert!(features.fetch_order_book);
        assert!(features.swap);
    }
}
