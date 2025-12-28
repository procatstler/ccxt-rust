//! KuCoin Futures Exchange Implementation
//!
//! KuCoin Futures API 구현

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, FundingRate,
    FundingRateHistory, Leverage, Market, MarketLimits, MarketPrecision, MarketType, MarginMode,
    MarginModeInfo, OHLCV, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Position, PositionSide, SignedRequest, TakerOrMaker, Ticker, Timeframe, TimeInForce, Trade,
};

type HmacSha256 = Hmac<Sha256>;

/// KuCoin Futures 거래소
pub struct KucoinFutures {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl KucoinFutures {
    const BASE_URL: &'static str = "https://api-futures.kucoin.com";
    const RATE_LIMIT_MS: u64 = 75;

    /// 새 KucoinFutures 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: false,
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
            fetch_positions: true,
            fetch_funding_rate: true,
            fetch_funding_rates: true,
            fetch_funding_rate_history: true,
            fetch_funding_history: true,
            fetch_mark_price: true,
            fetch_mark_prices: true,
            fetch_index_price: true,
            set_leverage: false,
            set_margin_mode: true,
            set_position_mode: true,
            fetch_position_mode: false,
            fetch_leverage: true,
            add_margin: true,
            fetch_positions_history: true,
            fetch_deposit_address: true,
            create_deposit_address: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/147508995-9e35030a-d046-43a1-a006-6fabd981b554.jpg".into()),
            api: api_urls,
            www: Some("https://futures.kucoin.com/".into()),
            doc: vec![
                "https://docs.kucoin.com/futures".into(),
                "https://docs.kucoin.com".into(),
            ],
            fees: Some("https://www.kucoin.com/vip/level".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".into());
        timeframes.insert(Timeframe::Minute5, "5".into());
        timeframes.insert(Timeframe::Minute15, "15".into());
        timeframes.insert(Timeframe::Minute30, "30".into());
        timeframes.insert(Timeframe::Hour1, "60".into());
        timeframes.insert(Timeframe::Hour2, "120".into());
        timeframes.insert(Timeframe::Hour4, "240".into());
        timeframes.insert(Timeframe::Hour8, "480".into());
        timeframes.insert(Timeframe::Hour12, "720".into());
        timeframes.insert(Timeframe::Day1, "1440".into());
        timeframes.insert(Timeframe::Week1, "10080".into());

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

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<KucoinResponse<T>> {
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

        self.client.get(&url, None, None).await
    }

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<KucoinResponse<T>> {
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
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Invalid secret".into(),
            })?;
        mac.update(sign_string.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        // Sign the passphrase
        let mut passphrase_mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Invalid secret".into(),
            })?;
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
                self.client.get(&full_path, query_params, Some(headers)).await
            }
            "POST" => {
                let json_body: Option<serde_json::Value> = if body.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&body).unwrap_or_default())
                };
                self.client.post(&full_path, json_body, Some(headers)).await
            }
            "DELETE" => {
                let query_params = if params.is_empty() { None } else { Some(params) };
                self.client.delete(&full_path, query_params, Some(headers)).await
            }
            _ => Err(CcxtError::BadRequest {
                message: "Invalid HTTP method".into(),
            }),
        }
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USDT:USDT → XBTUSDTM)
    fn to_market_id(&self, symbol: &str) -> String {
        // Remove the settlement currency suffix
        let base_symbol = symbol.split(':').next().unwrap_or(symbol);
        base_symbol.replace('/', "")
    }

    /// 마켓 ID → 심볼 변환
    fn to_symbol(&self, _market_id: &str, base: &str, quote: &str, settle: &str) -> String {
        format!("{base}/{quote}:{settle}")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &KucoinFuturesTicker, symbol: &str) -> Ticker {
        let timestamp = data.ts.map(|ts| ts / 1_000_000).or_else(|| Some(Utc::now().timestamp_millis()));

        Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime: timestamp.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
            }),
            high: data.high_price.as_ref().and_then(|v| v.parse().ok()),
            low: data.low_price.as_ref().and_then(|v| v.parse().ok()),
            bid: data.best_bid_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.best_bid_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.best_ask_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.best_ask_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: None,
            close: data.price.as_ref().and_then(|v| v.parse().ok()),
            last: data.price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.size,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &KucoinFuturesOrder, symbol: &str) -> Order {
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
            Some("stop") | Some("stop_limit") => OrderType::StopLimit,
            Some("stop_market") => OrderType::StopMarket,
            _ => OrderType::Limit,
        };

        let time_in_force = data.time_in_force.as_ref().and_then(|tif| match tif.as_str() {
            "GTC" => Some(TimeInForce::GTC),
            "IOC" => Some(TimeInForce::IOC),
            "FOK" => Some(TimeInForce::FOK),
            _ => None,
        });

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.size.as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data.filled_size.as_ref()
            .and_then(|f| f.parse().ok())
            .unwrap_or_default();
        let remaining = Some(amount - filled);
        let cost = data.filled_value.as_ref().and_then(|c| c.parse().ok());
        let average = if filled > Decimal::ZERO {
            cost.map(|c| c / filled)
        } else {
            None
        };

        Order {
            id: data.id.clone().unwrap_or_default(),
            client_order_id: data.client_oid.clone(),
            timestamp: data.created_at,
            datetime: data.created_at.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: None,
            last_update_timestamp: data.updated_at,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force,
            side,
            price,
            average,
            amount,
            filled,
            remaining,
            stop_price: data.stop_price.as_ref().and_then(|p| p.parse().ok()),
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: data.reduce_only,
            post_only: data.post_only,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, data: &KucoinFuturesBalance) -> Balances {
        let mut result = Balances::new();

        let currency = data.currency.clone();
        let total: Option<Decimal> = data.account_equity.as_ref().and_then(|v| v.parse().ok());
        let free: Option<Decimal> = data.available_balance.as_ref().and_then(|v| v.parse().ok());
        let used = match (total, free) {
            (Some(t), Some(f)) => Some(t - f),
            _ => None,
        };

        let balance = Balance {
            free,
            used,
            total,
            debt: None,
        };
        result.add(&currency, balance);

        result
    }

    /// 포지션 응답 파싱
    fn parse_position(&self, data: &KucoinFuturesPosition) -> Position {
        let side = match data.current_qty.as_ref()
            .and_then(|q| q.parse::<Decimal>().ok())
        {
            Some(qty) if qty < Decimal::ZERO => Some(PositionSide::Short),
            Some(qty) if qty > Decimal::ZERO => Some(PositionSide::Long),
            _ => None,
        };

        let contracts: Decimal = data.current_qty.as_ref()
            .and_then(|q| q.parse::<Decimal>().ok())
            .unwrap_or_default()
            .abs();

        let markets_by_id = self.markets_by_id.read().unwrap();
        let symbol = markets_by_id
            .get(&data.symbol)
            .cloned()
            .unwrap_or_else(|| data.symbol.clone());

        Position {
            symbol,
            id: data.id.clone(),
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp: None,
            datetime: None,
            contracts: Some(contracts),
            contract_size: Some(Decimal::ONE),
            side,
            notional: data.position_value.as_ref().and_then(|v| v.parse().ok()),
            leverage: data.real_leverage.as_ref().and_then(|v| v.parse().ok()),
            unrealized_pnl: data.unrealised_pnl.as_ref().and_then(|v| v.parse().ok()),
            realized_pnl: data.realised_pnl.as_ref().and_then(|v| v.parse().ok()),
            collateral: None,
            entry_price: data.avg_entry_price.as_ref().and_then(|v| v.parse().ok()),
            mark_price: data.mark_price.as_ref().and_then(|v| v.parse().ok()),
            liquidation_price: data.liquidation_price.as_ref().and_then(|v| v.parse().ok()),
            margin_mode: if data.is_cross.unwrap_or(false) {
                Some(MarginMode::Cross)
            } else {
                Some(MarginMode::Isolated)
            },
            hedged: None,
            maintenance_margin: data.maint_margin.as_ref().and_then(|v| v.parse().ok()),
            maintenance_margin_percentage: data.maint_margin_req.as_ref().and_then(|v| v.parse().ok()),
            initial_margin: None,
            initial_margin_percentage: None,
            margin_ratio: None,
            last_update_timestamp: None,
            last_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            percentage: None,
        }
    }

    /// 펀딩 레이트 파싱
    fn parse_funding_rate(&self, data: &KucoinFuturesFundingRate, symbol: &str) -> FundingRate {
        FundingRate {
            info: serde_json::to_value(data).unwrap_or_default(),
            symbol: symbol.to_string(),
            funding_rate: data.value,
            timestamp: data.timepoint,
            datetime: data.timepoint.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
            }),
            funding_timestamp: None,
            funding_datetime: None,
            mark_price: None,
            index_price: None,
            interest_rate: None,
            estimated_settle_price: None,
            previous_funding_rate: None,
            next_funding_rate: data.predicted_value,
            previous_funding_timestamp: None,
            previous_funding_datetime: None,
            next_funding_timestamp: None,
            next_funding_datetime: None,
            interval: Some("8h".to_string()),
        }
    }
}

