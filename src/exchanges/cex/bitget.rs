//! Bitget Exchange Implementation
//!
//! Bitget API 구현

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, FundingRate,
    FundingRateHistory, Leverage, Liquidation, MarginMode, MarginModeInfo, MarginModification,
    MarginModificationType, Market, MarketLimits, MarketPrecision, MarketType, OpenInterest, Order,
    OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Position, PositionMode,
    PositionModeInfo, PositionSide, SignedRequest, Ticker, Timeframe, Trade, Transaction,
    TransactionStatus, TransactionType, TransferEntry, WsExchange, WsMessage, OHLCV,
};

use super::bitget_ws::BitgetWs;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

type HmacSha256 = Hmac<Sha256>;

/// Bitget 거래소
pub struct Bitget {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    ws_client: Arc<TokioRwLock<BitgetWs>>,
}

impl Bitget {
    const BASE_URL: &'static str = "https://api.bitget.com";
    const RATE_LIMIT_MS: u64 = 100;

    /// 새 Bitget 인스턴스 생성
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
            logo: Some("https://user-images.githubusercontent.com/1294454/195989417-4253ddb0-afbe-4a1c-9dea-9dbcd121fa5d.jpg".into()),
            api: api_urls,
            www: Some("https://www.bitget.com".into()),
            doc: vec!["https://www.bitget.com/api-doc".into()],
            fees: Some("https://www.bitget.com/fee".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1min".into());
        timeframes.insert(Timeframe::Minute5, "5min".into());
        timeframes.insert(Timeframe::Minute15, "15min".into());
        timeframes.insert(Timeframe::Minute30, "30min".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Hour6, "6h".into());
        timeframes.insert(Timeframe::Hour12, "12h".into());
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
            ws_client: Arc::new(TokioRwLock::new(BitgetWs::new())),
        })
    }

    /// 심볼 변환 (BTC/USDT -> BTCUSDT)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace('/', "")
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
        let passphrase = self
            .config
            .password()
            .ok_or_else(|| CcxtError::AuthenticationError {
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

        // String to sign: timestamp + method + path + body
        let sign_string = format!(
            "{}{}{}{}",
            timestamp,
            method.to_uppercase(),
            full_path,
            body
        );

        // HMAC-SHA256 signature
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret".into(),
            }
        })?;
        mac.update(sign_string.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("ACCESS-KEY".into(), api_key.to_string());
        headers.insert("ACCESS-SIGN".into(), signature);
        headers.insert("ACCESS-TIMESTAMP".into(), timestamp);
        headers.insert("ACCESS-PASSPHRASE".into(), passphrase.to_string());
        headers.insert("Content-Type".into(), "application/json".into());
        headers.insert("locale".into(), "en-US".into());

        match method {
            "GET" => {
                let query_params = if params.is_empty() {
                    None
                } else {
                    Some(params)
                };
                self.client.get(path, query_params, Some(headers)).await
            },
            "POST" => {
                let json_body: Option<serde_json::Value> = if body.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&body).unwrap_or_default())
                };
                self.client.post(path, json_body, Some(headers)).await
            },
            "DELETE" => {
                let query_params = if params.is_empty() {
                    None
                } else {
                    Some(params)
                };
                self.client.delete(path, query_params, Some(headers)).await
            },
            _ => Err(CcxtError::BadRequest {
                message: "Invalid HTTP method".into(),
            }),
        }
    }

    fn parse_ticker(&self, data: &BitgetTicker, symbol: &str) -> Ticker {
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid_pr.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_sz.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask_pr.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_sz.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.close.as_ref().and_then(|v| v.parse().ok()),
            last: data.last_pr.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.change.as_ref().and_then(|v| v.parse().ok()),
            percentage: data
                .change_utc24h
                .as_ref()
                .and_then(|v| v.parse::<Decimal>().ok().map(|d| d * Decimal::from(100))),
            average: None,
            base_volume: data.base_volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.quote_volume.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order(&self, data: &BitgetOrder, symbol: &str) -> Order {
        let timestamp = data
            .c_time
            .as_ref()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let amount: Decimal = data
            .size
            .as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data
            .base_volume
            .as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let price: Option<Decimal> = data.price.as_ref().and_then(|v| v.parse().ok());
        let average: Option<Decimal> = data.price_avg.as_ref().and_then(|v| v.parse().ok());

        let status = match data.status.as_deref() {
            Some("new") | Some("partial_fill") => OrderStatus::Open,
            Some("full_fill") => OrderStatus::Closed,
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
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_oid.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.u_time.as_ref().and_then(|v| v.parse().ok()),
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
            cost: data.quote_volume.as_ref().and_then(|v| v.parse().ok()),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_balance(&self, accounts: &[BitgetAccount]) -> Balances {
        let mut balances = Balances::new();

        for account in accounts {
            let currency = account.coin.clone();
            let available: Decimal = account
                .available
                .as_ref()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default();
            let frozen: Decimal = account
                .frozen
                .as_ref()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default();

            balances.add(currency, Balance::new(available, frozen));
        }

        balances
    }

    fn parse_deposit(&self, deposit: &BitgetDeposit) -> Transaction {
        let timestamp = deposit.c_time.as_ref().and_then(|t| t.parse::<i64>().ok());
        let amount: Decimal = deposit
            .size
            .as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        let status = match deposit.status.as_deref() {
            Some("success") => TransactionStatus::Ok,
            Some("pending") => TransactionStatus::Pending,
            Some("failed") => TransactionStatus::Failed,
            _ => TransactionStatus::Pending,
        };

        Transaction {
            id: deposit.order_id.clone().unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: deposit.u_time.as_ref().and_then(|t| t.parse::<i64>().ok()),
            tx_type: TransactionType::Deposit,
            currency: deposit.coin.clone().unwrap_or_default(),
            network: deposit.chain.clone(),
            amount,
            status,
            address: deposit.to_address.clone(),
            tag: None,
            txid: deposit.tx_id.clone(),
            fee: None,
            internal: None,
            confirmations: deposit.confirm.as_ref().and_then(|c| c.parse::<u32>().ok()),
            info: serde_json::to_value(deposit).unwrap_or_default(),
        }
    }

    fn parse_withdrawal(&self, withdrawal: &BitgetWithdrawal) -> Transaction {
        let timestamp = withdrawal
            .c_time
            .as_ref()
            .and_then(|t| t.parse::<i64>().ok());
        let amount: Decimal = withdrawal
            .size
            .as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let fee_amount: Option<Decimal> = withdrawal.fee.as_ref().and_then(|v| v.parse().ok());

        let status = match withdrawal.status.as_deref() {
            Some("success") => TransactionStatus::Ok,
            Some("pending") => TransactionStatus::Pending,
            Some("failed") | Some("reject") => TransactionStatus::Failed,
            Some("cancel") => TransactionStatus::Canceled,
            _ => TransactionStatus::Pending,
        };

        Transaction {
            id: withdrawal.order_id.clone().unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: withdrawal
                .u_time
                .as_ref()
                .and_then(|t| t.parse::<i64>().ok()),
            tx_type: TransactionType::Withdrawal,
            currency: withdrawal.coin.clone().unwrap_or_default(),
            network: withdrawal.chain.clone(),
            amount,
            status,
            address: withdrawal.to_address.clone(),
            tag: None,
            txid: withdrawal.tx_id.clone(),
            fee: fee_amount.map(|cost| crate::types::Fee {
                cost: Some(cost),
                currency: withdrawal.coin.clone(),
                rate: None,
            }),
            internal: None,
            confirmations: withdrawal
                .confirm
                .as_ref()
                .and_then(|c| c.parse::<u32>().ok()),
            info: serde_json::to_value(withdrawal).unwrap_or_default(),
        }
    }

    /// 선물 API 호출 (퍼블릭)
    async fn mix_public_get<T: serde::de::DeserializeOwned + Default>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        let full_path = format!("/api/v2/mix/market{path}");

        let url = if let Some(p) = &params {
            let query: String = p
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{full_path}?{query}")
        } else {
            full_path
        };

        let response: BitgetResponse<T> = self.client.get(&url, None, None).await?;

        if response.code != "00000" {
            return Err(CcxtError::ExchangeError {
                message: response
                    .msg
                    .unwrap_or_else(|| format!("Bitget error: {}", response.code)),
            });
        }

        response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty response data".into(),
        })
    }

    /// 선물 API 호출 (프라이빗)
    async fn mix_private_request<T: serde::de::DeserializeOwned + Default>(
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
        let passphrase = self
            .config
            .password()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Passphrase required".into(),
            })?;

        let full_path = format!("/api/v2/mix{path}");
        let timestamp = Utc::now().timestamp_millis().to_string();

        let (url, body_str) = if method == "GET" {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            let url = if query.is_empty() {
                full_path.clone()
            } else {
                format!("{full_path}?{query}")
            };
            (url, String::new())
        } else {
            (
                full_path.clone(),
                serde_json::to_string(&params).unwrap_or_else(|_| "{}".into()),
            )
        };

        let sign_str = format!("{}{}{}{}", timestamp, method.to_uppercase(), url, body_str);

        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(sign_str.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("ACCESS-KEY".into(), api_key.to_string());
        headers.insert("ACCESS-SIGN".into(), signature);
        headers.insert("ACCESS-TIMESTAMP".into(), timestamp);
        headers.insert("ACCESS-PASSPHRASE".into(), passphrase.to_string());
        headers.insert("Content-Type".into(), "application/json".into());
        headers.insert("locale".into(), "en-US".into());

        let response: BitgetResponse<T> = match method.to_uppercase().as_str() {
            "GET" => self.client.get(&url, None, Some(headers)).await?,
            "POST" => {
                let json_body: serde_json::Value = if body_str.is_empty() {
                    serde_json::Value::Object(serde_json::Map::new())
                } else {
                    serde_json::from_str(&body_str).unwrap_or_default()
                };
                self.client
                    .post(&full_path, Some(json_body), Some(headers))
                    .await?
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("HTTP method: {method}"),
                })
            },
        };

        if response.code != "00000" {
            return Err(CcxtError::ExchangeError {
                message: response
                    .msg
                    .unwrap_or_else(|| format!("Bitget error: {}", response.code)),
            });
        }

        response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty response data".into(),
        })
    }

    /// 포지션 파싱
    fn parse_position(&self, data: &BitgetPosition) -> Position {
        let timestamp = data.u_time.as_ref().and_then(|t| t.parse::<i64>().ok());
        let symbol = data.symbol.replace("USDT", "/USDT");

        let size: Decimal = data
            .total
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let side = match data.hold_side.as_deref() {
            Some("long") => Some(PositionSide::Long),
            Some("short") => Some(PositionSide::Short),
            _ => None,
        };

        let contracts = Some(size.abs());
        let entry_price: Option<Decimal> =
            data.open_price_avg.as_ref().and_then(|p| p.parse().ok());
        let mark_price: Option<Decimal> = data.mark_price.as_ref().and_then(|p| p.parse().ok());
        let leverage: Option<Decimal> = data.leverage.as_ref().and_then(|l| l.parse().ok());
        let unrealized_pnl: Option<Decimal> =
            data.unrealized_pl.as_ref().and_then(|u| u.parse().ok());
        let liquidation_price: Option<Decimal> = data.liquidation_price.as_ref().and_then(|l| {
            let val: Decimal = l.parse().ok()?;
            if val == Decimal::ZERO {
                None
            } else {
                Some(val)
            }
        });
        let margin: Option<Decimal> = data.margin.as_ref().and_then(|m| m.parse().ok());

        let margin_mode = match data.margin_mode.as_deref() {
            Some("crossed") => Some(MarginMode::Cross),
            Some("isolated") => Some(MarginMode::Isolated),
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
            notional: None,
            leverage,
            collateral: margin,
            initial_margin: margin,
            initial_margin_percentage: None,
            maintenance_margin: None,
            maintenance_margin_percentage: None,
            entry_price,
            mark_price,
            last_price: None,
            liquidation_price,
            unrealized_pnl,
            realized_pnl: data.achieved_profits.as_ref().and_then(|p| p.parse().ok()),
            percentage: None,
            margin_mode,
            margin_ratio: None,
            hedged: Some(data.hold_side.is_some()),
            last_update_timestamp: timestamp,
            stop_loss_price: None,
            take_profit_price: None,
        }
    }

    /// 펀딩 레이트 파싱
    fn parse_funding_rate(&self, data: &BitgetFundingRateData) -> FundingRate {
        let timestamp = data
            .funding_time
            .as_ref()
            .and_then(|t| t.parse::<i64>().ok());
        let symbol = data.symbol.replace("USDT", "/USDT");

        FundingRate {
            symbol,
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            funding_rate: data.funding_rate.as_ref().and_then(|r| r.parse().ok()),
            mark_price: data.mark_price.as_ref().and_then(|m| m.parse().ok()),
            index_price: data.index_price.as_ref().and_then(|i| i.parse().ok()),
            interest_rate: None,
            estimated_settle_price: None,
            funding_timestamp: data.next_funding_time.as_ref().and_then(|t| t.parse().ok()),
            funding_datetime: None,
            next_funding_rate: None,
            next_funding_timestamp: data.next_funding_time.as_ref().and_then(|t| t.parse().ok()),
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
        data: &BitgetFundingRateHistoryData,
    ) -> FundingRateHistory {
        let timestamp = data
            .funding_time
            .as_ref()
            .and_then(|t| t.parse::<i64>().ok());
        let symbol = data.symbol.replace("USDT", "/USDT");
        let funding_rate: Decimal = data
            .funding_rate
            .as_ref()
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

    /// 선물 심볼 → 마켓 ID 변환 (BTC/USDT:USDT → BTCUSDT)
    fn to_mix_market_id(&self, symbol: &str) -> String {
        if symbol.contains(':') {
            let parts: Vec<&str> = symbol.split(':').collect();
            if let Some(base_quote) = parts.first() {
                return base_quote.replace("/", "");
            }
        }
        symbol.replace("/", "")
    }
}

#[async_trait]
impl Exchange for Bitget {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bitget
    }

    fn name(&self) -> &str {
        "Bitget"
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
        let response: BitgetResponse<Vec<BitgetSymbol>> =
            self.public_get("/api/v2/spot/public/symbols", None).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: response.msg.unwrap_or_else(|| "No data in response".into()),
        })?;

        let mut markets = Vec::new();

        for symbol_data in data {
            if symbol_data.status.as_deref() != Some("online") {
                continue;
            }

            let id = symbol_data.symbol.clone();
            let base = symbol_data.base_coin.clone();
            let quote = symbol_data.quote_coin.clone();
            let symbol = format!("{base}/{quote}");

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
                active: symbol_data.status.as_deref() == Some("online"),
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
                taker: symbol_data
                    .taker_fee_rate
                    .as_ref()
                    .and_then(|v| v.parse().ok()),
                maker: symbol_data
                    .maker_fee_rate
                    .as_ref()
                    .and_then(|v| v.parse().ok()),
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
            underlying: None,
            underlying_id: None,
                precision: MarketPrecision {
                    amount: symbol_data.quantity_precision,
                    price: symbol_data.price_precision,
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

        let response: BitgetResponse<Vec<BitgetTicker>> = self
            .public_get("/api/v2/spot/market/tickers", Some(params))
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;
        let ticker = data.first().ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        Ok(self.parse_ticker(ticker, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: BitgetResponse<Vec<BitgetTicker>> =
            self.public_get("/api/v2/spot/market/tickers", None).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let mut tickers = HashMap::new();

        for ticker_data in data {
            let market_id = match &ticker_data.symbol {
                Some(s) => s.clone(),
                None => continue,
            };

            let symbol_str = markets_by_id
                .get(&market_id)
                .cloned()
                .unwrap_or_else(|| market_id.clone());

            if let Some(filter) = symbols {
                if !filter.contains(&symbol_str.as_str()) {
                    continue;
                }
            }

            let ticker = self.parse_ticker(&ticker_data, &symbol_str);
            tickers.insert(symbol_str, ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(150).to_string());
        }

        let response: BitgetResponse<BitgetOrderBook> = self
            .public_get("/api/v2/spot/market/orderbook", Some(params))
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        let bids: Vec<OrderBookEntry> = data
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

        let asks: Vec<OrderBookEntry> = data
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

        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

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
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(500).to_string());
        }

        let response: BitgetResponse<Vec<BitgetTrade>> = self
            .public_get("/api/v2/spot/market/fills", Some(params))
            .await?;

        let data = response.data.unwrap_or_default();

        let trades: Vec<Trade> = data
            .iter()
            .map(|t| {
                let timestamp = t.ts.unwrap_or_else(|| Utc::now().timestamp_millis());
                let price: Decimal = t
                    .price
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
                let amount: Decimal = t
                    .size
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();

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
        params.insert("symbol".into(), market_id);
        params.insert("granularity".into(), interval.clone());
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: BitgetResponse<Vec<Vec<String>>> = self
            .public_get("/api/v2/spot/market/candles", Some(params))
            .await?;

        let data = response.data.unwrap_or_default();

        let ohlcv: Vec<OHLCV> = data
            .iter()
            .filter_map(|c| {
                if c.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].parse::<i64>().ok()?,
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
        let response: BitgetResponse<Vec<BitgetAccount>> = self
            .private_request("GET", "/api/v2/spot/account/assets", HashMap::new())
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
        params.insert("symbol".into(), market_id.clone());
        params.insert("clientOid".into(), client_oid);
        params.insert(
            "side".into(),
            match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            }
            .into(),
        );
        params.insert("size".into(), amount.to_string());
        params.insert("force".into(), "gtc".into());

        match order_type {
            OrderType::Limit => {
                let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                    message: "Limit order requires price".into(),
                })?;
                params.insert("orderType".into(), "limit".into());
                params.insert("price".into(), price_val.to_string());
            },
            OrderType::Market => {
                params.insert("orderType".into(), "market".into());
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type: {order_type:?}"),
                })
            },
        }

        let response: BitgetResponse<BitgetOrderResult> = self
            .private_request("POST", "/api/v2/spot/trade/place-order", params)
            .await?;

        let result = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: response.msg.unwrap_or_else(|| "Order failed".into()),
        })?;

        // Fetch the created order
        self.fetch_order(&result.order_id, symbol).await
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        // First fetch the order to return it
        let order = self.fetch_order(id, symbol).await.ok();

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let _response: BitgetResponse<serde_json::Value> = self
            .private_request("POST", "/api/v2/spot/trade/cancel-order", params)
            .await?;

        // Return the order if we fetched it, otherwise create a minimal cancelled order
        match order {
            Some(mut o) => {
                o.status = OrderStatus::Canceled;
                Ok(o)
            },
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
            }),
        }
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
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        if let Some(amt) = amount {
            params.insert("size".into(), amt.to_string());
        }
        if let Some(p) = price {
            params.insert("price".into(), p.to_string());
        }

        let _response: BitgetResponse<serde_json::Value> = self
            .private_request("POST", "/api/v2/spot/trade/modify-order", params)
            .await?;

        self.fetch_order(id, symbol).await
    }

    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Bitget cancelAllOrders requires a symbol".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let _response: BitgetResponse<serde_json::Value> = self
            .private_request("POST", "/api/v2/spot/trade/cancel-symbol-order", params)
            .await?;

        Ok(Vec::new())
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: BitgetResponse<Vec<BitgetOrder>> = self
            .private_request("GET", "/api/v2/spot/trade/orderInfo", params)
            .await?;

        let orders = response.data.ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.into(),
        })?;

        let order_data = orders.first().ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.into(),
        })?;

        Ok(self.parse_order(order_data, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BitgetResponse<Vec<BitgetOrder>> = self
            .private_request("GET", "/api/v2/spot/trade/unfilled-orders", params)
            .await?;

        let orders = response.data.unwrap_or_default();
        let symbol_str = symbol.unwrap_or("");

        Ok(orders
            .iter()
            .map(|o| {
                let sym = if symbol_str.is_empty() {
                    // Try to get symbol from markets_by_id
                    o.symbol
                        .as_ref()
                        .and_then(|s| self.markets_by_id.read().ok()?.get(s).cloned())
                        .unwrap_or_default()
                } else {
                    symbol_str.to_string()
                };
                self.parse_order(o, &sym)
            })
            .collect())
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: BitgetResponse<Vec<BitgetOrder>> = self
            .private_request("GET", "/api/v2/spot/trade/history-orders", params)
            .await?;

        let orders = response.data.unwrap_or_default();
        let symbol_str = symbol.unwrap_or("");

        Ok(orders
            .iter()
            .map(|o| {
                let sym = if symbol_str.is_empty() {
                    o.symbol
                        .as_ref()
                        .and_then(|s| self.markets_by_id.read().ok()?.get(s).cloned())
                        .unwrap_or_default()
                } else {
                    symbol_str.to_string()
                };
                self.parse_order(o, &sym)
            })
            .collect())
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
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: BitgetResponse<Vec<BitgetFill>> = self
            .private_request("GET", "/api/v2/spot/trade/fills", params)
            .await?;

        let fills = response.data.unwrap_or_default();

        let trades: Vec<Trade> = fills
            .iter()
            .map(|f| {
                let timestamp = f.c_time.as_ref().and_then(|t| t.parse::<i64>().ok());
                let price: Decimal = f
                    .price
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
                let amount: Decimal = f
                    .size
                    .as_ref()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
                let fee_amount: Option<Decimal> = f.fee.as_ref().and_then(|v| v.parse().ok());

                let symbol_str = f
                    .symbol
                    .as_ref()
                    .and_then(|s| self.markets_by_id.read().ok()?.get(s).cloned())
                    .unwrap_or_else(|| symbol.unwrap_or("").to_string());

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
                    taker_or_maker: f.trade_scope.as_ref().map(|t| {
                        if t == "maker" {
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
                        currency: f.fee_currency.clone(),
                        rate: None,
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
            params.insert("coin".into(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: BitgetResponse<Vec<BitgetDeposit>> = self
            .private_request("GET", "/api/v2/spot/wallet/deposit-records", params)
            .await?;

        let deposits = response.data.unwrap_or_default();

        let transactions: Vec<Transaction> =
            deposits.iter().map(|d| self.parse_deposit(d)).collect();

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
            params.insert("coin".into(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: BitgetResponse<Vec<BitgetWithdrawal>> = self
            .private_request("GET", "/api/v2/spot/wallet/withdrawal-records", params)
            .await?;

        let withdrawals = response.data.unwrap_or_default();

        let transactions: Vec<Transaction> = withdrawals
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
        params.insert("coin".into(), code.to_string());
        params.insert("transferType".into(), "on_chain".into());
        params.insert("address".into(), address.to_string());
        params.insert("size".into(), amount.to_string());

        if let Some(t) = tag {
            params.insert("tag".into(), t.to_string());
        }
        if let Some(n) = network {
            params.insert("chain".into(), n.to_string());
        }

        let response: BitgetResponse<BitgetWithdrawResponse> = self
            .private_request("POST", "/api/v2/spot/wallet/withdrawal", params)
            .await?;

        let withdraw_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: response.msg.unwrap_or_else(|| "Withdrawal failed".into()),
        })?;

        let info = serde_json::to_value(&withdraw_data).unwrap_or_default();

        Ok(Transaction {
            id: withdraw_data.order_id,
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

    async fn transfer(
        &self,
        code: &str,
        amount: Decimal,
        from_account: &str,
        to_account: &str,
    ) -> CcxtResult<TransferEntry> {
        // Bitget account types: spot, mix_usdt (USDT futures), mix_usd (Coin futures)
        let from_type = match from_account.to_lowercase().as_str() {
            "spot" => "spot",
            "futures" | "swap" | "usdt_futures" => "mix_usdt",
            "coin_futures" => "mix_usd",
            _ => from_account,
        };

        let to_type = match to_account.to_lowercase().as_str() {
            "spot" => "spot",
            "futures" | "swap" | "usdt_futures" => "mix_usdt",
            "coin_futures" => "mix_usd",
            _ => to_account,
        };

        let mut params = HashMap::new();
        params.insert("coin".into(), code.to_uppercase());
        params.insert("amount".into(), amount.to_string());
        params.insert("fromType".into(), from_type.to_string());
        params.insert("toType".into(), to_type.to_string());

        let response: BitgetResponse<BitgetTransferResult> = self
            .private_request("POST", "/api/v2/spot/wallet/transfer", params)
            .await?;

        let result = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty transfer response".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(TransferEntry::new()
            .with_id(result.transfer_id)
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
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<TransferEntry>> {
        let mut params = HashMap::new();

        if let Some(c) = code {
            params.insert("coin".into(), c.to_uppercase());
        }
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: BitgetResponse<Vec<BitgetTransferRecord>> = self
            .private_request("GET", "/api/v2/spot/account/transferRecords", params)
            .await?;

        let records = response.data.unwrap_or_default();

        let transfers: Vec<TransferEntry> = records
            .iter()
            .map(|item| {
                TransferEntry::new()
                    .with_id(item.transfer_id.clone())
                    .with_currency(item.coin.clone())
                    .with_amount(item.amount.parse().unwrap_or_default())
                    .with_from_account(item.from_type.clone())
                    .with_to_account(item.to_type.clone())
                    .with_timestamp(item.ctime.parse().unwrap_or_default())
                    .with_status(item.status.clone())
            })
            .collect();

        Ok(transfers)
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
        let mut params = HashMap::new();
        params.insert("productType".into(), "USDT-FUTURES".into());

        let response: Vec<BitgetPosition> = self
            .mix_private_request("GET", "/account/accounts", params)
            .await?;

        let mut positions: Vec<Position> = response
            .iter()
            .filter(|p| {
                // Filter out zero positions
                p.total
                    .as_ref()
                    .and_then(|t| t.parse::<Decimal>().ok())
                    .map(|t| t != Decimal::ZERO)
                    .unwrap_or(false)
            })
            .map(|p| self.parse_position(p))
            .collect();

        // Filter by symbols if provided
        if let Some(filter_symbols) = symbols {
            positions.retain(|p| filter_symbols.contains(&p.symbol.as_str()));
        }

        Ok(positions)
    }

    async fn set_leverage(&self, leverage: Decimal, symbol: &str) -> CcxtResult<Leverage> {
        let market_id = self.to_mix_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("productType".into(), "USDT-FUTURES".into());
        params.insert("leverage".into(), leverage.to_string());
        params.insert("holdSide".into(), "long".into());

        let _response: serde_json::Value = self
            .mix_private_request("POST", "/account/set-leverage", params.clone())
            .await?;

        // Set for short side too
        params.insert("holdSide".into(), "short".into());
        let _response: serde_json::Value = self
            .mix_private_request("POST", "/account/set-leverage", params)
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
        let market_id = self.to_mix_market_id(symbol);

        let mode_str = match margin_mode {
            MarginMode::Cross => "crossed",
            MarginMode::Isolated => "isolated",
            MarginMode::Unknown => {
                return Err(CcxtError::BadRequest {
                    message: "Unknown margin mode is not supported".into(),
                })
            },
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("productType".into(), "USDT-FUTURES".into());
        params.insert("marginMode".into(), mode_str.into());

        let _response: serde_json::Value = self
            .mix_private_request("POST", "/account/set-margin-mode", params)
            .await?;

        Ok(MarginModeInfo::new(symbol, margin_mode))
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> CcxtResult<FundingRate> {
        let market_id = self.to_mix_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("productType".into(), "USDT-FUTURES".into());

        let response: Vec<BitgetFundingRateData> = self
            .mix_public_get("/current-fund-rate", Some(params))
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
        let mut params = HashMap::new();
        params.insert("productType".into(), "USDT-FUTURES".into());

        let response: Vec<BitgetFundingRateData> = self
            .mix_public_get("/current-fund-rate", Some(params))
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
        let market_id = self.to_mix_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("productType".into(), "USDT-FUTURES".into());

        if let Some(l) = limit {
            params.insert("pageSize".into(), l.min(100).to_string());
        }

        let response: Vec<BitgetFundingRateHistoryData> = self
            .mix_public_get("/history-fund-rate", Some(params))
            .await?;

        let mut history: Vec<FundingRateHistory> = response
            .iter()
            .map(|d| self.parse_funding_rate_history(d))
            .collect();

        // Filter by since if provided
        if let Some(s) = since {
            history.retain(|h| h.timestamp.map(|t| t >= s).unwrap_or(true));
        }

        Ok(history)
    }

    async fn fetch_open_interest(&self, symbol: &str) -> CcxtResult<OpenInterest> {
        let market_id = self.to_mix_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("productType".into(), "USDT-FUTURES".into());

        let response: BitgetOpenInterestData =
            self.mix_public_get("/open-interest", Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();
        let amount: Decimal = response
            .amount
            .as_ref()
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
        // Bitget doesn't have a public liquidation endpoint
        // Return empty vec - liquidation data typically requires websocket
        Ok(Vec::new())
    }

    async fn fetch_index_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_mix_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("productType".into(), "USDT-FUTURES".into());

        let response: Vec<BitgetFundingRateData> = self
            .mix_public_get("/current-fund-rate", Some(params))
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

    async fn fetch_mark_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_mix_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("productType".into(), "USDT-FUTURES".into());

        let response: Vec<BitgetFundingRateData> = self
            .mix_public_get("/current-fund-rate", Some(params))
            .await?;

        let data = response.first().ok_or_else(|| CcxtError::BadResponse {
            message: "Empty mark price response".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();
        let mark_price: Decimal = data
            .mark_price
            .as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            mark_price: Some(mark_price),
            info: serde_json::to_value(data).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn fetch_mark_prices(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        // Bitget requires symbol for funding rate endpoint, so we need to iterate
        let mut params = HashMap::new();
        params.insert("productType".into(), "USDT-FUTURES".into());

        let response: Vec<BitgetFundingRateData> = self
            .mix_public_get("/current-fund-rate", Some(params))
            .await
            .unwrap_or_default();

        let timestamp = Utc::now().timestamp_millis();

        let mut tickers: HashMap<String, Ticker> = HashMap::new();

        for data in &response {
            let market_id = &data.symbol;
            let symbol = match self
                .markets_by_id
                .read()
                .ok()
                .and_then(|m| m.get(market_id).cloned())
            {
                Some(s) => s,
                None => continue,
            };

            if let Some(sym_list) = symbols {
                if !sym_list.contains(&symbol.as_str()) {
                    continue;
                }
            }

            let mark_price: Decimal = match data.mark_price.as_ref().and_then(|p| p.parse().ok()) {
                Some(p) => p,
                None => continue,
            };

            tickers.insert(
                symbol.clone(),
                Ticker {
                    symbol,
                    timestamp: Some(timestamp),
                    datetime: Some(Utc::now().to_rfc3339()),
                    mark_price: Some(mark_price),
                    info: serde_json::to_value(data).unwrap_or_default(),
                    ..Default::default()
                },
            );
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
        let market_id = self.to_mix_market_id(symbol);
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("granularity".into(), interval.clone());
        params.insert("productType".into(), "USDT-FUTURES".into());
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        // Bitget uses /api/v2/mix/market/mark-candles for mark price OHLCV
        let response: BitgetResponse<Vec<Vec<String>>> = self
            .public_get("/api/v2/mix/market/mark-candles", Some(params))
            .await?;

        let data = response.data.unwrap_or_default();

        let ohlcv: Vec<OHLCV> = data
            .iter()
            .filter_map(|c| {
                if c.len() < 5 {
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
        let market_id = self.to_mix_market_id(symbol);
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("granularity".into(), interval.clone());
        params.insert("productType".into(), "USDT-FUTURES".into());
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        // Bitget uses /api/v2/mix/market/index-candles for index price OHLCV
        let response: BitgetResponse<Vec<Vec<String>>> = self
            .public_get("/api/v2/mix/market/index-candles", Some(params))
            .await?;

        let data = response.data.unwrap_or_default();

        let ohlcv: Vec<OHLCV> = data
            .iter()
            .filter_map(|c| {
                if c.len() < 5 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].parse().ok()?,
                    open: c[1].parse().ok()?,
                    high: c[2].parse().ok()?,
                    low: c[3].parse().ok()?,
                    close: c[4].parse().ok()?,
                    volume: Decimal::ZERO, // Index price candles have no volume
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn set_position_mode(
        &self,
        hedged: bool,
        symbol: Option<&str>,
    ) -> CcxtResult<PositionModeInfo> {
        // hedged = true means hedge mode, false means one-way mode
        let pos_mode = if hedged { "hedge_mode" } else { "one_way_mode" };
        let product_type = "USDT-FUTURES";

        let mut params = HashMap::new();
        params.insert("productType".into(), product_type.to_string());
        params.insert("posMode".into(), pos_mode.to_string());

        let response: serde_json::Value = self
            .mix_private_request("POST", "/account/set-position-mode", params)
            .await?;

        let _ = symbol; // symbol is optional for Bitget

        let mode = if hedged {
            PositionMode::Hedged
        } else {
            PositionMode::OneWay
        };
        Ok(PositionModeInfo::new(mode).with_info(response))
    }

    async fn fetch_position_mode(&self, symbol: Option<&str>) -> CcxtResult<PositionModeInfo> {
        let product_type = "USDT-FUTURES";

        let mut params = HashMap::new();
        params.insert("productType".into(), product_type.to_string());

        let response: serde_json::Value = self
            .mix_private_request("GET", "/account/account", params)
            .await?;

        let _ = symbol; // symbol is optional

        // Parse position mode from response
        let pos_mode = response
            .get("posMode")
            .and_then(|v| v.as_str())
            .unwrap_or("one_way_mode");

        let mode = if pos_mode == "hedge_mode" {
            PositionMode::Hedged
        } else {
            PositionMode::OneWay
        };

        Ok(PositionModeInfo::new(mode).with_info(response))
    }

    async fn add_margin(&self, symbol: &str, amount: Decimal) -> CcxtResult<MarginModification> {
        let market_id = self.to_mix_market_id(symbol);
        let product_type = "USDT-FUTURES";

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("productType".into(), product_type.to_string());
        params.insert("marginCoin".into(), "USDT".to_string());
        params.insert("amount".into(), amount.to_string());
        params.insert("holdSide".into(), "long".to_string()); // Default to long

        let response: serde_json::Value = self
            .mix_private_request("POST", "/account/set-margin", params)
            .await?;

        Ok(MarginModification {
            info: response,
            symbol: symbol.to_string(),
            modification_type: Some(MarginModificationType::Add),
            amount: Some(amount),
            ..Default::default()
        })
    }

    async fn reduce_margin(&self, symbol: &str, amount: Decimal) -> CcxtResult<MarginModification> {
        // Bitget uses negative amount for reducing margin
        let market_id = self.to_mix_market_id(symbol);
        let product_type = "USDT-FUTURES";

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("productType".into(), product_type.to_string());
        params.insert("marginCoin".into(), "USDT".to_string());
        params.insert("amount".into(), format!("-{amount}"));
        params.insert("holdSide".into(), "long".to_string()); // Default to long

        let response: serde_json::Value = self
            .mix_private_request("POST", "/account/set-margin", params)
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

// === Bitget API Response Types ===

#[derive(Debug, Deserialize)]
struct BitgetResponse<T> {
    #[allow(dead_code)]
    code: String,
    #[serde(default)]
    data: Option<T>,
    #[serde(default)]
    msg: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitgetSymbol {
    symbol: String,
    #[serde(rename = "baseCoin")]
    base_coin: String,
    #[serde(rename = "quoteCoin")]
    quote_coin: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(rename = "takerFeeRate", default)]
    taker_fee_rate: Option<String>,
    #[serde(rename = "makerFeeRate", default)]
    maker_fee_rate: Option<String>,
    #[serde(rename = "quantityPrecision", default)]
    quantity_precision: Option<i32>,
    #[serde(rename = "pricePrecision", default)]
    price_precision: Option<i32>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitgetTicker {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(rename = "bidPr", default)]
    bid_pr: Option<String>,
    #[serde(rename = "bidSz", default)]
    bid_sz: Option<String>,
    #[serde(rename = "askPr", default)]
    ask_pr: Option<String>,
    #[serde(rename = "askSz", default)]
    ask_sz: Option<String>,
    #[serde(rename = "lastPr", default)]
    last_pr: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(rename = "high24h", default)]
    high24h: Option<String>,
    #[serde(rename = "low24h", default)]
    low24h: Option<String>,
    #[serde(default)]
    change: Option<String>,
    #[serde(rename = "changeUtc24h", default)]
    change_utc24h: Option<String>,
    #[serde(rename = "baseVolume", default)]
    base_volume: Option<String>,
    #[serde(rename = "quoteVolume", default)]
    quote_volume: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BitgetOrderBook {
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitgetTrade {
    #[serde(rename = "tradeId", default)]
    trade_id: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitgetOrder {
    #[serde(rename = "orderId", default)]
    order_id: Option<String>,
    #[serde(rename = "clientOid", default)]
    client_oid: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(rename = "cTime", default)]
    c_time: Option<String>,
    #[serde(rename = "uTime", default)]
    u_time: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(rename = "orderType", default)]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(rename = "priceAvg", default)]
    price_avg: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(rename = "baseVolume", default)]
    base_volume: Option<String>,
    #[serde(rename = "quoteVolume", default)]
    quote_volume: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BitgetOrderResult {
    #[serde(rename = "orderId", default)]
    order_id: String,
    #[allow(dead_code)]
    #[serde(rename = "clientOid", default)]
    client_oid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitgetAccount {
    #[serde(default)]
    coin: String,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    frozen: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    locked: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitgetFill {
    #[serde(rename = "tradeId", default)]
    trade_id: Option<String>,
    #[serde(rename = "orderId", default)]
    order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(rename = "cTime", default)]
    c_time: Option<String>,
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
    #[serde(rename = "tradeScope", default)]
    trade_scope: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitgetDeposit {
    #[serde(rename = "orderId", default)]
    order_id: Option<String>,
    #[serde(default)]
    coin: Option<String>,
    #[serde(default)]
    chain: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(rename = "toAddress", default)]
    to_address: Option<String>,
    #[serde(rename = "txId", default)]
    tx_id: Option<String>,
    #[serde(default)]
    confirm: Option<String>,
    #[serde(rename = "cTime", default)]
    c_time: Option<String>,
    #[serde(rename = "uTime", default)]
    u_time: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitgetWithdrawal {
    #[serde(rename = "orderId", default)]
    order_id: Option<String>,
    #[serde(default)]
    coin: Option<String>,
    #[serde(default)]
    chain: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(rename = "toAddress", default)]
    to_address: Option<String>,
    #[serde(rename = "txId", default)]
    tx_id: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    confirm: Option<String>,
    #[serde(rename = "cTime", default)]
    c_time: Option<String>,
    #[serde(rename = "uTime", default)]
    u_time: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitgetWithdrawResponse {
    #[serde(rename = "orderId", default)]
    order_id: String,
    #[serde(rename = "clientOid", default)]
    client_oid: Option<String>,
}

// === Futures Response Types ===

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitgetPosition {
    #[serde(default)]
    symbol: String,
    #[serde(rename = "holdSide", default)]
    hold_side: Option<String>,
    #[serde(default)]
    total: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(rename = "openPriceAvg", default)]
    open_price_avg: Option<String>,
    #[serde(rename = "markPrice", default)]
    mark_price: Option<String>,
    #[serde(rename = "liquidationPrice", default)]
    liquidation_price: Option<String>,
    #[serde(default)]
    leverage: Option<String>,
    #[serde(default)]
    margin: Option<String>,
    #[serde(rename = "marginMode", default)]
    margin_mode: Option<String>,
    #[serde(rename = "unrealizedPL", default)]
    unrealized_pl: Option<String>,
    #[serde(rename = "achievedProfits", default)]
    achieved_profits: Option<String>,
    #[serde(rename = "uTime", default)]
    u_time: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitgetFundingRateData {
    #[serde(default)]
    symbol: String,
    #[serde(rename = "fundingRate", default)]
    funding_rate: Option<String>,
    #[serde(rename = "fundingTime", default)]
    funding_time: Option<String>,
    #[serde(rename = "nextFundingTime", default)]
    next_funding_time: Option<String>,
    #[serde(rename = "markPrice", default)]
    mark_price: Option<String>,
    #[serde(rename = "indexPrice", default)]
    index_price: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitgetFundingRateHistoryData {
    #[serde(default)]
    symbol: String,
    #[serde(rename = "fundingRate", default)]
    funding_rate: Option<String>,
    #[serde(rename = "fundingTime", default)]
    funding_time: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitgetOpenInterestData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    ts: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BitgetTransferResult {
    transfer_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BitgetTransferRecord {
    transfer_id: String,
    coin: String,
    amount: String,
    from_type: String,
    to_type: String,
    ctime: String,
    status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Bitget::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Bitget);
        assert_eq!(exchange.name(), "Bitget");
    }
}
