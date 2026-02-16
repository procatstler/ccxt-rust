//! Gate.io Exchange Implementation
//!
//! Gate.io API 구현

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::RwLock as TokioRwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, ConvertQuote, ConvertTrade, DepositAddress, Exchange, ExchangeFeatures,
    ExchangeId, ExchangeUrls, FundingRate, FundingRateHistory, LedgerEntry, Leverage, Liquidation,
    MarginMode, MarginModeInfo, Market, MarketLimits, MarketPrecision, MarketType, OpenInterest,
    Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Position, PositionSide,
    SignedRequest, Ticker, Timeframe, Trade, Transaction, TransactionStatus, TransactionType,
    TransferEntry, WsExchange, WsMessage, OHLCV,
};

use super::gate_ws::GateWs;

type HmacSha512 = Hmac<Sha512>;

/// Gate.io 거래소
pub struct Gate {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    ws_client: Arc<TokioRwLock<GateWs>>,
}

impl Gate {
    const BASE_URL: &'static str = "https://api.gateio.ws/api/v4";
    const RATE_LIMIT_MS: u64 = 100;

    /// 새 Gate.io 인스턴스 생성
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
            logo: Some("https://user-images.githubusercontent.com/1294454/31784029-0313c702-b509-11e7-9ccc-bc0da6a256d2.jpg".into()),
            api: api_urls,
            www: Some("https://www.gate.io".into()),
            doc: vec!["https://www.gate.io/docs/developers/apiv4".into()],
            fees: Some("https://www.gate.io/fee".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Second1, "10s".into());
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute3, "3m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour2, "2h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Hour6, "6h".into());
        timeframes.insert(Timeframe::Hour8, "8h".into());
        timeframes.insert(Timeframe::Hour12, "12h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());
        timeframes.insert(Timeframe::Day3, "3d".into());
        timeframes.insert(Timeframe::Week1, "7d".into());
        timeframes.insert(Timeframe::Month1, "30d".into());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            ws_client: Arc::new(TokioRwLock::new(GateWs::new())),
        })
    }

    /// 심볼 변환 (BTC/USDT -> BTC_USDT)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        let full_path = format!("/spot{path}");
        self.client.get(&full_path, params, None).await
    }

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let api_secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?;

        let timestamp = Utc::now().timestamp();
        let full_path = format!("/api/v4/spot{path}");

        let (query_string, body) = if method == "GET" || method == "DELETE" {
            let query = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            (query, String::new())
        } else {
            let body = serde_json::to_string(&params).unwrap_or_default();
            (String::new(), body)
        };

        // Body hash
        let body_hash = {
            let mut hasher = Sha512::new();
            hasher.update(body.as_bytes());
            hex::encode(hasher.finalize())
        };

        // String to sign
        let sign_string =
            format!("{method}\n{full_path}\n{query_string}\n{body_hash}\n{timestamp}");

        // HMAC-SHA512
        let mut mac = HmacSha512::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret".into(),
            }
        })?;
        mac.update(sign_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("KEY".into(), api_key.to_string());
        headers.insert("SIGN".into(), signature);
        headers.insert("Timestamp".into(), timestamp.to_string());
        headers.insert("Content-Type".into(), "application/json".into());

        let api_path = format!("/spot{path}");

        match method {
            "GET" => {
                let query_params = if query_string.is_empty() {
                    None
                } else {
                    Some(params)
                };
                self.client
                    .get(&api_path, query_params, Some(headers))
                    .await
            },
            "POST" => {
                let json_body: Option<serde_json::Value> = if body.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&body).unwrap_or_default())
                };
                self.client.post(&api_path, json_body, Some(headers)).await
            },
            "DELETE" => {
                let query_params = if query_string.is_empty() {
                    None
                } else {
                    Some(params)
                };
                self.client
                    .delete(&api_path, query_params, Some(headers))
                    .await
            },
            _ => Err(CcxtError::BadRequest {
                message: "Invalid HTTP method".into(),
            }),
        }
    }

    fn parse_ticker(&self, data: &GateTicker, symbol: &str) -> Ticker {
        let timestamp = Utc::now().timestamp_millis();
        let last: Option<Decimal> = data.last.as_ref().and_then(|v| v.parse().ok());
        let high: Option<Decimal> = data.high_24h.as_ref().and_then(|v| v.parse().ok());
        let low: Option<Decimal> = data.low_24h.as_ref().and_then(|v| v.parse().ok());
        let bid: Option<Decimal> = data.highest_bid.as_ref().and_then(|v| v.parse().ok());
        let ask: Option<Decimal> = data.lowest_ask.as_ref().and_then(|v| v.parse().ok());
        let base_volume: Option<Decimal> = data.base_volume.as_ref().and_then(|v| v.parse().ok());
        let quote_volume: Option<Decimal> = data.quote_volume.as_ref().and_then(|v| v.parse().ok());
        let percentage: Option<Decimal> =
            data.change_percentage.as_ref().and_then(|v| v.parse().ok());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high,
            low,
            bid,
            bid_volume: None,
            ask,
            ask_volume: None,
            vwap: None,
            open: None,
            close: last,
            last,
            previous_close: None,
            change: None,
            percentage,
            average: None,
            base_volume,
            quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order(&self, data: &GateOrder, symbol: &str) -> Order {
        let timestamp = data
            .create_time
            .map(|t| (t * 1000.0) as i64)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let amount: Decimal = data.amount.parse().unwrap_or_default();
        let filled: Decimal = data
            .filled_total
            .as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let price: Option<Decimal> = data.price.as_ref().and_then(|v| v.parse().ok());
        let average: Option<Decimal> = data.avg_deal_price.as_ref().and_then(|v| v.parse().ok());

        let status = match data.status.as_deref() {
            Some("open") => OrderStatus::Open,
            Some("closed") => OrderStatus::Closed,
            Some("cancelled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
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
            client_order_id: data.text.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.update_time.map(|t| (t * 1000.0) as i64),
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
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: Some(filled * average.unwrap_or_default()),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_balance(&self, accounts: &[GateAccount]) -> Balances {
        let mut balances = Balances::new();

        for account in accounts {
            let currency = account.currency.clone();
            let available: Decimal = account.available.parse().unwrap_or_default();
            let locked: Decimal = account.locked.parse().unwrap_or_default();

            balances.add(currency, Balance::new(available, locked));
        }

        balances
    }

    /// 입금 내역 파싱
    fn parse_deposit(&self, data: &GateDeposit) -> Transaction {
        let status = match data.status.as_deref() {
            Some("DONE") => TransactionStatus::Ok,
            Some("CANCEL") | Some("FAIL") => TransactionStatus::Failed,
            _ => TransactionStatus::Pending,
        };

        let timestamp = data.timestamp.map(|t| (t * 1000.0) as i64);
        let amount: Decimal = data.amount.parse().unwrap_or_default();

        Transaction {
            id: data.id.clone().unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: None,
            tx_type: TransactionType::Deposit,
            currency: data.currency.clone(),
            network: data.chain.clone(),
            amount,
            status,
            address: data.address.clone(),
            tag: data.memo.clone(),
            txid: data.txid.clone(),
            fee: None,
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 출금 내역 파싱
    fn parse_withdrawal(&self, data: &GateWithdrawal) -> Transaction {
        let status = match data.status.as_deref() {
            Some("DONE") => TransactionStatus::Ok,
            Some("CANCEL") | Some("FAIL") | Some("BCODE") => TransactionStatus::Failed,
            Some("REQUEST") | Some("MANUAL") | Some("EXTPEND") => TransactionStatus::Pending,
            _ => TransactionStatus::Pending,
        };

        let timestamp = data.timestamp.map(|t| (t * 1000.0) as i64);
        let amount: Decimal = data.amount.parse().unwrap_or_default();
        let fee_amount: Option<Decimal> = data.fee.as_ref().and_then(|f| f.parse().ok());

        Transaction {
            id: data.id.clone().unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: None,
            tx_type: TransactionType::Withdrawal,
            currency: data.currency.clone(),
            network: data.chain.clone(),
            amount,
            status,
            address: data.address.clone(),
            tag: data.memo.clone(),
            txid: data.txid.clone(),
            fee: fee_amount.map(|cost| crate::types::Fee {
                cost: Some(cost),
                currency: Some(data.currency.clone()),
                rate: None,
            }),
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 선물 API 호출 (퍼블릭)
    async fn futures_public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        let full_path = format!("/futures/usdt{path}");
        self.client.get(&full_path, params, None).await
    }

    /// 선물 API 호출 (프라이빗)
    async fn futures_private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        let full_path = format!("/futures/usdt{path}");
        let timestamp = Utc::now().timestamp().to_string();

        let query_string = if !params.is_empty() {
            params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&")
        } else {
            String::new()
        };

        let body_hash = {
            let mut hasher = Sha512::new();
            hasher.update(b"");
            hex::encode(hasher.finalize())
        };

        let sign_string = format!(
            "{}\n{}\n{}\n{}\n{}",
            method, &full_path, query_string, body_hash, timestamp
        );

        let mut mac =
            HmacSha512::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(sign_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("KEY".into(), api_key.to_string());
        headers.insert("Timestamp".into(), timestamp);
        headers.insert("SIGN".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        let url = if query_string.is_empty() {
            full_path
        } else {
            format!("{full_path}?{query_string}")
        };

        match method.to_uppercase().as_str() {
            "GET" => self.client.get(&url, None, Some(headers)).await,
            "POST" => self.client.post(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 포지션 파싱
    fn parse_position(&self, data: &GatePosition) -> Position {
        let timestamp = data.update_time.map(|t| (t * 1000.0) as i64);
        let symbol = data.contract.replace("_", "/");

        let size: Decimal = data.size.map(Decimal::from).unwrap_or_default();
        let side = if size > Decimal::ZERO {
            Some(PositionSide::Long)
        } else if size < Decimal::ZERO {
            Some(PositionSide::Short)
        } else {
            None
        };

        let contracts = Some(size.abs());
        let entry_price: Option<Decimal> = data.entry_price.as_ref().and_then(|p| p.parse().ok());
        let mark_price: Option<Decimal> = data.mark_price.as_ref().and_then(|p| p.parse().ok());
        let leverage: Option<Decimal> = data.leverage.map(Decimal::from);
        let unrealized_pnl: Option<Decimal> =
            data.unrealised_pnl.as_ref().and_then(|u| u.parse().ok());
        let liquidation_price: Option<Decimal> = data.liq_price.as_ref().and_then(|l| {
            let val: Decimal = l.parse().ok()?;
            if val == Decimal::ZERO {
                None
            } else {
                Some(val)
            }
        });
        let margin: Option<Decimal> = data.margin.as_ref().and_then(|m| m.parse().ok());

        let margin_mode = match data.mode.as_deref() {
            Some("cross") => Some(MarginMode::Cross),
            Some("single") => Some(MarginMode::Isolated),
            _ => None,
        };

        Position {
            symbol,
            id: None,
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            contracts,
            contract_size: None,
            side,
            notional: data.value.as_ref().and_then(|v| v.parse().ok()),
            leverage,
            collateral: margin,
            initial_margin: data.margin.as_ref().and_then(|m| m.parse().ok()),
            initial_margin_percentage: None,
            maintenance_margin: data.maintenance_rate.as_ref().and_then(|m| m.parse().ok()),
            maintenance_margin_percentage: None,
            entry_price,
            mark_price,
            last_price: None,
            liquidation_price,
            unrealized_pnl,
            realized_pnl: data.realised_pnl.as_ref().and_then(|p| p.parse().ok()),
            percentage: None,
            margin_mode,
            margin_ratio: None,
            hedged: Some(data.mode.as_deref() != Some("single")),
            last_update_timestamp: timestamp,
            stop_loss_price: None,
            take_profit_price: None,
        }
    }

    /// 펀딩 레이트 파싱
    fn parse_funding_rate(&self, data: &GateFundingRateData) -> FundingRate {
        let timestamp = data.t.map(|t| t * 1000);
        let symbol = data.contract.replace("_", "/");

        FundingRate {
            symbol,
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            funding_rate: data.r.as_ref().and_then(|r| r.parse().ok()),
            mark_price: data.mark_price.as_ref().and_then(|m| m.parse().ok()),
            index_price: data.index_price.as_ref().and_then(|i| i.parse().ok()),
            interest_rate: None,
            estimated_settle_price: None,
            funding_timestamp: data.next_funding_time.map(|t| t * 1000),
            funding_datetime: None,
            next_funding_rate: data.next_funding_rate.as_ref().and_then(|r| r.parse().ok()),
            next_funding_timestamp: data.next_funding_time.map(|t| t * 1000),
            next_funding_datetime: None,
            previous_funding_rate: None,
            previous_funding_timestamp: None,
            previous_funding_datetime: None,
            interval: Some("8h".to_string()),
        }
    }

    /// 펀딩 레이트 기록 파싱
    fn parse_funding_rate_history(
        &self,
        data: &GateFundingRateHistoryData,
        contract: &str,
    ) -> FundingRateHistory {
        let timestamp = Some(data.t * 1000);
        let symbol = contract.replace("_", "/");
        let funding_rate: Decimal = data.r.parse().unwrap_or_default();

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

    /// 선물 심볼 → 마켓 ID 변환 (BTC/USDT:USDT → BTC_USDT)
    fn to_futures_market_id(&self, symbol: &str) -> String {
        // Handle BTC/USDT:USDT format for perpetual swaps
        if symbol.contains(':') {
            let parts: Vec<&str> = symbol.split(':').collect();
            if let Some(base_quote) = parts.first() {
                return base_quote.replace("/", "_");
            }
        }
        symbol.replace("/", "_")
    }
}

#[async_trait]
impl Exchange for Gate {
    fn id(&self) -> ExchangeId {
        ExchangeId::Gate
    }

    fn name(&self) -> &str {
        "Gate.io"
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
        let response: Vec<GateCurrencyPair> = self.public_get("/currency_pairs", None).await?;

        let mut markets = Vec::new();

        for pair in response {
            if pair.trade_status != "tradable" {
                continue;
            }

            let symbol = pair.id.replace("_", "/");
            let parts: Vec<&str> = pair.id.split('_').collect();
            if parts.len() != 2 {
                continue;
            }

            let base = parts[0].to_string();
            let quote = parts[1].to_string();

            let market = Market {
                id: pair.id.clone(),
                lowercase_id: Some(pair.id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: pair.base.clone(),
                quote_id: pair.quote.clone(),
                settle: None,
                settle_id: None,
                active: true,
                market_type: MarketType::Spot,
                spot: true,
                margin: pair.margin.unwrap_or(false),
                swap: false,
                future: false,
                option: false,
                index: false,
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: pair.fee.as_ref().and_then(|f| f.parse().ok()),
                maker: pair.fee.as_ref().and_then(|f| f.parse().ok()),
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
            underlying: None,
            underlying_id: None,
                precision: MarketPrecision {
                    amount: pair.amount_precision,
                    price: pair.precision,
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&pair).unwrap_or_default(),
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
        params.insert("currency_pair".into(), market_id);

        let response: Vec<GateTicker> = self.public_get("/tickers", Some(params)).await?;

        let ticker = response.first().ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        Ok(self.parse_ticker(ticker, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<GateTicker> = self.public_get("/tickers", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let mut tickers = HashMap::new();

        for data in response {
            let market_id = &data.currency_pair;
            if let Some(symbol) = markets_by_id.get(market_id) {
                if let Some(filter) = symbols {
                    if !filter.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let ticker = self.parse_ticker(&data, symbol);
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("currency_pair".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: GateOrderBook = self.public_get("/order_book", Some(params)).await?;

        let bids: Vec<OrderBookEntry> = response
            .bids
            .iter()
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

        let asks: Vec<OrderBookEntry> = response
            .asks
            .iter()
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

        let timestamp = response
            .current
            .map(|t| (t * 1000.0) as i64)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

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
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("currency_pair".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<GateTrade> = self.public_get("/trades", Some(params)).await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| {
                let timestamp = t
                    .create_time
                    .map(|ts| (ts * 1000.0) as i64)
                    .unwrap_or_else(|| Utc::now().timestamp_millis());
                let price: Decimal = t.price.parse().unwrap_or_default();
                let amount: Decimal = t.amount.parse().unwrap_or_default();

                Trade {
                    id: t.id.clone().unwrap_or_default(),
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
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("currency_pair".into(), market_id);
        params.insert("interval".into(), interval.clone());
        if let Some(s) = since {
            params.insert("from".into(), (s / 1000).to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<Vec<String>> = self.public_get("/candlesticks", Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                if c.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].parse::<i64>().ok()? * 1000,
                    open: c[5].parse().ok()?,
                    high: c[3].parse().ok()?,
                    low: c[4].parse().ok()?,
                    close: c[2].parse().ok()?,
                    volume: c[1].parse().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: Vec<GateAccount> = self
            .private_request("GET", "/accounts", HashMap::new())
            .await?;

        Ok(self.parse_balance(&response))
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
        params.insert("currency_pair".into(), market_id);
        params.insert(
            "side".into(),
            match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            }
            .into(),
        );
        params.insert("amount".into(), amount.to_string());

        match order_type {
            OrderType::Limit => {
                let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                    message: "Limit order requires price".into(),
                })?;
                params.insert("type".into(), "limit".into());
                params.insert("price".into(), price_val.to_string());
            },
            OrderType::Market => {
                params.insert("type".into(), "market".into());
                params.insert("time_in_force".into(), "ioc".into());
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type: {order_type:?}"),
                })
            },
        }

        let response: GateOrder = self.private_request("POST", "/orders", params).await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("currency_pair".into(), market_id);

        let path = format!("/orders/{id}");
        let response: GateOrder = self.private_request("DELETE", &path, params).await?;

        Ok(self.parse_order(&response, symbol))
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
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("currency_pair".into(), market_id);

        if let Some(amt) = amount {
            params.insert("amount".into(), amt.to_string());
        }

        if let Some(p) = price {
            params.insert("price".into(), p.to_string());
        }

        let path = format!("/orders/{id}");
        let response: GateOrder = self.private_request("PATCH", &path, params).await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("currency_pair".into(), market_id);

        let path = format!("/orders/{id}");
        let response: GateOrder = self.private_request("GET", &path, params).await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Symbol is required".into(),
        })?;
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("currency_pair".into(), market_id);
        params.insert("status".into(), "open".into());

        let response: Vec<GateOrder> = self.private_request("GET", "/orders", params).await?;

        Ok(response
            .iter()
            .map(|o| self.parse_order(o, symbol))
            .collect())
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Symbol is required".into(),
        })?;
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("currency_pair".into(), market_id);
        params.insert("status".into(), "finished".into());
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<GateOrder> = self.private_request("GET", "/orders", params).await?;

        Ok(response
            .iter()
            .map(|o| self.parse_order(o, symbol))
            .collect())
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Symbol is required".into(),
        })?;
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("currency_pair".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<GateMyTrade> = self.private_request("GET", "/my_trades", params).await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| {
                let timestamp = t.create_time.map(|ts| (ts * 1000.0) as i64);
                let price: Decimal = t.price.parse().unwrap_or_default();
                let amount: Decimal = t.amount.parse().unwrap_or_default();
                let fee_amount: Option<Decimal> = t.fee.as_ref().and_then(|f| f.parse().ok());

                Trade {
                    id: t.id.clone().unwrap_or_default(),
                    order: t.order_id.clone(),
                    timestamp,
                    datetime: timestamp.and_then(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                    }),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: t.side.clone(),
                    taker_or_maker: t.role.as_ref().map(|r| {
                        if r == "maker" {
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
                        currency: t.fee_currency.clone(),
                        rate: None,
                    }),
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
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
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<GateDeposit> = self.private_request("GET", "/deposits", params).await?;

        let transactions: Vec<Transaction> =
            response.iter().map(|d| self.parse_deposit(d)).collect();

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
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<GateWithdrawal> =
            self.private_request("GET", "/withdrawals", params).await?;

        let transactions: Vec<Transaction> =
            response.iter().map(|w| self.parse_withdrawal(w)).collect();

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

        let response: GateWithdrawResponse =
            self.private_request("POST", "/withdrawals", params).await?;

        let info = serde_json::to_value(&response).unwrap_or_default();

        Ok(Transaction {
            id: response.id,
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

        let response: GateDepositAddress = self
            .private_request("GET", "/wallet/deposit_address", params)
            .await?;

        // If network specified, find the matching one
        let addr_data = if let Some(n) = network {
            response
                .multichain_addresses
                .iter()
                .find(|a| a.chain.eq_ignore_ascii_case(n))
                .ok_or_else(|| CcxtError::BadResponse {
                    message: format!("No deposit address found for {code} on {n}"),
                })?
        } else {
            response
                .multichain_addresses
                .first()
                .ok_or_else(|| CcxtError::BadResponse {
                    message: format!("No deposit address found for {code}"),
                })?
        };

        Ok(DepositAddress {
            currency: code.to_string(),
            address: addr_data.address.clone(),
            tag: addr_data.payment_id.clone(),
            network: Some(addr_data.chain.clone()),
            info: serde_json::to_value(&response).unwrap_or_default(),
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

    async fn watch_ticker(
        &self,
        symbol: &str,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_ticker(symbol).await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_tickers(symbols).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_order_book(symbol, limit).await
    }

    async fn watch_trades(
        &self,
        symbol: &str,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_trades(symbol).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_ohlcv(symbol, timeframe).await
    }

    // === Futures Methods ===

    async fn fetch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>> {
        let response: Vec<GatePosition> = self
            .futures_private_request("GET", "/positions", HashMap::new())
            .await?;

        let mut positions: Vec<Position> =
            response.iter().map(|p| self.parse_position(p)).collect();

        // Filter by symbols if provided
        if let Some(filter_symbols) = symbols {
            positions.retain(|p| filter_symbols.contains(&p.symbol.as_str()));
        }

        Ok(positions)
    }

    async fn set_leverage(&self, leverage: Decimal, symbol: &str) -> CcxtResult<Leverage> {
        let market_id = self.to_futures_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("contract".into(), market_id);
        params.insert("leverage".into(), leverage.to_string());

        let _response: GatePosition = self
            .futures_private_request("POST", "/positions/leverage", params)
            .await?;

        Ok(Leverage::new(symbol, MarginMode::Cross, leverage, leverage))
    }

    async fn fetch_leverage(&self, symbol: &str) -> CcxtResult<Leverage> {
        let positions = self.fetch_positions(Some(&[symbol])).await?;

        if let Some(pos) = positions.first() {
            let leverage = pos.leverage.unwrap_or(Decimal::ONE);
            let margin_mode = pos.margin_mode.clone().unwrap_or(MarginMode::Cross);
            Ok(Leverage::new(symbol, margin_mode, leverage, leverage))
        } else {
            Ok(Leverage::new(
                symbol,
                MarginMode::Cross,
                Decimal::ONE,
                Decimal::ONE,
            ))
        }
    }

    async fn set_margin_mode(
        &self,
        margin_mode: MarginMode,
        symbol: &str,
    ) -> CcxtResult<MarginModeInfo> {
        let market_id = self.to_futures_market_id(symbol);

        let mode_str = match margin_mode {
            MarginMode::Cross => "0",
            MarginMode::Isolated => "1",
            MarginMode::Unknown => {
                return Err(CcxtError::BadRequest {
                    message: "Unknown margin mode is not supported".into(),
                })
            },
        };

        let mut params = HashMap::new();
        params.insert("contract".into(), market_id);
        params.insert("cross_margin".into(), mode_str.into());

        let _response: GatePosition = self
            .futures_private_request("POST", "/positions/margin", params)
            .await?;

        Ok(MarginModeInfo::new(symbol, margin_mode))
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> CcxtResult<FundingRate> {
        let market_id = self.to_futures_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("contract".into(), market_id);

        let response: Vec<GateFundingRateData> = self
            .futures_public_get("/funding_rate", Some(params))
            .await?;

        let data = response.first().ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        Ok(self.parse_funding_rate(data))
    }

    async fn fetch_funding_rates(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, FundingRate>> {
        let response: Vec<GateFundingRateData> = self
            .futures_public_get("/funding_rate", None)
            .await
            .unwrap_or_default();

        let mut rates = HashMap::new();

        for data in response {
            let rate = self.parse_funding_rate(&data);

            if let Some(filter) = symbols {
                if !filter.contains(&rate.symbol.as_str()) {
                    continue;
                }
            }

            rates.insert(rate.symbol.clone(), rate);
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
        params.insert("contract".into(), market_id.clone());

        if let Some(s) = since {
            params.insert("from".into(), (s / 1000).to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<GateFundingRateHistoryData> = self
            .futures_public_get("/funding_rate", Some(params))
            .await?;

        let history: Vec<FundingRateHistory> = response
            .iter()
            .map(|d| self.parse_funding_rate_history(d, &market_id))
            .collect();

        Ok(history)
    }

    async fn fetch_open_interest(&self, symbol: &str) -> CcxtResult<OpenInterest> {
        let market_id = self.to_futures_market_id(symbol);

        let response: Vec<GateContractStats> = self
            .futures_public_get(&format!("/contracts/{market_id}"), None)
            .await?;

        let data = response.first().ok_or_else(|| CcxtError::BadResponse {
            message: "Empty contract response".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();
        let amount: Decimal = data
            .open_interest
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let value: Decimal = data
            .open_interest_usd
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        Ok(OpenInterest {
            symbol: symbol.to_string(),
            open_interest_amount: Some(amount),
            open_interest_value: Some(value),
            base_volume: None,
            quote_volume: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    async fn fetch_liquidations(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Liquidation>> {
        let market_id = self.to_futures_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("contract".into(), market_id);

        if let Some(s) = since {
            params.insert("from".into(), (s / 1000).to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<GateLiquidation> = self
            .futures_public_get("/liquidations", Some(params))
            .await?;

        let liquidations: Vec<Liquidation> = response
            .iter()
            .map(|liq| {
                let price: Decimal = liq.price.parse().unwrap_or_default();
                let qty: Decimal = liq.size.parse().unwrap_or_default();
                let timestamp = liq.time.map(|t| t * 1000).unwrap_or_default();
                let side = if liq.side.as_deref() == Some("short") {
                    OrderSide::Buy // Short liquidation = forced buy
                } else {
                    OrderSide::Sell // Long liquidation = forced sell
                };

                Liquidation {
                    info: serde_json::to_value(liq).unwrap_or_default(),
                    symbol: symbol.to_string(),
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    price,
                    base_value: Some(qty),
                    quote_value: Some(price * qty),
                    contracts: Some(qty),
                    contract_size: None,
                    side: Some(side),
                }
            })
            .collect();

        Ok(liquidations)
    }

    async fn fetch_index_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_futures_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("contract".into(), market_id);

        let response: Vec<GateFundingRateData> = self
            .futures_public_get("/funding_rate", Some(params))
            .await?;

        let data = response.first().ok_or_else(|| CcxtError::BadResponse {
            message: "Empty funding rate response".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();
        let index_price: Decimal = data
            .index_price
            .as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let mark_price: Decimal = data
            .mark_price
            .as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last: Some(index_price),
            mark_price: Some(mark_price),
            index_price: Some(index_price),
            info: serde_json::to_value(data).unwrap_or_default(),
            ..Default::default()
        })
    }

    /// Fetch server time
    async fn fetch_time(&self) -> CcxtResult<i64> {
        // Gate.io doesn't have a dedicated time endpoint
        // We use current local time as approximation
        Ok(Utc::now().timestamp_millis())
    }

    /// Fetch exchange status
    async fn fetch_status(&self) -> CcxtResult<crate::types::ExchangeStatus> {
        let response: GateSystemStatus = self
            .public_get("/api/v4/spot/time", None::<HashMap<String, String>>)
            .await
            .map(|r: GateTimeResponse| {
                tracing::debug!("Gate.io server_time: {}", r.server_time);
                GateSystemStatus { status: "ok".into() }
            })
            .unwrap_or_else(|_| GateSystemStatus { status: "maintenance".into() });

        if response.status == "ok" {
            Ok(crate::types::ExchangeStatus::ok())
        } else {
            Ok(crate::types::ExchangeStatus::maintenance(None))
        }
    }

    /// Fetch trading fee for a symbol
    async fn fetch_trading_fee(&self, symbol: &str) -> CcxtResult<crate::types::TradingFee> {
        let market_id = self.to_market_id(symbol);

        let response: GateTradingFee = self
            .private_request(
                "GET",
                &format!("/api/v4/wallet/fee?currency_pair={}", market_id),
                HashMap::new(),
            )
            .await?;

        let maker: Decimal = response.maker_fee.parse().unwrap_or_default();
        let taker: Decimal = response.taker_fee.parse().unwrap_or_default();

        Ok(crate::types::TradingFee::new(symbol, maker, taker))
    }

    /// Fetch trading fees for all symbols
    async fn fetch_trading_fees(&self) -> CcxtResult<HashMap<String, crate::types::TradingFee>> {
        // Get the user's fee tier
        let response: GateUserFee = self
            .private_request("GET", "/api/v4/wallet/fee", HashMap::new())
            .await
            .unwrap_or_else(|_| GateUserFee {
                maker_fee: "0.002".into(),
                taker_fee: "0.002".into(),
            });

        let maker: Decimal = response.maker_fee.parse().unwrap_or_default();
        let taker: Decimal = response.taker_fee.parse().unwrap_or_default();

        // Apply to all cached markets
        let markets = self.markets.read().unwrap();
        let mut fees = HashMap::new();

        for (symbol, _market) in markets.iter() {
            if !symbol.contains(":") {
                // Only spot symbols
                fees.insert(
                    symbol.clone(),
                    crate::types::TradingFee::new(symbol, maker, taker),
                );
            }
        }

        Ok(fees)
    }

    /// Fetch transfer history
    async fn fetch_transfers(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<TransferEntry>> {
        let mut path = "/api/v4/wallet/sub_account_transfers".to_string();
        let mut params: Vec<String> = Vec::new();

        if let Some(c) = code {
            params.push(format!("currency={}", c.to_uppercase()));
        }
        if let Some(l) = limit {
            params.push(format!("limit={}", l.min(100)));
        }

        if !params.is_empty() {
            path = format!("{}?{}", path, params.join("&"));
        }

        let response: Vec<GateTransferEntry> = self
            .private_request("GET", &path, HashMap::new())
            .await
            .unwrap_or_default();

        let transfers: Vec<TransferEntry> = response
            .iter()
            .map(|item| {
                let timestamp = item.timestamp.as_ref()
                    .and_then(|ts| ts.parse::<f64>().ok())
                    .map(|ts| (ts * 1000.0) as i64)
                    .unwrap_or(0);

                TransferEntry::new()
                    .with_id(item.id.clone().unwrap_or_default())
                    .with_currency(item.currency.clone().unwrap_or_default())
                    .with_amount(item.amount.parse().unwrap_or_default())
                    .with_from_account(item.from.clone().unwrap_or_default())
                    .with_to_account(item.to.clone().unwrap_or_default())
                    .with_timestamp(timestamp)
                    .with_status("success".to_string())
            })
            .collect();

        Ok(transfers)
    }

    /// Fetch account ledger (transaction history)
    async fn fetch_ledger(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<LedgerEntry>> {
        let mut path = "/api/v4/wallet/transactions".to_string();
        let mut params: Vec<String> = Vec::new();

        if let Some(c) = code {
            params.push(format!("currency={}", c.to_uppercase()));
        }
        if let Some(s) = since {
            params.push(format!("from={}", s / 1000)); // Gate uses seconds
        }
        if let Some(l) = limit {
            params.push(format!("limit={}", l.min(100)));
        }

        if !params.is_empty() {
            path = format!("{}?{}", path, params.join("&"));
        }

        let response: Vec<GateLedgerEntry> = self
            .private_request("GET", &path, HashMap::new())
            .await
            .unwrap_or_default();

        let ledger: Vec<LedgerEntry> = response
            .iter()
            .map(|item| {
                let amount: Decimal = item.amount.parse().unwrap_or_default();
                let direction = if amount >= Decimal::ZERO { "in" } else { "out" };
                let timestamp = item.timestamp.as_ref()
                    .and_then(|ts| ts.parse::<f64>().ok())
                    .map(|ts| (ts * 1000.0) as i64)
                    .unwrap_or(0);

                let mut entry = LedgerEntry::new()
                    .with_id(item.id.clone().unwrap_or_default())
                    .with_type(item.entry_type.clone().unwrap_or_default())
                    .with_currency(item.currency.clone().unwrap_or_default())
                    .with_amount(amount.abs());

                entry.direction = Some(direction.to_string());
                entry.timestamp = Some(timestamp);

                if let Some(balance) = &item.balance {
                    entry.after = balance.parse().ok();
                }

                entry
            })
            .collect();

        Ok(ledger)
    }

    /// Fetch cross margin borrow rate for a currency
    async fn fetch_cross_borrow_rate(
        &self,
        code: &str,
    ) -> CcxtResult<crate::types::CrossBorrowRate> {
        let currency = code.to_uppercase();
        let path = format!("/api/v4/margin/cross/currencies/{}", currency);

        let response: GateCrossBorrowRate = self
            .public_get(&path, None)
            .await?;

        let rate: Decimal = response.rate.parse().unwrap_or_default();

        Ok(crate::types::CrossBorrowRate::new(rate)
            .with_currency(code)
            .with_period(86400000)) // daily
    }

    /// Fetch isolated margin borrow rate for a symbol
    async fn fetch_isolated_borrow_rate(
        &self,
        symbol: &str,
    ) -> CcxtResult<crate::types::IsolatedBorrowRate> {
        let market_id = self.to_market_id(symbol);
        let path = format!("/api/v4/margin/uni/currency_pairs/{}", market_id);

        let response: GateIsolatedBorrowRate = self
            .public_get(&path, None)
            .await?;

        let base_rate: Decimal = response.base_rate.parse().unwrap_or_default();
        let quote_rate: Decimal = response.quote_rate.parse().unwrap_or_default();

        let parts: Vec<&str> = symbol.split('/').collect();
        let (base, quote) = if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            (symbol.to_string(), "USDT".to_string())
        };

        Ok(crate::types::IsolatedBorrowRate::new(
            symbol, base, base_rate, quote, quote_rate,
        ))
    }

    // === Convert 기능 ===

    async fn fetch_convert_quote(
        &self,
        from_code: &str,
        to_code: &str,
        amount: Decimal,
    ) -> CcxtResult<ConvertQuote> {
        let path = "/api/v4/flash_swap/currency_pairs";

        // Get available pairs first
        let pairs: Vec<GateConvertPair> = self.public_get(path, None).await?;

        // Find matching pair
        let pair_name = format!("{}_{}", from_code.to_uppercase(), to_code.to_uppercase());
        let reverse_pair_name = format!("{}_{}", to_code.to_uppercase(), from_code.to_uppercase());

        let found_pair = pairs.iter().find(|p| p.currency_pair == pair_name || p.currency_pair == reverse_pair_name);

        let pair = match found_pair {
            Some(p) => p,
            None => {
                return Err(CcxtError::ExchangeError {
                    message: format!("Convert pair not found: {} to {}", from_code, to_code),
                });
            }
        };

        // Validate amount against pair limits
        let min_amount: Decimal = pair.min_amount.parse().unwrap_or_default();
        let max_amount: Decimal = pair.max_amount.parse().unwrap_or_default();
        if min_amount > Decimal::ZERO && amount < min_amount {
            return Err(CcxtError::ExchangeError {
                message: format!(
                    "Amount {} is below minimum {} for {}/{}",
                    amount, min_amount, pair.sell_currency, pair.buy_currency
                ),
            });
        }
        if max_amount > Decimal::ZERO && amount > max_amount {
            return Err(CcxtError::ExchangeError {
                message: format!(
                    "Amount {} exceeds maximum {} for {}/{}",
                    amount, max_amount, pair.sell_currency, pair.buy_currency
                ),
            });
        }

        // Request quote
        let quote_path = "/api/v4/flash_swap/orders/preview";
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("sell_currency".into(), from_code.to_uppercase());
        params.insert("buy_currency".into(), to_code.to_uppercase());
        params.insert("sell_amount".into(), amount.to_string());

        let response: GateConvertQuote = self
            .private_request("POST", quote_path, params)
            .await?;

        let price: Decimal = response.price.parse().unwrap_or_default();
        let sell_amount: Decimal = response.sell_amount.parse().unwrap_or(amount);

        // Generate a quote ID based on timestamp and currencies from response
        let quote_id = format!(
            "{}_{}_{}",
            chrono::Utc::now().timestamp_millis(),
            response.sell_currency,
            response.buy_currency
        );

        let mut quote = ConvertQuote::new(
            &quote_id,
            &response.sell_currency,
            &response.buy_currency,
            sell_amount,
            price,
        );
        let to_amount: Decimal = response.buy_amount.parse().unwrap_or_default();
        quote.to_amount = Some(to_amount);

        Ok(quote)
    }

    async fn create_convert_trade(&self, quote_id: &str) -> CcxtResult<ConvertTrade> {
        // Gate.io flash_swap doesn't use quote_id directly, need to re-parse the info
        // For now, create a direct order using stored quote info
        // In practice, you'd store quote info from fetch_convert_quote and use it here

        // Parse quote_id to extract currencies (format: timestamp_FROM_TO)
        let parts: Vec<&str> = quote_id.split('_').collect();
        if parts.len() < 3 {
            return Err(CcxtError::ExchangeError {
                message: "Invalid quote_id format".to_string(),
            });
        }

        let _timestamp = parts[0];
        let from_code = parts[1];
        let to_code = parts[2];

        // Create order directly (Gate.io flash_swap creates order in one step)
        let path = "/api/v4/flash_swap/orders";
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("sell_currency".into(), from_code.to_string());
        params.insert("buy_currency".into(), to_code.to_string());
        // Note: In real implementation, you'd need to track the amount from the quote
        params.insert("sell_amount".into(), "0".to_string()); // Placeholder

        let response: GateConvertOrder = self
            .private_request("POST", path, params)
            .await?;

        let from_amount: Decimal = response.sell_amount.parse().unwrap_or_default();
        let to_amount: Decimal = response.buy_amount.parse().unwrap_or_default();
        let price: Decimal = response.price.parse().unwrap_or_default();

        let mut trade = ConvertTrade::new(
            &response.id.to_string(),
            &response.sell_currency,
            &response.buy_currency,
            from_amount,
            to_amount,
            price,
        );
        trade.timestamp = response.create_time.map(|t| t * 1000);
        trade.status = Some(response.status);

        Ok(trade)
    }
}

// === Gate.io API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct GateCurrencyPair {
    id: String,
    base: String,
    quote: String,
    #[serde(default)]
    fee: Option<String>,
    trade_status: String,
    #[serde(default)]
    margin: Option<bool>,
    #[serde(default)]
    amount_precision: Option<i32>,
    #[serde(default)]
    precision: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateTicker {
    currency_pair: String,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    highest_bid: Option<String>,
    #[serde(default)]
    lowest_ask: Option<String>,
    #[serde(default)]
    change_percentage: Option<String>,
    #[serde(default)]
    base_volume: Option<String>,
    #[serde(default)]
    quote_volume: Option<String>,
    #[serde(default)]
    high_24h: Option<String>,
    #[serde(default)]
    low_24h: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GateOrderBook {
    #[serde(default)]
    current: Option<f64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    create_time: Option<f64>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    amount: String,
    #[serde(default)]
    price: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateOrder {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    create_time: Option<f64>,
    #[serde(default)]
    update_time: Option<f64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(rename = "type", default)]
    order_type: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    amount: String,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    avg_deal_price: Option<String>,
    #[serde(default)]
    filled_total: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GateAccount {
    currency: String,
    available: String,
    locked: String,
}

/// My trade response
#[derive(Debug, Deserialize, Serialize)]
struct GateMyTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    create_time: Option<f64>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    amount: String,
    #[serde(default)]
    price: String,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    fee_currency: Option<String>,
}

/// Deposit response
#[derive(Debug, Deserialize, Serialize)]
struct GateDeposit {
    #[serde(default)]
    id: Option<String>,
    currency: String,
    #[serde(default)]
    chain: Option<String>,
    #[serde(default)]
    amount: String,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    memo: Option<String>,
    #[serde(default)]
    txid: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    timestamp: Option<f64>,
}

/// Withdrawal response
#[derive(Debug, Deserialize, Serialize)]
struct GateWithdrawal {
    #[serde(default)]
    id: Option<String>,
    currency: String,
    #[serde(default)]
    chain: Option<String>,
    #[serde(default)]
    amount: String,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    memo: Option<String>,
    #[serde(default)]
    txid: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    timestamp: Option<f64>,
}

/// Withdraw response
#[derive(Debug, Deserialize, Serialize)]
struct GateWithdrawResponse {
    id: String,
}

/// Deposit address response
#[derive(Debug, Deserialize, Serialize)]
struct GateDepositAddress {
    #[serde(default)]
    currency: String,
    #[serde(default)]
    multichain_addresses: Vec<GateMultichainAddress>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateMultichainAddress {
    #[serde(default)]
    chain: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    payment_id: Option<String>,
}

// === Futures Response Types ===

/// Position response
#[derive(Debug, Deserialize, Serialize)]
struct GatePosition {
    contract: String,
    #[serde(default)]
    size: Option<i64>,
    #[serde(default)]
    leverage: Option<i64>,
    #[serde(default)]
    entry_price: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    liq_price: Option<String>,
    #[serde(default)]
    unrealised_pnl: Option<String>,
    #[serde(default)]
    realised_pnl: Option<String>,
    #[serde(default)]
    margin: Option<String>,
    #[serde(default)]
    maintenance_rate: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    update_time: Option<f64>,
}

/// Funding rate response
#[derive(Debug, Deserialize, Serialize)]
struct GateFundingRateData {
    contract: String,
    #[serde(default)]
    r: Option<String>,
    #[serde(default)]
    t: Option<i64>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    index_price: Option<String>,
    #[serde(default)]
    next_funding_time: Option<i64>,
    #[serde(default)]
    next_funding_rate: Option<String>,
}

/// Funding rate history response
#[derive(Debug, Deserialize, Serialize)]
struct GateFundingRateHistoryData {
    r: String,
    t: i64,
}

/// Contract stats response (for open interest)
#[derive(Debug, Deserialize, Serialize)]
struct GateContractStats {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    open_interest: Option<String>,
    #[serde(default)]
    open_interest_usd: Option<String>,
}

/// Liquidation response
#[derive(Debug, Deserialize, Serialize)]
struct GateLiquidation {
    #[serde(default)]
    contract: Option<String>,
    #[serde(default)]
    side: Option<String>,
    price: String,
    size: String,
    #[serde(default)]
    time: Option<i64>,
}

/// Time response
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GateTimeResponse {
    #[serde(default)]
    server_time: i64,
}

/// System status (internal use)
#[derive(Debug)]
struct GateSystemStatus {
    status: String,
}

/// Trading fee response
#[derive(Debug, Deserialize)]
struct GateTradingFee {
    #[serde(default)]
    maker_fee: String,
    #[serde(default)]
    taker_fee: String,
}

/// User fee tier
#[derive(Debug, Deserialize)]
struct GateUserFee {
    #[serde(default)]
    maker_fee: String,
    #[serde(default)]
    taker_fee: String,
}

/// Transfer entry from /api/v4/wallet/sub_account_transfers
#[derive(Debug, Deserialize, Default)]
struct GateTransferEntry {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    amount: String,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
}

/// Ledger entry from /api/v4/wallet/transactions
#[derive(Debug, Deserialize, Default)]
struct GateLedgerEntry {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default, rename = "type")]
    entry_type: Option<String>,
    #[serde(default)]
    amount: String,
    #[serde(default)]
    balance: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
}

/// Cross borrow rate response from /api/v4/margin/cross/currencies/{currency}
#[derive(Debug, Deserialize, Default)]
struct GateCrossBorrowRate {
    #[serde(default)]
    rate: String,
}

/// Isolated borrow rate response from /api/v4/margin/uni/currency_pairs/{pair}
#[derive(Debug, Deserialize, Default)]
struct GateIsolatedBorrowRate {
    #[serde(default)]
    base_rate: String,
    #[serde(default)]
    quote_rate: String,
}

/// Convert currency pair from /api/v4/flash_swap/currency_pairs
#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
struct GateConvertPair {
    #[serde(default)]
    currency_pair: String,
    #[serde(default)]
    sell_currency: String,
    #[serde(default)]
    buy_currency: String,
    #[serde(default)]
    min_amount: String,
    #[serde(default)]
    max_amount: String,
}

/// Convert quote response from /api/v4/flash_swap/orders/preview
#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
struct GateConvertQuote {
    #[serde(default)]
    sell_currency: String,
    #[serde(default)]
    buy_currency: String,
    #[serde(default)]
    sell_amount: String,
    #[serde(default)]
    buy_amount: String,
    #[serde(default)]
    price: String,
}

/// Convert order response from /api/v4/flash_swap/orders
#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
struct GateConvertOrder {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    sell_currency: String,
    #[serde(default)]
    buy_currency: String,
    #[serde(default)]
    sell_amount: String,
    #[serde(default)]
    buy_amount: String,
    #[serde(default)]
    price: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    create_time: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Gate::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Gate);
        assert_eq!(exchange.name(), "Gate.io");
    }
}
