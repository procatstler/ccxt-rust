//! DeepCoin Exchange Implementation
//!
//! DeepCoin은 2018년 설립된 글로벌 암호화폐 거래소로,
//! 현물 및 선물(USDT 무기한, 인버스) 거래를 지원합니다.
//! API는 OKX와 유사한 구조를 사용합니다.

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
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
    MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, Fee, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// DeepCoin 거래소
pub struct Deepcoin {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    passphrase: Option<String>,
}

// API Response structures
#[derive(Debug, Deserialize)]
struct DeepCoinResponse<T> {
    code: String,
    msg: Option<String>,
    data: Option<T>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeepCoinInstrument {
    #[serde(rename = "instId")]
    inst_id: String,
    #[serde(rename = "instType")]
    inst_type: String,
    #[serde(rename = "baseCcy")]
    base_ccy: Option<String>,
    #[serde(rename = "quoteCcy")]
    quote_ccy: Option<String>,
    #[serde(rename = "tickSz")]
    tick_sz: Option<String>,
    #[serde(rename = "lotSz")]
    lot_sz: Option<String>,
    #[serde(rename = "minSz")]
    min_sz: Option<String>,
    #[serde(rename = "maxSz")]
    max_sz: Option<String>,
    state: Option<String>,
    #[serde(rename = "ctVal")]
    ct_val: Option<String>,
    #[serde(rename = "ctMult")]
    ct_mult: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeepCoinTicker {
    #[serde(rename = "instId")]
    inst_id: String,
    last: Option<String>,
    #[serde(rename = "lastSz")]
    last_sz: Option<String>,
    #[serde(rename = "askPx")]
    ask_px: Option<String>,
    #[serde(rename = "askSz")]
    ask_sz: Option<String>,
    #[serde(rename = "bidPx")]
    bid_px: Option<String>,
    #[serde(rename = "bidSz")]
    bid_sz: Option<String>,
    open24h: Option<String>,
    high24h: Option<String>,
    low24h: Option<String>,
    #[serde(rename = "volCcy24h")]
    vol_ccy_24h: Option<String>,
    vol24h: Option<String>,
    ts: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeepCoinOrderBook {
    asks: Vec<Vec<String>>,
    bids: Vec<Vec<String>>,
    ts: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeepCoinCandle {
    ts: Option<String>,
    o: Option<String>,
    h: Option<String>,
    l: Option<String>,
    c: Option<String>,
    vol: Option<String>,
    #[serde(rename = "volCcy")]
    vol_ccy: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeepCoinTrade {
    #[serde(rename = "tradeId")]
    trade_id: Option<String>,
    #[serde(rename = "instId")]
    inst_id: Option<String>,
    px: Option<String>,
    sz: Option<String>,
    side: Option<String>,
    ts: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeepCoinBalance {
    ccy: String,
    #[serde(rename = "availBal")]
    avail_bal: Option<String>,
    #[serde(rename = "frozenBal")]
    frozen_bal: Option<String>,
    #[serde(rename = "bal")]
    bal: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeepCoinBalanceResponse {
    details: Option<Vec<DeepCoinBalance>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeepCoinOrderResponse {
    #[serde(rename = "ordId")]
    ord_id: String,
    #[serde(rename = "clOrdId")]
    cl_ord_id: Option<String>,
    #[serde(rename = "sCode")]
    s_code: Option<String>,
    #[serde(rename = "sMsg")]
    s_msg: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeepCoinOrderData {
    #[serde(rename = "ordId")]
    ord_id: String,
    #[serde(rename = "clOrdId")]
    cl_ord_id: Option<String>,
    #[serde(rename = "instId")]
    inst_id: Option<String>,
    #[serde(rename = "ordType")]
    ord_type: Option<String>,
    side: Option<String>,
    state: Option<String>,
    px: Option<String>,
    sz: Option<String>,
    #[serde(rename = "avgPx")]
    avg_px: Option<String>,
    #[serde(rename = "accFillSz")]
    acc_fill_sz: Option<String>,
    fee: Option<String>,
    #[serde(rename = "feeCcy")]
    fee_ccy: Option<String>,
    #[serde(rename = "cTime")]
    c_time: Option<String>,
    #[serde(rename = "uTime")]
    u_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeepCoinFillData {
    #[serde(rename = "tradeId")]
    trade_id: Option<String>,
    #[serde(rename = "ordId")]
    ord_id: Option<String>,
    #[serde(rename = "instId")]
    inst_id: Option<String>,
    #[serde(rename = "fillPx")]
    fill_px: Option<String>,
    #[serde(rename = "fillSz")]
    fill_sz: Option<String>,
    side: Option<String>,
    fee: Option<String>,
    #[serde(rename = "feeCcy")]
    fee_ccy: Option<String>,
    ts: Option<String>,
    #[serde(rename = "execType")]
    exec_type: Option<String>,
}

impl Deepcoin {
    const BASE_URL: &'static str = "https://api.deepcoin.com";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

    /// 새 DeepCoin 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        Self::with_passphrase(config, None)
    }

    /// Passphrase와 함께 DeepCoin 인스턴스 생성
    pub fn with_passphrase(config: ExchangeConfig, passphrase: Option<String>) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
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
            create_market_order: true,
            cancel_order: true,
            cancel_all_orders: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: false,
            fetch_deposit_address: false,
            fetch_positions: false,
            set_leverage: true,
            fetch_leverage: false,
            fetch_funding_rate: true,
            fetch_funding_rates: false,
            fetch_open_interest: false,
            fetch_liquidations: false,
            fetch_index_price: false,
            ws: false,
            watch_ticker: false,
            watch_tickers: false,
            watch_order_book: false,
            watch_trades: false,
            watch_ohlcv: false,
            watch_balance: false,
            watch_orders: false,
            watch_my_trades: false,
            watch_positions: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://www.deepcoin.com/favicon.ico".into()),
            api: api_urls,
            www: Some("https://www.deepcoin.com".into()),
            doc: vec!["https://www.deepcoin.com/en/docs".into()],
            fees: Some("https://www.deepcoin.com/en/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute3, "3m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1H".into());
        timeframes.insert(Timeframe::Hour2, "2H".into());
        timeframes.insert(Timeframe::Hour4, "4H".into());
        timeframes.insert(Timeframe::Hour6, "6H".into());
        timeframes.insert(Timeframe::Hour12, "12H".into());
        timeframes.insert(Timeframe::Day1, "1D".into());
        timeframes.insert(Timeframe::Week1, "1W".into());
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
            passphrase,
        })
    }

    fn create_signature(&self, timestamp: &str, method: &str, request_path: &str, body: &str) -> CcxtResult<String> {
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret not configured".into(),
        })?;

        // prehash = timestamp + method + requestPath + body
        let prehash = format!("{}{}{}{}", timestamp, method.to_uppercase(), request_path, body);

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError { message: e.to_string() })?;
        mac.update(prehash.as_bytes());
        let signature = mac.finalize().into_bytes();

        Ok(BASE64.encode(signature))
    }

    fn get_iso_timestamp() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }

    fn get_market_id(&self, symbol: &str) -> CcxtResult<String> {
        // Convert unified symbol to DeepCoin format
        // BTC/USDT -> BTC-USDT
        // BTC/USDT:USDT -> BTC-USDT-SWAP
        if symbol.contains(':') {
            let parts: Vec<&str> = symbol.split(':').collect();
            let base_quote = parts[0].replace('/', "-");
            Ok(format!("{base_quote}-SWAP"))
        } else {
            Ok(symbol.replace('/', "-"))
        }
    }

    fn parse_symbol(&self, inst_id: &str) -> String {
        // Convert DeepCoin format to unified symbol
        // BTC-USDT -> BTC/USDT
        // BTC-USDT-SWAP -> BTC/USDT:USDT
        if inst_id.ends_with("-SWAP") {
            let base_quote = inst_id.trim_end_matches("-SWAP");
            let parts: Vec<&str> = base_quote.split('-').collect();
            if parts.len() >= 2 {
                format!("{}/{}:{}", parts[0], parts[1], parts[1])
            } else {
                inst_id.replace('-', "/")
            }
        } else {
            inst_id.replace('-', "/")
        }
    }

    fn parse_order_status(&self, state: &str) -> OrderStatus {
        match state.to_lowercase().as_str() {
            "live" | "open" | "pending" => OrderStatus::Open,
            "partially_filled" | "partial" => OrderStatus::Open,
            "filled" | "complete" | "completed" => OrderStatus::Closed,
            "canceled" | "cancelled" => OrderStatus::Canceled,
            "expired" => OrderStatus::Expired,
            "rejected" => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        }
    }

    fn parse_order_type(&self, ord_type: &str) -> OrderType {
        match ord_type.to_lowercase().as_str() {
            "market" => OrderType::Market,
            "limit" => OrderType::Limit,
            "post_only" => OrderType::Limit,
            "fok" => OrderType::Limit,
            "ioc" => OrderType::Limit,
            "optimal_limit_ioc" => OrderType::Limit,
            _ => OrderType::Limit,
        }
    }

    fn parse_order_side(&self, side: &str) -> OrderSide {
        match side.to_lowercase().as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        }
    }

    fn count_decimals(value: &str) -> u8 {
        if let Some(dot_pos) = value.find('.') {
            let decimals = &value[dot_pos + 1..];
            decimals.trim_end_matches('0').len() as u8
        } else {
            0
        }
    }

    /// Private API request with authentication
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let timestamp = Self::get_iso_timestamp();
        let body_str = body.unwrap_or("");

        // Create signature: timestamp + method + path + body
        let prehash = format!("{}{}{}{}", timestamp, method.to_uppercase(), path, body_str);

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(prehash.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("DC-ACCESS-KEY".into(), api_key.to_string());
        headers.insert("DC-ACCESS-SIGN".into(), signature);
        headers.insert("DC-ACCESS-TIMESTAMP".into(), timestamp);
        headers.insert("Content-Type".into(), "application/json".into());

        if let Some(ref passphrase) = self.passphrase {
            headers.insert("DC-ACCESS-PASSPHRASE".into(), passphrase.clone());
        }

        match method.to_uppercase().as_str() {
            "GET" => {
                self.client.get(path, None, Some(headers)).await
            }
            "POST" => {
                let body_json: Option<serde_json::Value> = if body_str.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(body_str).unwrap_or(serde_json::json!({})))
                };
                self.client.post(path, body_json, Some(headers)).await
            }
            "DELETE" => {
                self.client.delete(path, None, Some(headers)).await
            }
            _ => Err(CcxtError::BadRequest {
                message: format!("Unsupported HTTP method: {method}"),
            }),
        }
    }
}

