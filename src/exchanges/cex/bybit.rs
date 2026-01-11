//! Bybit Exchange Implementation
//!
//! CCXT bybit.ts를 Rust로 포팅

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::RwLock;

use super::bybit_ws::BybitWs;
use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, ConvertCurrencyPair, ConvertQuote, ConvertTrade, Exchange, ExchangeFeatures,
    ExchangeId, ExchangeUrls, FundingRate, FundingRateHistory, Leverage, Liquidation, MarginMode,
    MarginModeInfo, Market, MarketLimits, MarketPrecision, MarketType, OpenInterest, Order,
    OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Position, PositionSide,
    SignedRequest, Ticker, TimeInForce, Timeframe, Trade, Transaction, TransactionStatus,
    TransactionType, WsExchange, WsMessage, OHLCV,
};
use std::sync::Arc;
use tokio::sync::RwLock as TokioRwLock;

type HmacSha256 = Hmac<Sha256>;

/// Bybit 거래소
pub struct Bybit {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    ws_client: Arc<TokioRwLock<BybitWs>>,
}

impl Bybit {
    const BASE_URL: &'static str = "https://api.bybit.com";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second
    const RECV_WINDOW: &'static str = "5000";

    /// 새 Bybit 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
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
            logo: Some("https://user-images.githubusercontent.com/51840849/76547799-daff5b80-649e-11ea-87fb-3be9bac08954.png".into()),
            api: api_urls,
            www: Some("https://www.bybit.com".into()),
            doc: vec![
                "https://bybit-exchange.github.io/docs/v5/intro".into(),
            ],
            fees: Some("https://www.bybit.com/en-US/help-center/bybitHC_Article?id=000001634".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".into());
        timeframes.insert(Timeframe::Minute3, "3".into());
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
            ws_client: Arc::new(TokioRwLock::new(BybitWs::new())),
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

        let response: BybitResponse<T> = self.client.get(&url, None, None).await?;

        if response.ret_code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response
                    .ret_msg
                    .unwrap_or_else(|| format!("Bybit error code: {}", response.ret_code)),
            });
        }

        response.result.ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty response data".into(),
        })
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
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        let timestamp = Utc::now().timestamp_millis().to_string();

        let mut headers = HashMap::new();
        headers.insert("X-BAPI-API-KEY".into(), api_key.to_string());
        headers.insert("X-BAPI-TIMESTAMP".into(), timestamp.clone());
        headers.insert("X-BAPI-RECV-WINDOW".into(), Self::RECV_WINDOW.into());

        let response: BybitResponse<T> = match method.to_uppercase().as_str() {
            "GET" => {
                // For GET requests, sign query string
                let query: String = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");

                let param_str = format!("{}{}{}{}", timestamp, api_key, Self::RECV_WINDOW, query);

                let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                    .expect("HMAC can take key of any size");
                mac.update(param_str.as_bytes());
                let signature = hex::encode(mac.finalize().into_bytes());

                headers.insert("X-BAPI-SIGN".into(), signature);

                let url = if query.is_empty() {
                    path.to_string()
                } else {
                    format!("{path}?{query}")
                };

                self.client.get(&url, None, Some(headers)).await?
            },
            "POST" => {
                // For POST requests, sign JSON body
                let body_str = serde_json::to_string(&params).unwrap_or_else(|_| "{}".into());

                let param_str =
                    format!("{}{}{}{}", timestamp, api_key, Self::RECV_WINDOW, body_str);

                let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                    .expect("HMAC can take key of any size");
                mac.update(param_str.as_bytes());
                let signature = hex::encode(mac.finalize().into_bytes());

                headers.insert("X-BAPI-SIGN".into(), signature);
                headers.insert("Content-Type".into(), "application/json".into());

                let json_body: serde_json::Value =
                    serde_json::from_str(&body_str).unwrap_or_default();
                self.client
                    .post(path, Some(json_body), Some(headers))
                    .await?
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("HTTP method: {method}"),
                })
            },
        };

        if response.ret_code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response
                    .ret_msg
                    .unwrap_or_else(|| format!("Bybit error code: {}", response.ret_code)),
            });
        }

        response.result.ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty response data".into(),
        })
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USDT → BTCUSDT)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// 마켓 ID → 심볼 변환 (BTCUSDT → BTC/USDT)
    #[allow(dead_code)]
    fn to_symbol(&self, _market_id: &str, base: &str, quote: &str) -> String {
        format!("{base}/{quote}")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &BybitTicker, symbol: &str) -> Ticker {
        let timestamp = Utc::now().timestamp_millis();

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low_price24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid1_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid1_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask1_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask1_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: None,
            close: data.last_price.as_ref().and_then(|v| v.parse().ok()),
            last: data.last_price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: data.prev_price24h.as_ref().and_then(|v| v.parse().ok()),
            change: data.price24h_pcnt.as_ref().and_then(|_| {
                let prev: Decimal = data.prev_price24h.as_ref()?.parse().ok()?;
                let last: Decimal = data.last_price.as_ref()?.parse().ok()?;
                Some(last - prev)
            }),
            percentage: data.price24h_pcnt.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.volume24h.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.turnover24h.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &BybitOrder) -> Order {
        let status = match data.order_status.as_str() {
            "New" | "PartiallyFilled" | "Untriggered" => OrderStatus::Open,
            "Filled" => OrderStatus::Closed,
            "Cancelled" | "PartiallyFilledCanceled" | "Deactivated" => OrderStatus::Canceled,
            "Rejected" => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_str() {
            "Limit" => OrderType::Limit,
            "Market" => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_str() {
            "Buy" => OrderSide::Buy,
            "Sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = data
            .time_in_force
            .as_ref()
            .and_then(|tif| match tif.as_str() {
                "GTC" => Some(TimeInForce::GTC),
                "IOC" => Some(TimeInForce::IOC),
                "FOK" => Some(TimeInForce::FOK),
                "PostOnly" => Some(TimeInForce::PO),
                _ => None,
            });

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.qty.parse().unwrap_or_default();
        let filled: Decimal = data
            .cum_exec_qty
            .as_ref()
            .and_then(|f| f.parse().ok())
            .unwrap_or_default();
        let remaining = Some(amount - filled);
        let average = data.avg_price.as_ref().and_then(|a| a.parse().ok());
        let cost = data.cum_exec_value.as_ref().and_then(|c| c.parse().ok());
        let timestamp = data.created_time.as_ref().and_then(|t| t.parse().ok());
        let symbol_str = data.symbol.replace("-", "/");
        let symbol = if symbol_str.contains('/') {
            symbol_str
        } else {
            // Try to parse BTCUSDT format
            self.markets_by_id
                .read()
                .unwrap()
                .get(&data.symbol)
                .cloned()
                .unwrap_or(data.symbol.clone())
        };

        Order {
            id: data.order_id.clone(),
            client_order_id: data.order_link_id.clone(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: data.updated_time.as_ref().and_then(|t| t.parse().ok()),
            last_update_timestamp: data.updated_time.as_ref().and_then(|t| t.parse().ok()),
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
            stop_price: data.trigger_price.as_ref().and_then(|p| p.parse().ok()),
            trigger_price: data.trigger_price.as_ref().and_then(|p| p.parse().ok()),
            take_profit_price: data.take_profit.as_ref().and_then(|p| p.parse().ok()),
            stop_loss_price: data.stop_loss.as_ref().and_then(|p| p.parse().ok()),
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: data.reduce_only,
            post_only: data.time_in_force.as_ref().map(|t| t == "PostOnly"),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[BybitCoin]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let available: Option<Decimal> = b
                .available_to_withdraw
                .as_ref()
                .and_then(|v| v.parse().ok());
            let total: Option<Decimal> = b.wallet_balance.as_ref().and_then(|v| v.parse().ok());
            let used = match (total, available) {
                (Some(t), Some(a)) => Some(t - a),
                _ => None,
            };

            let balance = Balance {
                free: available,
                used,
                total,
                debt: None,
            };
            result.add(&b.coin, balance);
        }

        result
    }

    /// 입금 내역 파싱
    fn parse_deposit(&self, data: &BybitDeposit) -> Transaction {
        let status = match data.status.as_deref() {
            Some("0") => TransactionStatus::Pending, // unknown
            Some("1") => TransactionStatus::Pending, // toBeConfirmed
            Some("2") => TransactionStatus::Pending, // processing
            Some("3") => TransactionStatus::Ok,      // success
            Some("4") => TransactionStatus::Failed,  // deposit failed
            _ => TransactionStatus::Pending,
        };

        let timestamp = data.success_at.as_ref().and_then(|t| t.parse::<i64>().ok());
        let amount: Decimal = data.amount.parse().unwrap_or_default();

        Transaction {
            id: data.id.clone().unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: None,
            tx_type: TransactionType::Deposit,
            currency: data.coin.clone(),
            network: data.chain.clone(),
            amount,
            status,
            address: data.to_address.clone(),
            tag: data.tag.clone(),
            txid: data.tx_id.clone(),
            fee: None,
            internal: None,
            confirmations: data
                .confirmations
                .as_ref()
                .and_then(|c| c.parse::<u32>().ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 출금 내역 파싱
    fn parse_withdrawal(&self, data: &BybitWithdrawal) -> Transaction {
        let status = match data.status.as_deref() {
            Some("SecurityCheck") | Some("Pending") | Some("CancelByUser") => {
                TransactionStatus::Pending
            },
            Some("success") => TransactionStatus::Ok,
            Some("Reject") | Some("Fail") | Some("BlockchainConfirmed") => {
                TransactionStatus::Failed
            },
            _ => TransactionStatus::Pending,
        };

        let timestamp = data
            .create_time
            .as_ref()
            .and_then(|t| t.parse::<i64>().ok());
        let amount: Decimal = data.amount.parse().unwrap_or_default();

        Transaction {
            id: data.withdraw_id.clone(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: data
                .update_time
                .as_ref()
                .and_then(|t| t.parse::<i64>().ok()),
            tx_type: TransactionType::Withdrawal,
            currency: data.coin.clone(),
            network: data.chain.clone(),
            amount,
            status,
            address: data.to_address.clone(),
            tag: data.tag.clone(),
            txid: data.tx_id.clone(),
            fee: data
                .withdraw_fee
                .as_ref()
                .and_then(|f| f.parse::<Decimal>().ok())
                .map(|cost| crate::types::Fee {
                    cost: Some(cost),
                    currency: Some(data.coin.clone()),
                    rate: None,
                }),
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 포지션 파싱
    fn parse_position(&self, data: &BybitPosition) -> Position {
        let timestamp = data
            .updated_time
            .as_ref()
            .and_then(|t| t.parse::<i64>().ok());
        let symbol = self
            .markets_by_id
            .read()
            .unwrap()
            .get(&data.symbol)
            .cloned()
            .unwrap_or_else(|| data.symbol.clone());

        let side = match data.side.as_deref() {
            Some("Buy") => Some(PositionSide::Long),
            Some("Sell") => Some(PositionSide::Short),
            _ => None,
        };

        let contracts: Option<Decimal> = data.size.as_ref().and_then(|s| s.parse().ok());
        let entry_price: Option<Decimal> = data.avg_price.as_ref().and_then(|p| p.parse().ok());
        let mark_price: Option<Decimal> = data.mark_price.as_ref().and_then(|p| p.parse().ok());
        let leverage: Option<Decimal> = data.leverage.as_ref().and_then(|l| l.parse().ok());
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
        let margin: Option<Decimal> = data.position_im.as_ref().and_then(|m| m.parse().ok());

        let margin_mode = match data.trade_mode.as_deref() {
            Some("0") => Some(MarginMode::Cross),
            Some("1") => Some(MarginMode::Isolated),
            _ => None,
        };

        Position {
            symbol,
            id: data.position_idx.as_ref().map(|i| i.to_string()),
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            contracts,
            contract_size: None,
            side,
            notional: data.position_value.as_ref().and_then(|v| v.parse().ok()),
            leverage,
            collateral: margin,
            initial_margin: data.position_im.as_ref().and_then(|m| m.parse().ok()),
            initial_margin_percentage: None,
            maintenance_margin: data.position_mm.as_ref().and_then(|m| m.parse().ok()),
            maintenance_margin_percentage: None,
            entry_price,
            mark_price,
            last_price: None,
            liquidation_price,
            unrealized_pnl,
            realized_pnl: data.cum_realised_pnl.as_ref().and_then(|p| p.parse().ok()),
            percentage: None,
            margin_mode,
            margin_ratio: None,
            hedged: Some(data.position_idx.as_ref().map(|i| *i != 0).unwrap_or(false)),
            last_update_timestamp: timestamp,
            stop_loss_price: data.stop_loss.as_ref().and_then(|s| s.parse().ok()),
            take_profit_price: data.take_profit.as_ref().and_then(|t| t.parse().ok()),
        }
    }

    /// 펀딩 레이트 파싱
    fn parse_funding_rate(&self, data: &BybitFundingRateData) -> FundingRate {
        let timestamp = data
            .funding_rate_timestamp
            .as_ref()
            .and_then(|t| t.parse::<i64>().ok());
        let symbol = self
            .markets_by_id
            .read()
            .unwrap()
            .get(&data.symbol)
            .cloned()
            .unwrap_or_else(|| data.symbol.clone());

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
            funding_timestamp: data
                .funding_rate_timestamp
                .as_ref()
                .and_then(|t| t.parse().ok()),
            funding_datetime: None,
            next_funding_rate: None,
            next_funding_timestamp: None,
            next_funding_datetime: None,
            previous_funding_rate: None,
            previous_funding_timestamp: None,
            previous_funding_datetime: None,
            interval: Some("8h".to_string()),
        }
    }

    /// 펀딩 레이트 기록 파싱
    fn parse_funding_rate_history(&self, data: &BybitFundingRateHistoryData) -> FundingRateHistory {
        let timestamp = data
            .funding_rate_timestamp
            .as_ref()
            .and_then(|t| t.parse::<i64>().ok());
        let symbol = self
            .markets_by_id
            .read()
            .unwrap()
            .get(&data.symbol)
            .cloned()
            .unwrap_or_else(|| data.symbol.clone());
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
    fn to_linear_market_id(&self, symbol: &str) -> String {
        // Handle BTC/USDT:USDT format for perpetual swaps
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
impl Exchange for Bybit {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bybit
    }

    fn name(&self) -> &str {
        "Bybit"
    }

    fn version(&self) -> &str {
        "v5"
    }

    fn countries(&self) -> &[&str] {
        &["VG", "SC"] // British Virgin Islands, Seychelles
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
        let mut params = HashMap::new();
        params.insert("category".into(), "spot".into());

        let response: BybitInstrumentsResult = self
            .public_get("/v5/market/instruments-info", Some(params))
            .await?;

        let mut markets = Vec::new();

        for inst in response.list {
            let base = inst.base_coin.clone();
            let quote = inst.quote_coin.clone();
            let symbol = format!("{base}/{quote}");

            let lot_sz: Option<Decimal> = inst
                .lot_size_filter
                .as_ref()
                .and_then(|f| f.base_precision.as_ref())
                .and_then(|v| v.parse().ok());
            let tick_sz: Option<Decimal> = inst
                .price_filter
                .as_ref()
                .and_then(|f| f.tick_size.as_ref())
                .and_then(|v| v.parse().ok());
            let min_qty: Option<Decimal> = inst
                .lot_size_filter
                .as_ref()
                .and_then(|f| f.min_order_qty.as_ref())
                .and_then(|v| v.parse().ok());
            let max_qty: Option<Decimal> = inst
                .lot_size_filter
                .as_ref()
                .and_then(|f| f.max_order_qty.as_ref())
                .and_then(|v| v.parse().ok());

            let market = Market {
                id: inst.symbol.clone(),
                lowercase_id: Some(inst.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: inst.base_coin.clone(),
                quote_id: inst.quote_coin.clone(),
                settle: None,
                settle_id: None,
                active: inst.status == "Trading",
                market_type: MarketType::Spot,
                spot: true,
                margin: inst
                    .margin_trading
                    .as_ref()
                    .map(|m| m != "none")
                    .unwrap_or(false),
                swap: false,
                future: false,
                option: false,
                index: false,
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: Some(Decimal::new(1, 3)), // 0.1%
                maker: Some(Decimal::new(1, 3)), // 0.1%
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
                        min: min_qty,
                        max: max_qty,
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
        params.insert("category".into(), "spot".into());
        params.insert("symbol".into(), market_id);

        let response: BybitTickersResult =
            self.public_get("/v5/market/tickers", Some(params)).await?;

        let ticker_data = response.list.first().ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        Ok(self.parse_ticker(ticker_data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let mut params = HashMap::new();
        params.insert("category".into(), "spot".into());

        let response: BybitTickersResult =
            self.public_get("/v5/market/tickers", Some(params)).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for data in response.list {
            let symbol = markets_by_id
                .get(&data.symbol)
                .cloned()
                .unwrap_or_else(|| data.symbol.clone());

            if let Some(filter) = symbols {
                if !filter.contains(&symbol.as_str()) {
                    continue;
                }
            }

            let ticker = self.parse_ticker(&data, &symbol);
            tickers.insert(symbol, ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("category".into(), "spot".into());
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(200).to_string());
        }

        let response: BybitOrderBook = self
            .public_get("/v5/market/orderbook", Some(params))
            .await?;

        let bids: Vec<OrderBookEntry> = response
            .b
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

        let asks: Vec<OrderBookEntry> = response
            .a
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

        let timestamp = response.ts.and_then(|t| t.parse::<i64>().ok());

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            nonce: response.u,
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
        params.insert("category".into(), "spot".into());
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(60).to_string());
        }

        let response: BybitTradesResult = self
            .public_get("/v5/market/recent-trade", Some(params))
            .await?;

        let trades: Vec<Trade> = response
            .list
            .iter()
            .map(|t| {
                let timestamp = t.time.as_ref().and_then(|ts| ts.parse::<i64>().ok());
                Trade {
                    id: t.exec_id.clone().unwrap_or_default(),
                    order: None,
                    timestamp,
                    datetime: timestamp.and_then(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                    }),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: t.side.clone(),
                    taker_or_maker: None,
                    price: t.price.parse().unwrap_or_default(),
                    amount: t.size.parse().unwrap_or_default(),
                    cost: Some(
                        t.price.parse::<Decimal>().unwrap_or_default()
                            * t.size.parse::<Decimal>().unwrap_or_default(),
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
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("category".into(), "spot".into());
        params.insert("symbol".into(), market_id);
        params.insert("interval".into(), interval.clone());
        if let Some(s) = since {
            params.insert("start".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: BybitKlineResult = self.public_get("/v5/market/kline", Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response
            .list
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
        let mut params = HashMap::new();
        params.insert("accountType".into(), "UNIFIED".into());

        let response: BybitAccountResult = self
            .private_request("GET", "/v5/account/wallet-balance", params)
            .await?;

        let account = response
            .list
            .first()
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "Empty balance response".into(),
            })?;

        Ok(self.parse_balance(&account.coin))
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
            OrderType::Limit => "Limit",
            OrderType::Market => "Market",
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type: {order_type:?}"),
                })
            },
        };

        let side_str = match side {
            OrderSide::Buy => "Buy",
            OrderSide::Sell => "Sell",
        };

        let mut params = HashMap::new();
        params.insert("category".into(), "spot".into());
        params.insert("symbol".into(), market_id);
        params.insert("side".into(), side_str.into());
        params.insert("orderType".into(), ord_type.into());
        params.insert("qty".into(), amount.to_string());

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
            params.insert("timeInForce".into(), "GTC".into());
        }

        let response: BybitOrderResult = self
            .private_request("POST", "/v5/order/create", params)
            .await?;

        // Fetch the order details
        self.fetch_order(&response.order_id, symbol).await
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("category".into(), "spot".into());
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.into());

        let _response: BybitOrderResult = self
            .private_request("POST", "/v5/order/cancel", params)
            .await?;

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

        let mut params = HashMap::new();
        params.insert("category".into(), "spot".into());
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.into());

        if let Some(amt) = amount {
            params.insert("qty".into(), amt.to_string());
        }

        if let Some(p) = price {
            params.insert("price".into(), p.to_string());
        }

        let _response: BybitOrderResult = self
            .private_request("POST", "/v5/order/amend", params)
            .await?;

        self.fetch_order(id, symbol).await
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("category".into(), "spot".into());
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.into());

        let response: BybitOrdersResult = self
            .private_request("GET", "/v5/order/realtime", params)
            .await?;

        let order_data = response
            .list
            .first()
            .ok_or_else(|| CcxtError::OrderNotFound {
                order_id: id.into(),
            })?;

        Ok(self.parse_order(order_data))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("category".into(), "spot".into());

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BybitOrdersResult = self
            .private_request("GET", "/v5/order/realtime", params)
            .await?;

        let orders: Vec<Order> = response.list.iter().map(|o| self.parse_order(o)).collect();

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("category".into(), "spot".into());

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(50).to_string());
        }

        let response: BybitOrdersResult = self
            .private_request("GET", "/v5/order/history", params)
            .await?;

        let orders: Vec<Order> = response
            .list
            .iter()
            .filter(|o| {
                o.order_status == "Filled"
                    || o.order_status == "Cancelled"
                    || o.order_status == "PartiallyFilledCanceled"
            })
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
        let mut params = HashMap::new();
        params.insert("category".into(), "spot".into());

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: BybitExecutionResult = self
            .private_request("GET", "/v5/execution/list", params)
            .await?;

        let trades: Vec<Trade> = response
            .list
            .iter()
            .map(|e| {
                let timestamp = e.exec_time.as_ref().and_then(|t| t.parse::<i64>().ok());
                let price: Decimal = e.exec_price.parse().unwrap_or_default();
                let amount: Decimal = e.exec_qty.parse().unwrap_or_default();
                let fee_amount: Option<Decimal> = e.exec_fee.as_ref().and_then(|f| f.parse().ok());

                let symbol_str = self
                    .markets_by_id
                    .read()
                    .unwrap()
                    .get(&e.symbol)
                    .cloned()
                    .unwrap_or_else(|| e.symbol.clone());

                Trade {
                    id: e.exec_id.clone(),
                    order: Some(e.order_id.clone()),
                    timestamp,
                    datetime: timestamp.and_then(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                    }),
                    symbol: symbol_str,
                    trade_type: None,
                    side: e.side.clone(),
                    taker_or_maker: e.is_maker.map(|m| {
                        if m {
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
                        currency: e.fee_currency.clone(),
                        rate: None,
                    }),
                    fees: Vec::new(),
                    info: serde_json::to_value(e).unwrap_or_default(),
                }
            })
            .collect();

        Ok(trades)
    }

    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("category".into(), "spot".into());

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }

        let response: BybitCancelAllResult = self
            .private_request("POST", "/v5/order/cancel-all", params)
            .await?;

        // Return empty or cancelled orders info
        let orders: Vec<Order> = response
            .list
            .iter()
            .map(|o| Order {
                id: o.order_id.clone(),
                client_order_id: o.order_link_id.clone(),
                timestamp: None,
                datetime: None,
                last_trade_timestamp: None,
                last_update_timestamp: None,
                status: OrderStatus::Canceled,
                symbol: symbol.unwrap_or("").to_string(),
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
                info: serde_json::to_value(o).unwrap_or_default(),
            })
            .collect();

        Ok(orders)
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
            params.insert("limit".into(), l.min(50).to_string());
        }

        let response: BybitDepositResult = self
            .private_request("GET", "/v5/asset/deposit/query-record", params)
            .await?;

        let transactions: Vec<Transaction> = response
            .rows
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
            params.insert("coin".into(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(50).to_string());
        }

        let response: BybitWithdrawResult = self
            .private_request("GET", "/v5/asset/withdraw/query-record", params)
            .await?;

        let transactions: Vec<Transaction> = response
            .rows
            .iter()
            .map(|w| self.parse_withdrawal(w))
            .collect();

        Ok(transactions)
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<crate::types::DepositAddress> {
        let mut params = HashMap::new();
        params.insert("coin".into(), code.to_string());

        if let Some(n) = network {
            params.insert("chainType".into(), n.to_string());
        }

        let response: BybitDepositAddressResult = self
            .private_request("GET", "/v5/asset/deposit/query-address", params)
            .await?;

        let chain = response
            .chains
            .first()
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "No deposit address found".into(),
            })?;

        let mut deposit_addr =
            crate::types::DepositAddress::new(&response.coin, &chain.address_deposit);

        if let Some(ref chain_type) = chain.chain_type {
            deposit_addr = deposit_addr.with_network(chain_type.clone());
        }
        if let Some(ref tag) = chain.tag {
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
        let mut params = HashMap::new();
        params.insert("coin".into(), code.to_string());
        params.insert("amount".into(), amount.to_string());
        params.insert("address".into(), address.to_string());
        params.insert(
            "timestamp".into(),
            Utc::now().timestamp_millis().to_string(),
        );

        if let Some(t) = tag {
            params.insert("tag".into(), t.to_string());
        }
        if let Some(n) = network {
            params.insert("chain".into(), n.to_string());
        }

        let response: BybitWithdrawResponse = self
            .private_request("POST", "/v5/asset/withdraw/create", params)
            .await?;

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
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let url = format!("{}{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();

        if api == "private" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();
            let timestamp = Utc::now().timestamp_millis().to_string();

            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");

            let param_str = format!("{}{}{}{}", timestamp, api_key, Self::RECV_WINDOW, query);

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(param_str.as_bytes());
            let signature = hex::encode(mac.finalize().into_bytes());

            headers.insert("X-BAPI-API-KEY".into(), api_key.to_string());
            headers.insert("X-BAPI-SIGN".into(), signature);
            headers.insert("X-BAPI-TIMESTAMP".into(), timestamp);
            headers.insert("X-BAPI-RECV-WINDOW".into(), Self::RECV_WINDOW.into());
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        }
    }

    // === WebSocket API ===

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
        params.insert("category".into(), "linear".into());

        let response: BybitPositionsResult = self
            .private_request("GET", "/v5/position/list", params)
            .await?;

        let mut positions: Vec<Position> = response
            .list
            .iter()
            .map(|p| self.parse_position(p))
            .collect();

        // Filter by symbols if provided
        if let Some(filter_symbols) = symbols {
            positions.retain(|p| filter_symbols.contains(&p.symbol.as_str()));
        }

        Ok(positions)
    }

    async fn set_leverage(&self, leverage: Decimal, symbol: &str) -> CcxtResult<Leverage> {
        let market_id = self.to_linear_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("category".into(), "linear".into());
        params.insert("symbol".into(), market_id);
        params.insert("buyLeverage".into(), leverage.to_string());
        params.insert("sellLeverage".into(), leverage.to_string());

        let _response: serde_json::Value = self
            .private_request("POST", "/v5/position/set-leverage", params)
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
        let market_id = self.to_linear_market_id(symbol);

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
        params.insert("category".into(), "linear".into());
        params.insert("symbol".into(), market_id);
        params.insert("tradeMode".into(), mode_str.into());
        // Setting to cross/isolated requires sending buy/sell leverage
        params.insert("buyLeverage".into(), "10".into());
        params.insert("sellLeverage".into(), "10".into());

        let _response: serde_json::Value = self
            .private_request("POST", "/v5/position/switch-isolated", params)
            .await?;

        Ok(MarginModeInfo::new(symbol, margin_mode))
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> CcxtResult<FundingRate> {
        let market_id = self.to_linear_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("category".into(), "linear".into());
        params.insert("symbol".into(), market_id);

        let response: BybitTickersResult =
            self.public_get("/v5/market/tickers", Some(params)).await?;

        let data = response.list.first().ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        // Build FundingRate from ticker data
        let timestamp = Utc::now().timestamp_millis();
        Ok(FundingRate {
            symbol: symbol.to_string(),
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
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
        })
    }

    async fn fetch_funding_rates(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, FundingRate>> {
        let mut params = HashMap::new();
        params.insert("category".into(), "linear".into());

        let response: BybitTickersResult =
            self.public_get("/v5/market/tickers", Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();
        let mut rates = HashMap::new();

        for data in response.list {
            let symbol = self
                .markets_by_id
                .read()
                .unwrap()
                .get(&data.symbol)
                .cloned()
                .unwrap_or_else(|| data.symbol.clone());

            if let Some(filter) = symbols {
                if !filter.contains(&symbol.as_str()) {
                    continue;
                }
            }

            let rate = FundingRate {
                symbol: symbol.clone(),
                info: serde_json::to_value(&data).unwrap_or_default(),
                timestamp: Some(timestamp),
                datetime: Some(Utc::now().to_rfc3339()),
                funding_rate: data.funding_rate.as_ref().and_then(|r| r.parse().ok()),
                mark_price: data.mark_price.as_ref().and_then(|m| m.parse().ok()),
                index_price: data.index_price.as_ref().and_then(|i| i.parse().ok()),
                interest_rate: None,
                estimated_settle_price: None,
                funding_timestamp: data.next_funding_time.as_ref().and_then(|t| t.parse().ok()),
                funding_datetime: None,
                next_funding_rate: None,
                next_funding_timestamp: data
                    .next_funding_time
                    .as_ref()
                    .and_then(|t| t.parse().ok()),
                next_funding_datetime: None,
                previous_funding_rate: None,
                previous_funding_timestamp: None,
                previous_funding_datetime: None,
                interval: Some("8h".to_string()),
            };

            rates.insert(symbol, rate);
        }

        Ok(rates)
    }

    async fn fetch_funding_rate_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<FundingRateHistory>> {
        let market_id = self.to_linear_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("category".into(), "linear".into());
        params.insert("symbol".into(), market_id);

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(200).to_string());
        }

        let response: BybitFundingRateHistoryResult = self
            .public_get("/v5/market/funding/history", Some(params))
            .await?;

        let history: Vec<FundingRateHistory> = response
            .list
            .iter()
            .map(|d| self.parse_funding_rate_history(d))
            .collect();

        Ok(history)
    }

    async fn fetch_open_interest(&self, symbol: &str) -> CcxtResult<OpenInterest> {
        let market_id = self.to_linear_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("category".into(), "linear".into());
        params.insert("symbol".into(), market_id);

        let response: BybitOpenInterestResult = self
            .public_get("/v5/market/open-interest", Some(params))
            .await?;

        let data = response
            .list
            .first()
            .ok_or_else(|| CcxtError::BadResponse {
                message: "Empty open interest response".into(),
            })?;

        let timestamp: i64 = data.timestamp.parse().unwrap_or_default();
        let amount: Decimal = data.open_interest.parse().unwrap_or_default();

        Ok(OpenInterest {
            symbol: symbol.to_string(),
            open_interest_amount: Some(amount),
            open_interest_value: None,
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
        let market_id = self.to_linear_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("category".into(), "linear".into());
        params.insert("symbol".into(), market_id);

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(200).to_string());
        }

        let _response: BybitLiquidationsResult = self
            .public_get("/v5/market/recent-trade", Some(params))
            .await?;

        // Bybit doesn't have a dedicated liquidation endpoint in v5
        // Filter trades that look like liquidations (usually identified by size patterns)
        // For now, return empty as proper liquidation data requires websocket
        Ok(Vec::new())
    }

    async fn fetch_index_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_linear_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("category".into(), "linear".into());
        params.insert("symbol".into(), market_id);

        let response: BybitTickersResult =
            self.public_get("/v5/market/tickers", Some(params)).await?;

        let data = response
            .list
            .first()
            .ok_or_else(|| CcxtError::BadResponse {
                message: "Empty ticker response".into(),
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
        let market_id = self.to_linear_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("category".into(), "linear".into());
        params.insert("symbol".into(), market_id);

        let response: BybitTickersResult =
            self.public_get("/v5/market/tickers", Some(params)).await?;

        let data = response
            .list
            .first()
            .ok_or_else(|| CcxtError::BadResponse {
                message: "Empty ticker response".into(),
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
        let mut params = HashMap::new();
        params.insert("category".into(), "linear".into());

        let response: BybitTickersResult =
            self.public_get("/v5/market/tickers", Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();

        let mut tickers: HashMap<String, Ticker> = HashMap::new();

        for data in &response.list {
            let symbol = match self
                .markets_by_id
                .read()
                .ok()
                .and_then(|m| m.get(&data.symbol).cloned())
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
        let market_id = self.to_linear_market_id(symbol);
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("category".into(), "linear".into());
        params.insert("symbol".into(), market_id);
        params.insert("interval".into(), interval.clone());
        if let Some(s) = since {
            params.insert("start".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        // Bybit uses /v5/market/mark-price-kline for mark price candles
        let response: BybitKlineResult = self
            .public_get("/v5/market/mark-price-kline", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .list
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
        let market_id = self.to_linear_market_id(symbol);
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("category".into(), "linear".into());
        params.insert("symbol".into(), market_id);
        params.insert("interval".into(), interval.clone());
        if let Some(s) = since {
            params.insert("start".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        // Bybit uses /v5/market/index-price-kline for index price candles
        let response: BybitKlineResult = self
            .public_get("/v5/market/index-price-kline", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .list
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

    // === Margin Trading ===

    async fn borrow_cross_margin(
        &self,
        code: &str,
        amount: Decimal,
    ) -> CcxtResult<crate::types::MarginLoan> {
        let mut params = HashMap::new();
        params.insert("coin".into(), code.to_uppercase());
        params.insert("amount".into(), amount.to_string());

        let response: BybitBorrowResponse = self
            .private_request("POST", "/v5/account/borrow", params)
            .await?;

        let coin = response.coin.clone();
        let amt = response.amount.parse().unwrap_or(amount);

        Ok(crate::types::MarginLoan::new()
            .with_currency(coin)
            .with_amount(amt)
            .with_info(serde_json::to_value(&response).unwrap_or_default()))
    }

    async fn repay_cross_margin(
        &self,
        code: &str,
        amount: Decimal,
    ) -> CcxtResult<crate::types::MarginLoan> {
        let mut params = HashMap::new();
        params.insert("coin".into(), code.to_uppercase());
        params.insert("amount".into(), amount.to_string());

        let response: BybitRepayResponse = self
            .private_request("POST", "/v5/account/repay", params)
            .await?;

        Ok(crate::types::MarginLoan::new()
            .with_currency(code)
            .with_amount(amount)
            .with_info(serde_json::to_value(&response).unwrap_or_default()))
    }

    async fn fetch_cross_borrow_rate(
        &self,
        code: &str,
    ) -> CcxtResult<crate::types::CrossBorrowRate> {
        let mut params = HashMap::new();
        params.insert("coin".into(), code.to_uppercase());

        let response: BybitLoanInfoResponse = self
            .private_request("GET", "/v5/spot-cross-margin-trade/loan-info", params)
            .await?;

        let rate = response
            .interest_rate
            .parse::<Decimal>()
            .unwrap_or_default();

        Ok(crate::types::CrossBorrowRate::new(rate).with_currency(code))
    }

    // === Convert API ===

    /// Fetch available convert currency pairs
    async fn fetch_convert_currencies(&self) -> CcxtResult<Vec<ConvertCurrencyPair>> {
        let mut params = HashMap::new();
        params.insert("accountType".to_string(), "eb_convert_funding".to_string());

        let response: BybitConvertCoins = self
            .private_request("GET", "/v5/asset/convert/query-coin-list", params)
            .await?;

        let mut pairs = Vec::new();
        for coin in response.coins {
            for to_coin in &coin.to_coins {
                let mut pair = ConvertCurrencyPair::new(&coin.coin, to_coin);
                pair.min_amount = coin.min_single_amount.as_ref().and_then(|s| s.parse().ok());
                pair.max_amount = coin.max_single_amount.as_ref().and_then(|s| s.parse().ok());
                pair.info = serde_json::json!({
                    "fromCoin": &coin.coin,
                    "toCoin": to_coin,
                    "minSingleAmount": &coin.min_single_amount,
                    "maxSingleAmount": &coin.max_single_amount,
                });
                pairs.push(pair);
            }
        }

        Ok(pairs)
    }

    /// Get a convert quote
    async fn fetch_convert_quote(
        &self,
        from_code: &str,
        to_code: &str,
        amount: Decimal,
    ) -> CcxtResult<ConvertQuote> {
        let mut params = HashMap::new();
        params.insert("fromCoin".to_string(), from_code.to_uppercase());
        params.insert("toCoin".to_string(), to_code.to_uppercase());
        params.insert("requestCoin".to_string(), from_code.to_uppercase());
        params.insert("requestAmount".to_string(), amount.to_string());
        params.insert("accountType".to_string(), "eb_convert_funding".to_string());

        let response: BybitConvertQuote = self
            .private_request("POST", "/v5/asset/convert/request-quote", params)
            .await?;

        let from_amount: Decimal = response.from_coin_amount.parse().unwrap_or_default();
        let to_amount: Decimal = response.to_coin_amount.parse().unwrap_or_default();
        let exchange_rate: Decimal = response.exchange_rate.parse().unwrap_or_default();

        let mut quote = ConvertQuote::new(
            &response.quote_tx_id,
            from_code,
            to_code,
            from_amount,
            exchange_rate,
        );
        quote.to_amount = Some(to_amount);
        let timestamp = Utc::now().timestamp_millis();
        quote.timestamp = Some(timestamp);
        quote.datetime = Some(Utc::now().to_rfc3339());
        quote.expire_timestamp = response
            .expired_time
            .as_ref()
            .map(|t| t.parse::<i64>().unwrap_or_default());
        quote.info = serde_json::to_value(&response).unwrap_or_default();

        Ok(quote)
    }

    /// Accept a convert quote and execute the trade
    async fn create_convert_trade(&self, quote_id: &str) -> CcxtResult<ConvertTrade> {
        let mut params = HashMap::new();
        params.insert("quoteTxId".to_string(), quote_id.to_string());

        let response: BybitConvertResult = self
            .private_request("POST", "/v5/asset/convert/confirm-quote", params)
            .await?;

        let from_amount: Decimal = response.from_coin_amount.parse().unwrap_or_default();
        let to_amount: Decimal = response.to_coin_amount.parse().unwrap_or_default();
        let exchange_rate: Decimal = response.exchange_rate.parse().unwrap_or_default();

        let mut trade = ConvertTrade::new(
            &response.exchange_tx_id,
            &response.from_coin,
            &response.to_coin,
            from_amount,
            to_amount,
            exchange_rate,
        );
        trade.timestamp = response
            .created_time
            .as_ref()
            .map(|t| t.parse::<i64>().unwrap_or_default());
        trade.datetime = trade.timestamp.map(|t| {
            chrono::DateTime::from_timestamp_millis(t)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        });
        trade.info = serde_json::to_value(&response).unwrap_or_default();
        trade.status = response.status;

        Ok(trade)
    }

    /// Fetch a convert trade by ID
    async fn fetch_convert_trade(&self, id: &str) -> CcxtResult<ConvertTrade> {
        // Bybit doesn't have a direct endpoint for single trade lookup
        // Use history with filter
        let history = self.fetch_convert_trade_history(None, Some(100)).await?;

        history
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| CcxtError::ExchangeError {
                message: format!("Convert trade {id} not found"),
            })
    }

    /// Fetch convert trade history
    async fn fetch_convert_trade_history(
        &self,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<ConvertTrade>> {
        let mut params = HashMap::new();
        params.insert("accountType".to_string(), "eb_convert_funding".to_string());

        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(100).to_string());
        }

        let response: BybitConvertHistory = self
            .private_request("GET", "/v5/asset/convert/query-order-list", params)
            .await?;

        let trades: Vec<ConvertTrade> = response
            .list
            .into_iter()
            .filter(|order| {
                if let Some(since_ts) = since {
                    order
                        .created_time
                        .as_ref()
                        .and_then(|t| t.parse::<i64>().ok())
                        .map(|t| t >= since_ts)
                        .unwrap_or(true)
                } else {
                    true
                }
            })
            .map(|order| {
                let from_amount: Decimal = order.from_coin_amount.parse().unwrap_or_default();
                let to_amount: Decimal = order.to_coin_amount.parse().unwrap_or_default();
                let exchange_rate: Decimal = order.exchange_rate.parse().unwrap_or_default();

                let mut trade = ConvertTrade::new(
                    &order.exchange_tx_id,
                    &order.from_coin,
                    &order.to_coin,
                    from_amount,
                    to_amount,
                    exchange_rate,
                );
                trade.timestamp = order
                    .created_time
                    .as_ref()
                    .map(|t| t.parse::<i64>().unwrap_or_default());
                trade.datetime = trade.timestamp.map(|t| {
                    chrono::DateTime::from_timestamp_millis(t)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                });
                trade.status = order.status.clone();
                trade.info = serde_json::to_value(&order).unwrap_or_default();
                trade
            })
            .collect();

        Ok(trades)
    }
}

impl Bybit {
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

// === Bybit API Response Types ===

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BybitResponse<T> {
    ret_code: i32,
    #[serde(default)]
    ret_msg: Option<String>,
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
struct BybitInstrumentsResult {
    list: Vec<BybitInstrument>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitInstrument {
    symbol: String,
    base_coin: String,
    quote_coin: String,
    status: String,
    #[serde(default)]
    margin_trading: Option<String>,
    #[serde(default)]
    lot_size_filter: Option<BybitLotSizeFilter>,
    #[serde(default)]
    price_filter: Option<BybitPriceFilter>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitLotSizeFilter {
    #[serde(default)]
    base_precision: Option<String>,
    #[serde(default)]
    min_order_qty: Option<String>,
    #[serde(default)]
    max_order_qty: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitPriceFilter {
    #[serde(default)]
    tick_size: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BybitTickersResult {
    list: Vec<BybitTicker>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitTicker {
    symbol: String,
    #[serde(default)]
    last_price: Option<String>,
    #[serde(default)]
    bid1_price: Option<String>,
    #[serde(default)]
    bid1_size: Option<String>,
    #[serde(default)]
    ask1_price: Option<String>,
    #[serde(default)]
    ask1_size: Option<String>,
    #[serde(default)]
    high_price24h: Option<String>,
    #[serde(default)]
    low_price24h: Option<String>,
    #[serde(default)]
    prev_price24h: Option<String>,
    #[serde(default)]
    volume24h: Option<String>,
    #[serde(default)]
    turnover24h: Option<String>,
    #[serde(default)]
    price24h_pcnt: Option<String>,
    // Futures fields
    #[serde(default)]
    funding_rate: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    index_price: Option<String>,
    #[serde(default)]
    next_funding_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BybitOrderBook {
    #[serde(default)]
    a: Vec<Vec<String>>,
    #[serde(default)]
    b: Vec<Vec<String>>,
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    u: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct BybitTradesResult {
    list: Vec<BybitTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitTrade {
    #[serde(default)]
    exec_id: Option<String>,
    price: String,
    size: String,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BybitKlineResult {
    list: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct BybitOrdersResult {
    list: Vec<BybitOrder>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitOrder {
    order_id: String,
    #[serde(default)]
    order_link_id: Option<String>,
    symbol: String,
    #[serde(default)]
    order_type: String,
    side: String,
    order_status: String,
    qty: String,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    avg_price: Option<String>,
    #[serde(default)]
    cum_exec_qty: Option<String>,
    #[serde(default)]
    cum_exec_value: Option<String>,
    #[serde(default)]
    time_in_force: Option<String>,
    #[serde(default)]
    trigger_price: Option<String>,
    #[serde(default)]
    take_profit: Option<String>,
    #[serde(default)]
    stop_loss: Option<String>,
    #[serde(default)]
    created_time: Option<String>,
    #[serde(default)]
    updated_time: Option<String>,
    #[serde(default)]
    reduce_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BybitOrderResult {
    order_id: String,
}

#[derive(Debug, Deserialize)]
struct BybitAccountResult {
    list: Vec<BybitAccount>,
}

#[derive(Debug, Deserialize)]
struct BybitAccount {
    coin: Vec<BybitCoin>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BybitCoin {
    coin: String,
    #[serde(default)]
    wallet_balance: Option<String>,
    #[serde(default)]
    available_to_withdraw: Option<String>,
}

/// Execution (trade) result
#[derive(Debug, Deserialize)]
struct BybitExecutionResult {
    list: Vec<BybitExecution>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitExecution {
    exec_id: String,
    order_id: String,
    symbol: String,
    exec_price: String,
    exec_qty: String,
    #[serde(default)]
    exec_fee: Option<String>,
    #[serde(default)]
    fee_currency: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    is_maker: Option<bool>,
    #[serde(default)]
    exec_time: Option<String>,
}

/// Cancel all orders result
#[derive(Debug, Deserialize)]
struct BybitCancelAllResult {
    #[serde(default)]
    list: Vec<BybitCancelledOrder>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitCancelledOrder {
    order_id: String,
    #[serde(default)]
    order_link_id: Option<String>,
}

/// Deposit result
#[derive(Debug, Deserialize)]
struct BybitDepositResult {
    #[serde(default)]
    rows: Vec<BybitDeposit>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitDeposit {
    #[serde(default)]
    id: Option<String>,
    coin: String,
    #[serde(default)]
    chain: Option<String>,
    amount: String,
    #[serde(default)]
    to_address: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    tx_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    success_at: Option<String>,
    #[serde(default)]
    confirmations: Option<String>,
}

/// Withdrawal result
#[derive(Debug, Deserialize)]
struct BybitWithdrawResult {
    #[serde(default)]
    rows: Vec<BybitWithdrawal>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitWithdrawal {
    withdraw_id: String,
    coin: String,
    #[serde(default)]
    chain: Option<String>,
    amount: String,
    #[serde(default)]
    to_address: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    tx_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    withdraw_fee: Option<String>,
    #[serde(default)]
    create_time: Option<String>,
    #[serde(default)]
    update_time: Option<String>,
}

/// Deposit address result
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BybitDepositAddressResult {
    coin: String,
    chains: Vec<BybitChainAddress>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BybitChainAddress {
    address_deposit: String,
    #[serde(default)]
    chain_type: Option<String>,
    #[serde(default)]
    tag: Option<String>,
}

/// Withdraw response
#[derive(Debug, Deserialize, Serialize)]
struct BybitWithdrawResponse {
    id: String,
}

/// Position result
#[derive(Debug, Deserialize)]
struct BybitPositionsResult {
    list: Vec<BybitPosition>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitPosition {
    symbol: String,
    #[serde(default)]
    position_idx: Option<i32>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    avg_price: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    liq_price: Option<String>,
    #[serde(default)]
    leverage: Option<String>,
    #[serde(default)]
    position_value: Option<String>,
    #[serde(default)]
    unrealised_pnl: Option<String>,
    #[serde(default)]
    cum_realised_pnl: Option<String>,
    #[serde(default)]
    position_im: Option<String>,
    #[serde(default)]
    position_mm: Option<String>,
    #[serde(default)]
    trade_mode: Option<String>,
    #[serde(default)]
    updated_time: Option<String>,
    #[serde(default)]
    stop_loss: Option<String>,
    #[serde(default)]
    take_profit: Option<String>,
}

/// Funding rate data (for ticker response)
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitFundingRateData {
    symbol: String,
    #[serde(default)]
    funding_rate: Option<String>,
    #[serde(default)]
    funding_rate_timestamp: Option<String>,
}

/// Funding rate history result
#[derive(Debug, Deserialize)]
struct BybitFundingRateHistoryResult {
    list: Vec<BybitFundingRateHistoryData>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitFundingRateHistoryData {
    symbol: String,
    #[serde(default)]
    funding_rate: Option<String>,
    #[serde(default)]
    funding_rate_timestamp: Option<String>,
}

/// Open interest result
#[derive(Debug, Deserialize)]
struct BybitOpenInterestResult {
    list: Vec<BybitOpenInterestData>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitOpenInterestData {
    symbol: String,
    open_interest: String,
    timestamp: String,
}

/// Liquidations result (placeholder for consistency)
#[derive(Debug, Deserialize)]
struct BybitLiquidationsResult {
    #[serde(default)]
    list: Vec<serde_json::Value>,
}

/// Borrow response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitBorrowResponse {
    coin: String,
    amount: String,
}

/// Repay response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitRepayResponse {
    #[serde(default)]
    result_status: Option<String>,
}

/// Loan info response (for borrow rate)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BybitLoanInfoResponse {
    coin: String,
    #[serde(default)]
    interest_rate: String,
    #[serde(default)]
    loan_able_amount: Option<String>,
    #[serde(default)]
    max_loan_amount: Option<String>,
}

// === Bybit Convert API Response Types ===

/// Convert coin list from /v5/asset/convert/query-coin-list
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitConvertCoins {
    #[serde(default)]
    coins: Vec<BybitConvertCoin>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitConvertCoin {
    coin: String,
    #[serde(default)]
    to_coins: Vec<String>,
    #[serde(default)]
    min_single_amount: Option<String>,
    #[serde(default)]
    max_single_amount: Option<String>,
}

/// Convert quote from /v5/asset/convert/request-quote
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitConvertQuote {
    quote_tx_id: String,
    #[serde(default)]
    exchange_rate: String,
    #[serde(default)]
    from_coin: String,
    #[serde(default)]
    from_coin_amount: String,
    #[serde(default)]
    to_coin: String,
    #[serde(default)]
    to_coin_amount: String,
    #[serde(default)]
    expired_time: Option<String>,
}

/// Convert result from /v5/asset/convert/confirm-quote
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitConvertResult {
    exchange_tx_id: String,
    #[serde(default)]
    exchange_rate: String,
    #[serde(default)]
    from_coin: String,
    #[serde(default)]
    from_coin_amount: String,
    #[serde(default)]
    to_coin: String,
    #[serde(default)]
    to_coin_amount: String,
    #[serde(default)]
    created_time: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

/// Convert order history from /v5/asset/convert/query-order-list
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitConvertHistory {
    #[serde(default)]
    list: Vec<BybitConvertOrder>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitConvertOrder {
    exchange_tx_id: String,
    #[serde(default)]
    exchange_rate: String,
    #[serde(default)]
    from_coin: String,
    #[serde(default)]
    from_coin_amount: String,
    #[serde(default)]
    to_coin: String,
    #[serde(default)]
    to_coin_amount: String,
    #[serde(default)]
    created_time: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let bybit = Bybit::new(config).unwrap();

        assert_eq!(bybit.to_market_id("BTC/USDT"), "BTCUSDT");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let bybit = Bybit::new(config).unwrap();

        assert_eq!(bybit.id(), ExchangeId::Bybit);
        assert_eq!(bybit.name(), "Bybit");
        assert!(bybit.has().spot);
        assert!(bybit.has().margin);
        assert!(bybit.has().swap);
    }
}
