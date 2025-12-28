//! OKX Exchange Implementation
//!
//! CCXT okx.ts를 Rust로 포팅

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, FundingRate,
    FundingRateHistory, Leverage, Liquidation, Market, MarketLimits, MarketPrecision,
    MarketType, MarginMode, MarginModeInfo, OpenInterest, Order, OrderBook, OrderBookEntry,
    OrderRequest, OrderSide, OrderStatus, OrderType, Position, PositionSide, SignedRequest,
    Ticker, Timeframe, TimeInForce, Trade, Transaction, TransactionStatus, TransactionType,
    TransferEntry, WsExchange, WsMessage, OHLCV,
};

use super::okx_ws::OkxWs;

type HmacSha256 = Hmac<Sha256>;

/// OKX 거래소
pub struct Okx {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    passphrase: Option<String>,
    ws_client: Arc<TokioRwLock<OkxWs>>,
}

impl Okx {
    const BASE_URL: &'static str = "https://www.okx.com";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

    /// 새 OKX 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        Self::with_passphrase(config, None)
    }

    /// Passphrase와 함께 OKX 인스턴스 생성
    pub fn with_passphrase(config: ExchangeConfig, passphrase: Option<String>) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: true,
            swap: true,
            future: true,
            option: true,
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
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            fetch_deposit_address: true,
            fetch_positions: true,
            set_leverage: true,
            fetch_leverage: true,
            fetch_funding_rate: true,
            fetch_funding_rates: true,
            fetch_open_interest: true,
            fetch_liquidations: true,
            fetch_index_price: true,
            ws: true,
            watch_ticker: true,
            watch_tickers: true,
            watch_order_book: true,
            watch_trades: true,
            watch_ohlcv: true,
            watch_balance: true,
            watch_orders: true,
            watch_my_trades: true,
            watch_positions: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/152485636-38b19e4a-bece-4dec-979a-5982859ffc04.jpg".into()),
            api: api_urls,
            www: Some("https://www.okx.com".into()),
            doc: vec![
                "https://www.okx.com/docs-v5/en/".into(),
            ],
            fees: Some("https://www.okx.com/fees".into()),
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
            ws_client: Arc::new(TokioRwLock::new(OkxWs::new())),
        })
    }

    /// 공개 API 호출
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

        let response: OkxResponse<T> = self.client.get(&url, None, None).await?;

        if response.code != "0" {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_else(|| format!("OKX error code: {}", response.code)),
            });
        }

        response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty response data".into(),
        })
    }

    /// 비공개 API 호출
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
        let passphrase = self.passphrase.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Passphrase required for OKX".into(),
        })?;

        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let body_str = body.unwrap_or("");

        // Create signature: timestamp + method + path + body
        let prehash = format!("{}{}{}{}", timestamp, method.to_uppercase(), path, body_str);

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(prehash.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("OK-ACCESS-KEY".into(), api_key.to_string());
        headers.insert("OK-ACCESS-SIGN".into(), signature);
        headers.insert("OK-ACCESS-TIMESTAMP".into(), timestamp);
        headers.insert("OK-ACCESS-PASSPHRASE".into(), passphrase.clone());
        headers.insert("Content-Type".into(), "application/json".into());

        let response: OkxResponse<T> = match method.to_uppercase().as_str() {
            "GET" => self.client.get(path, None, Some(headers)).await?,
            "POST" => {
                let json_body: Option<serde_json::Value> = body.map(|b| serde_json::from_str(b).unwrap_or_default());
                self.client.post(path, json_body, Some(headers)).await?
            }
            _ => return Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        };

        if response.code != "0" {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_else(|| format!("OKX error code: {}", response.code)),
            });
        }

        response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty response data".into(),
        })
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USDT → BTC-USDT)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// 마켓 ID → 심볼 변환 (BTC-USDT → BTC/USDT)
    #[allow(dead_code)]
    fn to_symbol(&self, market_id: &str) -> String {
        market_id.replace("-", "/")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &OkxTicker) -> Ticker {
        let timestamp = data.ts.parse::<i64>().unwrap_or_else(|_| Utc::now().timestamp_millis());
        let symbol = data.inst_id.replace("-", "/");

        Ticker {
            symbol,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid_px.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_sz.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask_px.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_sz.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open24h.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.vol24h.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.vol_ccy24h.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &OkxOrder) -> Order {
        let status = match data.state.as_str() {
            "live" => OrderStatus::Open,
            "partially_filled" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "canceled" | "cancelled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match data.ord_type.as_str() {
            "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            "post_only" => OrderType::LimitMaker,
            "ioc" => OrderType::Limit,
            "fok" => OrderType::Limit,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = match data.ord_type.as_str() {
            "ioc" => Some(TimeInForce::IOC),
            "fok" => Some(TimeInForce::FOK),
            "post_only" => Some(TimeInForce::PO),
            _ => Some(TimeInForce::GTC),
        };

        let price: Option<Decimal> = data.px.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.sz.parse().unwrap_or_default();
        let filled: Decimal = data.acc_fill_sz.as_ref().and_then(|f| f.parse().ok()).unwrap_or_default();
        let remaining = Some(amount - filled);
        let average = data.avg_px.as_ref().and_then(|a| a.parse().ok());
        let cost = average.map(|avg: Decimal| avg * filled);
        let timestamp = data.c_time.as_ref().and_then(|t| t.parse().ok());
        let symbol = data.inst_id.replace("-", "/");

        Order {
            id: data.ord_id.clone(),
            client_order_id: data.cl_ord_id.clone(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: data.u_time.as_ref().and_then(|t| t.parse().ok()),
            last_update_timestamp: data.u_time.as_ref().and_then(|t| t.parse().ok()),
            status,
            symbol,
            order_type,
            time_in_force,
            side,
            price,
            average,
            amount,
            filled,
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: data.reduce_only.as_ref().map(|r| r == "true"),
            post_only: Some(data.ord_type == "post_only"),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[OkxBalanceDetail]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let available: Option<Decimal> = b.avail_bal.as_ref().and_then(|v| v.parse().ok());
            let frozen: Option<Decimal> = b.frozen_bal.as_ref().and_then(|v| v.parse().ok());
            let total = match (available, frozen) {
                (Some(a), Some(f)) => Some(a + f),
                (Some(a), None) => Some(a),
                _ => None,
            };

            let balance = Balance {
                free: available,
                used: frozen,
                total,
                debt: None,
            };
            result.add(&b.ccy, balance);
        }

        result
    }

    /// 입금 내역 파싱
    fn parse_deposit(&self, data: &OkxDeposit) -> Transaction {
        let status = match data.state.as_str() {
            "0" => TransactionStatus::Pending,  // waiting for confirmation
            "1" => TransactionStatus::Pending,  // deposit credited
            "2" => TransactionStatus::Ok,       // completed
            "8" => TransactionStatus::Pending,  // waiting for manual review
            "11" => TransactionStatus::Pending, // match the address
            "12" => TransactionStatus::Pending, // on-chain confirmation
            _ => TransactionStatus::Pending,
        };

        let timestamp = data.ts.parse::<i64>().ok();
        let amount: Decimal = data.amt.parse().unwrap_or_default();

        Transaction {
            id: data.dep_id.clone(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: None,
            tx_type: TransactionType::Deposit,
            currency: data.ccy.clone(),
            network: data.chain.clone(),
            amount,
            status,
            address: data.to.clone(),
            tag: None,
            txid: data.tx_id.clone(),
            fee: None,
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 출금 내역 파싱
    fn parse_withdrawal(&self, data: &OkxWithdrawal) -> Transaction {
        let status = match data.state.as_str() {
            "-3" => TransactionStatus::Canceled, // canceling
            "-2" => TransactionStatus::Canceled, // canceled
            "-1" => TransactionStatus::Failed,   // failed
            "0" => TransactionStatus::Pending,   // waiting withdrawal
            "1" => TransactionStatus::Pending,   // withdrawing
            "2" => TransactionStatus::Ok,        // withdraw success
            "7" => TransactionStatus::Pending,   // approved
            "10" => TransactionStatus::Pending,  // waiting transfer
            "4" | "5" | "6" | "8" | "9" => TransactionStatus::Pending,
            _ => TransactionStatus::Pending,
        };

        let timestamp = data.ts.parse::<i64>().ok();
        let amount: Decimal = data.amt.parse().unwrap_or_default();
        let fee_amount: Option<Decimal> = data.fee.as_ref().and_then(|f| f.parse().ok());

        Transaction {
            id: data.wd_id.clone(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: None,
            tx_type: TransactionType::Withdrawal,
            currency: data.ccy.clone(),
            network: data.chain.clone(),
            amount,
            status,
            address: data.to.clone(),
            tag: None,
            txid: data.tx_id.clone(),
            fee: fee_amount.map(|cost| crate::types::Fee {
                cost: Some(cost),
                currency: Some(data.ccy.clone()),
                rate: None,
            }),
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 포지션 파싱
    fn parse_position(&self, data: &OkxPosition) -> Position {
        let timestamp = data.u_time.as_ref().and_then(|t| t.parse::<i64>().ok());
        let symbol = data.inst_id.replace("-", "/");

        let side = match data.pos_side.as_deref() {
            Some("long") => Some(PositionSide::Long),
            Some("short") => Some(PositionSide::Short),
            Some("net") => {
                // Determine side based on position amount
                let pos: Decimal = data.pos.as_ref().and_then(|p| p.parse().ok()).unwrap_or_default();
                if pos > Decimal::ZERO {
                    Some(PositionSide::Long)
                } else if pos < Decimal::ZERO {
                    Some(PositionSide::Short)
                } else {
                    None
                }
            }
            _ => None,
        };

        let contracts: Option<Decimal> = data.pos.as_ref().and_then(|p| p.parse().ok()).map(|p: Decimal| p.abs());
        let entry_price: Option<Decimal> = data.avg_px.as_ref().and_then(|p| p.parse().ok());
        let mark_price: Option<Decimal> = data.mark_px.as_ref().and_then(|p| p.parse().ok());
        let notional: Option<Decimal> = data.notional_usd.as_ref().and_then(|n| n.parse().ok());
        let leverage: Option<Decimal> = data.lever.as_ref().and_then(|l| l.parse().ok());
        let unrealized_pnl: Option<Decimal> = data.upl.as_ref().and_then(|u| u.parse().ok());
        let liquidation_price: Option<Decimal> = data.liq_px.as_ref().and_then(|l| {
            let val: Decimal = l.parse().ok()?;
            if val == Decimal::ZERO { None } else { Some(val) }
        });
        let margin: Option<Decimal> = data.margin.as_ref().and_then(|m| m.parse().ok());

        let margin_mode = match data.mgn_mode.as_deref() {
            Some("cross") => Some(MarginMode::Cross),
            Some("isolated") => Some(MarginMode::Isolated),
            _ => None,
        };

        Position {
            symbol,
            id: data.pos_id.clone(),
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            contracts,
            contract_size: None,
            side,
            notional,
            leverage,
            collateral: margin,
            initial_margin: None,
            initial_margin_percentage: None,
            maintenance_margin: None,
            maintenance_margin_percentage: None,
            entry_price,
            mark_price,
            last_price: None,
            liquidation_price,
            unrealized_pnl,
            realized_pnl: None,
            percentage: None,
            margin_mode,
            margin_ratio: None,
            hedged: Some(data.pos_side.as_deref() != Some("net")),
            last_update_timestamp: timestamp,
            stop_loss_price: None,
            take_profit_price: None,
        }
    }

    /// 펀딩 레이트 파싱
    fn parse_funding_rate(&self, data: &OkxFundingRateData) -> FundingRate {
        let timestamp = data.ts.as_ref().and_then(|t| t.parse::<i64>().ok());
        let symbol = data.inst_id.replace("-", "/");

        FundingRate {
            symbol,
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            funding_rate: data.funding_rate.as_ref().and_then(|r| r.parse().ok()),
            mark_price: None,
            index_price: None,
            interest_rate: None,
            estimated_settle_price: None,
            funding_timestamp: data.next_funding_time.as_ref().and_then(|t| t.parse().ok()),
            funding_datetime: None,
            next_funding_rate: data.next_funding_rate.as_ref().and_then(|r| r.parse().ok()),
            next_funding_timestamp: data.next_funding_time.as_ref().and_then(|t| t.parse().ok()),
            next_funding_datetime: None,
            previous_funding_rate: None,
            previous_funding_timestamp: None,
            previous_funding_datetime: None,
            interval: Some("8h".to_string()), // OKX default funding interval
        }
    }

    /// 펀딩 레이트 기록 파싱
    fn parse_funding_rate_history(&self, data: &OkxFundingRateHistoryData) -> FundingRateHistory {
        let timestamp = data.funding_time.as_ref().and_then(|t| t.parse::<i64>().ok());
        let symbol = data.inst_id.replace("-", "/");
        let funding_rate: Decimal = data.funding_rate.as_ref()
            .and_then(|r| r.parse().ok())
            .unwrap_or_default();

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

    /// 선물 심볼 → 마켓 ID 변환 (BTC/USDT:USDT → BTC-USDT-SWAP)
    fn to_swap_market_id(&self, symbol: &str) -> String {
        // Handle BTC/USDT:USDT format for perpetual swaps
        if symbol.contains(':') {
            let parts: Vec<&str> = symbol.split(':').collect();
            if let Some(base_quote) = parts.first() {
                let bq: Vec<&str> = base_quote.split('/').collect();
                if bq.len() == 2 {
                    return format!("{}-{}-SWAP", bq[0], bq[1]);
                }
            }
        }
        // Fall back to regular conversion
        format!("{}-SWAP", symbol.replace("/", "-"))
    }
}

#[async_trait]
impl Exchange for Okx {
    fn id(&self) -> ExchangeId {
        ExchangeId::Okx
    }

    fn name(&self) -> &str {
        "OKX"
    }

    fn version(&self) -> &str {
        "v5"
    }

    fn countries(&self) -> &[&str] {
        &["SC"] // Seychelles
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
        // Fetch spot markets
        let mut params = HashMap::new();
        params.insert("instType".into(), "SPOT".into());

        let response: Vec<OkxInstrument> = self
            .public_get("/api/v5/public/instruments", Some(params))
            .await?;

        let mut markets = Vec::new();

        for inst in response {
            let base = inst.base_ccy.clone();
            let quote = inst.quote_ccy.clone();
            let symbol = format!("{base}/{quote}");

            let lot_sz: Option<Decimal> = inst.lot_sz.as_ref().and_then(|v| v.parse().ok());
            let tick_sz: Option<Decimal> = inst.tick_sz.as_ref().and_then(|v| v.parse().ok());
            let min_sz: Option<Decimal> = inst.min_sz.as_ref().and_then(|v| v.parse().ok());

            let market = Market {
                id: inst.inst_id.clone(),
                lowercase_id: Some(inst.inst_id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: inst.base_ccy.clone(),
                quote_id: inst.quote_ccy.clone(),
                settle: None,
                settle_id: None,
                active: inst.state == "live",
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
                taker: Some(Decimal::new(1, 3)), // 0.1%
                maker: Some(Decimal::new(8, 4)), // 0.08%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: lot_sz.map(Self::count_decimals),
                    price: tick_sz.map(Self::count_decimals),
                    cost: None,
                    base: lot_sz.map(Self::count_decimals),
                    quote: tick_sz.map(Self::count_decimals),
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax {
                        min: min_sz,
                        max: inst.max_lmt_sz.as_ref().and_then(|v| v.parse().ok()),
                    },
                    price: crate::types::MinMax::default(),
                    cost: crate::types::MinMax::default(),
                    leverage: crate::types::MinMax::default(),
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&inst).unwrap_or_default(),
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
        params.insert("instId".into(), market_id);

        let response: Vec<OkxTicker> = self
            .public_get("/api/v5/market/ticker", Some(params))
            .await?;

        let ticker_data = response.first().ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        Ok(self.parse_ticker(ticker_data))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let mut params = HashMap::new();
        params.insert("instType".into(), "SPOT".into());

        let response: Vec<OkxTicker> = self
            .public_get("/api/v5/market/tickers", Some(params))
            .await?;

        let mut tickers = HashMap::new();

        for data in response {
            let ticker = self.parse_ticker(&data);

            if let Some(filter) = symbols {
                if !filter.contains(&ticker.symbol.as_str()) {
                    continue;
                }
            }

            tickers.insert(ticker.symbol.clone(), ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);
        if let Some(l) = limit {
            params.insert("sz".into(), l.min(400).to_string());
        }

        let response: Vec<OkxOrderBook> = self
            .public_get("/api/v5/market/books", Some(params))
            .await?;

        let book_data = response.first().ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        let bids: Vec<OrderBookEntry> = book_data
            .bids
            .iter()
            .filter_map(|b| {
                if b.len() >= 2 {
                    Some(OrderBookEntry {
                        price: b[0].parse().unwrap_or_default(),
                        amount: b[1].parse().unwrap_or_default(),
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = book_data
            .asks
            .iter()
            .filter_map(|a| {
                if a.len() >= 2 {
                    Some(OrderBookEntry {
                        price: a[0].parse().unwrap_or_default(),
                        amount: a[1].parse().unwrap_or_default(),
                    })
                } else {
                    None
                }
            })
            .collect();

        let timestamp = book_data.ts.parse::<i64>().ok();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
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
        params.insert("instId".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<OkxTrade> = self
            .public_get("/api/v5/market/trades", Some(params))
            .await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| {
                let timestamp = t.ts.parse::<i64>().ok();
                Trade {
                    id: t.trade_id.clone(),
                    order: None,
                    timestamp,
                    datetime: timestamp.and_then(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                    }),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(t.side.clone()),
                    taker_or_maker: None,
                    price: t.px.parse().unwrap_or_default(),
                    amount: t.sz.parse().unwrap_or_default(),
                    cost: Some(
                        t.px.parse::<Decimal>().unwrap_or_default()
                            * t.sz.parse::<Decimal>().unwrap_or_default(),
                    ),
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
        let bar = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {timeframe:?}"),
        })?;

        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);
        params.insert("bar".into(), bar.clone());
        if let Some(s) = since {
            params.insert("after".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(300).to_string());
        }

        let response: Vec<Vec<String>> = self
            .public_get("/api/v5/market/candles", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                if c.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].parse().ok()?,
                    open: c[1].parse().ok()?,
                    high: c[2].parse().ok()?,
                    low: c[3].parse().ok()?,
                    close: c[4].parse().ok()?,
                    volume: c[5].parse().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: Vec<OkxBalance> = self
            .private_request("GET", "/api/v5/account/balance", None)
            .await?;

        let balance_data = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty balance response".into(),
        })?;

        Ok(self.parse_balance(&balance_data.details))
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

        let ord_type = match order_type {
            OrderType::Limit => "limit",
            OrderType::Market => "market",
            OrderType::LimitMaker => "post_only",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {order_type:?}"),
            }),
        };

        let side_str = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };

        let mut body = serde_json::json!({
            "instId": market_id,
            "tdMode": "cash",
            "side": side_str,
            "ordType": ord_type,
            "sz": amount.to_string(),
        });

        if order_type == OrderType::Limit || order_type == OrderType::LimitMaker {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            body["px"] = serde_json::json!(price_val.to_string());
        }

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let response: Vec<OkxOrderResponse> = self
            .private_request("POST", "/api/v5/trade/order", Some(&body_str))
            .await?;

        let order_resp = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty order response".into(),
        })?;

        if order_resp.s_code != "0" {
            return Err(CcxtError::ExchangeError {
                message: order_resp.s_msg.clone().unwrap_or_else(|| "Order failed".into()),
            });
        }

        // Fetch the order details
        self.fetch_order(&order_resp.ord_id, symbol).await
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let body = serde_json::json!({
            "instId": market_id,
            "ordId": id,
        });

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let response: Vec<OkxOrderResponse> = self
            .private_request("POST", "/api/v5/trade/cancel-order", Some(&body_str))
            .await?;

        let order_resp = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty cancel response".into(),
        })?;

        if order_resp.s_code != "0" {
            return Err(CcxtError::ExchangeError {
                message: order_resp.s_msg.clone().unwrap_or_else(|| "Cancel failed".into()),
            });
        }

        self.fetch_order(id, symbol).await
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

        let mut body = serde_json::json!({
            "instId": market_id,
            "ordId": id,
        });

        if let Some(amt) = amount {
            body["newSz"] = serde_json::json!(amt.to_string());
        }

        if let Some(p) = price {
            body["newPx"] = serde_json::json!(p.to_string());
        }

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let response: Vec<OkxOrderResponse> = self
            .private_request("POST", "/api/v5/trade/amend-order", Some(&body_str))
            .await?;

        let order_resp = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty amend response".into(),
        })?;

        if order_resp.s_code != "0" {
            return Err(CcxtError::ExchangeError {
                message: order_resp.s_msg.clone().unwrap_or_else(|| "Amend failed".into()),
            });
        }

        self.fetch_order(id, symbol).await
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let path = format!("/api/v5/trade/order?instId={market_id}&ordId={id}");
        let response: Vec<OkxOrder> = self
            .private_request("GET", &path, None)
            .await?;

        let order_data = response.first().ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.into(),
        })?;

        Ok(self.parse_order(order_data))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let path = if let Some(s) = symbol {
            let market_id = self.to_market_id(s);
            format!("/api/v5/trade/orders-pending?instId={market_id}")
        } else {
            "/api/v5/trade/orders-pending".into()
        };

        let response: Vec<OkxOrder> = self
            .private_request("GET", &path, None)
            .await?;

        let orders: Vec<Order> = response.iter().map(|o| self.parse_order(o)).collect();

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut path = if let Some(s) = symbol {
            let market_id = self.to_market_id(s);
            format!("/api/v5/trade/orders-history-archive?instType=SPOT&instId={market_id}")
        } else {
            "/api/v5/trade/orders-history-archive?instType=SPOT".into()
        };

        if let Some(l) = limit {
            path = format!("{}&limit={}", path, l.min(100));
        }

        let response: Vec<OkxOrder> = self
            .private_request("GET", &path, None)
            .await?;

        let orders: Vec<Order> = response
            .iter()
            .filter(|o| o.state == "filled" || o.state == "canceled" || o.state == "cancelled")
            .map(|o| self.parse_order(o))
            .collect();

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut path = if let Some(s) = symbol {
            let market_id = self.to_market_id(s);
            format!("/api/v5/trade/fills?instType=SPOT&instId={market_id}")
        } else {
            "/api/v5/trade/fills?instType=SPOT".into()
        };

        if let Some(l) = limit {
            path = format!("{}&limit={}", path, l.min(100));
        }

        let response: Vec<OkxFill> = self
            .private_request("GET", &path, None)
            .await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|f| {
                let timestamp = f.ts.parse::<i64>().ok();
                let price: Decimal = f.fill_px.parse().unwrap_or_default();
                let amount: Decimal = f.fill_sz.parse().unwrap_or_default();
                let fee_amount: Option<Decimal> = f.fee.as_ref().and_then(|fee| fee.parse().ok());

                Trade {
                    id: f.trade_id.clone(),
                    order: Some(f.ord_id.clone()),
                    timestamp,
                    datetime: timestamp.and_then(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                    }),
                    symbol: f.inst_id.replace("-", "/"),
                    trade_type: None,
                    side: Some(f.side.clone()),
                    taker_or_maker: f.exec_type.as_ref().map(|e| {
                        if e == "M" {
                            crate::types::TakerOrMaker::Maker
                        } else {
                            crate::types::TakerOrMaker::Taker
                        }
                    }),
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: fee_amount.map(|cost| crate::types::Fee {
                        cost: Some(cost.abs()),
                        currency: f.fee_ccy.clone(),
                        rate: None,
                    }),
                    fees: Vec::new(),
                    info: serde_json::to_value(f).unwrap_or_default(),
                }
            })
            .collect();

        Ok(trades)
    }

    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        // OKX doesn't have a single cancel-all endpoint
        // We need to fetch open orders and cancel them one by one
        let open_orders = self.fetch_open_orders(symbol, None, None).await?;
        let mut canceled_orders = Vec::new();

        for order in open_orders {
            if let Ok(canceled) = self.cancel_order(&order.id, &order.symbol).await {
                canceled_orders.push(canceled);
            }
        }

        Ok(canceled_orders)
    }

    async fn create_orders(&self, orders: Vec<OrderRequest>) -> CcxtResult<Vec<Order>> {
        // OKX supports batch orders via /api/v5/trade/batch-orders
        let mut batch_orders = Vec::new();

        for order in &orders {
            let market_id = self.to_market_id(&order.symbol);
            let mut params = serde_json::Map::new();
            params.insert("instId".into(), market_id.into());
            params.insert("tdMode".into(), "cross".into());
            params.insert("side".into(), match order.side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            }.into());
            params.insert("ordType".into(), match order.order_type {
                OrderType::Limit => "limit",
                OrderType::Market => "market",
                OrderType::StopLimit => "conditional",
                _ => "limit",
            }.into());
            params.insert("sz".into(), order.amount.to_string().into());

            if let Some(price) = order.price {
                params.insert("px".into(), price.to_string().into());
            }

            if let Some(stop_price) = order.stop_price {
                params.insert("slTriggerPx".into(), stop_price.to_string().into());
            }

            batch_orders.push(serde_json::Value::Object(params));
        }

        let body_str = serde_json::to_string(&batch_orders).map_err(|e| CcxtError::BadRequest {
            message: format!("Failed to serialize batch orders: {e}"),
        })?;

        let response: Vec<OkxOrderResult> = self
            .private_request("POST", "/api/v5/trade/batch-orders", Some(&body_str))
            .await?;

        let mut result = Vec::new();
        for (i, res) in response.iter().enumerate() {
            if let Some(order) = orders.get(i) {
                result.push(Order {
                    id: res.ord_id.clone().unwrap_or_default(),
                    client_order_id: res.cl_ord_id.clone(),
                    timestamp: Some(Utc::now().timestamp_millis()),
                    datetime: Some(Utc::now().to_rfc3339()),
                    last_trade_timestamp: None,
                    last_update_timestamp: None,
                    status: OrderStatus::Open,
                    symbol: order.symbol.clone(),
                    order_type: order.order_type,
                    time_in_force: None,
                    side: order.side,
                    price: order.price,
                    average: None,
                    amount: order.amount,
                    filled: Decimal::ZERO,
                    remaining: None,
                    stop_price: order.stop_price,
                    trigger_price: None,
                    take_profit_price: order.take_profit_price,
                    stop_loss_price: order.stop_loss_price,
                    cost: None,
                    trades: Vec::new(),
                    fee: None,
                    fees: Vec::new(),
                    reduce_only: order.reduce_only,
                    post_only: order.post_only,
                    info: serde_json::to_value(res).unwrap_or_default(),
                });
            }
        }

        Ok(result)
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut path = "/api/v5/trade/orders-history".to_string();
        let mut query_parts = Vec::new();

        if let Some(s) = symbol {
            query_parts.push(format!("instId={}", self.to_market_id(s)));
        }
        query_parts.push("instType=SWAP".to_string());
        if let Some(l) = limit {
            query_parts.push(format!("limit={}", l.min(100)));
        }

        if !query_parts.is_empty() {
            path = format!("{}?{}", path, query_parts.join("&"));
        }

        let response: Vec<OkxOrder> = self
            .private_request("GET", &path, None::<&str>)
            .await?;

        let orders: Vec<Order> = response
            .iter()
            .map(|o| self.parse_order(o))
            .collect();

        Ok(orders)
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut path = "/api/v5/asset/deposit-history".to_string();
        let mut query_parts = Vec::new();

        if let Some(c) = code {
            query_parts.push(format!("ccy={c}"));
        }
        if let Some(l) = limit {
            query_parts.push(format!("limit={}", l.min(100)));
        }

        if !query_parts.is_empty() {
            path = format!("{}?{}", path, query_parts.join("&"));
        }

        let response: Vec<OkxDeposit> = self
            .private_request("GET", &path, None)
            .await?;

        let transactions: Vec<Transaction> = response
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
        let mut path = "/api/v5/asset/withdrawal-history".to_string();
        let mut query_parts = Vec::new();

        if let Some(c) = code {
            query_parts.push(format!("ccy={c}"));
        }
        if let Some(l) = limit {
            query_parts.push(format!("limit={}", l.min(100)));
        }

        if !query_parts.is_empty() {
            path = format!("{}?{}", path, query_parts.join("&"));
        }

        let response: Vec<OkxWithdrawal> = self
            .private_request("GET", &path, None)
            .await?;

        let transactions: Vec<Transaction> = response
            .iter()
            .map(|w| self.parse_withdrawal(w))
            .collect();

        Ok(transactions)
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        _network: Option<&str>,
    ) -> CcxtResult<crate::types::DepositAddress> {
        let path = format!("/api/v5/asset/deposit-address?ccy={code}");

        let response: Vec<OkxDepositAddress> = self
            .private_request("GET", &path, None)
            .await?;

        let addr = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "No deposit address found".into(),
        })?;

        let mut deposit_addr = crate::types::DepositAddress::new(&addr.ccy, &addr.addr);

        if let Some(ref chain) = addr.chain {
            deposit_addr = deposit_addr.with_network(chain.clone());
        }
        if let Some(ref tag) = addr.tag {
            if !tag.is_empty() {
                deposit_addr = deposit_addr.with_tag(tag.clone());
            }
        }

        Ok(deposit_addr)
    }

    async fn withdraw(
        &self,
        code: &str,
        amount: Decimal,
        address: &str,
        tag: Option<&str>,
        network: Option<&str>,
    ) -> CcxtResult<Transaction> {
        let chain = network.unwrap_or(code);

        let mut body = serde_json::json!({
            "ccy": code,
            "amt": amount.to_string(),
            "dest": "4", // On-chain withdrawal
            "toAddr": address,
            "chain": format!("{}-{}", code, chain),
        });

        if let Some(t) = tag {
            body["tag"] = serde_json::json!(t);
        }

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let response: Vec<OkxWithdrawResponse> = self
            .private_request("POST", "/api/v5/asset/withdrawal", Some(&body_str))
            .await?;

        let resp = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty withdrawal response".into(),
        })?;

        Ok(Transaction {
            id: resp.wd_id.clone(),
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
            info: serde_json::to_value(resp).unwrap_or_default(),
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
        api: &str,
        method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        let url = format!("{}{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();

        if api == "private" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();
            let passphrase = self.passphrase.clone().unwrap_or_default();

            let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
            let body_str = body.unwrap_or("");
            let prehash = format!("{}{}{}{}", timestamp, method.to_uppercase(), path, body_str);

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(prehash.as_bytes());
            let signature = BASE64.encode(mac.finalize().into_bytes());

            headers.insert("OK-ACCESS-KEY".into(), api_key.to_string());
            headers.insert("OK-ACCESS-SIGN".into(), signature);
            headers.insert("OK-ACCESS-TIMESTAMP".into(), timestamp);
            headers.insert("OK-ACCESS-PASSPHRASE".into(), passphrase);
            headers.insert("Content-Type".into(), "application/json".into());
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: body.map(|b| b.to_string()),
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
        let response: Vec<OkxPosition> = self
            .private_request("GET", "/api/v5/account/positions", None)
            .await?;

        let mut positions: Vec<Position> = response
            .iter()
            .map(|p| self.parse_position(p))
            .collect();

        // Filter by symbols if provided
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
        let market_id = self.to_swap_market_id(symbol);

        let body = serde_json::json!({
            "instId": market_id,
            "lever": leverage.to_string(),
            "mgnMode": "cross",  // Default to cross margin
        });

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let response: Vec<OkxLeverageResponse> = self
            .private_request("POST", "/api/v5/account/set-leverage", Some(&body_str))
            .await?;

        let resp = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty leverage response".into(),
        })?;

        let lever: Decimal = resp.lever.as_ref()
            .and_then(|l| l.parse().ok())
            .unwrap_or(leverage);

        let margin_mode = match resp.mgn_mode.as_deref() {
            Some("cross") => MarginMode::Cross,
            Some("isolated") => MarginMode::Isolated,
            _ => MarginMode::Cross,
        };

        Ok(Leverage::new(symbol, margin_mode, lever, lever))
    }

    async fn fetch_leverage(
        &self,
        symbol: &str,
    ) -> CcxtResult<Leverage> {
        let market_id = self.to_swap_market_id(symbol);
        let path = format!("/api/v5/account/leverage-info?instId={market_id}&mgnMode=cross");

        let response: Vec<OkxLeverageResponse> = self
            .private_request("GET", &path, None)
            .await?;

        let resp = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty leverage response".into(),
        })?;

        let lever: Decimal = resp.lever.as_ref()
            .and_then(|l| l.parse().ok())
            .unwrap_or_default();

        let margin_mode = match resp.mgn_mode.as_deref() {
            Some("cross") => MarginMode::Cross,
            Some("isolated") => MarginMode::Isolated,
            _ => MarginMode::Cross,
        };

        Ok(Leverage::new(symbol, margin_mode, lever, lever))
    }

    async fn set_margin_mode(
        &self,
        margin_mode: MarginMode,
        symbol: &str,
    ) -> CcxtResult<MarginModeInfo> {
        let market_id = self.to_swap_market_id(symbol);

        let mode_str = match margin_mode {
            MarginMode::Cross => "cross",
            MarginMode::Isolated => "isolated",
            MarginMode::Unknown => return Err(CcxtError::BadRequest {
                message: "Unknown margin mode is not supported".into(),
            }),
        };

        let body = serde_json::json!({
            "instId": market_id,
            "mgnMode": mode_str,
        });

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let _response: Vec<serde_json::Value> = self
            .private_request("POST", "/api/v5/account/set-isolated-mode", Some(&body_str))
            .await?;

        Ok(MarginModeInfo::new(symbol, margin_mode))
    }

    async fn fetch_funding_rate(
        &self,
        symbol: &str,
    ) -> CcxtResult<FundingRate> {
        let market_id = self.to_swap_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);

        let response: Vec<OkxFundingRateData> = self
            .public_get("/api/v5/public/funding-rate", Some(params))
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
        // OKX doesn't have a bulk funding rate endpoint, so we fetch all SWAP instruments
        let mut params = HashMap::new();
        params.insert("instType".into(), "SWAP".into());

        let response: Vec<OkxFundingRateData> = self
            .public_get("/api/v5/public/funding-rate", Some(params))
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
        let market_id = self.to_swap_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);

        if let Some(s) = since {
            params.insert("before".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<OkxFundingRateHistoryData> = self
            .public_get("/api/v5/public/funding-rate-history", Some(params))
            .await?;

        let history: Vec<FundingRateHistory> = response
            .iter()
            .map(|d| self.parse_funding_rate_history(d))
            .collect();

        Ok(history)
    }

    async fn fetch_open_interest(&self, symbol: &str) -> CcxtResult<OpenInterest> {
        let market_id = self.to_swap_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);

        let response: Vec<OkxOpenInterestData> = self
            .public_get("/api/v5/public/open-interest", Some(params))
            .await?;

        let data = response.first().ok_or_else(|| CcxtError::BadResponse {
            message: "Empty open interest response".into(),
        })?;

        let timestamp: i64 = data.ts.parse().unwrap_or_default();
        let amount: Decimal = data.oi.parse().unwrap_or_default();
        let value: Decimal = data.oi_ccy.parse().unwrap_or_default();

        Ok(OpenInterest {
            symbol: symbol.to_string(),
            open_interest_amount: Some(amount),
            open_interest_value: Some(value),
            base_volume: None,
            quote_volume: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    async fn fetch_liquidations(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Liquidation>> {
        let market_id = self.to_swap_market_id(symbol);
        let inst_type = if symbol.contains(":USDT") {
            "SWAP"
        } else if symbol.contains("-") {
            "FUTURES"
        } else {
            "SWAP"
        };

        let mut params = HashMap::new();
        params.insert("instType".into(), inst_type.to_string());

        if let Some(s) = since {
            params.insert("before".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<OkxLiquidationData> = self
            .public_get("/api/v5/public/liquidation-orders", Some(params))
            .await?;

        let liquidations: Vec<Liquidation> = response
            .iter()
            .filter(|liq| liq.inst_id == market_id)
            .flat_map(|liq| {
                liq.details.iter().map(|detail| {
                    let price: Decimal = detail.bkpx.parse().unwrap_or_default();
                    let qty: Decimal = detail.sz.parse().unwrap_or_default();
                    let timestamp: i64 = detail.ts.parse().unwrap_or_default();
                    let side = if detail.side == "sell" {
                        OrderSide::Sell
                    } else {
                        OrderSide::Buy
                    };

                    Liquidation {
                        info: serde_json::to_value(detail).unwrap_or_default(),
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
            })
            .collect();

        Ok(liquidations)
    }

    async fn fetch_index_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_swap_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);

        let response: Vec<OkxMarkPriceData> = self
            .public_get("/api/v5/public/mark-price", Some(params))
            .await?;

        let data = response.first().ok_or_else(|| CcxtError::BadResponse {
            message: "Empty mark price response".into(),
        })?;

        let mark_price: Decimal = data.mark_px.parse().unwrap_or_default();
        let timestamp: i64 = data.ts.parse().unwrap_or_default();

        // Fetch index price separately
        let mut index_params = HashMap::new();
        index_params.insert("instId".into(), self.to_swap_market_id(symbol));

        let index_response: Vec<OkxIndexTickerData> = self
            .public_get("/api/v5/market/index-tickers", Some(index_params))
            .await
            .unwrap_or_default();

        let index_price = index_response
            .first()
            .and_then(|d| d.idx_px.parse().ok())
            .unwrap_or(mark_price);

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last: Some(index_price),
            mark_price: Some(mark_price),
            index_price: Some(index_price),
            info: serde_json::to_value(data).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn fetch_mark_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_swap_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);

        let response: Vec<OkxMarkPriceData> = self
            .public_get("/api/v5/public/mark-price", Some(params))
            .await?;

        let data = response.first().ok_or_else(|| CcxtError::BadResponse {
            message: "Empty mark price response".into(),
        })?;

        let mark_price: Decimal = data.mark_px.parse().unwrap_or_default();
        let timestamp: i64 = data.ts.parse().unwrap_or_default();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            mark_price: Some(mark_price),
            info: serde_json::to_value(data).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn fetch_mark_prices(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        // OKX /api/v5/public/mark-price supports instType=SWAP/FUTURES
        let mut params = HashMap::new();
        params.insert("instType".into(), "SWAP".into());

        let response: Vec<OkxMarkPriceData> = self
            .public_get("/api/v5/public/mark-price", Some(params))
            .await?;

        let mut tickers: HashMap<String, Ticker> = HashMap::new();

        for data in &response {
            let inst_id = &data.inst_id;
            let symbol = match self.markets_by_id.read().ok().and_then(|m| m.get(inst_id).cloned()) {
                Some(s) => s,
                None => continue,
            };

            if let Some(sym_list) = symbols {
                if !sym_list.contains(&symbol.as_str()) {
                    continue;
                }
            }

            let mark_price: Decimal = match data.mark_px.parse() {
                Ok(p) => p,
                Err(_) => continue,
            };
            let timestamp: i64 = data.ts.parse().unwrap_or_default();

            tickers.insert(symbol.clone(), Ticker {
                symbol,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
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
        let market_id = self.to_swap_market_id(symbol);
        let bar = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {timeframe:?}"),
        })?;

        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);
        params.insert("bar".into(), bar.clone());
        if let Some(s) = since {
            params.insert("after".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(300).to_string());
        }

        // OKX uses /api/v5/market/mark-price-candles for mark price OHLCV
        let response: Vec<Vec<String>> = self
            .public_get("/api/v5/market/mark-price-candles", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                if c.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].parse().ok()?,
                    open: c[1].parse().ok()?,
                    high: c[2].parse().ok()?,
                    low: c[3].parse().ok()?,
                    close: c[4].parse().ok()?,
                    volume: Decimal::ZERO, // Mark price candles have no volume
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_index_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.to_swap_market_id(symbol);
        let bar = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {timeframe:?}"),
        })?;

        let mut params = HashMap::new();
        params.insert("instId".into(), market_id);
        params.insert("bar".into(), bar.clone());
        if let Some(s) = since {
            params.insert("after".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(300).to_string());
        }

        // OKX uses /api/v5/market/index-candles for index price OHLCV
        let response: Vec<Vec<String>> = self
            .public_get("/api/v5/market/index-candles", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                if c.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].parse().ok()?,
                    open: c[1].parse().ok()?,
                    high: c[2].parse().ok()?,
                    low: c[3].parse().ok()?,
                    close: c[4].parse().ok()?,
                    volume: Decimal::ZERO, // Index candles have no volume
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn transfer(
        &self,
        code: &str,
        amount: Decimal,
        from_account: &str,
        to_account: &str,
    ) -> CcxtResult<TransferEntry> {
        // OKX account types: 1=spot, 5=margin, 6=funding, 18=unified
        let from_type = match from_account.to_lowercase().as_str() {
            "spot" | "trading" => "18",      // unified account
            "funding" => "6",
            "margin" => "5",
            _ => "18",
        };

        let to_type = match to_account.to_lowercase().as_str() {
            "spot" | "trading" => "18",
            "funding" => "6",
            "margin" => "5",
            _ => "18",
        };

        let body = serde_json::json!({
            "ccy": code.to_uppercase(),
            "amt": amount.to_string(),
            "from": from_type,
            "to": to_type,
        });

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let response: Vec<OkxTransferResponse> = self
            .private_request("POST", "/api/v5/asset/transfer", Some(&body_str))
            .await?;

        let result = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty transfer response".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(TransferEntry::new()
            .with_id(result.trans_id.clone())
            .with_currency(code.to_string())
            .with_amount(amount)
            .with_from_account(from_account.to_string())
            .with_to_account(to_account.to_string())
            .with_timestamp(timestamp)
            .with_status("success".to_string()))
    }

    async fn fetch_transfers(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<TransferEntry>> {
        let mut params: HashMap<String, String> = HashMap::new();

        if let Some(c) = code {
            params.insert("ccy".into(), c.to_uppercase());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let path = format!(
            "/api/v5/asset/transfer-state?{}",
            params.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        );

        let response: Vec<OkxTransferHistory> = self
            .private_request("GET", &path, None)
            .await?;

        let transfers: Vec<TransferEntry> = response
            .iter()
            .map(|item| {
                let from = match item.from.as_str() {
                    "18" => "spot",
                    "6" => "funding",
                    "5" => "margin",
                    _ => &item.from,
                };
                let to = match item.to.as_str() {
                    "18" => "spot",
                    "6" => "funding",
                    "5" => "margin",
                    _ => &item.to,
                };

                TransferEntry::new()
                    .with_id(item.trans_id.clone())
                    .with_currency(item.ccy.clone())
                    .with_amount(item.amt.parse().unwrap_or_default())
                    .with_from_account(from.to_string())
                    .with_to_account(to.to_string())
                    .with_timestamp(item.ts.parse().unwrap_or_default())
                    .with_status(item.state.clone())
            })
            .collect();

        Ok(transfers)
    }

    // === Margin Trading ===

    async fn borrow_cross_margin(
        &self,
        code: &str,
        amount: Decimal,
    ) -> CcxtResult<crate::types::MarginLoan> {
        let body = serde_json::json!({
            "ccy": code.to_uppercase(),
            "amt": amount.to_string(),
            "side": "borrow",
        });

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let response: Vec<OkxBorrowRepayResponse> = self
            .private_request("POST", "/api/v5/account/borrow-repay", Some(&body_str))
            .await?;

        let result = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty borrow response".into(),
        })?;

        Ok(crate::types::MarginLoan::new()
            .with_id(result.ord_id.clone())
            .with_currency(code)
            .with_amount(amount)
            .with_info(serde_json::to_value(result).unwrap_or_default()))
    }

    async fn repay_cross_margin(
        &self,
        code: &str,
        amount: Decimal,
    ) -> CcxtResult<crate::types::MarginLoan> {
        // OKX requires the ordId when repaying
        // For simplicity, we'll use a generic repay - in practice you'd need to specify the loan ID
        let body = serde_json::json!({
            "ccy": code.to_uppercase(),
            "amt": amount.to_string(),
            "side": "repay",
        });

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let response: Vec<OkxBorrowRepayResponse> = self
            .private_request("POST", "/api/v5/account/borrow-repay", Some(&body_str))
            .await?;

        let result = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty repay response".into(),
        })?;

        Ok(crate::types::MarginLoan::new()
            .with_id(result.ord_id.clone())
            .with_currency(code)
            .with_amount(amount)
            .with_info(serde_json::to_value(result).unwrap_or_default()))
    }

    async fn fetch_cross_borrow_rate(
        &self,
        code: &str,
    ) -> CcxtResult<crate::types::CrossBorrowRate> {
        let path = format!("/api/v5/account/interest-rate?ccy={}", code.to_uppercase());

        let response: Vec<OkxInterestRate> = self
            .private_request("GET", &path, None)
            .await?;

        let rate_data = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: format!("No borrow rate data for {code}"),
        })?;

        let rate = rate_data.interest_rate.parse::<Decimal>().unwrap_or_default();

        Ok(crate::types::CrossBorrowRate::new(rate)
            .with_currency(code))
    }
}