#[async_trait]
impl Exchange for Deepcoin {
    fn id(&self) -> ExchangeId {
        ExchangeId::Deepcoin
    }

    fn name(&self) -> &str {
        "DeepCoin"
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
        path: &str,
        _api: &str,
        method: &str,
        _params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        let mut request_headers = headers.unwrap_or_default();

        if let (Some(api_key), Some(secret)) = (self.config.api_key(), self.config.secret()) {
            let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
            let body_str = body.unwrap_or("");
            let pre_sign = format!("{}{}{}{}", timestamp, method.to_uppercase(), path, body_str);

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(pre_sign.as_bytes());
            let signature = BASE64.encode(mac.finalize().into_bytes());

            request_headers.insert("DC-ACCESS-KEY".into(), api_key.to_string());
            request_headers.insert("DC-ACCESS-SIGN".into(), signature);
            request_headers.insert("DC-ACCESS-TIMESTAMP".into(), timestamp);

            if let Some(ref passphrase) = self.passphrase {
                request_headers.insert("DC-ACCESS-PASSPHRASE".into(), passphrase.clone());
            }

            request_headers.insert("Content-Type".into(), "application/json".into());
        }

        SignedRequest {
            url: path.to_string(),
            method: method.to_string(),
            headers: request_headers,
            body: body.map(|s| s.to_string()),
        }
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        if !reload {
            let markets = self.markets.read().unwrap();
            if !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let market_list = self.fetch_markets().await?;
        let mut markets = self.markets.write().unwrap();

        for market in &market_list {
            markets.insert(market.symbol.clone(), market.clone());
        }

        Ok(markets.clone())
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        self.rate_limiter.throttle(1.0).await;

        let mut markets = Vec::new();

        // Fetch spot markets
        let mut spot_params = HashMap::new();
        spot_params.insert("instType".to_string(), "SPOT".to_string());

        let spot_response: DeepCoinResponse<Vec<DeepCoinInstrument>> = self
            .client
            .get("/deepcoin/market/instruments", Some(spot_params), None)
            .await?;

        if let Some(instruments) = spot_response.data {
            for inst in instruments {
                let base = inst.base_ccy.clone().unwrap_or_default();
                let quote = inst.quote_ccy.clone().unwrap_or_default();
                let symbol = format!("{base}/{quote}");

                let tick_sz = inst.tick_sz.as_deref().unwrap_or("0.00000001");
                let lot_sz = inst.lot_sz.as_deref().unwrap_or("0.00000001");

                let market = Market {
                    id: inst.inst_id.clone(),
                    lowercase_id: Some(inst.inst_id.to_lowercase()),
                    symbol: symbol.clone(),
                    base: base.clone(),
                    quote: quote.clone(),
                    base_id: base.clone(),
                    quote_id: quote.clone(),
                    active: inst.state.as_deref() == Some("live"),
                    market_type: MarketType::Spot,
                    spot: true,
                    margin: false,
                    swap: false,
                    future: false,
                    option: false,
                    contract: false,
                    settle: None,
                    settle_id: None,
                    contract_size: None,
                    linear: None,
                    inverse: None,
                    expiry: None,
                    expiry_datetime: None,
                    strike: None,
                    option_type: None,
                    taker: Some(Decimal::new(10, 4)), // 0.001 = 0.1%
                    maker: Some(Decimal::new(10, 4)),
                    precision: MarketPrecision {
                        amount: Some(Self::count_decimals(lot_sz) as i32),
                        price: Some(Self::count_decimals(tick_sz) as i32),
                        cost: None,
                        base: None,
                        quote: None,
                    },
                    limits: MarketLimits {
                        amount: crate::types::MinMax {
                            min: Decimal::from_str(inst.min_sz.as_deref().unwrap_or("0")).ok(),
                            max: Decimal::from_str(inst.max_sz.as_deref().unwrap_or("0")).ok(),
                        },
                        price: crate::types::MinMax { min: None, max: None },
                        cost: crate::types::MinMax { min: None, max: None },
                        leverage: crate::types::MinMax { min: None, max: None },
                    },
                    info: serde_json::to_value(&inst).unwrap_or_default(),
                    index: false,
                    sub_type: None,
                    margin_modes: None,
                    tier_based: false,
                    percentage: true,
                    created: None,
                };

                markets.push(market);
            }
        }

        // Fetch swap/perpetual markets
        let mut swap_params = HashMap::new();
        swap_params.insert("instType".to_string(), "SWAP".to_string());

        let swap_response: DeepCoinResponse<Vec<DeepCoinInstrument>> = self
            .client
            .get("/deepcoin/market/instruments", Some(swap_params), None)
            .await?;

        if let Some(instruments) = swap_response.data {
            for inst in instruments {
                let base = inst.base_ccy.clone().unwrap_or_default();
                let quote = inst.quote_ccy.clone().unwrap_or_default();
                let symbol = format!("{base}/{quote}:{quote}");
                let is_linear = !inst.inst_id.ends_with("-USD-SWAP");

                let tick_sz = inst.tick_sz.as_deref().unwrap_or("0.00000001");
                let lot_sz = inst.lot_sz.as_deref().unwrap_or("0.00000001");

                let market = Market {
                    id: inst.inst_id.clone(),
                    lowercase_id: Some(inst.inst_id.to_lowercase()),
                    symbol: symbol.clone(),
                    base: base.clone(),
                    quote: quote.clone(),
                    base_id: base.clone(),
                    quote_id: quote.clone(),
                    active: inst.state.as_deref() == Some("live"),
                    market_type: MarketType::Swap,
                    spot: false,
                    margin: false,
                    swap: true,
                    future: false,
                    option: false,
                    contract: true,
                    settle: Some(quote.clone()),
                    settle_id: Some(quote.clone()),
                    contract_size: inst.ct_val.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    linear: Some(is_linear),
                    inverse: Some(!is_linear),
                    expiry: None,
                    expiry_datetime: None,
                    strike: None,
                    option_type: None,
                    taker: Some(Decimal::new(5, 4)), // 0.0005 = 0.05%
                    maker: Some(Decimal::new(2, 4)), // 0.0002 = 0.02%
                    precision: MarketPrecision {
                        amount: Some(Self::count_decimals(lot_sz) as i32),
                        price: Some(Self::count_decimals(tick_sz) as i32),
                        cost: None,
                        base: None,
                        quote: None,
                    },
                    limits: MarketLimits {
                        amount: crate::types::MinMax {
                            min: Decimal::from_str(inst.min_sz.as_deref().unwrap_or("0")).ok(),
                            max: Decimal::from_str(inst.max_sz.as_deref().unwrap_or("0")).ok(),
                        },
                        price: crate::types::MinMax { min: None, max: None },
                        cost: crate::types::MinMax { min: None, max: None },
                        leverage: crate::types::MinMax { min: None, max: None },
                    },
                    info: serde_json::to_value(&inst).unwrap_or_default(),
                    index: false,
                    sub_type: None,
                    margin_modes: None,
                    tier_based: false,
                    percentage: true,
                    created: None,
                };

                markets.push(market);
            }
        }

        // Update internal markets cache
        {
            let mut markets_lock = self.markets.write().unwrap();
            let mut markets_by_id_lock = self.markets_by_id.write().unwrap();

            for market in &markets {
                markets_lock.insert(market.symbol.clone(), market.clone());
                markets_by_id_lock.insert(market.id.clone(), market.symbol.clone());
            }
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.rate_limiter.throttle(1.0).await;

        let inst_id = self.get_market_id(symbol)?;
        let mut params = HashMap::new();
        params.insert("instId".to_string(), inst_id);

        let response: DeepCoinResponse<Vec<DeepCoinTicker>> = self
            .client
            .get("/deepcoin/market/tickers", Some(params), None)
            .await?;

        let ticker_data = response
            .data
            .and_then(|mut v| v.pop())
            .ok_or_else(|| CcxtError::ExchangeError {
                message: format!("Ticker not found for {symbol}"),
            })?;

        let timestamp = ticker_data
            .ts
            .as_ref()
            .and_then(|ts| ts.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let last = ticker_data
            .last
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok());

        let open = ticker_data
            .open24h
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok());

        let change = match (last, open) {
            (Some(l), Some(o)) if o > Decimal::ZERO => Some(l - o),
            _ => None,
        };

        let percentage = match (change, open) {
            (Some(c), Some(o)) if o > Decimal::ZERO => {
                Some(c / o * Decimal::from(100))
            }
            _ => None,
        };

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: ticker_data.high24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: ticker_data.low24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: ticker_data.bid_px.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: ticker_data.bid_sz.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask: ticker_data.ask_px.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: ticker_data.ask_sz.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            vwap: None,
            open,
            close: last,
            last,
            previous_close: None,
            change,
            percentage,
            average: None,
            base_volume: ticker_data.vol24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: ticker_data.vol_ccy_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&ticker_data).unwrap_or_default(),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.rate_limiter.throttle(1.0).await;

        let mut params = HashMap::new();
        params.insert("instType".to_string(), "SPOT".to_string());

        let response: DeepCoinResponse<Vec<DeepCoinTicker>> = self
            .client
            .get("/deepcoin/market/tickers", Some(params), None)
            .await?;

        let mut tickers = HashMap::new();

        if let Some(ticker_list) = response.data {
            for ticker_data in ticker_list {
                let symbol = self.parse_symbol(&ticker_data.inst_id);

                // Filter by symbols if provided
                if let Some(syms) = symbols {
                    if !syms.iter().any(|s| *s == symbol) {
                        continue;
                    }
                }

                let timestamp = ticker_data
                    .ts
                    .as_ref()
                    .and_then(|ts| ts.parse::<i64>().ok())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let last = ticker_data
                    .last
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok());

                let open = ticker_data
                    .open24h
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok());

                let change = match (last, open) {
                    (Some(l), Some(o)) if o > Decimal::ZERO => Some(l - o),
                    _ => None,
                };

                let percentage = match (change, open) {
                    (Some(c), Some(o)) if o > Decimal::ZERO => {
                        Some(c / o * Decimal::from(100))
                    }
                    _ => None,
                };

                tickers.insert(symbol.clone(), Ticker {
                    symbol: symbol.clone(),
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    high: ticker_data.high24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    low: ticker_data.low24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    bid: ticker_data.bid_px.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    bid_volume: ticker_data.bid_sz.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    ask: ticker_data.ask_px.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    ask_volume: ticker_data.ask_sz.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    vwap: None,
                    open,
                    close: last,
                    last,
                    previous_close: None,
                    change,
                    percentage,
                    average: None,
                    base_volume: ticker_data.vol24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    quote_volume: ticker_data.vol_ccy_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    index_price: None,
                    mark_price: None,
                    info: serde_json::to_value(&ticker_data).unwrap_or_default(),
                });
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.rate_limiter.throttle(1.0).await;

        let inst_id = self.get_market_id(symbol)?;
        let sz = limit.unwrap_or(20).to_string();
        let mut params = HashMap::new();
        params.insert("instId".to_string(), inst_id);
        params.insert("sz".to_string(), sz);

        let response: DeepCoinResponse<Vec<DeepCoinOrderBook>> = self
            .client
            .get("/deepcoin/market/books", Some(params), None)
            .await?;

        let book_data = response
            .data
            .and_then(|mut v| v.pop())
            .ok_or_else(|| CcxtError::ExchangeError {
                message: format!("Order book not found for {symbol}"),
            })?;

        let timestamp = book_data
            .ts
            .as_ref()
            .and_then(|ts| ts.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let asks: Vec<OrderBookEntry> = book_data
            .asks
            .iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&entry[0]).unwrap_or(Decimal::ZERO),
                        amount: Decimal::from_str(&entry[1]).unwrap_or(Decimal::ZERO),
                    })
                } else {
                    None
                }
            })
            .collect();

        let bids: Vec<OrderBookEntry> = book_data
            .bids
            .iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&entry[0]).unwrap_or(Decimal::ZERO),
                        amount: Decimal::from_str(&entry[1]).unwrap_or(Decimal::ZERO),
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        self.rate_limiter.throttle(1.0).await;

        let inst_id = self.get_market_id(symbol)?;
        let limit_str = limit.unwrap_or(100).to_string();
        let mut params = HashMap::new();
        params.insert("instId".to_string(), inst_id);
        params.insert("limit".to_string(), limit_str);

        let response: DeepCoinResponse<Vec<DeepCoinTrade>> = self
            .client
            .get("/deepcoin/market/trades", Some(params), None)
            .await?;

        let mut trades = Vec::new();

        if let Some(trade_list) = response.data {
            for trade_data in trade_list {
                let timestamp = trade_data
                    .ts
                    .as_ref()
                    .and_then(|ts| ts.parse::<i64>().ok())
                    .unwrap_or(0);

                let price = trade_data
                    .px
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok())
                    .unwrap_or(Decimal::ZERO);

                let amount = trade_data
                    .sz
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok())
                    .unwrap_or(Decimal::ZERO);

                trades.push(Trade {
                    id: trade_data.trade_id.unwrap_or_default(),
                    order: None,
                    info: serde_json::json!({}),
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    taker_or_maker: None,
                    side: trade_data.side.clone(),
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: vec![],
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
        self.rate_limiter.throttle(1.0).await;

        let inst_id = self.get_market_id(symbol)?;
        let bar = self
            .timeframes
            .get(&timeframe)
            .cloned()
            .unwrap_or_else(|| "1m".into());

        let limit_str = limit.unwrap_or(100).to_string();
        let mut params = HashMap::new();
        params.insert("instId".to_string(), inst_id);
        params.insert("bar".to_string(), bar);
        params.insert("limit".to_string(), limit_str);

        if let Some(s) = since {
            params.insert("after".to_string(), s.to_string());
        }

        let response: DeepCoinResponse<Vec<Vec<String>>> = self
            .client
            .get("/deepcoin/market/candles", Some(params), None)
            .await?;

        let mut ohlcvs = Vec::new();

        if let Some(candle_list) = response.data {
            for candle in candle_list {
                if candle.len() >= 6 {
                    let timestamp = candle[0].parse::<i64>().unwrap_or(0);

                    ohlcvs.push(OHLCV {
                        timestamp,
                        open: Decimal::from_str(&candle[1]).unwrap_or(Decimal::ZERO),
                        high: Decimal::from_str(&candle[2]).unwrap_or(Decimal::ZERO),
                        low: Decimal::from_str(&candle[3]).unwrap_or(Decimal::ZERO),
                        close: Decimal::from_str(&candle[4]).unwrap_or(Decimal::ZERO),
                        volume: Decimal::from_str(&candle[5]).unwrap_or(Decimal::ZERO),
                    });
                }
            }
        }

        Ok(ohlcvs)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: DeepCoinResponse<Vec<DeepCoinBalanceResponse>> = self
            .private_request("GET", "/deepcoin/account/balances", None)
            .await?;

        let mut balances = Balances {
            info: serde_json::json!({}),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies: HashMap::new(),
        };

        if let Some(data_list) = response.data {
            for balance_resp in data_list {
                if let Some(details) = balance_resp.details {
                    for bal in details {
                        let currency = bal.ccy.clone();
                        let free = bal
                            .avail_bal
                            .as_ref()
                            .and_then(|v| Decimal::from_str(v).ok())
                            .unwrap_or(Decimal::ZERO);
                        let used = bal
                            .frozen_bal
                            .as_ref()
                            .and_then(|v| Decimal::from_str(v).ok())
                            .unwrap_or(Decimal::ZERO);
                        let total = bal
                            .bal
                            .as_ref()
                            .and_then(|v| Decimal::from_str(v).ok())
                            .unwrap_or(free + used);

                        if total > Decimal::ZERO {
                            balances.currencies.insert(
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
                }
            }
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
        let inst_id = self.get_market_id(symbol)?;
        let side_str = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };
        let ord_type = match order_type {
            OrderType::Market => "market",
            OrderType::Limit => "limit",
            _ => "limit",
        };

        let mut body = serde_json::json!({
            "instId": inst_id,
            "side": side_str,
            "ordType": ord_type,
            "sz": amount.to_string(),
        });

        if order_type == OrderType::Limit {
            if let Some(px) = price {
                body["px"] = serde_json::json!(px.to_string());
            }
        }

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let response: DeepCoinResponse<Vec<DeepCoinOrderResponse>> = self
            .private_request("POST", "/deepcoin/trade/order", Some(&body_str))
            .await?;

        let order_resp = response
            .data
            .and_then(|mut v| v.pop())
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "Failed to create order".into(),
            })?;

        // Check for order errors
        if let Some(code) = &order_resp.s_code {
            if code != "0" {
                return Err(CcxtError::ExchangeError {
                    message: order_resp.s_msg.unwrap_or_else(|| "Order failed".into()),
                });
            }
        }

        let timestamp = Utc::now().timestamp_millis();

        Ok(Order {
            id: order_resp.ord_id.clone(),
            client_order_id: order_resp.cl_ord_id.clone(),
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
            info: serde_json::to_value(&order_resp).unwrap_or_default(),
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let inst_id = self.get_market_id(symbol)?;

        let body = serde_json::json!({
            "instId": inst_id,
            "ordId": id,
        });

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let response: DeepCoinResponse<Vec<DeepCoinOrderResponse>> = self
            .private_request("POST", "/deepcoin/trade/cancel-order", Some(&body_str))
            .await?;

        let order_resp = response
            .data
            .and_then(|mut v| v.pop())
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "Failed to cancel order".into(),
            })?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Order {
            id: order_resp.ord_id.clone(),
            client_order_id: order_resp.cl_ord_id.clone(),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            timestamp: Some(timestamp),
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
            info: serde_json::to_value(&order_resp).unwrap_or_default(),
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let inst_id = self.get_market_id(symbol)?;
        let path = format!("/deepcoin/trade/orderByID?instId={inst_id}&ordId={id}");

        let response: DeepCoinResponse<Vec<DeepCoinOrderData>> = self
            .private_request("GET", &path, None)
            .await?;

        let order_data = response
            .data
            .and_then(|mut v| v.pop())
            .ok_or_else(|| CcxtError::OrderNotFound {
                order_id: id.to_string(),
            })?;
        let timestamp = order_data
            .c_time
            .as_ref()
            .and_then(|ts| ts.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let amount = order_data
            .sz
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or(Decimal::ZERO);

        let filled = order_data
            .acc_fill_sz
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or(Decimal::ZERO);

        let price = order_data.px.as_ref().and_then(|v| Decimal::from_str(v).ok());
        let avg_price = order_data.avg_px.as_ref().and_then(|v| Decimal::from_str(v).ok());

        let fee = order_data.fee.as_ref().and_then(|f| {
            let cost = Decimal::from_str(f).ok()?;
            Some(Fee {
                currency: order_data.fee_ccy.clone(),
                cost: Some(cost.abs()),
                rate: None,
            })
        });

        Ok(Order {
            id: order_data.ord_id.clone(),
            client_order_id: order_data.cl_ord_id.clone(),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            timestamp: Some(timestamp),
            last_trade_timestamp: order_data
                .u_time
                .as_ref()
                .and_then(|ts| ts.parse::<i64>().ok()),
            last_update_timestamp: None,
            status: order_data
                .state
                .as_ref()
                .map(|s| self.parse_order_status(s))
                .unwrap_or(OrderStatus::Open),
            symbol: symbol.to_string(),
            order_type: order_data
                .ord_type
                .as_ref()
                .map(|t| self.parse_order_type(t))
                .unwrap_or(OrderType::Limit),
            time_in_force: None,
            side: order_data
                .side
                .as_ref()
                .map(|s| self.parse_order_side(s))
                .unwrap_or(OrderSide::Buy),
            price,
            average: avg_price,
            amount,
            filled,
            remaining: Some(amount - filled),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: avg_price.map(|ap| ap * filled),
            trades: vec![],
            reduce_only: None,
            post_only: None,
            fee,
            fees: vec![],
            info: serde_json::to_value(&order_data).unwrap_or_default(),
        })
    }

    async fn fetch_orders(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut query_parts = Vec::new();

        if let Some(s) = symbol {
            let inst_id = self.get_market_id(s)?;
            query_parts.push(format!("instId={inst_id}"));
        }

        if let Some(l) = limit {
            query_parts.push(format!("limit={l}"));
        }

        let path = if query_parts.is_empty() {
            "/deepcoin/trade/orders-history".to_string()
        } else {
            format!("/deepcoin/trade/orders-history?{}", query_parts.join("&"))
        };

        let response: DeepCoinResponse<Vec<DeepCoinOrderData>> = self
            .private_request("GET", &path, None)
            .await?;

        let mut orders = Vec::new();

        if let Some(order_list) = response.data {
            for order_data in order_list {
                let sym = if let Some(ref inst) = order_data.inst_id {
                    self.parse_symbol(inst)
                } else {
                    symbol.unwrap_or_default().to_string()
                };

                let timestamp = order_data
                    .c_time
                    .as_ref()
                    .and_then(|ts| ts.parse::<i64>().ok())
                    .unwrap_or(0);

                let amount = order_data
                    .sz
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok())
                    .unwrap_or(Decimal::ZERO);

                let filled = order_data
                    .acc_fill_sz
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok())
                    .unwrap_or(Decimal::ZERO);

                orders.push(Order {
                    id: order_data.ord_id.clone(),
                    client_order_id: order_data.cl_ord_id.clone(),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    timestamp: Some(timestamp),
                    last_trade_timestamp: order_data
                        .u_time
                        .as_ref()
                        .and_then(|ts| ts.parse::<i64>().ok()),
                    last_update_timestamp: None,
                    status: order_data
                        .state
                        .as_ref()
                        .map(|s| self.parse_order_status(s))
                        .unwrap_or(OrderStatus::Open),
                    symbol: sym,
                    order_type: order_data
                        .ord_type
                        .as_ref()
                        .map(|t| self.parse_order_type(t))
                        .unwrap_or(OrderType::Limit),
                    time_in_force: None,
                    side: order_data
                        .side
                        .as_ref()
                        .map(|s| self.parse_order_side(s))
                        .unwrap_or(OrderSide::Buy),
                    price: order_data.px.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    average: order_data.avg_px.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    amount,
                    filled,
                    remaining: Some(amount - filled),
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
        }

        Ok(orders)
    }

    async fn fetch_open_orders(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut query_parts = Vec::new();

        if let Some(s) = symbol {
            let inst_id = self.get_market_id(s)?;
            query_parts.push(format!("instId={inst_id}"));
        }

        if let Some(l) = limit {
            query_parts.push(format!("limit={l}"));
        }

        let path = if query_parts.is_empty() {
            "/deepcoin/trade/v2/orders-pending".to_string()
        } else {
            format!("/deepcoin/trade/v2/orders-pending?{}", query_parts.join("&"))
        };

        let response: DeepCoinResponse<Vec<DeepCoinOrderData>> = self
            .private_request("GET", &path, None)
            .await?;

        let mut orders = Vec::new();

        if let Some(order_list) = response.data {
            for order_data in order_list {
                let sym = if let Some(ref inst) = order_data.inst_id {
                    self.parse_symbol(inst)
                } else {
                    symbol.unwrap_or_default().to_string()
                };

                let timestamp = order_data
                    .c_time
                    .as_ref()
                    .and_then(|ts| ts.parse::<i64>().ok())
                    .unwrap_or(0);

                let amount = order_data
                    .sz
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok())
                    .unwrap_or(Decimal::ZERO);

                let filled = order_data
                    .acc_fill_sz
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok())
                    .unwrap_or(Decimal::ZERO);

                orders.push(Order {
                    id: order_data.ord_id.clone(),
                    client_order_id: order_data.cl_ord_id.clone(),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    timestamp: Some(timestamp),
                    last_trade_timestamp: order_data
                        .u_time
                        .as_ref()
                        .and_then(|ts| ts.parse::<i64>().ok()),
                    last_update_timestamp: None,
                    status: OrderStatus::Open,
                    symbol: sym,
                    order_type: order_data
                        .ord_type
                        .as_ref()
                        .map(|t| self.parse_order_type(t))
                        .unwrap_or(OrderType::Limit),
                    time_in_force: None,
                    side: order_data
                        .side
                        .as_ref()
                        .map(|s| self.parse_order_side(s))
                        .unwrap_or(OrderSide::Buy),
                    price: order_data.px.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    average: order_data.avg_px.as_ref().and_then(|v| Decimal::from_str(v).ok()),
                    amount,
                    filled,
                    remaining: Some(amount - filled),
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
        }

        Ok(orders)
    }

    async fn fetch_closed_orders(&self, symbol: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let orders = self.fetch_orders(symbol, since, limit).await?;
        Ok(orders
            .into_iter()
            .filter(|o| matches!(o.status, OrderStatus::Closed | OrderStatus::Canceled))
            .collect())
    }

    async fn fetch_my_trades(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let mut query_parts = vec!["instType=SPOT".to_string()];

        if let Some(s) = symbol {
            let inst_id = self.get_market_id(s)?;
            query_parts.push(format!("instId={inst_id}"));
        }

        if let Some(l) = limit {
            query_parts.push(format!("limit={l}"));
        }

        let path = format!("/deepcoin/trade/fills?{}", query_parts.join("&"));

        let response: DeepCoinResponse<Vec<DeepCoinFillData>> = self
            .private_request("GET", &path, None)
            .await?;

        let mut trades = Vec::new();

        if let Some(fill_list) = response.data {
            for fill_data in fill_list {
                let sym = if let Some(ref inst) = fill_data.inst_id {
                    self.parse_symbol(inst)
                } else {
                    symbol.unwrap_or_default().to_string()
                };

                let timestamp = fill_data
                    .ts
                    .as_ref()
                    .and_then(|ts| ts.parse::<i64>().ok())
                    .unwrap_or(0);

                let price = fill_data
                    .fill_px
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok())
                    .unwrap_or(Decimal::ZERO);

                let amount = fill_data
                    .fill_sz
                    .as_ref()
                    .and_then(|v| Decimal::from_str(v).ok())
                    .unwrap_or(Decimal::ZERO);

                let fee = fill_data.fee.as_ref().and_then(|f| {
                    let cost = Decimal::from_str(f).ok()?;
                    Some(Fee {
                        currency: fill_data.fee_ccy.clone(),
                        cost: Some(cost.abs()),
                        rate: None,
                    })
                });

                trades.push(Trade {
                    id: fill_data.trade_id.clone().unwrap_or_default(),
                    order: fill_data.ord_id.clone(),
                    info: serde_json::json!({}),
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: sym,
                    trade_type: None,
                    taker_or_maker: fill_data.exec_type.as_ref().and_then(|e| {
                        match e.as_str() {
                            "M" => Some(crate::types::TakerOrMaker::Maker),
                            "T" => Some(crate::types::TakerOrMaker::Taker),
                            _ => None,
                        }
                    }),
                    side: fill_data.side.clone(),
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee,
                    fees: vec![],
                });
            }
        }

        Ok(trades)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deepcoin_creation() {
        let config = ExchangeConfig::default();
        let exchange = Deepcoin::new(config);
        assert!(exchange.is_ok());
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Deepcoin::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Deepcoin);
        assert_eq!(exchange.name(), "DeepCoin");
    }

    #[test]
    fn test_urls() {
        let config = ExchangeConfig::default();
        let exchange = Deepcoin::new(config).unwrap();
        let urls = exchange.urls();
        assert!(urls.www.is_some());
        assert!(!urls.doc.is_empty());
    }

    #[test]
    fn test_get_market_id() {
        let config = ExchangeConfig::default();
        let exchange = Deepcoin::new(config).unwrap();

        assert_eq!(exchange.get_market_id("BTC/USDT").unwrap(), "BTC-USDT");
        assert_eq!(exchange.get_market_id("BTC/USDT:USDT").unwrap(), "BTC-USDT-SWAP");
    }

    #[test]
    fn test_parse_symbol() {
        let config = ExchangeConfig::default();
        let exchange = Deepcoin::new(config).unwrap();

        assert_eq!(exchange.parse_symbol("BTC-USDT"), "BTC/USDT");
        assert_eq!(exchange.parse_symbol("BTC-USDT-SWAP"), "BTC/USDT:USDT");
    }

    #[test]
    fn test_parse_order_status() {
        let config = ExchangeConfig::default();
        let exchange = Deepcoin::new(config).unwrap();

        assert_eq!(exchange.parse_order_status("live"), OrderStatus::Open);
        assert_eq!(exchange.parse_order_status("filled"), OrderStatus::Closed);
        assert_eq!(exchange.parse_order_status("canceled"), OrderStatus::Canceled);
    }

    #[test]
    fn test_parse_order_type() {
        let config = ExchangeConfig::default();
        let exchange = Deepcoin::new(config).unwrap();

        assert_eq!(exchange.parse_order_type("market"), OrderType::Market);
        assert_eq!(exchange.parse_order_type("limit"), OrderType::Limit);
    }

    #[test]
    fn test_parse_order_side() {
        let config = ExchangeConfig::default();
        let exchange = Deepcoin::new(config).unwrap();

        assert_eq!(exchange.parse_order_side("buy"), OrderSide::Buy);
        assert_eq!(exchange.parse_order_side("sell"), OrderSide::Sell);
    }

    #[test]
    fn test_count_decimals() {
        assert_eq!(Deepcoin::count_decimals("0.00000001"), 8);
        assert_eq!(Deepcoin::count_decimals("0.001"), 3);
        assert_eq!(Deepcoin::count_decimals("1"), 0);
        assert_eq!(Deepcoin::count_decimals("1.0"), 0);
    }

    #[test]
    fn test_timeframes() {
        let config = ExchangeConfig::default();
        let exchange = Deepcoin::new(config).unwrap();
        let timeframes = exchange.timeframes();

        assert!(timeframes.contains_key(&Timeframe::Minute1));
        assert!(timeframes.contains_key(&Timeframe::Hour1));
        assert!(timeframes.contains_key(&Timeframe::Day1));
    }
}
