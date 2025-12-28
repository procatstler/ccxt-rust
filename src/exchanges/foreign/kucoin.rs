//! Kucoin Exchange Implementation
//!
//! Kucoin API 구현

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::RwLock as TokioRwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, DepositAddress, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls,
    FundingRate, FundingRateHistory, Leverage, Liquidation, Market, MarketLimits, MarketPrecision,
    MarketType, MarginMode, MarginModeInfo, MarginModification, MarginModificationType, OpenInterest,
    Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Position, PositionMode,
    PositionModeInfo, PositionSide, SignedRequest, Ticker, Timeframe, Trade, Transaction,
    TransactionStatus, TransactionType, TransferEntry, WsExchange, WsMessage, OHLCV,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use super::kucoin_ws::KucoinWs;

type HmacSha256 = Hmac<Sha256>;

/// Kucoin 거래소
pub struct Kucoin {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    ws_client: Arc<TokioRwLock<KucoinWs>>,
}

impl Kucoin {
    const BASE_URL: &'static str = "https://api.kucoin.com";
    const RATE_LIMIT_MS: u64 = 100;

    /// 새 Kucoin 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: true,
            swap: true,
            future: true,
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
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            fetch_positions: true,
            set_leverage: true,
            fetch_leverage: true,
            fetch_funding_rate: true,
            fetch_funding_rates: true,
            fetch_open_interest: true,
            fetch_liquidations: true,
            fetch_index_price: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/51840849/87295558-132aaf80-c50e-11ea-9801-a2fb0c57c799.jpg".into()),
            api: api_urls,
            www: Some("https://www.kucoin.com".into()),
            doc: vec!["https://docs.kucoin.com".into()],
            fees: Some("https://www.kucoin.com/fee".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1min".into());
        timeframes.insert(Timeframe::Minute3, "3min".into());
        timeframes.insert(Timeframe::Minute5, "5min".into());
        timeframes.insert(Timeframe::Minute15, "15min".into());
        timeframes.insert(Timeframe::Minute30, "30min".into());
        timeframes.insert(Timeframe::Hour1, "1hour".into());
        timeframes.insert(Timeframe::Hour2, "2hour".into());
        timeframes.insert(Timeframe::Hour4, "4hour".into());
        timeframes.insert(Timeframe::Hour6, "6hour".into());
        timeframes.insert(Timeframe::Hour8, "8hour".into());
        timeframes.insert(Timeframe::Hour12, "12hour".into());
        timeframes.insert(Timeframe::Day1, "1day".into());
        timeframes.insert(Timeframe::Week1, "1week".into());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            ws_client: Arc::new(TokioRwLock::new(KucoinWs::new())),
        })
    }

    /// 심볼 변환 (BTC/USDT -> BTC-USDT)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace('/', "-")
    }

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// 비공개 API 호출
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
        let api_secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;
        let passphrase = self.config.password().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API passphrase required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();

        let (query_string, body) = if method == "GET" || method == "DELETE" {
            let query = if params.is_empty() {
                String::new()
            } else {
                let qs = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");
                format!("?{qs}")
            };
            (query, String::new())
        } else {
            let body = serde_json::to_string(&params).unwrap_or_default();
            (String::new(), body)
        };

        let full_path = format!("{path}{query_string}");

        // String to sign: timestamp + method + endpoint + body
        let sign_string = format!("{timestamp}{method}{full_path}{body}");

        // HMAC-SHA256 signature
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError { message: "Invalid secret".into() })?;
        mac.update(sign_string.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        // Sign the passphrase
        let mut passphrase_mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError { message: "Invalid secret".into() })?;
        passphrase_mac.update(passphrase.as_bytes());
        let signed_passphrase = BASE64.encode(passphrase_mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("KC-API-KEY".into(), api_key.to_string());
        headers.insert("KC-API-SIGN".into(), signature);
        headers.insert("KC-API-TIMESTAMP".into(), timestamp);
        headers.insert("KC-API-PASSPHRASE".into(), signed_passphrase);
        headers.insert("KC-API-KEY-VERSION".into(), "2".into());
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => {
                let query_params = if params.is_empty() { None } else { Some(params) };
                self.client.get(path, query_params, Some(headers)).await
            }
            "POST" => {
                let json_body: Option<serde_json::Value> = if body.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&body).unwrap_or_default())
                };
                self.client.post(path, json_body, Some(headers)).await
            }
            "DELETE" => {
                let query_params = if params.is_empty() { None } else { Some(params) };
                self.client.delete(path, query_params, Some(headers)).await
            }
            _ => Err(CcxtError::BadRequest { message: "Invalid HTTP method".into() }),
        }
    }

    fn parse_ticker(&self, data: &KucoinTicker, symbol: &str) -> Ticker {
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            ),
            high: data.high.as_ref().and_then(|v| v.parse().ok()),
            low: data.low.as_ref().and_then(|v| v.parse().ok()),
            bid: data.buy.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: None,
            ask: data.sell.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.change_price.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.change_rate.as_ref().and_then(|v| {
                v.parse::<Decimal>().ok().map(|d| d * Decimal::from(100))
            }),
            average: data.average_price.as_ref().and_then(|v| v.parse().ok()),
            base_volume: data.vol.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.vol_value.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order(&self, data: &KucoinOrder, symbol: &str) -> Order {
        let timestamp = data.created_at.unwrap_or_else(|| Utc::now().timestamp_millis());

        let amount: Decimal = data.size.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data.deal_size.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let price: Option<Decimal> = data.price.as_ref().and_then(|v| v.parse().ok());
        let average: Option<Decimal> = if filled > Decimal::ZERO {
            data.deal_funds.as_ref()
                .and_then(|v| v.parse::<Decimal>().ok())
                .map(|funds| funds / filled)
        } else {
            None
        };

        let status = if data.is_active.unwrap_or(false) {
            OrderStatus::Open
        } else if data.cancel_exist.unwrap_or(false) {
            OrderStatus::Canceled
        } else {
            OrderStatus::Closed
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        Order {
            id: data.id.clone().unwrap_or_default(),
            client_order_id: data.client_oid.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average,
            amount,
            filled,
            remaining: Some(amount - filled),
            stop_price: data.stop_price.as_ref().and_then(|v| v.parse().ok()),
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: data.deal_funds.as_ref().and_then(|v| v.parse().ok()),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: data.post_only,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_balance(&self, accounts: &[KucoinAccount]) -> Balances {
        let mut balances = Balances::new();

        for account in accounts {
            if account.account_type.as_deref() != Some("trade") {
                continue;
            }

            let currency = account.currency.clone();
            let available: Decimal = account.available.as_ref()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default();
            let holds: Decimal = account.holds.as_ref()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default();

            balances.add(currency, Balance::new(available, holds));
        }

        balances
    }

    /// 입금 내역 파싱
    fn parse_deposit(&self, data: &KucoinDeposit) -> Transaction {
        let status = match data.status.as_deref() {
            Some("SUCCESS") => TransactionStatus::Ok,
            Some("FAILURE") => TransactionStatus::Failed,
            _ => TransactionStatus::Pending,
        };

        let timestamp = data.created_at;
        let amount: Decimal = data.amount.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        Transaction {
            id: data.id.clone().unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: data.updated_at,
            tx_type: TransactionType::Deposit,
            currency: data.currency.clone(),
            network: data.chain.clone(),
            amount,
            status,
            address: data.address.clone(),
            tag: data.memo.clone(),
            txid: data.wallet_tx_id.clone(),
            fee: None,
            internal: data.is_inner,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 출금 내역 파싱
    fn parse_withdrawal(&self, data: &KucoinWithdrawal) -> Transaction {
        let status = match data.status.as_deref() {
            Some("SUCCESS") => TransactionStatus::Ok,
            Some("FAILURE") => TransactionStatus::Failed,
            _ => TransactionStatus::Pending,
        };

        let timestamp = data.created_at;
        let amount: Decimal = data.amount.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let fee_amount: Option<Decimal> = data.fee.as_ref().and_then(|v| v.parse().ok());

        Transaction {
            id: data.id.clone().unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: data.updated_at,
            tx_type: TransactionType::Withdrawal,
            currency: data.currency.clone(),
            network: data.chain.clone(),
            amount,
            status,
            address: data.address.clone(),
            tag: data.memo.clone(),
            txid: data.wallet_tx_id.clone(),
            fee: fee_amount.map(|cost| crate::types::Fee {
                cost: Some(cost),
                currency: Some(data.currency.clone()),
                rate: None,
            }),
            internal: data.is_inner,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 선물 API 호출 (퍼블릭)
    async fn futures_public_get<T: serde::de::DeserializeOwned + Default>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        // Create a separate client for futures API
        let futures_client = HttpClient::new("https://api-futures.kucoin.com", &self.config)?;

        let response: KucoinResponse<T> = futures_client.get(path, params, None).await?;

        if response.code != "200000" {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_else(|| format!("Kucoin error: {}", response.code)),
            });
        }

        Ok(response.data.unwrap_or_default())
    }

    /// 선물 API 호출 (프라이빗)
    async fn futures_private_request<T: serde::de::DeserializeOwned + Default>(
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

        let (query_params, body_str) = if method == "GET" {
            (Some(params.clone()), String::new())
        } else {
            (None, serde_json::to_string(&params).unwrap_or_else(|_| "{}".into()))
        };

        let sign_path = if method == "GET" && !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{path}?{query}")
        } else {
            path.to_string()
        };

        let sign_str = format!("{}{}{}{}", timestamp, method.to_uppercase(), sign_path, body_str);

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(sign_str.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        // Sign the passphrase
        let mut pass_mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        pass_mac.update(passphrase.as_bytes());
        let signed_passphrase = BASE64.encode(pass_mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("KC-API-KEY".into(), api_key.to_string());
        headers.insert("KC-API-SIGN".into(), signature);
        headers.insert("KC-API-TIMESTAMP".into(), timestamp);
        headers.insert("KC-API-PASSPHRASE".into(), signed_passphrase);
        headers.insert("KC-API-KEY-VERSION".into(), "2".into());
        headers.insert("Content-Type".into(), "application/json".into());

        // Create a separate client for futures API
        let futures_client = HttpClient::new("https://api-futures.kucoin.com", &self.config)?;

        let response: KucoinResponse<T> = match method.to_uppercase().as_str() {
            "GET" => futures_client.get(path, query_params, Some(headers)).await?,
            "POST" => {
                let json_body: serde_json::Value = if body_str.is_empty() {
                    serde_json::Value::Object(serde_json::Map::new())
                } else {
                    serde_json::from_str(&body_str).unwrap_or_default()
                };
                futures_client.post(path, Some(json_body), Some(headers)).await?
            }
            _ => return Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        };

        if response.code != "200000" {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_else(|| format!("Kucoin error: {}", response.code)),
            });
        }

        Ok(response.data.unwrap_or_default())
    }

    /// 포지션 파싱
    fn parse_position(&self, data: &KucoinPosition) -> Position {
        let timestamp = data.current_timestamp;
        let symbol = data.symbol.replace("USDTM", "/USDT:USDT");

        let size: Decimal = data.current_qty.unwrap_or_default();
        let side = if size > Decimal::ZERO {
            Some(PositionSide::Long)
        } else if size < Decimal::ZERO {
            Some(PositionSide::Short)
        } else {
            None
        };

        let contracts = Some(size.abs());
        let entry_price: Option<Decimal> = data.avg_entry_price;
        let mark_price: Option<Decimal> = data.mark_price;
        let leverage: Option<Decimal> = data.real_leverage;
        let unrealized_pnl: Option<Decimal> = data.unrealised_pnl;
        let liquidation_price: Option<Decimal> = data.liquidation_price.and_then(|l| {
            if l == Decimal::ZERO { None } else { Some(l) }
        });
        let margin: Option<Decimal> = data.pos_margin;

        let margin_mode = if data.cross_mode.unwrap_or(false) {
            Some(MarginMode::Cross)
        } else {
            Some(MarginMode::Isolated)
        };

        Position {
            symbol,
            id: data.id.clone(),
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            contracts,
            contract_size: None,
            side,
            notional: data.pos_cost,
            leverage,
            collateral: margin,
            initial_margin: margin,
            initial_margin_percentage: None,
            maintenance_margin: data.maint_margin,
            maintenance_margin_percentage: None,
            entry_price,
            mark_price,
            last_price: None,
            liquidation_price,
            unrealized_pnl,
            realized_pnl: data.realised_pnl,
            percentage: data.unrealised_pnl_pcnt,
            margin_mode,
            margin_ratio: None,
            hedged: Some(false),
            last_update_timestamp: timestamp,
            stop_loss_price: None,
            take_profit_price: None,
        }
    }

    /// 펀딩 레이트 파싱
    fn parse_funding_rate(&self, data: &KucoinFundingRateData) -> FundingRate {
        let timestamp = data.time_point;
        let symbol = data.symbol.replace("USDTM", "/USDT:USDT");

        FundingRate {
            symbol,
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            funding_rate: data.value,
            mark_price: None,
            index_price: None,
            interest_rate: None,
            estimated_settle_price: None,
            funding_timestamp: data.time_point,
            funding_datetime: None,
            next_funding_rate: data.predicted_value,
            next_funding_timestamp: None,
            next_funding_datetime: None,
            previous_funding_rate: None,
            previous_funding_timestamp: None,
            previous_funding_datetime: None,
            interval: Some("8h".to_string()),
        }
    }

    /// 펀딩 레이트 기록 파싱
    fn parse_funding_rate_history(&self, data: &KucoinFundingRateHistoryData) -> FundingRateHistory {
        let timestamp = Some(data.time_point);
        let symbol = data.symbol.replace("USDTM", "/USDT:USDT");
        let funding_rate: Decimal = data.funding_rate;

        FundingRateHistory {
            symbol,
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            funding_rate,
        }
    }

    /// 선물 심볼 → 마켓 ID 변환 (BTC/USDT:USDT → XBTUSDTM)
    fn to_futures_market_id(&self, symbol: &str) -> String {
        if symbol.contains(':') {
            let parts: Vec<&str> = symbol.split(':').collect();
            if let Some(base_quote) = parts.first() {
                let bq: Vec<&str> = base_quote.split('/').collect();
                if bq.len() == 2 {
                    let base = if bq[0] == "BTC" { "XBT" } else { bq[0] };
                    return format!("{}{}M", base, bq[1]);
                }
            }
        }
        symbol.replace("/", "").replace("BTC", "XBT") + "M"
    }
}

#[async_trait]
impl Exchange for Kucoin {
    fn id(&self) -> ExchangeId {
        ExchangeId::Kucoin
    }

    fn name(&self) -> &str {
        "Kucoin"
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
        let response: KucoinResponse<Vec<KucoinSymbol>> = self.public_get("/api/v2/symbols", None).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let mut markets = Vec::new();

        for symbol_data in data {
            if !symbol_data.enable_trading.unwrap_or(false) {
                continue;
            }

            let id = symbol_data.symbol.clone();
            let symbol = id.replace('-', "/");
            let base = symbol_data.base_currency.clone();
            let quote = symbol_data.quote_currency.clone();

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                settle: None,
                settle_id: None,
                active: symbol_data.enable_trading.unwrap_or(false),
                market_type: MarketType::Spot,
                spot: true,
                margin: symbol_data.is_margin_enabled.unwrap_or(false),
                swap: false,
                future: false,
                option: false,
                index: false,
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: Some(Decimal::new(1, 3)), // 0.001
                maker: Some(Decimal::new(1, 3)), // 0.001
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: symbol_data.base_increment.as_ref()
                        .and_then(|v| count_decimals(v)),
                    price: symbol_data.price_increment.as_ref()
                        .and_then(|v| count_decimals(v)),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&symbol_data).unwrap_or_default(),
                tier_based: false,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: KucoinResponse<KucoinTicker> = self.public_get("/api/v1/market/stats", Some(params)).await?;

        let ticker = response.data
            .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.into() })?;

        Ok(self.parse_ticker(&ticker, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: KucoinResponse<KucoinAllTickers> = self.public_get("/api/v1/market/allTickers", None).await?;

        let all_tickers = response.data
            .ok_or_else(|| CcxtError::ExchangeError { message: "No data in response".into() })?;

        let mut tickers = HashMap::new();

        for data in all_tickers.ticker {
            let market_id = match &data.symbol {
                Some(s) => s.clone(),
                None => continue,
            };
            let symbol_str = market_id.replace('-', "/");

            if let Some(filter) = symbols {
                if !filter.contains(&symbol_str.as_str()) {
                    continue;
                }
            }

            let ticker = Ticker {
                symbol: symbol_str.clone(),
                timestamp: Some(all_tickers.time.unwrap_or_else(|| Utc::now().timestamp_millis())),
                datetime: Some(Utc::now().to_rfc3339()),
                high: data.high.as_ref().and_then(|v| v.parse().ok()),
                low: data.low.as_ref().and_then(|v| v.parse().ok()),
                bid: data.buy.as_ref().and_then(|v| v.parse().ok()),
                bid_volume: None,
                ask: data.sell.as_ref().and_then(|v| v.parse().ok()),
                ask_volume: None,
                vwap: None,
                open: None,
                close: data.last.as_ref().and_then(|v| v.parse().ok()),
                last: data.last.as_ref().and_then(|v| v.parse().ok()),
                previous_close: None,
                change: data.change_price.as_ref().and_then(|v| v.parse().ok()),
                percentage: data.change_rate.as_ref().and_then(|v| {
                    v.parse::<Decimal>().ok().map(|d| d * Decimal::from(100))
                }),
                average: data.average_price.as_ref().and_then(|v| v.parse().ok()),
                base_volume: data.vol.as_ref().and_then(|v| v.parse().ok()),
                quote_volume: data.vol_value.as_ref().and_then(|v| v.parse().ok()),
                index_price: None,
                mark_price: None,
                info: serde_json::to_value(&data).unwrap_or_default(),
            };

            tickers.insert(symbol_str, ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let path = match limit {
            Some(l) if l <= 20 => "/api/v1/market/orderbook/level2_20",
            _ => "/api/v1/market/orderbook/level2_100",
        };

        let response: KucoinResponse<KucoinOrderBook> = self.public_get(path, Some(params)).await?;

        let data = response.data
            .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.into() })?;

        let bids: Vec<OrderBookEntry> = data.bids.iter()
            .filter_map(|b| {
                if b.len() >= 2 {
                    Some(OrderBookEntry {
                        price: b[0].parse().ok()?,
                        amount: b[1].parse().ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter()
            .filter_map(|a| {
                if a.len() >= 2 {
                    Some(OrderBookEntry {
                        price: a[0].parse().ok()?,
                        amount: a[1].parse().ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            ),
            nonce: data.sequence.map(|s| s as i64),
            bids,
            asks,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: KucoinResponse<Vec<KucoinTrade>> = self.public_get("/api/v1/market/histories", Some(params)).await?;

        let data = response.data.unwrap_or_default();

        let trades: Vec<Trade> = data.iter().map(|t| {
            // Kucoin time is in nanoseconds
            let timestamp = t.time.map(|ts| ts / 1_000_000).unwrap_or_else(|| Utc::now().timestamp_millis());
            let price: Decimal = t.price.as_ref()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default();
            let amount: Decimal = t.size.as_ref()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default();

            Trade {
                id: t.sequence.clone().unwrap_or_default(),
                order: None,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                ),
                symbol: symbol.to_string(),
                trade_type: None,
                side: t.side.clone(),
                taker_or_maker: None,
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(t).unwrap_or_default(),
            }
        }).collect();

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

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("type".into(), interval.clone());
        if let Some(s) = since {
            params.insert("startAt".into(), (s / 1000).to_string());
        }

        let response: KucoinResponse<Vec<Vec<String>>> = self.public_get("/api/v1/market/candles", Some(params)).await?;

        let data = response.data.unwrap_or_default();

        let mut ohlcv: Vec<OHLCV> = data.iter().filter_map(|c| {
            if c.len() < 7 {
                return None;
            }
            Some(OHLCV {
                timestamp: c[0].parse::<i64>().ok()? * 1000,
                open: c[1].parse().ok()?,
                close: c[2].parse().ok()?,
                high: c[3].parse().ok()?,
                low: c[4].parse().ok()?,
                volume: c[5].parse().ok()?,
            })
        }).collect();

        // Kucoin returns data in descending order
        ohlcv.reverse();

        if let Some(l) = limit {
            ohlcv.truncate(l as usize);
        }

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: KucoinResponse<Vec<KucoinAccount>> = self
            .private_request("GET", "/api/v1/accounts", HashMap::new())
            .await?;

        let accounts = response.data.unwrap_or_default();
        Ok(self.parse_balance(&accounts))
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
        let client_oid = uuid::Uuid::new_v4().to_string();

        let mut params = HashMap::new();
        params.insert("clientOid".into(), client_oid);
        params.insert("symbol".into(), market_id);
        params.insert("side".into(), match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }.into());
        params.insert("size".into(), amount.to_string());

        match order_type {
            OrderType::Limit => {
                let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                    message: "Limit order requires price".into(),
                })?;
                params.insert("type".into(), "limit".into());
                params.insert("price".into(), price_val.to_string());
            }
            OrderType::Market => {
                params.insert("type".into(), "market".into());
            }
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {order_type:?}"),
            }),
        }

        let response: KucoinResponse<KucoinOrderId> = self
            .private_request("POST", "/api/v1/orders", params)
            .await?;

        let order_id = response.data
            .ok_or_else(|| CcxtError::ExchangeError { message: "No order ID returned".into() })?;

        // Fetch the created order
        self.fetch_order(&order_id.order_id, symbol).await
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        // First fetch the order to return it
        let order = self.fetch_order(id, symbol).await.ok();

        let path = format!("/api/v1/orders/{id}");
        let _response: KucoinResponse<KucoinCancelledOrders> = self
            .private_request("DELETE", &path, HashMap::new())
            .await?;

        // Return the order if we fetched it, otherwise create a minimal cancelled order
        match order {
            Some(mut o) => {
                o.status = OrderStatus::Canceled;
                Ok(o)
            }
            None => Ok(Order {
                id: id.to_string(),
                client_order_id: None,
                timestamp: Some(Utc::now().timestamp_millis()),
                datetime: Some(Utc::now().to_rfc3339()),
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
                trades: Vec::new(),
                fee: None,
                fees: Vec::new(),
                reduce_only: None,
                post_only: None,
                info: serde_json::Value::Null,
            })
        }
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let path = format!("/api/v1/orders/{id}");
        let response: KucoinResponse<KucoinOrder> = self
            .private_request("GET", &path, HashMap::new())
            .await?;

        let order_data = response.data
            .ok_or_else(|| CcxtError::OrderNotFound { order_id: id.into() })?;

        Ok(self.parse_order(&order_data, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".into(), "active".into());

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }

        let response: KucoinResponse<KucoinOrderList> = self
            .private_request("GET", "/api/v1/orders", params)
            .await?;

        let order_list = response.data.unwrap_or_default();
        let symbol_str = symbol.unwrap_or("");

        Ok(order_list.items.iter().map(|o| {
            let sym = if symbol_str.is_empty() {
                o.symbol.as_ref().map(|s| s.replace('-', "/")).unwrap_or_default()
            } else {
                symbol_str.to_string()
            };
            self.parse_order(o, &sym)
        }).collect())
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".into(), "done".into());

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("pageSize".into(), l.min(100).to_string());
        }

        let response: KucoinResponse<KucoinOrderList> = self
            .private_request("GET", "/api/v1/orders", params)
            .await?;

        let order_list = response.data.unwrap_or_default();
        let symbol_str = symbol.unwrap_or("");

        Ok(order_list.items.iter().map(|o| {
            let sym = if symbol_str.is_empty() {
                o.symbol.as_ref().map(|s| s.replace('-', "/")).unwrap_or_default()
            } else {
                symbol_str.to_string()
            };
            self.parse_order(o, &sym)
        }).collect())
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("pageSize".into(), l.min(100).to_string());
        }

        let response: KucoinResponse<KucoinFillList> = self
            .private_request("GET", "/api/v1/fills", params)
            .await?;

        let fill_list = response.data.unwrap_or_default();

        let trades: Vec<Trade> = fill_list
            .items
            .iter()
            .map(|f| {
                let timestamp = f.created_at;
                let price: Decimal = f.price.as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
                let amount: Decimal = f.size.as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
                let fee_amount: Option<Decimal> = f.fee.as_ref().and_then(|v| v.parse().ok());

                let symbol_str = f.symbol.as_ref()
                    .map(|s| s.replace('-', "/"))
                    .unwrap_or_default();

                Trade {
                    id: f.trade_id.clone().unwrap_or_default(),
                    order: f.order_id.clone(),
                    timestamp,
                    datetime: timestamp.and_then(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                    }),
                    symbol: symbol_str,
                    trade_type: None,
                    side: f.side.clone(),
                    taker_or_maker: f.liquidity.as_ref().map(|l| {
                        if l == "maker" {
                            crate::types::TakerOrMaker::Maker
                        } else {
                            crate::types::TakerOrMaker::Taker
                        }
                    }),
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: fee_amount.map(|cost| crate::types::Fee {
                        cost: Some(cost),
                        currency: f.fee_currency.clone(),
                        rate: f.fee_rate.as_ref().and_then(|r| r.parse().ok()),
                    }),
                    fees: Vec::new(),
                    info: serde_json::to_value(f).unwrap_or_default(),
                }
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();

        if let Some(c) = code {
            params.insert("currency".into(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("pageSize".into(), l.min(100).to_string());
        }

        let response: KucoinResponse<KucoinDepositList> = self
            .private_request("GET", "/api/v1/deposits", params)
            .await?;

        let deposit_list = response.data.unwrap_or_default();

        let transactions: Vec<Transaction> = deposit_list
            .items
            .iter()
            .map(|d| self.parse_deposit(d))
            .collect();

        Ok(transactions)
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();

        if let Some(c) = code {
            params.insert("currency".into(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("pageSize".into(), l.min(100).to_string());
        }

        let response: KucoinResponse<KucoinWithdrawalList> = self
            .private_request("GET", "/api/v1/withdrawals", params)
            .await?;

        let withdrawal_list = response.data.unwrap_or_default();

        let transactions: Vec<Transaction> = withdrawal_list
            .items
            .iter()
            .map(|w| self.parse_withdrawal(w))
            .collect();

        Ok(transactions)
    }

    async fn withdraw(
        &self,
        code: &str,
        amount: Decimal,
        address: &str,
        tag: Option<&str>,
        network: Option<&str>,
    ) -> CcxtResult<Transaction> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_string());
        params.insert("amount".into(), amount.to_string());
        params.insert("address".into(), address.to_string());

        if let Some(t) = tag {
            params.insert("memo".into(), t.to_string());
        }
        if let Some(n) = network {
            params.insert("chain".into(), n.to_string());
        }

        let response: KucoinResponse<KucoinWithdrawResponse> = self
            .private_request("POST", "/api/v1/withdrawals", params)
            .await?;

        let withdraw_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No withdrawal ID returned".into(),
        })?;

        let info = serde_json::to_value(&withdraw_data).unwrap_or_default();

        Ok(Transaction {
            id: withdraw_data.withdrawal_id,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            updated: None,
            tx_type: TransactionType::Withdrawal,
            currency: code.to_string(),
            network: network.map(String::from),
            amount,
            status: TransactionStatus::Pending,
            address: Some(address.to_string()),
            tag: tag.map(String::from),
            txid: None,
            fee: None,
            internal: None,
            confirmations: None,
            info,
        })
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<DepositAddress> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_string());
        if let Some(n) = network {
            params.insert("chain".into(), n.to_string());
        }

        let response: KucoinResponse<KucoinDepositAddress> = self
            .private_request("GET", "/api/v2/deposit-addresses", params)
            .await?;

        let addr = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: format!("No deposit address found for {code}"),
        })?;

        let network = addr.chain.clone();
        Ok(DepositAddress {
            currency: code.to_string(),
            address: addr.address.clone(),
            tag: addr.memo.clone(),
            network: Some(network),
            info: serde_json::to_value(&addr).unwrap_or_default(),
        })
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

    // === WebSocket Methods ===

    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_ticker(symbol).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_tickers(symbols).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_order_book(symbol, limit).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_trades(symbol).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_ohlcv(symbol, timeframe).await
    }

    // === Futures Methods ===

    async fn fetch_positions(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<Vec<Position>> {
        let response: Vec<KucoinPosition> = self
            .futures_private_request("GET", "/api/v1/positions", HashMap::new())
            .await?;

        let mut positions: Vec<Position> = response
            .iter()
            .filter(|p| {
                p.current_qty
                    .map(|q| q != Decimal::ZERO)
                    .unwrap_or(false)
            })
            .map(|p| self.parse_position(p))
            .collect();

        if let Some(filter_symbols) = symbols {
            positions.retain(|p| filter_symbols.contains(&p.symbol.as_str()));
        }

        Ok(positions)
    }

    async fn set_leverage(
        &self,
        leverage: Decimal,
        symbol: &str,
    ) -> CcxtResult<Leverage> {
        let market_id = self.to_futures_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("leverage".into(), leverage.to_string());

        let _response: serde_json::Value = self
            .futures_private_request("POST", "/api/v1/position/risk-limit-level/change", params)
            .await?;

        Ok(Leverage::new(symbol, MarginMode::Cross, leverage, leverage))
    }

    async fn fetch_leverage(
        &self,
        symbol: &str,
    ) -> CcxtResult<Leverage> {
        let positions = self.fetch_positions(Some(&[symbol])).await?;

        if let Some(pos) = positions.first() {
            let leverage = pos.leverage.unwrap_or(Decimal::ONE);
            let margin_mode = pos.margin_mode.clone().unwrap_or(MarginMode::Cross);
            Ok(Leverage::new(symbol, margin_mode, leverage, leverage))
        } else {
            Ok(Leverage::new(symbol, MarginMode::Cross, Decimal::ONE, Decimal::ONE))
        }
    }

    async fn set_margin_mode(
        &self,
        margin_mode: MarginMode,
        symbol: &str,
    ) -> CcxtResult<MarginModeInfo> {
        let market_id = self.to_futures_market_id(symbol);

        let is_isolated = match margin_mode {
            MarginMode::Cross => false,
            MarginMode::Isolated => true,
            MarginMode::Unknown => return Err(CcxtError::BadRequest {
                message: "Unknown margin mode is not supported".into(),
            }),
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("isIsolated".into(), is_isolated.to_string());

        let _response: serde_json::Value = self
            .futures_private_request("POST", "/api/v1/position/margin/auto-deposit-status", params)
            .await?;

        Ok(MarginModeInfo::new(symbol, margin_mode))
    }

    async fn fetch_funding_rate(
        &self,
        symbol: &str,
    ) -> CcxtResult<FundingRate> {
        let market_id = self.to_futures_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: KucoinFundingRateData = self
            .futures_public_get("/api/v1/funding-rate/current", Some(params))
            .await?;

        Ok(self.parse_funding_rate(&response))
    }

    async fn fetch_funding_rates(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, FundingRate>> {
        // Kucoin requires symbol for funding rate, so we need to fetch each one
        // Get all active contracts first
        let contracts: Vec<KucoinContract> = self
            .futures_public_get("/api/v1/contracts/active", None)
            .await
            .unwrap_or_default();

        let mut rates = HashMap::new();

        for contract in contracts {
            let mut params = HashMap::new();
            params.insert("symbol".into(), contract.symbol.clone());

            if let Ok(data) = self.futures_public_get::<KucoinFundingRateData>("/api/v1/funding-rate/current", Some(params)).await {
                let rate = self.parse_funding_rate(&data);

                if let Some(filter) = symbols {
                    if !filter.contains(&rate.symbol.as_str()) {
                        continue;
                    }
                }

                rates.insert(rate.symbol.clone(), rate);
            }
        }

        Ok(rates)
    }

    async fn fetch_funding_rate_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<FundingRateHistory>> {
        let market_id = self.to_futures_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        if let Some(s) = since {
            params.insert("from".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("maxCount".into(), l.min(100).to_string());
        }

        let response: KucoinFundingRateHistoryResult = self
            .futures_public_get("/api/v1/contract/funding-rates", Some(params))
            .await?;

        let history: Vec<FundingRateHistory> = response
            .data_list
            .iter()
            .map(|d| self.parse_funding_rate_history(d))
            .collect();

        Ok(history)
    }

    async fn fetch_open_interest(&self, symbol: &str) -> CcxtResult<OpenInterest> {
        let market_id = self.to_futures_market_id(symbol);

        let response: KucoinOpenInterestData = self
            .futures_public_get(&format!("/api/v1/openInterest?symbol={market_id}"), None)
            .await?;

        let timestamp = Utc::now().timestamp_millis();
        let amount: Decimal = response.open_interest.as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        Ok(OpenInterest {
            symbol: symbol.to_string(),
            open_interest_amount: Some(amount),
            open_interest_value: None,
            base_volume: None,
            quote_volume: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_liquidations(
        &self,
        _symbol: &str,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Liquidation>> {
        // Kucoin Futures doesn't have a public liquidation endpoint in REST API
        // Return empty vec - liquidation data typically requires websocket
        Ok(Vec::new())
    }

    async fn fetch_index_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_futures_market_id(symbol);

        let response: KucoinMarkPriceData = self
            .futures_public_get(&format!("/api/v1/mark-price/{market_id}/current"), None)
            .await?;

        let timestamp = Utc::now().timestamp_millis();
        let index_price: Decimal = response.index_price.as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let mark_price: Decimal = response.value.as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last: Some(index_price),
            mark_price: Some(mark_price),
            index_price: Some(index_price),
            info: serde_json::to_value(&response).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn fetch_mark_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_futures_market_id(symbol);

        let response: KucoinMarkPriceData = self
            .futures_public_get(&format!("/api/v1/mark-price/{market_id}/current"), None)
            .await?;

        let timestamp = Utc::now().timestamp_millis();
        let mark_price: Decimal = response.value.as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            mark_price: Some(mark_price),
            info: serde_json::to_value(&response).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn fetch_mark_prices(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        // Kucoin uses /api/v1/contracts/active for contract list
        let response: Vec<KucoinContractData> = self
            .futures_public_get("/api/v1/contracts/active", None)
            .await?;

        let timestamp = Utc::now().timestamp_millis();

        let mut tickers: HashMap<String, Ticker> = HashMap::new();

        for data in &response {
            let market_id = &data.symbol;
            let symbol = match self.markets_by_id.read().ok().and_then(|m| m.get(market_id).cloned()) {
                Some(s) => s,
                None => continue,
            };

            if let Some(sym_list) = symbols {
                if !sym_list.contains(&symbol.as_str()) {
                    continue;
                }
            }

            let mark_price = match data.mark_price.and_then(|p| Decimal::try_from(p).ok()) {
                Some(p) => p,
                None => continue,
            };

            tickers.insert(symbol.clone(), Ticker {
                symbol,
                timestamp: Some(timestamp),
                datetime: Some(Utc::now().to_rfc3339()),
                mark_price: Some(mark_price),
                info: serde_json::to_value(data).unwrap_or_default(),
                ..Default::default()
            });
        }

        Ok(tickers)
    }

    async fn fetch_mark_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        // Kucoin Futures doesn't have dedicated mark price OHLCV endpoint
        // Return NotSupported
        let _ = (symbol, timeframe, since, limit);
        Err(CcxtError::NotSupported {
            feature: "fetch_mark_ohlcv is not supported by Kucoin Futures".into(),
        })
    }

    async fn fetch_index_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        // Kucoin Futures doesn't have dedicated index price OHLCV endpoint
        // Return NotSupported
        let _ = (symbol, timeframe, since, limit);
        Err(CcxtError::NotSupported {
            feature: "fetch_index_ohlcv is not supported by Kucoin Futures".into(),
        })
    }

    async fn edit_order(
        &self,
        id: &str,
        symbol: &str,
        _order_type: Option<OrderType>,
        _side: Option<OrderSide>,
        amount: Option<Decimal>,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        // Kucoin doesn't support order modification - cancel and recreate
        // For now, just fetch the order
        let order = self.fetch_order(id, symbol).await?;

        if amount.is_none() && price.is_none() {
            return Ok(order);
        }

        Err(CcxtError::NotSupported {
            feature: "Order editing is not supported by Kucoin. Cancel and recreate instead.".into(),
        })
    }

    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(sym) = symbol {
            params.insert("symbol".into(), self.to_market_id(sym));
        }

        let _response: KucoinResponse<KucoinCancelledOrders> = self
            .private_request("DELETE", "/api/v1/orders", params)
            .await?;

        // Return empty vec as Kucoin doesn't return cancelled order details
        Ok(Vec::new())
    }

    async fn transfer(
        &self,
        code: &str,
        amount: Decimal,
        from_account: &str,
        to_account: &str,
    ) -> CcxtResult<TransferEntry> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_string());
        params.insert("amount".into(), amount.to_string());
        params.insert("from".into(), from_account.to_string());
        params.insert("to".into(), to_account.to_string());
        params.insert("clientOid".into(), uuid::Uuid::new_v4().to_string());

        let response: KucoinTransferResult = self
            .private_request("POST", "/api/v2/accounts/inner-transfer", params)
            .await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(TransferEntry::new()
            .with_currency(code)
            .with_amount(amount)
            .with_id(response.order_id)
            .with_from_account(from_account)
            .with_to_account(to_account)
            .with_timestamp(timestamp)
            .with_status("ok"))
    }

    async fn fetch_transfers(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<TransferEntry>> {
        let mut params = HashMap::new();
        if let Some(currency) = code {
            params.insert("currency".into(), currency.to_string());
        }
        if let Some(ts) = since {
            params.insert("startAt".into(), ts.to_string());
        }
        if let Some(lim) = limit {
            params.insert("pageSize".into(), lim.to_string());
        }

        let response: KucoinTransferList = self
            .private_request("GET", "/api/v1/accounts/ledgers", params)
            .await?;

        let entries = response.items.unwrap_or_default();
        let mut transfers = Vec::new();

        for item in entries {
            let amount: Decimal = item.amount.parse().unwrap_or_default();
            let timestamp: i64 = item.created_at.unwrap_or(0);

            transfers.push(TransferEntry::new()
                .with_currency(&item.currency)
                .with_amount(amount)
                .with_id(item.id)
                .with_timestamp(timestamp));
        }

        Ok(transfers)
    }

    async fn set_position_mode(
        &self,
        _hedged: bool,
        _symbol: Option<&str>,
    ) -> CcxtResult<PositionModeInfo> {
        // Kucoin Futures only supports one-way mode for most contracts
        Err(CcxtError::NotSupported {
            feature: "Position mode change is not supported by Kucoin Futures".into(),
        })
    }

    async fn fetch_position_mode(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<PositionModeInfo> {
        // Kucoin Futures uses one-way mode by default
        Ok(PositionModeInfo::new(PositionMode::OneWay))
    }

    async fn add_margin(
        &self,
        symbol: &str,
        amount: Decimal,
    ) -> CcxtResult<MarginModification> {
        let market_id = self.to_futures_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("margin".into(), amount.to_string());
        params.insert("bizNo".into(), uuid::Uuid::new_v4().to_string());

        let response: serde_json::Value = self
            .futures_private_request("POST", "/api/v1/position/margin/deposit-margin", params)
            .await?;

        Ok(MarginModification {
            info: response,
            symbol: symbol.to_string(),
            modification_type: Some(MarginModificationType::Add),
            amount: Some(amount),
            ..Default::default()
        })
    }

    async fn reduce_margin(
        &self,
        symbol: &str,
        amount: Decimal,
    ) -> CcxtResult<MarginModification> {
        let market_id = self.to_futures_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("withdrawAmount".into(), amount.to_string());

        let response: serde_json::Value = self
            .futures_private_request("POST", "/api/v1/position/margin/withdraw-margin", params)
            .await?;

        Ok(MarginModification {
            info: response,
            symbol: symbol.to_string(),
            modification_type: Some(MarginModificationType::Reduce),
            amount: Some(amount),
            ..Default::default()
        })
    }
}

// Helper function to count decimal places
fn count_decimals(s: &str) -> Option<i32> {
    if let Some(pos) = s.find('.') {
        Some((s.len() - pos - 1) as i32)
    } else {
        Some(0)
    }
}

// === Kucoin API Response Types ===

#[derive(Debug, Deserialize)]
struct KucoinResponse<T> {
    #[allow(dead_code)]
    code: String,
    #[serde(default)]
    data: Option<T>,
    #[allow(dead_code)]
    #[serde(default)]
    msg: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KucoinSymbol {
    symbol: String,
    #[serde(rename = "baseCurrency")]
    base_currency: String,
    #[serde(rename = "quoteCurrency")]
    quote_currency: String,
    #[serde(rename = "feeCurrency", default)]
    #[allow(dead_code)]
    fee_currency: Option<String>,
    #[serde(rename = "enableTrading", default)]
    enable_trading: Option<bool>,
    #[serde(rename = "isMarginEnabled", default)]
    is_margin_enabled: Option<bool>,
    #[serde(rename = "baseIncrement", default)]
    base_increment: Option<String>,
    #[serde(rename = "priceIncrement", default)]
    price_increment: Option<String>,
    #[serde(rename = "quoteIncrement", default)]
    #[allow(dead_code)]
    quote_increment: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KucoinTicker {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    buy: Option<String>,
    #[serde(default)]
    sell: Option<String>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    vol: Option<String>,
    #[serde(rename = "volValue", default)]
    vol_value: Option<String>,
    #[serde(rename = "changePrice", default)]
    change_price: Option<String>,
    #[serde(rename = "changeRate", default)]
    change_rate: Option<String>,
    #[serde(rename = "averagePrice", default)]
    average_price: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct KucoinAllTickers {
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    ticker: Vec<KucoinTicker>,
}

#[derive(Debug, Default, Deserialize)]
struct KucoinOrderBook {
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    sequence: Option<u64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KucoinTrade {
    #[serde(default)]
    sequence: Option<String>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KucoinOrder {
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "clientOid", default)]
    client_oid: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(rename = "createdAt", default)]
    created_at: Option<i64>,
    #[serde(default)]
    side: Option<String>,
    #[serde(rename = "type", default)]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(rename = "dealSize", default)]
    deal_size: Option<String>,
    #[serde(rename = "dealFunds", default)]
    deal_funds: Option<String>,
    #[serde(rename = "isActive", default)]
    is_active: Option<bool>,
    #[serde(rename = "cancelExist", default)]
    cancel_exist: Option<bool>,
    #[serde(rename = "stopPrice", default)]
    stop_price: Option<String>,
    #[serde(rename = "postOnly", default)]
    post_only: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct KucoinOrderId {
    #[serde(rename = "orderId", default)]
    order_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct KucoinOrderList {
    #[serde(default)]
    items: Vec<KucoinOrder>,
}

#[derive(Debug, Default, Deserialize)]
struct KucoinCancelledOrders {
    #[serde(rename = "cancelledOrderIds", default)]
    #[allow(dead_code)]
    cancelled_order_ids: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct KucoinTransferResult {
    #[serde(rename = "orderId", default)]
    order_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct KucoinTransferList {
    #[serde(default)]
    items: Option<Vec<KucoinTransferRecord>>,
}

#[derive(Debug, Deserialize)]
struct KucoinTransferRecord {
    id: String,
    currency: String,
    amount: String,
    #[serde(rename = "createdAt", default)]
    created_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct KucoinAccount {
    #[serde(default)]
    currency: String,
    #[serde(rename = "type", default)]
    account_type: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    balance: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    holds: Option<String>,
}

/// Fill (trade) list response
#[derive(Debug, Default, Deserialize)]
struct KucoinFillList {
    #[serde(default)]
    items: Vec<KucoinFill>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KucoinFill {
    #[serde(rename = "tradeId", default)]
    trade_id: Option<String>,
    #[serde(rename = "orderId", default)]
    order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(rename = "feeCurrency", default)]
    fee_currency: Option<String>,
    #[serde(rename = "feeRate", default)]
    fee_rate: Option<String>,
    #[serde(default)]
    liquidity: Option<String>,
    #[serde(rename = "createdAt", default)]
    created_at: Option<i64>,
}

/// Deposit list response
#[derive(Debug, Default, Deserialize)]
struct KucoinDepositList {
    #[serde(default)]
    items: Vec<KucoinDeposit>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KucoinDeposit {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    currency: String,
    #[serde(default)]
    chain: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    memo: Option<String>,
    #[serde(rename = "walletTxId", default)]
    wallet_tx_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(rename = "isInner", default)]
    is_inner: Option<bool>,
    #[serde(rename = "createdAt", default)]
    created_at: Option<i64>,
    #[serde(rename = "updatedAt", default)]
    updated_at: Option<i64>,
}

/// Withdrawal list response
#[derive(Debug, Default, Deserialize)]
struct KucoinWithdrawalList {
    #[serde(default)]
    items: Vec<KucoinWithdrawal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KucoinWithdrawal {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    currency: String,
    #[serde(default)]
    chain: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    memo: Option<String>,
    #[serde(rename = "walletTxId", default)]
    wallet_tx_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(rename = "isInner", default)]
    is_inner: Option<bool>,
    #[serde(rename = "createdAt", default)]
    created_at: Option<i64>,
    #[serde(rename = "updatedAt", default)]
    updated_at: Option<i64>,
}

/// Withdraw response
#[derive(Debug, Default, Deserialize, Serialize)]
struct KucoinWithdrawResponse {
    #[serde(rename = "withdrawalId", default)]
    withdrawal_id: String,
}

/// Deposit address response
#[derive(Debug, Default, Deserialize, Serialize)]
struct KucoinDepositAddress {
    #[serde(default)]
    address: String,
    #[serde(default)]
    memo: Option<String>,
    #[serde(default)]
    chain: String,
}

// === Futures Response Types ===

#[derive(Debug, Default, Deserialize, Serialize)]
struct KucoinPosition {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    symbol: String,
    #[serde(rename = "currentQty", default)]
    current_qty: Option<Decimal>,
    #[serde(rename = "avgEntryPrice", default)]
    avg_entry_price: Option<Decimal>,
    #[serde(rename = "markPrice", default)]
    mark_price: Option<Decimal>,
    #[serde(rename = "liquidationPrice", default)]
    liquidation_price: Option<Decimal>,
    #[serde(rename = "realLeverage", default)]
    real_leverage: Option<Decimal>,
    #[serde(rename = "posMargin", default)]
    pos_margin: Option<Decimal>,
    #[serde(rename = "posCost", default)]
    pos_cost: Option<Decimal>,
    #[serde(rename = "maintMargin", default)]
    maint_margin: Option<Decimal>,
    #[serde(rename = "unrealisedPnl", default)]
    unrealised_pnl: Option<Decimal>,
    #[serde(rename = "realisedPnl", default)]
    realised_pnl: Option<Decimal>,
    #[serde(rename = "unrealisedPnlPcnt", default)]
    unrealised_pnl_pcnt: Option<Decimal>,
    #[serde(rename = "crossMode", default)]
    cross_mode: Option<bool>,
    #[serde(rename = "currentTimestamp", default)]
    current_timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KucoinFundingRateData {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    value: Option<Decimal>,
    #[serde(rename = "predictedValue", default)]
    predicted_value: Option<Decimal>,
    #[serde(rename = "timePoint", default)]
    time_point: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KucoinFundingRateHistoryResult {
    #[serde(rename = "dataList", default)]
    data_list: Vec<KucoinFundingRateHistoryData>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KucoinFundingRateHistoryData {
    #[serde(default)]
    symbol: String,
    #[serde(rename = "fundingRate", default)]
    funding_rate: Decimal,
    #[serde(rename = "timePoint", default)]
    time_point: i64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KucoinContract {
    #[serde(default)]
    symbol: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KucoinOpenInterestData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(rename = "openInterest", default)]
    open_interest: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KucoinMarkPriceData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(rename = "indexPrice", default)]
    index_price: Option<String>,
    #[serde(rename = "timePoint", default)]
    time_point: Option<i64>,
}

/// Contract info response for mark prices
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinContractData {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    mark_price: Option<f64>,
    #[serde(default)]
    index_price: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Kucoin::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Kucoin);
        assert_eq!(exchange.name(), "Kucoin");
    }
}