impl Okx {
    /// 소수점 자릿수 계산
    fn count_decimals(value: Decimal) -> i32 {
        let s = value.to_string();
        if let Some(pos) = s.find('.') {
            (s.len() - pos - 1) as i32
        } else {
            0
        }
    }
}

// === OKX API Response Types ===

#[derive(Debug, Deserialize)]
struct OkxResponse<T> {
    code: String,
    msg: Option<String>,
    data: Option<T>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxInstrument {
    inst_id: String,
    inst_type: String,
    base_ccy: String,
    quote_ccy: String,
    state: String,
    #[serde(default)]
    lot_sz: Option<String>,
    #[serde(default)]
    tick_sz: Option<String>,
    #[serde(default)]
    min_sz: Option<String>,
    #[serde(default)]
    max_lmt_sz: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxTicker {
    inst_id: String,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    bid_px: Option<String>,
    #[serde(default)]
    bid_sz: Option<String>,
    #[serde(default)]
    ask_px: Option<String>,
    #[serde(default)]
    ask_sz: Option<String>,
    #[serde(default)]
    open24h: Option<String>,
    #[serde(default)]
    high24h: Option<String>,
    #[serde(default)]
    low24h: Option<String>,
    #[serde(default)]
    vol24h: Option<String>,
    #[serde(default)]
    vol_ccy24h: Option<String>,
    ts: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxOrderBook {
    asks: Vec<Vec<String>>,
    bids: Vec<Vec<String>>,
    ts: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxTrade {
    trade_id: String,
    px: String,
    sz: String,
    side: String,
    ts: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxOrder {
    ord_id: String,
    #[serde(default)]
    cl_ord_id: Option<String>,
    inst_id: String,
    ord_type: String,
    side: String,
    state: String,
    sz: String,
    #[serde(default)]
    px: Option<String>,
    #[serde(default)]
    avg_px: Option<String>,
    #[serde(default)]
    acc_fill_sz: Option<String>,
    #[serde(default)]
    c_time: Option<String>,
    #[serde(default)]
    u_time: Option<String>,
    #[serde(default)]
    reduce_only: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxOrderResponse {
    ord_id: String,
    s_code: String,
    #[serde(default)]
    s_msg: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxOrderResult {
    #[serde(default)]
    ord_id: Option<String>,
    #[serde(default)]
    cl_ord_id: Option<String>,
    #[serde(default)]
    s_code: Option<String>,
    #[serde(default)]
    s_msg: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxBalance {
    details: Vec<OkxBalanceDetail>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxBalanceDetail {
    ccy: String,
    #[serde(default)]
    avail_bal: Option<String>,
    #[serde(default)]
    frozen_bal: Option<String>,
}

/// Fill (trade) response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxFill {
    trade_id: String,
    ord_id: String,
    inst_id: String,
    side: String,
    fill_px: String,
    fill_sz: String,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    fee_ccy: Option<String>,
    #[serde(default)]
    exec_type: Option<String>,
    ts: String,
}

/// Deposit history response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxDeposit {
    dep_id: String,
    ccy: String,
    #[serde(default)]
    chain: Option<String>,
    amt: String,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    tx_id: Option<String>,
    state: String,
    ts: String,
}

/// Withdrawal history response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxWithdrawal {
    wd_id: String,
    ccy: String,
    #[serde(default)]
    chain: Option<String>,
    amt: String,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    tx_id: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    state: String,
    ts: String,
}

/// Deposit address response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxDepositAddress {
    ccy: String,
    addr: String,
    #[serde(default)]
    chain: Option<String>,
    #[serde(default)]
    tag: Option<String>,
}

/// Withdraw response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxWithdrawResponse {
    wd_id: String,
}

/// Position response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxPosition {
    inst_id: String,
    #[serde(default)]
    pos_id: Option<String>,
    #[serde(default)]
    pos_side: Option<String>,
    #[serde(default)]
    pos: Option<String>,
    #[serde(default)]
    avg_px: Option<String>,
    #[serde(default)]
    mark_px: Option<String>,
    #[serde(default)]
    liq_px: Option<String>,
    #[serde(default)]
    lever: Option<String>,
    #[serde(default)]
    notional_usd: Option<String>,
    #[serde(default)]
    upl: Option<String>,
    #[serde(default)]
    margin: Option<String>,
    #[serde(default)]
    mgn_mode: Option<String>,
    #[serde(default)]
    u_time: Option<String>,
}

/// Leverage response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxLeverageResponse {
    #[serde(default)]
    inst_id: Option<String>,
    #[serde(default)]
    lever: Option<String>,
    #[serde(default)]
    mgn_mode: Option<String>,
    #[serde(default)]
    pos_side: Option<String>,
}

/// Funding rate response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxFundingRateData {
    inst_id: String,
    #[serde(default)]
    funding_rate: Option<String>,
    #[serde(default)]
    next_funding_rate: Option<String>,
    #[serde(default)]
    next_funding_time: Option<String>,
    #[serde(default)]
    ts: Option<String>,
}

/// Funding rate history response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxFundingRateHistoryData {
    inst_id: String,
    #[serde(default)]
    funding_rate: Option<String>,
    #[serde(default)]
    funding_time: Option<String>,
}