#[async_trait]
impl Exchange for KucoinFutures {
    fn id(&self) -> ExchangeId {
        ExchangeId::KucoinFutures
    }

    fn name(&self) -> &str {
        "KuCoin Futures"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["SC"]
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
        let response: KucoinResponse<Vec<KucoinFuturesContract>> =
            self.public_get("/api/v1/contracts/active", None).await?;

        let contracts = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let mut markets = Vec::new();

        for contract in contracts {
            let base = contract.base_currency.clone();
            let quote = contract.quote_currency.clone().unwrap_or_else(|| "USDT".to_string());
            let settle = quote.clone();
            let symbol = self.to_symbol(&contract.symbol, &base, &quote, &settle);

            let market = Market {
                id: contract.symbol.clone(),
                lowercase_id: Some(contract.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                settle: Some(settle.clone()),
                settle_id: Some(settle.clone()),
                active: true,
                market_type: if contract.symbol.ends_with("M") {
                    MarketType::Swap
                } else {
                    MarketType::Future
                },
                spot: false,
                margin: false,
                swap: contract.symbol.ends_with("M"),
                future: !contract.symbol.ends_with("M"),
                option: false,
                index: false,
                contract: true,
                linear: Some(true),
                inverse: Some(false),
                sub_type: Some("linear".into()),
                taker: Some(Decimal::new(6, 4)), // 0.0006
                maker: Some(Decimal::new(2, 4)), // 0.0002
                contract_size: Some(contract.multiplier.unwrap_or(Decimal::ONE)),
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: contract.lot_size.and_then(|v| v.to_string().split('.').nth(1).map(|s| s.len() as i32)),
                    price: contract.tick_size.and_then(|v| v.to_string().split('.').nth(1).map(|s| s.len() as i32)),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&contract).unwrap_or_default(),
                tier_based: true,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: KucoinResponse<KucoinFuturesTicker> =
            self.public_get("/api/v1/ticker", Some(params)).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        Ok(self.parse_ticker(&data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: KucoinResponse<Vec<KucoinFuturesContract>> =
            self.public_get("/api/v1/contracts/active", None).await?;

        let contracts = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for contract in contracts {
            if let Some(symbol) = markets_by_id.get(&contract.symbol) {
                if let Some(filter) = symbols {
                    if !filter.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                // Create a ticker from contract data
                let ticker = Ticker {
                    symbol: symbol.clone(),
                    timestamp: None,
                    datetime: None,
                    high: contract.high_price,
                    low: contract.low_price,
                    bid: None,
                    bid_volume: None,
                    ask: None,
                    ask_volume: None,
                    vwap: None,
                    open: None,
                    close: contract.last_trade_price,
                    last: contract.last_trade_price,
                    previous_close: None,
                    change: None,
                    percentage: None,
                    average: None,
                    base_volume: contract.volume_of_24h,
                    quote_volume: None,
                    index_price: contract.index_price,
                    mark_price: contract.mark_price,
                    info: serde_json::to_value(&contract).unwrap_or_default(),
                };
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let path = if let Some(l) = limit {
            format!("/api/v1/level2/depth{}", l)
        } else {
            "/api/v1/level2/snapshot".to_string()
        };

        let response: KucoinResponse<KucoinFuturesOrderBook> =
            self.public_get(&path, Some(params)).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let bids: Vec<OrderBookEntry> = data
            .bids
            .unwrap_or_default()
            .iter()
            .map(|b| OrderBookEntry {
                price: b[0].parse().unwrap_or_default(),
                amount: b[1].parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .unwrap_or_default()
            .iter()
            .map(|a| OrderBookEntry {
                price: a[0].parse().unwrap_or_default(),
                amount: a[1].parse().unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: data.timestamp,
            datetime: data.timestamp.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
            }),
            nonce: data.sequence,
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
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: KucoinResponse<Vec<KucoinFuturesTrade>> =
            self.public_get("/api/v1/trade/history", Some(params)).await?;

        let trades_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let trades: Vec<Trade> = trades_data
            .iter()
            .map(|t| {
                let timestamp = t.ts.map(|ts| ts / 1_000_000);
                Trade {
                    id: t.trade_id.clone().unwrap_or_default(),
                    order: None,
                    timestamp,
                    datetime: timestamp.and_then(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                    }),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: t.side.clone(),
                    taker_or_maker: None,
                    price: t.price.as_ref().and_then(|p| p.parse().ok()).unwrap_or_default(),
                    amount: t.size.unwrap_or_default(),
                    cost: t.price.as_ref().and_then(|p| p.parse::<Decimal>().ok())
                        .map(|p| p * t.size.unwrap_or_default()),
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
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let granularity = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {timeframe:?}"),
        })?.clone();

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("granularity".into(), granularity.clone());

        if let Some(s) = since {
            params.insert("from".into(), (s / 1000).to_string());
        }
        if let Some(l) = limit {
            // KuCoin returns up to 500 candles
            let to = since.unwrap_or_else(|| Utc::now().timestamp_millis())
                + (l.min(500) as i64 * granularity.parse::<i64>().unwrap_or(60) * 60 * 1000);
            params.insert("to".into(), (to / 1000).to_string());
        }

        let response: KucoinResponse<Vec<Vec<serde_json::Value>>> =
            self.public_get("/api/v1/kline/query", Some(params)).await?;

        let candles = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let ohlcv: Vec<OHLCV> = candles
            .iter()
            .filter_map(|c| {
                if c.len() < 6 {
                    return None;
                }
                // [timestamp, open, close, high, low, volume]
                Some(OHLCV {
                    timestamp: c[0].as_i64()? * 1000,
                    open: c[1].as_str()?.parse().ok()?,
                    high: c[3].as_str()?.parse().ok()?,
                    low: c[4].as_str()?.parse().ok()?,
                    close: c[2].as_str()?.parse().ok()?,
                    volume: c[5].as_str()?.parse().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        // KuCoin Futures requires currency parameter
        let currency = self.config.password().unwrap_or("USDT");
        let mut params = HashMap::new();
        params.insert("currency".into(), currency.to_string());

        let response: KucoinResponse<KucoinFuturesBalance> =
            self.private_request("GET", "/api/v1/account-overview", params).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
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
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("side".into(), match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }.to_string());
        params.insert("type".into(), match order_type {
            OrderType::Limit => "limit",
            OrderType::Market => "market",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {order_type:?}"),
            }),
        }.to_string());

        // Convert amount to contract size
        let leverage = amount.to_string();
        params.insert("size".into(), leverage);

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
        }

        let response: KucoinResponse<KucoinFuturesOrderId> =
            self.private_request("POST", "/api/v1/orders", params).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        // Fetch the created order to return full details
        self.fetch_order(&data.order_id, symbol).await
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let params = HashMap::new();
        let path = format!("/api/v1/orders/{}", id);

        let _response: KucoinResponse<KucoinFuturesCancelledOrders> =
            self.private_request("DELETE", &path, params).await?;

        // Fetch the cancelled order to return full details
        self.fetch_order(id, symbol).await
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("orderId".into(), id.to_string());

        let path = format!("/api/v1/orders/{}", id);

        let response: KucoinResponse<KucoinFuturesOrder> =
            self.private_request("GET", &path, params).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let symbol = data.symbol.as_ref()
            .and_then(|s| markets_by_id.get(s))
            .cloned()
            .unwrap_or_else(|| _symbol.to_string());

        Ok(self.parse_order(&data, &symbol))
    }

    async fn fetch_open_orders(&self, symbol: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(sym) = symbol {
            let markets = self.markets.read().unwrap();
            let market = markets.get(sym).ok_or_else(|| CcxtError::BadSymbol {
                symbol: sym.to_string(),
            })?;
            params.insert("symbol".into(), market.id.clone());
        }

        params.insert("status".into(), "active".to_string());

        let response: KucoinResponse<KucoinFuturesOrderList> =
            self.private_request("GET", "/api/v1/orders", params).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = data
            .items
            .iter()
            .filter_map(|o| {
                let sym = o.symbol.as_ref()
                    .and_then(|s| markets_by_id.get(s))
                    .cloned()?;
                Some(self.parse_order(o, &sym))
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_closed_orders(&self, symbol: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(sym) = symbol {
            let markets = self.markets.read().unwrap();
            let market = markets.get(sym).ok_or_else(|| CcxtError::BadSymbol {
                symbol: sym.to_string(),
            })?;
            params.insert("symbol".into(), market.id.clone());
        }

        params.insert("status".into(), "done".to_string());

        let response: KucoinResponse<KucoinFuturesOrderList> =
            self.private_request("GET", "/api/v1/orders", params).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = data
            .items
            .iter()
            .filter_map(|o| {
                let sym = o.symbol.as_ref()
                    .and_then(|s| markets_by_id.get(s))
                    .cloned()?;
                Some(self.parse_order(o, &sym))
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();

        if let Some(sym) = symbol {
            let markets = self.markets.read().unwrap();
            let market = markets.get(sym).ok_or_else(|| CcxtError::BadSymbol {
                symbol: sym.to_string(),
            })?;
            params.insert("symbol".into(), market.id.clone());
        }

        let response: KucoinResponse<KucoinFuturesFillList> =
            self.private_request("GET", "/api/v1/fills", params).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let trades: Vec<Trade> = data
            .items
            .iter()
            .filter_map(|f| {
                let sym = f.symbol.as_ref()
                    .and_then(|s| markets_by_id.get(s))
                    .cloned()?;

                Some(Trade {
                    id: f.trade_id.clone()?,
                    order: f.order_id.clone(),
                    timestamp: f.created_at,
                    datetime: f.created_at.and_then(|t| {
                        chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
                    }),
                    symbol: sym,
                    trade_type: None,
                    side: f.side.clone(),
                    taker_or_maker: f.liquidity.as_ref().and_then(|l| match l.as_str() {
                        "taker" => Some(TakerOrMaker::Taker),
                        "maker" => Some(TakerOrMaker::Maker),
                        _ => None,
                    }),
                    price: f.price.as_ref().and_then(|p| p.parse().ok()).unwrap_or_default(),
                    amount: f.size.unwrap_or_default(),
                    cost: f.value.as_ref().and_then(|v| v.parse().ok()),
                    fee: f.fee.as_ref().map(|fee_val| {
                        crate::types::Fee {
                            cost: fee_val.parse().ok(),
                            currency: f.fee_currency.clone(),
                            rate: None,
                        }
                    }),
                    fees: Vec::new(),
                    info: serde_json::to_value(f).unwrap_or_default(),
                })
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_position(&self, symbol: &str) -> CcxtResult<Position> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: KucoinResponse<KucoinFuturesPosition> =
            self.private_request("GET", "/api/v1/position", params).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        Ok(self.parse_position(&data))
    }

    async fn fetch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>> {
        let params = HashMap::new();

        let response: KucoinResponse<Vec<KucoinFuturesPosition>> =
            self.private_request("GET", "/api/v1/positions", params).await?;

        let positions_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let mut positions: Vec<Position> = positions_data
            .iter()
            .map(|p| self.parse_position(p))
            .collect();

        if let Some(syms) = symbols {
            positions.retain(|p| syms.contains(&p.symbol.as_str()));
        }

        Ok(positions)
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> CcxtResult<FundingRate> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: KucoinResponse<KucoinFuturesFundingRate> =
            self.public_get("/api/v1/funding-rate/{symbol}/current", Some(params)).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        Ok(self.parse_funding_rate(&data, symbol))
    }

    async fn fetch_funding_rate_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<FundingRateHistory>> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        if let Some(s) = since {
            params.insert("from".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("to".into(), (since.unwrap_or(0) + (l as i64 * 28800000)).to_string());
        }

        let response: KucoinResponse<Vec<KucoinFuturesFundingRateHistory>> =
            self.private_request("GET", "/api/v1/funding-history", params).await?;

        let history_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let history: Vec<FundingRateHistory> = history_data
            .iter()
            .filter_map(|h| {
                Some(FundingRateHistory {
                    info: serde_json::to_value(h).unwrap_or_default(),
                    symbol: symbol.to_string(),
                    funding_rate: h.funding_rate?,
                    timestamp: h.timepoint,
                    datetime: h.timepoint.and_then(|t| {
                        chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
                    }),
                })
            })
            .collect();

        Ok(history)
    }

    async fn fetch_positions_history(
        &self,
        symbols: Option<&[&str]>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Position>> {
        let mut params = HashMap::new();

        if let Some(syms) = symbols {
            if syms.len() == 1 {
                let market_id = {
                    let markets = self.markets.read().unwrap();
                    let market = markets.get(syms[0]).ok_or_else(|| CcxtError::BadSymbol {
                        symbol: syms[0].to_string(),
                    })?;
                    market.id.clone()
                };
                params.insert("symbol".into(), market_id);
            }
        }

        if let Some(s) = since {
            params.insert("from".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: KucoinResponse<Vec<KucoinFuturesPositionHistory>> =
            self.private_request("GET", "/api/v1/history-positions", params).await?;

        let history_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let markets_by_id_snapshot: HashMap<String, String> = {
            let markets_by_id = self.markets_by_id.read().unwrap();
            markets_by_id.clone()
        };

        let positions: Vec<Position> = history_data
            .iter()
            .filter_map(|h| {
                let sym = markets_by_id_snapshot.get(&h.symbol).cloned()?;

                Some(Position {
                    symbol: sym,
                    id: None,
                    info: serde_json::to_value(h).unwrap_or_default(),
                    timestamp: h.open_time,
                    datetime: h.open_time.and_then(|t| {
                        chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
                    }),
                    contracts: None,
                    contract_size: None,
                    side: None,
                    notional: None,
                    leverage: None,
                    unrealized_pnl: None,
                    realized_pnl: h.realised_profit.as_ref().and_then(|v| v.parse().ok()),
                    collateral: None,
                    entry_price: None,
                    mark_price: None,
                    liquidation_price: None,
                    margin_mode: None,
                    hedged: None,
                    maintenance_margin: None,
                    maintenance_margin_percentage: None,
                    initial_margin: None,
                    initial_margin_percentage: None,
                    margin_ratio: None,
                    last_update_timestamp: h.close_time,
                    last_price: None,
                    stop_loss_price: None,
                    take_profit_price: None,
                    percentage: None,
                })
            })
            .collect();

        Ok(positions)
    }

    async fn set_margin_mode(
        &self,
        margin_mode: MarginMode,
        symbol: &str,
    ) -> CcxtResult<MarginModeInfo> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("marginMode".into(), match margin_mode {
            MarginMode::Cross => "CROSS",
            MarginMode::Isolated => "ISOLATED",
            MarginMode::Unknown => return Err(CcxtError::BadRequest {
                message: "Unknown margin mode".into(),
            }),
        }.to_string());

        let _response: KucoinResponse<serde_json::Value> =
            self.private_request("POST", "/api/v2/position/changeMarginMode", params).await?;

        Ok(MarginModeInfo::new(symbol, margin_mode))
    }

    async fn add_margin(&self, symbol: &str, amount: Decimal) -> CcxtResult<crate::types::MarginModification> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("margin".into(), amount.to_string());

        let response: KucoinResponse<serde_json::Value> =
            self.private_request("POST", "/api/v1/position/margin/deposit-margin", params).await?;

        let code = response.code.clone();
        Ok(crate::types::MarginModification {
            info: response.data.unwrap_or_default(),
            symbol: symbol.to_string(),
            modification_type: Some(crate::types::MarginModificationType::Add),
            margin_mode: None,
            amount: Some(amount),
            total: None,
            code: Some(code.clone()),
            status: Some(if code == "200000" { "ok".to_string() } else { "error".to_string() }),
            timestamp: None,
            datetime: None,
        })
    }

    async fn fetch_leverage(&self, symbol: &str) -> CcxtResult<Leverage> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: KucoinResponse<KucoinFuturesPosition> =
            self.private_request("GET", "/api/v1/position", params).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        let leverage_value: Decimal = data.real_leverage.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| Decimal::ONE);

        Ok(Leverage {
            info: serde_json::to_value(&data).unwrap_or_default(),
            symbol: symbol.to_string(),
            margin_mode: MarginMode::Cross, // KuCoin Futures defaults to cross margin
            long_leverage: leverage_value,
            short_leverage: leverage_value,
        })
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<crate::types::DepositAddress> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_uppercase());
        if let Some(net) = network {
            params.insert("chain".into(), net.to_string());
        }

        let response: KucoinResponse<KucoinDepositAddress> =
            self.private_request("GET", "/api/v1/deposit-address", params).await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No data in response".into(),
        })?;

        Ok(crate::types::DepositAddress {
            info: serde_json::to_value(&data).unwrap_or_default(),
            currency: code.to_string(),
            network: data.chain.clone(),
            address: data.address,
            tag: data.memo,
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
}

// Response wrapper
#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
struct KucoinResponse<T> {
    code: String,
    #[serde(default)]
    data: Option<T>,
}

// Contract data structure
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesContract {
    symbol: String,
    base_currency: String,
    quote_currency: Option<String>,
    #[serde(default)]
    multiplier: Option<Decimal>,
    #[serde(default)]
    tick_size: Option<Decimal>,
    #[serde(default)]
    lot_size: Option<Decimal>,
    #[serde(default)]
    index_price: Option<Decimal>,
    #[serde(default)]
    mark_price: Option<Decimal>,
    #[serde(default)]
    last_trade_price: Option<Decimal>,
    #[serde(default)]
    high_price: Option<Decimal>,
    #[serde(default)]
    low_price: Option<Decimal>,
    #[serde(rename = "volumeOf24h", default)]
    volume_of_24h: Option<Decimal>,
}

// Ticker data structure
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesTicker {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<Decimal>,
    #[serde(default)]
    best_bid_price: Option<String>,
    #[serde(default)]
    best_bid_size: Option<String>,
    #[serde(default)]
    best_ask_price: Option<String>,
    #[serde(default)]
    best_ask_size: Option<String>,
    #[serde(default)]
    high_price: Option<String>,
    #[serde(default)]
    low_price: Option<String>,
}

// OrderBook data structure
#[derive(Debug, Default, Deserialize)]
struct KucoinFuturesOrderBook {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    sequence: Option<i64>,
    #[serde(default)]
    bids: Option<Vec<Vec<String>>>,
    #[serde(default)]
    asks: Option<Vec<Vec<String>>>,
}

// Trade data structure
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesTrade {
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<Decimal>,
}

// Order data structure
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesOrder {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    client_oid: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(rename = "type", default)]
    order_type: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    filled_size: Option<String>,
    #[serde(default)]
    filled_value: Option<String>,
    #[serde(default)]
    is_active: Option<bool>,
    #[serde(default)]
    cancel_exist: Option<bool>,
    #[serde(default)]
    created_at: Option<i64>,
    #[serde(default)]
    updated_at: Option<i64>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    time_in_force: Option<String>,
    #[serde(default)]
    reduce_only: Option<bool>,
    #[serde(default)]
    post_only: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct KucoinFuturesOrderId {
    #[serde(rename = "orderId")]
    order_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct KucoinFuturesOrderList {
    #[serde(default)]
    items: Vec<KucoinFuturesOrder>,
}

#[derive(Debug, Default, Deserialize)]
struct KucoinFuturesCancelledOrders {
    #[serde(rename = "cancelledOrderIds", default)]
    cancelled_order_ids: Vec<String>,
}

// Balance data structure
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesBalance {
    #[serde(default)]
    currency: String,
    #[serde(default)]
    account_equity: Option<String>,
    #[serde(default)]
    available_balance: Option<String>,
}

// Position data structure
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesPosition {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    current_qty: Option<String>,
    #[serde(default)]
    position_value: Option<String>,
    #[serde(default)]
    avg_entry_price: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    liquidation_price: Option<String>,
    #[serde(default)]
    unrealised_pnl: Option<String>,
    #[serde(default)]
    realised_pnl: Option<String>,
    #[serde(default)]
    real_leverage: Option<String>,
    #[serde(default)]
    is_cross: Option<bool>,
    #[serde(default)]
    maint_margin: Option<String>,
    #[serde(default)]
    maint_margin_req: Option<String>,
}

// Fill (trade) data structure
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesFill {
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<Decimal>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    fee_currency: Option<String>,
    #[serde(default)]
    liquidity: Option<String>,
    #[serde(default)]
    created_at: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct KucoinFuturesFillList {
    #[serde(default)]
    items: Vec<KucoinFuturesFill>,
}

// Funding rate data structure
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesFundingRate {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    value: Option<Decimal>,
    #[serde(default)]
    predicted_value: Option<Decimal>,
    #[serde(default)]
    timepoint: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesFundingRateHistory {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    funding_rate: Option<Decimal>,
    #[serde(default)]
    timepoint: Option<i64>,
}

// Position history data structure
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinFuturesPositionHistory {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    open_time: Option<i64>,
    #[serde(default)]
    close_time: Option<i64>,
    #[serde(default)]
    realised_profit: Option<String>,
}

// Deposit address data structure
#[derive(Debug, Default, Deserialize, Serialize)]
struct KucoinDepositAddress {
    #[serde(default)]
    address: String,
    #[serde(default)]
    memo: Option<String>,
    #[serde(default)]
    chain: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kucoin_futures_new() {
        let config = ExchangeConfig::default();
        let exchange = KucoinFutures::new(config);
        assert!(exchange.is_ok());

        let exchange = exchange.unwrap();
        assert_eq!(exchange.id(), ExchangeId::KucoinFutures);
        assert_eq!(exchange.name(), "KuCoin Futures");
        assert_eq!(exchange.version(), "v1");
    }

    #[test]
    fn test_kucoin_futures_features() {
        let config = ExchangeConfig::default();
        let exchange = KucoinFutures::new(config).unwrap();
        let features = exchange.has();

        assert!(features.swap);
        assert!(features.future);
        assert!(!features.spot);
        assert!(features.fetch_markets);
        assert!(features.fetch_ticker);
        assert!(features.fetch_order_book);
        assert!(features.fetch_positions);
        assert!(features.fetch_funding_rate);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::default();
        let exchange = KucoinFutures::new(config).unwrap();

        let market_id = exchange.to_market_id("BTC/USDT:USDT");
        assert_eq!(market_id, "BTCUSDT");

        let symbol = exchange.to_symbol("XBTUSDTM", "BTC", "USDT", "USDT");
        assert_eq!(symbol, "BTC/USDT:USDT");
    }

    #[tokio::test]
    async fn test_fetch_markets() {
        let config = ExchangeConfig::default();
        let exchange = KucoinFutures::new(config).unwrap();

        // This test would require network access
        // In a real scenario, you'd mock the HTTP client
        // let markets = exchange.fetch_markets().await;
        // assert!(markets.is_ok());
    }

    #[test]
    fn test_parse_ticker() {
        let config = ExchangeConfig::default();
        let exchange = KucoinFutures::new(config).unwrap();

        let ticker_data = KucoinFuturesTicker {
            symbol: Some("XBTUSDTM".to_string()),
            ts: Some(1638574296209786785),
            price: Some("50000.5".to_string()),
            size: Some(Decimal::new(100, 0)),
            best_bid_price: Some("50000.0".to_string()),
            best_bid_size: Some("10".to_string()),
            best_ask_price: Some("50001.0".to_string()),
            best_ask_size: Some("15".to_string()),
            high_price: Some("51000.0".to_string()),
            low_price: Some("49000.0".to_string()),
        };

        let ticker = exchange.parse_ticker(&ticker_data, "BTC/USDT:USDT");
        assert_eq!(ticker.symbol, "BTC/USDT:USDT");
        assert!(ticker.last.is_some());
        assert!(ticker.bid.is_some());
        assert!(ticker.ask.is_some());
    }

    #[test]
    fn test_parse_order() {
        let config = ExchangeConfig::default();
        let exchange = KucoinFutures::new(config).unwrap();

        let order_data = KucoinFuturesOrder {
            id: Some("order123".to_string()),
            client_oid: Some("client123".to_string()),
            symbol: Some("XBTUSDTM".to_string()),
            order_type: Some("limit".to_string()),
            side: Some("buy".to_string()),
            price: Some("50000.0".to_string()),
            size: Some("1".to_string()),
            filled_size: Some("0.5".to_string()),
            filled_value: Some("25000.0".to_string()),
            is_active: Some(true),
            cancel_exist: Some(false),
            created_at: Some(1638574296000),
            updated_at: Some(1638574296000),
            stop_price: None,
            time_in_force: Some("GTC".to_string()),
            reduce_only: Some(false),
            post_only: Some(false),
        };

        let order = exchange.parse_order(&order_data, "BTC/USDT:USDT");
        assert_eq!(order.id, "order123");
        assert_eq!(order.symbol, "BTC/USDT:USDT");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.status, OrderStatus::Open);
    }

    #[test]
    fn test_parse_position() {
        let config = ExchangeConfig::default();
        let exchange = KucoinFutures::new(config).unwrap();

        let position_data = KucoinFuturesPosition {
            id: Some("pos123".to_string()),
            symbol: "XBTUSDTM".to_string(),
            current_qty: Some("10".to_string()),
            position_value: Some("500000".to_string()),
            avg_entry_price: Some("50000.0".to_string()),
            mark_price: Some("50500.0".to_string()),
            liquidation_price: Some("45000.0".to_string()),
            unrealised_pnl: Some("5000".to_string()),
            realised_pnl: Some("1000".to_string()),
            real_leverage: Some("10".to_string()),
            is_cross: Some(true),
            maint_margin: Some("2500".to_string()),
            maint_margin_req: Some("0.005".to_string()),
        };

        let position = exchange.parse_position(&position_data);
        assert_eq!(position.symbol, "XBTUSDTM");
        assert_eq!(position.side, Some(PositionSide::Long));
        assert_eq!(position.margin_mode, Some(MarginMode::Cross));
    }
}
