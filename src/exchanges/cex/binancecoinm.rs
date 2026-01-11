//! Binance COIN-M Futures (Inverse) Exchange Implementation
//!
//! Binance COIN-M (Delivery) Futures API 구현
//! 역방향 무기한 선물 및 분기 선물 지원

#![allow(dead_code)]

use async_trait::async_trait;
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
    MarginMode, Market, MarketLimits, MarketPrecision, MarketType, OpenInterest, Order, OrderBook,
    OrderBookEntry, OrderSide, OrderStatus, OrderType, Position, PositionSide, SignedRequest,
    TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// Binance COIN-M (Delivery) Futures 거래소
pub struct BinanceCoinM {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl BinanceCoinM {
    const BASE_URL: &'static str = "https://dapi.binance.com";
    const RATE_LIMIT_MS: u64 = 50;

    /// 새 BinanceCoinM 인스턴스 생성
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
            fetch_mark_price: true,
            fetch_mark_prices: true,
            fetch_index_price: true,
            fetch_open_interest: true,
            set_leverage: true,
            set_margin_mode: true,
            set_position_mode: true,
            fetch_position_mode: true,
            add_margin: true,
            reduce_margin: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some(
                "https://github.com/user-attachments/assets/387cfc4e-5f33-48cd-8f5c-cd4854dabf0c"
                    .into(),
            ),
            api: api_urls,
            www: Some("https://www.binance.com".into()),
            doc: vec!["https://binance-docs.github.io/apidocs/delivery/en/".into()],
            fees: Some("https://www.binance.com/en/fee/deliveryFee".into()),
        };

        let mut timeframes = HashMap::new();
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
        timeframes.insert(Timeframe::Week1, "1w".into());
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

        self.client.get(&url, None, None).await
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

        let mut query_params = params.clone();
        query_params.insert("timestamp".into(), timestamp);