/// Open interest response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxOpenInterestData {
    inst_id: String,
    oi: String,
    oi_ccy: String,
    ts: String,
}

/// Liquidation response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxLiquidationData {
    inst_id: String,
    #[serde(default)]
    details: Vec<OkxLiquidationDetail>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxLiquidationDetail {
    side: String,
    sz: String,
    bkpx: String,
    ts: String,
}

/// Mark price response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxMarkPriceData {
    inst_id: String,
    mark_px: String,
    ts: String,
}

/// Index ticker response
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxIndexTickerData {
    #[serde(default)]
    inst_id: String,
    #[serde(default)]
    idx_px: String,
}

/// Transfer response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxTransferResponse {
    trans_id: String,
}

/// Transfer history item
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxTransferHistory {
    trans_id: String,
    ccy: String,
    amt: String,
    from: String,
    to: String,
    ts: String,
    state: String,
}

/// Borrow/repay response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxBorrowRepayResponse {
    amt: String,
    ccy: String,
    ord_id: String,
    side: String,
    state: String,
}

/// Interest rate response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxInterestRate {
    ccy: String,
    #[serde(default)]
    interest_rate: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let okx = Okx::new(config).unwrap();

        assert_eq!(okx.to_market_id("BTC/USDT"), "BTC-USDT");
        assert_eq!(okx.to_symbol("BTC-USDT"), "BTC/USDT");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let okx = Okx::new(config).unwrap();

        assert_eq!(okx.id(), ExchangeId::Okx);
        assert_eq!(okx.name(), "OKX");
        assert!(okx.has().spot);
        assert!(okx.has().margin);
        assert!(okx.has().swap);
    }
}