        let query: String = query_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(query.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let signed_query = format!("{query}&signature={signature}");

        let mut headers = HashMap::new();
        headers.insert("X-MBX-APIKEY".into(), api_key.to_string());

        let url = format!("{path}?{signed_query}");

        match method {
            "GET" => self.client.get(&url, None, Some(headers)).await,
            "POST" => {
                headers.insert(
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                );
                self.client
                    .post(&format!("{path}?{signed_query}"), None, Some(headers))
                    .await
            },
            "DELETE" => self.client.delete(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USD:BTC → BTCUSD_PERP)
    fn to_market_id(&self, symbol: &str) -> String {
        let base_symbol = symbol.split(':').next().unwrap_or(symbol);
        let clean = base_symbol.replace("/", "");
        // COIN-M perpetuals use _PERP suffix
        if symbol.contains(":") && !symbol.contains("_") {
            format!("{clean}_PERP")
        } else {
            clean
        }
    }

    /// 마켓 ID → 심볼 변환 (BTCUSD_PERP → BTC/USD:BTC)
    fn to_symbol(&self, _market_id: &str, base: &str, quote: &str, settle: &str) -> String {
        format!("{base}/{quote}:{settle}")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &BinanceCoinMTicker, symbol: &str) -> Ticker {
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price,
            low: data.low_price,
            bid: data.bid_price,
            bid_volume: data.bid_qty,
            ask: data.ask_price,
            ask_volume: data.ask_qty,
            vwap: data.weighted_avg_price,
            open: data.open_price,
            close: data.last_price,
            last: data.last_price,
            previous_close: data.prev_close_price,
            change: data.price_change,
            percentage: data.price_change_percent,
            average: None,
            base_volume: data.volume,
            quote_volume: data.quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &BinanceCoinMOrder, symbol: &str) -> Order {
        let status = match data.status.as_str() {
            "NEW" => OrderStatus::Open,
            "PARTIALLY_FILLED" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" => OrderStatus::Canceled,
            "PENDING_CANCEL" => OrderStatus::Canceled,
            "REJECTED" => OrderStatus::Rejected,
            "EXPIRED" | "EXPIRED_IN_MATCH" => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            "STOP" => OrderType::StopLimit,
            "STOP_MARKET" => OrderType::StopMarket,
            "TAKE_PROFIT" => OrderType::TakeProfit,
            "TAKE_PROFIT_MARKET" => OrderType::TakeProfitMarket,
            "TRAILING_STOP_MARKET" => OrderType::TrailingStopMarket,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = data
            .time_in_force
            .as_ref()
            .and_then(|tif| match tif.as_str() {
                "GTC" => Some(TimeInForce::GTC),
                "IOC" => Some(TimeInForce::IOC),
                "FOK" => Some(TimeInForce::FOK),
                "GTX" => Some(TimeInForce::PO),
                _ => None,
            });

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.orig_qty.parse().unwrap_or_default();
        let filled: Decimal = data.executed_qty.parse().unwrap_or_default();
        let remaining = Some(amount - filled);
        let cost = data.cum_base.as_ref().and_then(|c| c.parse().ok());
        let average = if filled > Decimal::ZERO {
            data.avg_price.as_ref().and_then(|p| p.parse().ok())
        } else {
            None
        };

        Order {
            id: data.order_id.to_string(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(data.time.unwrap_or_default()),
            datetime: data
                .time
                .and_then(|t| chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())),
            last_trade_timestamp: data.update_time,
            last_update_timestamp: data.update_time,
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
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, assets: &[BinanceCoinMAsset]) -> Balances {
        let mut result = Balances::new();

        for asset in assets {
            let total: Option<Decimal> = asset.wallet_balance.parse().ok();
            let used: Option<Decimal> = asset.initial_margin.parse().ok();
            let free = match (total, used) {
                (Some(t), Some(u)) => Some(t - u),
                _ => None,
            };

            let balance = Balance {
                free,
                used,
                total,
                debt: None,
            };
            result.add(&asset.asset, balance);
        }

        result
    }

    /// 포지션 응답 파싱
    fn parse_position(&self, data: &BinanceCoinMPosition) -> Position {
        let side = {
            let amt: Decimal = data.position_amt.parse().unwrap_or_default();
            if amt < Decimal::ZERO {
                Some(PositionSide::Short)
            } else if amt > Decimal::ZERO {
                Some(PositionSide::Long)
            } else {
                None
            }
        };

        let contracts: Decimal = data
            .position_amt
            .parse::<Decimal>()
            .unwrap_or_default()
            .abs();

        let margin_mode = if data.isolated {
            Some(MarginMode::Isolated)
        } else {
            Some(MarginMode::Cross)
        };

        let markets_by_id = self.markets_by_id.read().unwrap();
        let symbol = markets_by_id
            .get(&data.symbol)
            .cloned()
            .unwrap_or_else(|| data.symbol.clone());

        Position {
            symbol,
            id: None,
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp: Some(data.update_time),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.update_time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            contracts: Some(contracts),
            contract_size: Some(Decimal::new(100, 0)), // COIN-M uses 100 USD contract size
            side,
            notional: data.notional_value.parse().ok(),
            leverage: data.leverage.parse().ok(),
            unrealized_pnl: data.unrealized_profit.parse().ok(),
            realized_pnl: None,
            collateral: None,
            entry_price: data.entry_price.parse().ok(),
            mark_price: data.mark_price.parse().ok(),
            liquidation_price: data.liquidation_price.parse().ok(),
            margin_mode,
            hedged: Some(data.position_side != "BOTH"),
            maintenance_margin: data.maint_margin.parse().ok(),
            maintenance_margin_percentage: None,
            initial_margin: data.initial_margin.parse().ok(),
            initial_margin_percentage: None,
            margin_ratio: None,
            last_update_timestamp: Some(data.update_time),
            last_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            percentage: None,
        }
    }
}

#[async_trait]
impl Exchange for BinanceCoinM {
    fn id(&self) -> ExchangeId {
        ExchangeId::BinanceCoinM
    }

    fn name(&self) -> &str {
        "Binance COIN-M"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["JP", "MT"]
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
        let response: BinanceCoinMExchangeInfo =
            self.public_get("/dapi/v1/exchangeInfo", None).await?;

        let mut markets = Vec::new();

        for symbol_info in response.symbols {
            if symbol_info.status != "TRADING" {
                continue;
            }

            let base = symbol_info.base_asset.clone();
            let quote = symbol_info.quote_asset.clone();
            let settle = symbol_info.margin_asset.clone();

            // Determine market type
            let (is_swap, is_future, symbol) = if symbol_info.contract_type == "PERPETUAL" {
                (true, false, format!("{base}/{quote}:{settle}"))
            } else {
                // Quarterly futures
                (
                    false,
                    true,
                    format!(
                        "{base}/{quote}:{settle}-{}",
                        symbol_info.symbol.split('_').next_back().unwrap_or("")
                    ),
                )
            };

            let market = Market {
                id: symbol_info.symbol.clone(),
                lowercase_id: Some(symbol_info.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: symbol_info.base_asset.clone(),
                quote_id: symbol_info.quote_asset.clone(),
                settle: Some(settle.clone()),
                settle_id: Some(symbol_info.margin_asset.clone()),
                active: true,
                market_type: if is_swap {
                    MarketType::Swap
                } else {
                    MarketType::Future
                },
                spot: false,
                margin: false,
                swap: is_swap,
                future: is_future,
                option: false,
                index: false,
                contract: true,
                linear: Some(false), // COIN-M is inverse
                inverse: Some(true),
                sub_type: Some("inverse".into()),
                taker: Some(Decimal::new(5, 4)),           // 0.05%
                maker: Some(Decimal::new(1, 4)),           // 0.01%
                contract_size: Some(Decimal::new(100, 0)), // 100 USD per contract
                expiry: symbol_info.delivery_date,
                expiry_datetime: symbol_info.delivery_date.and_then(|d| {
                    chrono::DateTime::from_timestamp_millis(d).map(|dt| dt.to_rfc3339())
                }),
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: symbol_info.quantity_precision,
                    price: symbol_info.price_precision,
                    cost: None,
                    base: symbol_info.base_asset_precision,
                    quote: symbol_info.quote_precision,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: symbol_info.onboard_date,
                info: serde_json::to_value(&symbol_info).unwrap_or_default(),
                tier_based: true,
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

        let response: BinanceCoinMTicker = self
            .public_get("/dapi/v1/ticker/24hr", Some(params))
            .await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<BinanceCoinMTicker> =
            self.public_get("/dapi/v1/ticker/24hr", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let mut tickers = HashMap::new();

        for data in response {
            if let Some(symbol) = markets_by_id.get(&data.symbol) {
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
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BinanceCoinMOrderBook =
            self.public_get("/dapi/v1/depth", Some(params)).await?;

        let bids: Vec<OrderBookEntry> = response
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: b[0].parse().unwrap_or_default(),
                amount: b[1].parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: a[0].parse().unwrap_or_default(),
                amount: a[1].parse().unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: response.time,
            datetime: response
                .time
                .and_then(|t| chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())),
            nonce: Some(response.last_update_id),
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
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<BinanceCoinMTrade> =
            self.public_get("/dapi/v1/trades", Some(params)).await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| Trade {
                id: t.id.to_string(),
                order: None,
                timestamp: Some(t.time),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(t.time)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol.to_string(),
                trade_type: None,
                side: if t.is_buyer_maker {
                    Some("sell".into())
                } else {
                    Some("buy".into())
                },
                taker_or_maker: if t.is_buyer_maker {
                    Some(TakerOrMaker::Maker)
                } else {
                    Some(TakerOrMaker::Taker)
                },
                price: t.price.parse().unwrap_or_default(),
                amount: t.qty.parse().unwrap_or_default(),
                cost: t.base_qty.as_ref().and_then(|q| q.parse().ok()),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(t).unwrap_or_default(),
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
        params.insert("interval".into(), interval.clone());
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1500).to_string());
        }

        let response: Vec<Vec<serde_json::Value>> =
            self.public_get("/dapi/v1/klines", Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                if c.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].as_i64()?,
                    open: c[1].as_str()?.parse().ok()?,
                    high: c[2].as_str()?.parse().ok()?,
                    low: c[3].as_str()?.parse().ok()?,
                    close: c[4].as_str()?.parse().ok()?,
                    volume: c[5].as_str()?.parse().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: BinanceCoinMAccount = self
            .private_request("GET", "/dapi/v1/account", params)
            .await?;

        Ok(self.parse_balance(&response.assets))
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
        params.insert("symbol".into(), market_id);
        params.insert(
            "side".into(),
            match side {
                OrderSide::Buy => "BUY",
                OrderSide::Sell => "SELL",
            }
            .into(),
        );
        params.insert(
            "type".into(),
            match order_type {
                OrderType::Limit => "LIMIT",
                OrderType::Market => "MARKET",
                OrderType::StopLimit => "STOP",
                OrderType::StopMarket => "STOP_MARKET",
                OrderType::TakeProfit => "TAKE_PROFIT",
                OrderType::TakeProfitMarket => "TAKE_PROFIT_MARKET",
                _ => {
                    return Err(CcxtError::NotSupported {
                        feature: format!("Order type: {order_type:?}"),
                    })
                },
            }
            .into(),
        );
        params.insert("quantity".into(), amount.to_string());

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
            params.insert("timeInForce".into(), "GTC".into());
        }

        let response: BinanceCoinMOrder = self
            .private_request("POST", "/dapi/v1/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: BinanceCoinMOrder = self
            .private_request("DELETE", "/dapi/v1/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: BinanceCoinMOrder = self
            .private_request("GET", "/dapi/v1/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }

        let response: Vec<BinanceCoinMOrder> = self
            .private_request("GET", "/dapi/v1/openOrders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = response
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.symbol)
                    .cloned()
                    .unwrap_or_else(|| o.symbol.clone());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Binance COIN-M cancelAllOrders requires a symbol".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: Vec<BinanceCoinMOrder> = self
            .private_request("DELETE", "/dapi/v1/allOpenOrders", params)
            .await?;

        let orders: Vec<Order> = response
            .iter()
            .map(|o| self.parse_order(o, symbol))
            .collect();

        Ok(orders)
    }

    async fn fetch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>> {
        let params = HashMap::new();

        let response: Vec<BinanceCoinMPosition> = self
            .private_request("GET", "/dapi/v1/positionRisk", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let positions: Vec<Position> = response
            .iter()
            .filter(|p| {
                let amt: Decimal = p.position_amt.parse().unwrap_or_default();
                amt != Decimal::ZERO
            })
            .filter(|p| {
                if let Some(filter) = symbols {
                    let sym = markets_by_id.get(&p.symbol).map(|s| s.as_str());
                    sym.map(|s| filter.contains(&s)).unwrap_or(false)
                } else {
                    true
                }
            })
            .map(|p| self.parse_position(p))
            .collect();

        Ok(positions)
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> CcxtResult<FundingRate> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: BinanceCoinMFundingRate = self
            .public_get("/dapi/v1/premiumIndex", Some(params))
            .await?;

        Ok(FundingRate {
            symbol: symbol.to_string(),
            timestamp: Some(response.time),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(response.time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            funding_rate: response.last_funding_rate.parse().ok(),
            funding_timestamp: response.next_funding_time,
            funding_datetime: response
                .next_funding_time
                .and_then(|t| chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())),
            mark_price: response.mark_price.parse().ok(),
            index_price: response.index_price.parse().ok(),
            interest_rate: response.interest_rate.as_ref().and_then(|r| r.parse().ok()),
            estimated_settle_price: None,
            next_funding_timestamp: response.next_funding_time,
            next_funding_datetime: response
                .next_funding_time
                .and_then(|t| chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())),
            next_funding_rate: None,
            previous_funding_timestamp: None,
            previous_funding_datetime: None,
            previous_funding_rate: None,
            interval: Some("8h".to_string()),
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_open_interest(&self, symbol: &str) -> CcxtResult<OpenInterest> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: BinanceCoinMOpenInterest = self
            .public_get("/dapi/v1/openInterest", Some(params))
            .await?;

        Ok(OpenInterest {
            symbol: symbol.to_string(),
            timestamp: Some(response.time),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(response.time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            open_interest_amount: response.open_interest.parse().ok(),
            open_interest_value: None,
            base_volume: None,
            quote_volume: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn set_leverage(
        &self,
        leverage: Decimal,
        symbol: &str,
    ) -> CcxtResult<crate::types::Leverage> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("leverage".into(), leverage.to_string());

        let response: BinanceCoinMLeverageResponse = self
            .private_request("POST", "/dapi/v1/leverage", params)
            .await?;

        Ok(crate::types::Leverage {
            symbol: symbol.to_string(),
            margin_mode: MarginMode::Cross,
            long_leverage: Decimal::from(response.leverage),
            short_leverage: Decimal::from(response.leverage),
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn set_margin_mode(
        &self,
        margin_mode: MarginMode,
        symbol: &str,
    ) -> CcxtResult<crate::types::MarginModeInfo> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert(
            "marginType".into(),
            match margin_mode {
                MarginMode::Isolated => "ISOLATED",
                MarginMode::Cross => "CROSSED",
                MarginMode::Unknown => {
                    return Err(CcxtError::BadRequest {
                        message: "Unknown margin mode".into(),
                    })
                },
            }
            .into(),
        );

        let _: serde_json::Value = self
            .private_request("POST", "/dapi/v1/marginType", params)
            .await?;

        Ok(crate::types::MarginModeInfo::new(symbol, margin_mode))
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
        let mut url = format!("{}{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();

        if api == "private" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();

            let timestamp = Utc::now().timestamp_millis().to_string();
            let mut query_params = params.clone();
            query_params.insert("timestamp".into(), timestamp);

            let query: String = query_params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(query.as_bytes());
            let signature = hex::encode(mac.finalize().into_bytes());

            url = format!("{url}?{query}&signature={signature}");
            headers.insert("X-MBX-APIKEY".into(), api_key.to_string());
        } else if !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}?{query}");
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        }
    }
}

// === Binance COIN-M API Response Types ===

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceCoinMExchangeInfo {
    symbols: Vec<BinanceCoinMSymbol>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceCoinMSymbol {
    symbol: String,
    #[serde(default)]
    pair: Option<String>,
    contract_type: String,
    #[serde(default)]
    delivery_date: Option<i64>,
    #[serde(default)]
    onboard_date: Option<i64>,
    status: String,
    base_asset: String,
    quote_asset: String,
    margin_asset: String,
    #[serde(default)]
    price_precision: Option<i32>,
    #[serde(default)]
    quantity_precision: Option<i32>,
    #[serde(default)]
    base_asset_precision: Option<i32>,
    #[serde(default)]
    quote_precision: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceCoinMTicker {
    symbol: String,
    #[serde(default)]
    price_change: Option<Decimal>,
    #[serde(default)]
    price_change_percent: Option<Decimal>,
    #[serde(default)]
    weighted_avg_price: Option<Decimal>,
    #[serde(default)]
    prev_close_price: Option<Decimal>,
    #[serde(default)]
    last_price: Option<Decimal>,
    #[serde(default)]
    bid_price: Option<Decimal>,
    #[serde(default)]
    bid_qty: Option<Decimal>,
    #[serde(default)]
    ask_price: Option<Decimal>,
    #[serde(default)]
    ask_qty: Option<Decimal>,
    #[serde(default)]
    open_price: Option<Decimal>,
    #[serde(default)]
    high_price: Option<Decimal>,
    #[serde(default)]
    low_price: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    quote_volume: Option<Decimal>,
    #[serde(default)]
    open_time: Option<i64>,
    #[serde(default)]
    close_time: Option<i64>,
    #[serde(default)]
    time: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceCoinMOrderBook {
    last_update_id: i64,
    #[serde(default)]
    time: Option<i64>,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceCoinMTrade {
    id: i64,
    price: String,
    qty: String,
    #[serde(default)]
    base_qty: Option<String>,
    time: i64,
    is_buyer_maker: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceCoinMOrder {
    symbol: String,
    order_id: i64,
    #[serde(default)]
    client_order_id: Option<String>,
    price: Option<String>,
    orig_qty: String,
    executed_qty: String,
    #[serde(default)]
    cum_base: Option<String>,
    status: String,
    #[serde(rename = "type")]
    order_type: String,
    side: String,
    #[serde(default)]
    time_in_force: Option<String>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    avg_price: Option<String>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    update_time: Option<i64>,
    #[serde(default)]
    reduce_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceCoinMAccount {
    assets: Vec<BinanceCoinMAsset>,
    #[serde(default)]
    positions: Vec<BinanceCoinMPosition>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceCoinMAsset {
    asset: String,
    wallet_balance: String,
    #[serde(default)]
    unrealized_profit: String,
    #[serde(default)]
    margin_balance: String,
    #[serde(default)]
    maint_margin: String,
    initial_margin: String,
    #[serde(default)]
    position_initial_margin: String,
    #[serde(default)]
    open_order_initial_margin: String,
    #[serde(default)]
    cross_wallet_balance: String,
    #[serde(default)]
    cross_un_pnl: String,
    #[serde(default)]
    available_balance: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceCoinMPosition {
    symbol: String,
    position_amt: String,
    entry_price: String,
    #[serde(default)]
    mark_price: String,
    #[serde(default)]
    unrealized_profit: String,
    #[serde(default)]
    liquidation_price: String,
    leverage: String,
    #[serde(default)]
    max_qty: String,
    #[serde(default)]
    margin_type: String,
    isolated: bool,
    #[serde(default)]
    isolated_wallet: String,
    position_side: String,
    #[serde(default)]
    notional_value: String,
    #[serde(default)]
    initial_margin: String,
    #[serde(default)]
    maint_margin: String,
    update_time: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceCoinMFundingRate {
    symbol: String,
    #[serde(default)]
    pair: Option<String>,
    mark_price: String,
    index_price: String,
    #[serde(default)]
    estimated_settle_price: Option<String>,
    last_funding_rate: String,
    #[serde(default)]
    interest_rate: Option<String>,
    #[serde(default)]
    next_funding_time: Option<i64>,
    time: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceCoinMOpenInterest {
    symbol: String,
    #[serde(default)]
    pair: Option<String>,
    open_interest: String,
    #[serde(default)]
    contract_type: Option<String>,
    time: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceCoinMLeverageResponse {
    leverage: i32,
    #[serde(default)]
    max_notional_value: Option<String>,
    symbol: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = BinanceCoinM::new(config).unwrap();

        assert_eq!(exchange.to_market_id("BTC/USD:BTC"), "BTCUSD_PERP");
        assert_eq!(exchange.to_market_id("ETH/USD:ETH"), "ETHUSD_PERP");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = BinanceCoinM::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::BinanceCoinM);
        assert_eq!(exchange.name(), "Binance COIN-M");
        assert!(!exchange.has().spot);
        assert!(exchange.has().swap);
        assert!(exchange.has().future);
    }
}
