//! Binance Futures (USDⓈ-M) Exchange Implementation
//!
//! Binance USDⓈ-M Futures API 구현

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
    Balance, Balances, BorrowInterest, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls,
    FundingRate, FundingRateHistory, Leverage, Liquidation, MarginMode, MarginModeInfo, Market,
    MarketLimits, MarketPrecision, MarketType, OpenInterest, Order, OrderBook, OrderBookEntry,
    OrderRequest, OrderSide, OrderStatus, OrderType, Position, PositionSide, SignedRequest,
    TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade, TransferEntry, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// Binance USDⓈ-M Futures 거래소
pub struct BinanceFutures {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl BinanceFutures {
    const BASE_URL: &'static str = "https://fapi.binance.com";
    const RATE_LIMIT_MS: u64 = 50;

    /// 새 BinanceFutures 인스턴스 생성
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
            fetch_mark_ohlcv: true,
            fetch_index_ohlcv: true,
            fetch_long_short_ratio: true,
            fetch_funding_history: true,
            fetch_open_interest: true,
            fetch_liquidations: true,
            set_leverage: true,
            set_margin_mode: true,
            set_position_mode: true,
            fetch_position_mode: true,
            add_margin: true,
            reduce_margin: true,
            fetch_position_history: true,
            fetch_positions_history: true,
            fetch_deposit_address: true,
            fetch_deposit_addresses: true,
            create_deposit_address: true,
            fetch_convert_currencies: true,
            fetch_convert_quote: true,
            create_convert_trade: true,
            fetch_convert_trade: true,
            fetch_convert_trade_history: true,
            ws: true,
            watch_ticker: true,
            watch_tickers: true,
            watch_order_book: true,
            watch_trades: true,
            watch_ohlcv: true,
            watch_balance: true,
            watch_orders: true,
            watch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/29604020-d5483cdc-87ee-11e7-94c7-d1a8d9169293.jpg".into()),
            api: api_urls,
            www: Some("https://www.binance.com".into()),
            doc: vec![
                "https://binance-docs.github.io/apidocs/futures/en/".into(),
            ],
            fees: Some("https://www.binance.com/en/fee/futureFee".into()),
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

    /// Spot API 호출 (api.binance.com)
    async fn spot_private_request<T: serde::de::DeserializeOwned>(
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

        let base_url = "https://api.binance.com";
        let url = format!("{base_url}{path}?{signed_query}");

        // Use reqwest directly for Spot API calls
        let client = reqwest::Client::new();
        let response = match method {
            "GET" => {
                client
                    .get(&url)
                    .headers(Self::to_reqwest_headers(&headers))
                    .send()
                    .await
            },
            "POST" => {
                client
                    .post(&url)
                    .headers(Self::to_reqwest_headers(&headers))
                    .send()
                    .await
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("HTTP method: {method}"),
                })
            },
        };

        let response = response.map_err(|e| CcxtError::NetworkError {
            url: url.clone(),
            message: e.to_string(),
        })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(CcxtError::ExchangeError {
                message: format!("HTTP {status}: {text}"),
            });
        }

        response.json().await.map_err(|e| CcxtError::ParseError {
            data_type: std::any::type_name::<T>().to_string(),
            message: e.to_string(),
        })
    }

    fn to_reqwest_headers(headers: &HashMap<String, String>) -> reqwest::header::HeaderMap {
        let mut header_map = reqwest::header::HeaderMap::new();
        for (key, value) in headers {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                reqwest::header::HeaderValue::from_str(value),
            ) {
                header_map.insert(name, val);
            }
        }
        header_map
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USDT:USDT → BTCUSDT)
    fn to_market_id(&self, symbol: &str) -> String {
        let base_symbol = symbol.split(':').next().unwrap_or(symbol);
        base_symbol.replace("/", "")
    }

    /// 마켓 ID → 심볼 변환 (BTCUSDT → BTC/USDT:USDT)
    fn to_symbol(&self, _market_id: &str, base: &str, quote: &str) -> String {
        format!("{base}/{quote}:{quote}")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &BinanceFuturesTicker, symbol: &str) -> Ticker {
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
    fn parse_order(&self, data: &BinanceFuturesOrder, symbol: &str) -> Order {
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
                "GTX" => Some(TimeInForce::PO), // GTX is similar to Post-Only
                _ => None,
            });

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.orig_qty.parse().unwrap_or_default();
        let filled: Decimal = data.executed_qty.parse().unwrap_or_default();
        let remaining = Some(amount - filled);
        let cost = data.cum_quote.as_ref().and_then(|c| c.parse().ok());
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
    fn parse_balance(&self, assets: &[BinanceFuturesAsset]) -> Balances {
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
    fn parse_position(&self, data: &BinanceFuturesPosition) -> Position {
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
            .unwrap_or_else(|| self.to_symbol(&data.symbol, &data.symbol, "USDT"));

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
            contract_size: Some(Decimal::ONE),
            side,
            notional: data.notional.parse().ok(),
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
impl Exchange for BinanceFutures {
    fn id(&self) -> ExchangeId {
        ExchangeId::BinanceFutures
    }

    fn name(&self) -> &str {
        "Binance USDⓈ-M Futures"
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
        let response: BinanceFuturesExchangeInfo =
            self.public_get("/fapi/v1/exchangeInfo", None).await?;

        let mut markets = Vec::new();

        for symbol_info in response.symbols {
            if symbol_info.status != "TRADING" {
                continue;
            }

            if symbol_info.contract_type != "PERPETUAL" {
                continue;
            }

            let base = symbol_info.base_asset.clone();
            let quote = symbol_info.quote_asset.clone();
            let settle = symbol_info.margin_asset.clone();
            let symbol = format!("{base}/{quote}:{settle}");

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
                sub_type: Some("linear".into()),
                taker: Some(Decimal::new(4, 4)),
                maker: Some(Decimal::new(2, 4)),
                contract_size: Some(Decimal::ONE),
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
            underlying: None,
            underlying_id: None,
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

        let response: BinanceFuturesTicker = self
            .public_get("/fapi/v1/ticker/24hr", Some(params))
            .await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<BinanceFuturesTicker> =
            self.public_get("/fapi/v1/ticker/24hr", None).await?;

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

        let response: BinanceFuturesOrderBook =
            self.public_get("/fapi/v1/depth", Some(params)).await?;

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
            timestamp: Some(response.timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(response.timestamp)
                .map(|dt| dt.to_rfc3339()),
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

        let response: Vec<BinanceFuturesTrade> =
            self.public_get("/fapi/v1/trades", Some(params)).await?;

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
                cost: Some(
                    t.price.parse::<Decimal>().unwrap_or_default()
                        * t.qty.parse::<Decimal>().unwrap_or_default(),
                ),
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
            self.public_get("/fapi/v1/klines", Some(params)).await?;

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

        let response: BinanceFuturesAccountInfo = self
            .private_request("GET", "/fapi/v2/account", params)
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
                OrderType::TrailingStopMarket => "TRAILING_STOP_MARKET",
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

        let response: BinanceFuturesOrder = self
            .private_request("POST", "/fapi/v1/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: BinanceFuturesOrder = self
            .private_request("DELETE", "/fapi/v1/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn edit_order(
        &self,
        id: &str,
        symbol: &str,
        _order_type: Option<OrderType>,
        side: Option<OrderSide>,
        amount: Option<Decimal>,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        // Binance Futures requires side for edit
        if let Some(s) = side {
            params.insert(
                "side".into(),
                match s {
                    OrderSide::Buy => "BUY",
                    OrderSide::Sell => "SELL",
                }
                .into(),
            );
        }

        if let Some(amt) = amount {
            params.insert("quantity".into(), amt.to_string());
        }

        if let Some(p) = price {
            params.insert("price".into(), p.to_string());
        }

        let response: BinanceFuturesOrder = self
            .private_request("PUT", "/fapi/v1/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: BinanceFuturesOrder = self
            .private_request("GET", "/fapi/v1/order", params)
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

        let response: Vec<BinanceFuturesOrder> = self
            .private_request("GET", "/fapi/v1/openOrders", params)
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
            message: "Binance Futures cancelAllOrders requires a symbol".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let _response: serde_json::Value = self
            .private_request("DELETE", "/fapi/v1/allOpenOrders", params)
            .await?;

        Ok(Vec::new())
    }

    async fn create_orders(&self, orders: Vec<OrderRequest>) -> CcxtResult<Vec<Order>> {
        // Binance Futures supports batch orders via /fapi/v1/batchOrders
        let mut batch_orders = Vec::new();

        for order in &orders {
            let market_id = self.to_market_id(&order.symbol);
            let mut params = serde_json::Map::new();
            params.insert("symbol".into(), market_id.into());
            params.insert(
                "side".into(),
                match order.side {
                    OrderSide::Buy => "BUY",
                    OrderSide::Sell => "SELL",
                }
                .into(),
            );
            params.insert(
                "type".into(),
                match order.order_type {
                    OrderType::Limit => "LIMIT",
                    OrderType::Market => "MARKET",
                    OrderType::StopLimit => "STOP",
                    OrderType::StopMarket => "STOP_MARKET",
                    _ => "LIMIT",
                }
                .into(),
            );
            params.insert("quantity".into(), order.amount.to_string().into());

            if let Some(price) = order.price {
                params.insert("price".into(), price.to_string().into());
                params.insert("timeInForce".into(), "GTC".into());
            }

            if let Some(stop_price) = order.stop_price {
                params.insert("stopPrice".into(), stop_price.to_string().into());
            }

            batch_orders.push(serde_json::Value::Object(params));
        }

        let batch_json =
            serde_json::to_string(&batch_orders).map_err(|e| CcxtError::BadRequest {
                message: format!("Failed to serialize batch orders: {e}"),
            })?;

        let mut request_params = HashMap::new();
        request_params.insert("batchOrders".into(), batch_json);

        let response: Vec<BinanceFuturesOrder> = self
            .private_request("POST", "/fapi/v1/batchOrders", request_params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let result: Vec<Order> = response
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.symbol)
                    .cloned()
                    .unwrap_or_else(|| o.symbol.clone());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(result)
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Binance Futures fetchOrders requires a symbol".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        if let Some(ts) = since {
            params.insert("startTime".into(), ts.to_string());
        }
        if let Some(lim) = limit {
            params.insert("limit".into(), lim.to_string());
        }

        let response: Vec<BinanceFuturesOrder> = self
            .private_request("GET", "/fapi/v1/allOrders", params)
            .await?;

        let orders: Vec<Order> = response
            .iter()
            .map(|o| self.parse_order(o, symbol))
            .collect();

        Ok(orders)
    }

    async fn fetch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>> {
        let params = HashMap::new();

        let response: Vec<BinanceFuturesPosition> = self
            .private_request("GET", "/fapi/v2/positionRisk", params)
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
                    if let Some(sym) = markets_by_id.get(&p.symbol) {
                        filter.contains(&sym.as_str())
                    } else {
                        false
                    }
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

        let response: BinanceFundingRateResponse = self
            .public_get("/fapi/v1/premiumIndex", Some(params))
            .await?;

        let timestamp = response.time;
        let funding_rate: Option<Decimal> = response.last_funding_rate.parse().ok();
        let mark_price: Option<Decimal> = response.mark_price.parse().ok();
        let index_price: Option<Decimal> = response.index_price.parse().ok();
        let next_funding_time = response.next_funding_time;

        Ok(FundingRate {
            symbol: symbol.to_string(),
            info: serde_json::to_value(&response).unwrap_or_default(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            funding_rate,
            mark_price,
            index_price,
            interest_rate: None,
            estimated_settle_price: None,
            funding_timestamp: Some(next_funding_time),
            funding_datetime: Some(
                chrono::DateTime::from_timestamp_millis(next_funding_time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            next_funding_timestamp: Some(next_funding_time),
            next_funding_datetime: Some(
                chrono::DateTime::from_timestamp_millis(next_funding_time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            next_funding_rate: None,
            previous_funding_timestamp: None,
            previous_funding_datetime: None,
            previous_funding_rate: None,
            interval: Some("8h".to_string()),
        })
    }

    async fn fetch_funding_rate_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<FundingRateHistory>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<BinanceFundingRateHistoryItem> = self
            .public_get("/fapi/v1/fundingRate", Some(params))
            .await?;

        let rates: Vec<FundingRateHistory> = response
            .iter()
            .filter_map(|r| {
                let funding_rate: Decimal = r.funding_rate.parse().ok()?;
                Some(FundingRateHistory {
                    info: serde_json::to_value(r).unwrap_or_default(),
                    symbol: symbol.to_string(),
                    funding_rate,
                    timestamp: Some(r.funding_time),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(r.funding_time)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                })
            })
            .collect();

        Ok(rates)
    }

    async fn fetch_mark_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: BinanceFundingRateResponse = self
            .public_get("/fapi/v1/premiumIndex", Some(params))
            .await?;

        let timestamp = response.time;
        let mark_price: Decimal = response.mark_price.parse().unwrap_or_default();
        let index_price: Decimal = response.index_price.parse().unwrap_or_default();

        Ok(Ticker {
            symbol: symbol.to_string(),
            info: serde_json::to_value(&response).unwrap_or_default(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last: Some(mark_price),
            close: Some(mark_price),
            mark_price: Some(mark_price),
            index_price: Some(index_price),
            ..Default::default()
        })
    }

    async fn fetch_mark_prices(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<BinanceFundingRateResponse> =
            self.public_get("/fapi/v1/premiumIndex", None).await?;

        let mut result = HashMap::new();
        for item in response {
            // Convert market_id to symbol
            if let Some(symbol) = self.symbol(&item.symbol) {
                // If symbols filter provided, check if this symbol should be included
                if let Some(filter) = symbols {
                    if !filter.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let timestamp = item.time;
                let mark_price: Decimal = item.mark_price.parse().unwrap_or_default();
                let index_price: Decimal = item.index_price.parse().unwrap_or_default();

                result.insert(
                    symbol.clone(),
                    Ticker {
                        symbol,
                        info: serde_json::to_value(&item).unwrap_or_default(),
                        timestamp: Some(timestamp),
                        datetime: Some(
                            chrono::DateTime::from_timestamp_millis(timestamp)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_default(),
                        ),
                        last: Some(mark_price),
                        close: Some(mark_price),
                        mark_price: Some(mark_price),
                        index_price: Some(index_price),
                        ..Default::default()
                    },
                );
            }
        }

        Ok(result)
    }

    async fn fetch_index_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        // Index price is returned from the same endpoint as mark price
        self.fetch_mark_price(symbol).await
    }

    async fn fetch_mark_ohlcv(
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
            .cloned()
            .unwrap_or("1h".into());

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("interval".into(), interval);

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1500).to_string());
        }

        let response: Vec<Vec<serde_json::Value>> = self
            .public_get("/fapi/v1/markPriceKlines", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|candle| {
                let timestamp = candle.first()?.as_i64()?;
                let open: Decimal = candle.get(1)?.as_str()?.parse().ok()?;
                let high: Decimal = candle.get(2)?.as_str()?.parse().ok()?;
                let low: Decimal = candle.get(3)?.as_str()?.parse().ok()?;
                let close: Decimal = candle.get(4)?.as_str()?.parse().ok()?;
                // Mark price klines don't have volume
                Some(OHLCV {
                    timestamp,
                    open,
                    high,
                    low,
                    close,
                    volume: Decimal::ZERO,
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
        let market_id = self.to_market_id(symbol);
        let interval = self
            .timeframes
            .get(&timeframe)
            .cloned()
            .unwrap_or("1h".into());

        let mut params = HashMap::new();
        // Use pair instead of symbol for index price (e.g., BTCUSDT -> BTCUSDT)
        params.insert("pair".into(), market_id);
        params.insert("interval".into(), interval);

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1500).to_string());
        }

        let response: Vec<Vec<serde_json::Value>> = self
            .public_get("/fapi/v1/indexPriceKlines", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|candle| {
                let timestamp = candle.first()?.as_i64()?;
                let open: Decimal = candle.get(1)?.as_str()?.parse().ok()?;
                let high: Decimal = candle.get(2)?.as_str()?.parse().ok()?;
                let low: Decimal = candle.get(3)?.as_str()?.parse().ok()?;
                let close: Decimal = candle.get(4)?.as_str()?.parse().ok()?;
                Some(OHLCV {
                    timestamp,
                    open,
                    high,
                    low,
                    close,
                    volume: Decimal::ZERO,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_long_short_ratio(
        &self,
        symbol: &str,
        timeframe: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::LongShortRatio>> {
        let market_id = self.to_market_id(symbol);
        let period = timeframe.unwrap_or("5m");

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("period".into(), period.to_string());

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(500).to_string());
        }

        let response: Vec<BinanceLongShortRatio> = self
            .public_get("/futures/data/globalLongShortAccountRatio", Some(params))
            .await?;

        let ratios: Vec<crate::types::LongShortRatio> = response
            .iter()
            .filter_map(|r| {
                let ratio: Decimal = r.long_short_ratio.parse().ok()?;
                Some(crate::types::LongShortRatio {
                    info: serde_json::to_value(r).unwrap_or_default(),
                    symbol: symbol.to_string(),
                    timestamp: Some(r.timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(r.timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    timeframe: Some(period.to_string()),
                    long_short_ratio: ratio,
                })
            })
            .collect();

        Ok(ratios)
    }

    async fn fetch_funding_history(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::FundingHistory>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            let market_id = self.to_market_id(s);
            params.insert("symbol".into(), market_id);
        }
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<BinanceFundingHistoryItem> = self
            .private_request("GET", "/fapi/v1/income", params)
            .await?;

        // Filter for funding fee entries only
        let history: Vec<crate::types::FundingHistory> = response
            .iter()
            .filter(|r| r.income_type == "FUNDING_FEE")
            .filter_map(|r| {
                let amount: Decimal = r.income.parse().ok()?;
                let sym = self.symbol(&r.symbol).unwrap_or_else(|| r.symbol.clone());
                Some(crate::types::FundingHistory {
                    info: serde_json::to_value(r).unwrap_or_default(),
                    symbol: sym,
                    code: r.asset.clone(),
                    id: r.tran_id.to_string(),
                    amount,
                    timestamp: Some(r.time),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(r.time)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                })
            })
            .collect();

        Ok(history)
    }

    async fn set_position_mode(
        &self,
        hedged: bool,
        _symbol: Option<&str>,
    ) -> CcxtResult<crate::types::PositionModeInfo> {
        let mut params = HashMap::new();
        params.insert("dualSidePosition".into(), hedged.to_string());

        let _response: serde_json::Value = self
            .private_request("POST", "/fapi/v1/positionSide/dual", params)
            .await?;

        Ok(crate::types::PositionModeInfo::new(if hedged {
            crate::types::PositionMode::Hedged
        } else {
            crate::types::PositionMode::OneWay
        }))
    }

    async fn fetch_position_mode(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<crate::types::PositionModeInfo> {
        let response: BinancePositionModeResponse = self
            .private_request("GET", "/fapi/v1/positionSide/dual", HashMap::new())
            .await?;

        Ok(crate::types::PositionModeInfo::new(
            if response.dual_side_position {
                crate::types::PositionMode::Hedged
            } else {
                crate::types::PositionMode::OneWay
            },
        ))
    }

    async fn add_margin(
        &self,
        symbol: &str,
        amount: Decimal,
    ) -> CcxtResult<crate::types::MarginModification> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("amount".into(), amount.to_string());
        params.insert("type".into(), "1".to_string()); // 1 = add margin

        let response: BinanceMarginModResponse = self
            .private_request("POST", "/fapi/v1/positionMargin", params)
            .await?;

        Ok(crate::types::MarginModification {
            info: serde_json::to_value(&response).unwrap_or_default(),
            symbol: symbol.to_string(),
            modification_type: Some(crate::types::MarginModificationType::Add),
            margin_mode: None,
            amount: Some(amount),
            total: None,
            code: Some(response.code.to_string()),
            status: Some(if response.code == 200 {
                "ok".to_string()
            } else {
                "error".to_string()
            }),
            timestamp: None,
            datetime: None,
        })
    }

    async fn reduce_margin(
        &self,
        symbol: &str,
        amount: Decimal,
    ) -> CcxtResult<crate::types::MarginModification> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("amount".into(), amount.to_string());
        params.insert("type".into(), "2".to_string()); // 2 = reduce margin

        let response: BinanceMarginModResponse = self
            .private_request("POST", "/fapi/v1/positionMargin", params)
            .await?;

        Ok(crate::types::MarginModification {
            info: serde_json::to_value(&response).unwrap_or_default(),
            symbol: symbol.to_string(),
            modification_type: Some(crate::types::MarginModificationType::Reduce),
            margin_mode: None,
            amount: Some(amount),
            total: None,
            code: Some(response.code.to_string()),
            status: Some(if response.code == 200 {
                "ok".to_string()
            } else {
                "error".to_string()
            }),
            timestamp: None,
            datetime: None,
        })
    }

    async fn fetch_position_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Position>> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("incomeType".into(), "REALIZED_PNL".to_string());

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<BinancePositionHistoryItem> = self
            .private_request("GET", "/fapi/v1/income", params)
            .await?;

        let positions: Vec<Position> = response
            .iter()
            .map(|r| {
                let realized_pnl: Decimal = r.income.parse().unwrap_or_default();
                Position {
                    symbol: symbol.to_string(),
                    id: Some(r.tran_id.to_string()),
                    info: serde_json::to_value(r).unwrap_or_default(),
                    timestamp: Some(r.time),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(r.time)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    contracts: None,
                    contract_size: None,
                    side: None,
                    notional: None,
                    leverage: None,
                    unrealized_pnl: None,
                    realized_pnl: Some(realized_pnl),
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
                    last_update_timestamp: None,
                    last_price: None,
                    stop_loss_price: None,
                    take_profit_price: None,
                    percentage: None,
                }
            })
            .collect();

        Ok(positions)
    }

    async fn fetch_positions_history(
        &self,
        symbols: Option<&[&str]>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Position>> {
        let mut params = HashMap::new();
        params.insert("incomeType".into(), "REALIZED_PNL".to_string());

        if let Some(syms) = symbols {
            if syms.len() == 1 {
                let market_id = self.to_market_id(syms[0]);
                params.insert("symbol".into(), market_id);
            }
        }

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<BinancePositionHistoryItem> = self
            .private_request("GET", "/fapi/v1/income", params)
            .await?;

        let positions: Vec<Position> = response
            .iter()
            .filter_map(|r| {
                let sym = self.symbol(&r.symbol).unwrap_or_else(|| {
                    self.to_symbol(&r.symbol, &r.symbol.replace("USDT", ""), "USDT")
                });
                let realized_pnl: Decimal = r.income.parse().ok()?;

                Some(Position {
                    symbol: sym,
                    id: Some(r.tran_id.to_string()),
                    info: serde_json::to_value(r).unwrap_or_default(),
                    timestamp: Some(r.time),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(r.time)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    contracts: None,
                    contract_size: None,
                    side: None,
                    notional: None,
                    leverage: None,
                    unrealized_pnl: None,
                    realized_pnl: Some(realized_pnl),
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
                    last_update_timestamp: None,
                    last_price: None,
                    stop_loss_price: None,
                    take_profit_price: None,
                    percentage: None,
                })
            })
            .collect();

        Ok(positions)
    }

    async fn set_leverage(&self, leverage: Decimal, symbol: &str) -> CcxtResult<Leverage> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("leverage".into(), leverage.to_string());

        let response: BinanceFuturesLeverageResponse = self
            .private_request("POST", "/fapi/v1/leverage", params)
            .await?;

        Ok(Leverage {
            symbol: symbol.to_string(),
            margin_mode: MarginMode::Cross,
            long_leverage: Decimal::from(response.leverage),
            short_leverage: Decimal::from(response.leverage),
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_leverage(&self, symbol: &str) -> CcxtResult<Leverage> {
        // Fetch positions to get current leverage
        let positions = self.fetch_positions(Some(&[symbol])).await?;

        if let Some(position) = positions.first() {
            Ok(Leverage {
                symbol: symbol.to_string(),
                margin_mode: position.margin_mode.clone().unwrap_or(MarginMode::Cross),
                long_leverage: position.leverage.unwrap_or(Decimal::ONE),
                short_leverage: position.leverage.unwrap_or(Decimal::ONE),
                info: position.info.clone(),
            })
        } else {
            Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })
        }
    }

    async fn set_margin_mode(
        &self,
        margin_mode: MarginMode,
        symbol: &str,
    ) -> CcxtResult<MarginModeInfo> {
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
            .private_request("POST", "/fapi/v1/marginType", params)
            .await?;

        Ok(MarginModeInfo::new(symbol, margin_mode))
    }

    async fn fetch_margin_mode(&self, symbol: &str) -> CcxtResult<MarginModeInfo> {
        // Fetch positions to get current margin mode
        let positions = self.fetch_positions(Some(&[symbol])).await?;

        if let Some(position) = positions.first() {
            let margin_mode = position.margin_mode.clone().unwrap_or(MarginMode::Cross);
            Ok(MarginModeInfo::new(symbol, margin_mode))
        } else {
            Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })
        }
    }

    async fn close_position(&self, symbol: &str, side: Option<OrderSide>) -> CcxtResult<Order> {
        // Fetch current position
        let positions = self.fetch_positions(Some(&[symbol])).await?;

        let position = positions
            .into_iter()
            .find(|p| p.contracts.map(|c| c != Decimal::ZERO).unwrap_or(false))
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("No open position found for {symbol}"),
            })?;

        let contracts = position.contracts.unwrap_or(Decimal::ZERO);
        let pos_side = position.side.clone().unwrap_or(PositionSide::Unknown);

        // Determine order side: use provided side, or derive from position
        let order_side = side.unwrap_or_else(|| {
            match pos_side {
                PositionSide::Long => OrderSide::Sell,
                PositionSide::Short => OrderSide::Buy,
                PositionSide::Unknown => {
                    // Determine side based on contract sign
                    if contracts > Decimal::ZERO {
                        OrderSide::Sell
                    } else {
                        OrderSide::Buy
                    }
                },
            }
        });

        let amount = contracts.abs();
        let market_id = self.to_market_id(symbol);

        // Create market order with reduceOnly to close position
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert(
            "side".into(),
            match order_side {
                OrderSide::Buy => "BUY",
                OrderSide::Sell => "SELL",
            }
            .into(),
        );
        params.insert("type".into(), "MARKET".into());
        params.insert("quantity".into(), amount.to_string());
        params.insert("reduceOnly".into(), "true".into());

        let response: BinanceFuturesOrder = self
            .private_request("POST", "/fapi/v1/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<crate::types::DepositAddress> {
        let mut params = HashMap::new();
        params.insert("coin".into(), code.to_uppercase());
        if let Some(net) = network {
            params.insert("network".into(), net.to_string());
        }

        let response: BinanceDepositAddressResponse = self
            .spot_private_request("GET", "/sapi/v1/capital/deposit/address", params)
            .await?;

        Ok(crate::types::DepositAddress {
            info: serde_json::to_value(&response).unwrap_or_default(),
            currency: response.coin,
            network: Some(response.network),
            address: response.address,
            tag: if response.tag.is_empty() {
                None
            } else {
                Some(response.tag)
            },
        })
    }

    async fn fetch_deposit_addresses(
        &self,
        codes: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, crate::types::DepositAddress>> {
        let mut result = HashMap::new();

        // Binance doesn't have a bulk deposit address API, so we need to fetch one by one
        if let Some(codes_list) = codes {
            for code in codes_list {
                if let Ok(addr) = self.fetch_deposit_address(code, None).await {
                    result.insert(code.to_string(), addr);
                }
            }
        }

        Ok(result)
    }

    async fn fetch_deposit_addresses_by_network(
        &self,
        code: &str,
    ) -> CcxtResult<HashMap<String, crate::types::DepositAddress>> {
        // Get all networks for a coin
        let response: Vec<BinanceDepositAddressResponse> = self
            .spot_private_request("GET", "/sapi/v1/capital/deposit/address/list", {
                let mut params = HashMap::new();
                params.insert("coin".into(), code.to_uppercase());
                params
            })
            .await?;

        let mut result = HashMap::new();
        for addr in response {
            result.insert(
                addr.network.clone(),
                crate::types::DepositAddress {
                    info: serde_json::to_value(&addr).unwrap_or_default(),
                    currency: addr.coin,
                    network: Some(addr.network.clone()),
                    address: addr.address,
                    tag: if addr.tag.is_empty() {
                        None
                    } else {
                        Some(addr.tag)
                    },
                },
            );
        }

        Ok(result)
    }

    async fn create_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<crate::types::DepositAddress> {
        // Binance doesn't have a separate endpoint to create deposit addresses
        // It automatically creates one when you first request it
        self.fetch_deposit_address(code, network).await
    }

    async fn fetch_convert_currencies(&self) -> CcxtResult<Vec<crate::types::ConvertCurrencyPair>> {
        let response: BinanceConvertExchangeInfo = self
            .spot_private_request("GET", "/sapi/v1/convert/exchangeInfo", HashMap::new())
            .await?;

        let pairs: Vec<crate::types::ConvertCurrencyPair> = response
            .into_iter()
            .map(|item| crate::types::ConvertCurrencyPair {
                info: serde_json::to_value(&item).unwrap_or_default(),
                from_currency: item.from_asset,
                to_currency: item.to_asset,
                min_amount: item.from_asset_min_amount.parse().ok(),
                max_amount: item.from_asset_max_amount.parse().ok(),
            })
            .collect();

        Ok(pairs)
    }

    async fn fetch_convert_quote(
        &self,
        from_code: &str,
        to_code: &str,
        amount: Decimal,
    ) -> CcxtResult<crate::types::ConvertQuote> {
        let mut params = HashMap::new();
        params.insert("fromAsset".into(), from_code.to_uppercase());
        params.insert("toAsset".into(), to_code.to_uppercase());
        params.insert("fromAmount".into(), amount.to_string());

        let response: BinanceConvertQuoteResponse = self
            .spot_private_request("POST", "/sapi/v1/convert/getQuote", params)
            .await?;

        Ok(crate::types::ConvertQuote {
            info: serde_json::to_value(&response).unwrap_or_default(),
            id: response.quote_id,
            from_currency: from_code.to_uppercase(),
            to_currency: to_code.to_uppercase(),
            amount,
            price: response.ratio.parse().unwrap_or_default(),
            to_amount: response.to_amount.parse().ok(),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            expire_timestamp: None,
        })
    }

    async fn create_convert_trade(&self, quote_id: &str) -> CcxtResult<crate::types::ConvertTrade> {
        let mut params = HashMap::new();
        params.insert("quoteId".into(), quote_id.to_string());

        let response: BinanceConvertAcceptResponse = self
            .spot_private_request("POST", "/sapi/v1/convert/acceptQuote", params)
            .await?;

        Ok(crate::types::ConvertTrade {
            info: serde_json::to_value(&response).unwrap_or_default(),
            id: response.order_id,
            from_currency: String::new(), // Not returned in response
            to_currency: String::new(),
            from_amount: Decimal::ZERO,
            to_amount: Decimal::ZERO,
            price: Decimal::ZERO,
            timestamp: Some(response.create_time),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(response.create_time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            status: Some(response.order_status),
        })
    }

    async fn fetch_convert_trade(&self, id: &str) -> CcxtResult<crate::types::ConvertTrade> {
        let mut params = HashMap::new();
        params.insert("orderId".into(), id.to_string());

        let response: BinanceConvertOrderStatus = self
            .spot_private_request("GET", "/sapi/v1/convert/orderStatus", params)
            .await?;

        Ok(crate::types::ConvertTrade {
            info: serde_json::to_value(&response).unwrap_or_default(),
            id: response.order_id,
            from_currency: response.from_asset,
            to_currency: response.to_asset,
            from_amount: response.from_amount.parse().unwrap_or_default(),
            to_amount: response.to_amount.parse().unwrap_or_default(),
            price: response.ratio.parse().unwrap_or_default(),
            timestamp: Some(response.create_time),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(response.create_time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            status: Some(response.order_status),
        })
    }

    async fn fetch_convert_trade_history(
        &self,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::ConvertTrade>> {
        let mut params = HashMap::new();

        let start_time = since.unwrap_or_else(|| {
            Utc::now().timestamp_millis() - 30 * 24 * 60 * 60 * 1000 // Default 30 days
        });
        let end_time = Utc::now().timestamp_millis();

        params.insert("startTime".into(), start_time.to_string());
        params.insert("endTime".into(), end_time.to_string());

        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: BinanceConvertTradeFlow = self
            .spot_private_request("GET", "/sapi/v1/convert/tradeFlow", params)
            .await?;

        let trades: Vec<crate::types::ConvertTrade> = response
            .list
            .into_iter()
            .map(|item| crate::types::ConvertTrade {
                info: serde_json::to_value(&item).unwrap_or_default(),
                id: item.quote_id,
                from_currency: item.from_asset,
                to_currency: item.to_asset,
                from_amount: item.from_amount.parse().unwrap_or_default(),
                to_amount: item.to_amount.parse().unwrap_or_default(),
                price: item.ratio.parse().unwrap_or_default(),
                timestamp: Some(item.create_time),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(item.create_time)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                status: Some(item.order_status),
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_open_interest(&self, symbol: &str) -> CcxtResult<OpenInterest> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: BinanceOpenInterest = self
            .public_get("/fapi/v1/openInterest", Some(params))
            .await?;

        let timestamp = response
            .time
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let amount: Decimal = response.open_interest.parse().unwrap_or_default();

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
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_liquidations(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Liquidation>> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<BinanceLiquidation> = self
            .public_get("/fapi/v1/forceOrders", Some(params))
            .await?;

        let liquidations: Vec<Liquidation> = response
            .iter()
            .map(|liq| {
                let price: Decimal = liq.price.parse().unwrap_or_default();
                let qty: Decimal = liq.orig_qty.parse().unwrap_or_default();
                let quote_value = price * qty;
                let side = if liq.side == "SELL" {
                    OrderSide::Sell
                } else {
                    OrderSide::Buy
                };

                Liquidation {
                    info: serde_json::to_value(liq).unwrap_or_default(),
                    symbol: symbol.to_string(),
                    timestamp: Some(liq.time),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(liq.time)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    price,
                    base_value: Some(qty),
                    quote_value: Some(quote_value),
                    contracts: Some(qty),
                    contract_size: None,
                    side: Some(side),
                }
            })
            .collect();

        Ok(liquidations)
    }

    async fn transfer(
        &self,
        code: &str,
        amount: Decimal,
        from_account: &str,
        to_account: &str,
    ) -> CcxtResult<TransferEntry> {
        // Binance uses /sapi/v1/asset/transfer for universal transfer
        let transfer_type = match (
            from_account.to_lowercase().as_str(),
            to_account.to_lowercase().as_str(),
        ) {
            ("spot", "futures") | ("spot", "um_futures") => "MAIN_UMFUTURE",
            ("futures", "spot") | ("um_futures", "spot") => "UMFUTURE_MAIN",
            ("spot", "cm_futures") => "MAIN_CMFUTURE",
            ("cm_futures", "spot") => "CMFUTURE_MAIN",
            ("spot", "margin") => "MAIN_MARGIN",
            ("margin", "spot") => "MARGIN_MAIN",
            ("spot", "funding") => "MAIN_FUNDING",
            ("funding", "spot") => "FUNDING_MAIN",
            _ => {
                return Err(CcxtError::BadRequest {
                    message: format!("Unsupported transfer: {from_account} -> {to_account}"),
                })
            },
        };

        let mut params = HashMap::new();
        params.insert("asset".into(), code.to_uppercase());
        params.insert("amount".into(), amount.to_string());
        params.insert("type".into(), transfer_type.to_string());

        let response: BinanceTransferResponse = self
            .spot_private_request("POST", "/sapi/v1/asset/transfer", params)
            .await?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(TransferEntry::new()
            .with_id(response.tran_id.to_string())
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
        // Binance requires a specific transfer type for the query
        let transfer_types = [
            "MAIN_UMFUTURE",
            "UMFUTURE_MAIN",
            "MAIN_CMFUTURE",
            "CMFUTURE_MAIN",
        ];
        let mut all_transfers = Vec::new();

        for transfer_type in transfer_types {
            let mut params = HashMap::new();
            params.insert("type".into(), transfer_type.to_string());

            if let Some(s) = since {
                params.insert("startTime".into(), s.to_string());
            }
            if let Some(l) = limit {
                params.insert("size".into(), l.min(100).to_string());
            }

            let response: BinanceTransferHistory = self
                .spot_private_request("GET", "/sapi/v1/asset/transfer", params)
                .await?;

            for item in response.rows {
                if let Some(c) = code {
                    if item.asset.to_lowercase() != c.to_lowercase() {
                        continue;
                    }
                }

                let (from, to) = match transfer_type {
                    "MAIN_UMFUTURE" => ("spot", "futures"),
                    "UMFUTURE_MAIN" => ("futures", "spot"),
                    "MAIN_CMFUTURE" => ("spot", "cm_futures"),
                    "CMFUTURE_MAIN" => ("cm_futures", "spot"),
                    _ => ("unknown", "unknown"),
                };

                all_transfers.push(
                    TransferEntry::new()
                        .with_id(item.tran_id.to_string())
                        .with_currency(item.asset)
                        .with_amount(item.amount.parse().unwrap_or_default())
                        .with_from_account(from.to_string())
                        .with_to_account(to.to_string())
                        .with_timestamp(item.timestamp)
                        .with_status(item.status),
                );
            }
        }

        Ok(all_transfers)
    }

    async fn borrow_margin(
        &self,
        code: &str,
        amount: Decimal,
        symbol: Option<&str>,
    ) -> CcxtResult<BorrowInterest> {
        // Binance Futures doesn't have direct margin borrowing
        // For cross margin, use /sapi/v1/margin/loan
        let mut params = HashMap::new();
        params.insert("asset".into(), code.to_uppercase());
        params.insert("amount".into(), amount.to_string());

        if let Some(sym) = symbol {
            // Isolated margin
            params.insert("symbol".into(), self.to_market_id(sym));
            params.insert("isIsolated".into(), "TRUE".into());
        }

        let _response: BinanceMarginLoanResponse = self
            .spot_private_request("POST", "/sapi/v1/margin/loan", params)
            .await?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(BorrowInterest::new()
            .with_currency(code.to_string())
            .with_amount_borrowed(amount)
            .with_timestamp(timestamp))
    }

    async fn repay_margin(
        &self,
        code: &str,
        amount: Decimal,
        symbol: Option<&str>,
    ) -> CcxtResult<BorrowInterest> {
        let mut params = HashMap::new();
        params.insert("asset".into(), code.to_uppercase());
        params.insert("amount".into(), amount.to_string());

        if let Some(sym) = symbol {
            // Isolated margin
            params.insert("symbol".into(), self.to_market_id(sym));
            params.insert("isIsolated".into(), "TRUE".into());
        }

        let _response: BinanceMarginLoanResponse = self
            .spot_private_request("POST", "/sapi/v1/margin/repay", params)
            .await?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(BorrowInterest::new()
            .with_currency(code.to_string())
            .with_amount_borrowed(amount)
            .with_timestamp(timestamp))
    }

    async fn fetch_borrow_interest(
        &self,
        code: Option<&str>,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<BorrowInterest>> {
        let mut params = HashMap::new();

        if let Some(c) = code {
            params.insert("asset".into(), c.to_uppercase());
        }

        if let Some(sym) = symbol {
            params.insert("symbol".into(), self.to_market_id(sym));
            params.insert("isolatedSymbol".into(), self.to_market_id(sym));
        }

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("size".into(), l.min(100).to_string());
        }

        let response: BinanceBorrowInterestHistory = self
            .spot_private_request("GET", "/sapi/v1/margin/interestHistory", params)
            .await?;

        let interests: Vec<BorrowInterest> = response
            .rows
            .iter()
            .map(|item| BorrowInterest {
                info: serde_json::to_value(item).unwrap_or_default(),
                symbol: item.isolated_symbol.clone(),
                currency: Some(item.asset.clone()),
                interest: item.interest.parse().ok(),
                interest_rate: item.interest_rate.parse().ok(),
                amount_borrowed: item.principal.parse().ok(),
                margin_mode: if item.isolated_symbol.is_some() {
                    Some(MarginMode::Isolated)
                } else {
                    Some(MarginMode::Cross)
                },
                timestamp: Some(item.interest_accured_time),
                datetime: chrono::DateTime::from_timestamp_millis(item.interest_accured_time)
                    .map(|dt| dt.to_rfc3339()),
            })
            .collect();

        Ok(interests)
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

// === Binance Futures API Response Types ===

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFuturesExchangeInfo {
    symbols: Vec<BinanceFuturesSymbol>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFuturesSymbol {
    symbol: String,
    status: String,
    base_asset: String,
    quote_asset: String,
    margin_asset: String,
    contract_type: String,
    #[serde(default)]
    price_precision: Option<i32>,
    #[serde(default)]
    quantity_precision: Option<i32>,
    #[serde(default)]
    base_asset_precision: Option<i32>,
    #[serde(default)]
    quote_precision: Option<i32>,
    #[serde(default)]
    onboard_date: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFuturesTicker {
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
    time: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFuturesOrderBook {
    last_update_id: i64,
    #[serde(default, rename = "T")]
    timestamp: i64,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFuturesTrade {
    id: i64,
    price: String,
    qty: String,
    time: i64,
    is_buyer_maker: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFuturesOrder {
    symbol: String,
    order_id: i64,
    #[serde(default)]
    client_order_id: Option<String>,
    price: Option<String>,
    #[serde(default)]
    avg_price: Option<String>,
    orig_qty: String,
    executed_qty: String,
    #[serde(default)]
    cum_quote: Option<String>,
    status: String,
    #[serde(rename = "type")]
    order_type: String,
    side: String,
    #[serde(default)]
    time_in_force: Option<String>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    update_time: Option<i64>,
    #[serde(default)]
    reduce_only: Option<bool>,
    #[serde(default)]
    position_side: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFuturesAccountInfo {
    assets: Vec<BinanceFuturesAsset>,
    #[serde(default)]
    positions: Vec<BinanceFuturesPosition>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFuturesAsset {
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
    available_balance: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFuturesPosition {
    symbol: String,
    position_amt: String,
    entry_price: String,
    mark_price: String,
    unrealized_profit: String,
    liquidation_price: String,
    leverage: String,
    #[serde(default)]
    max_notional_value: String,
    #[serde(default)]
    margin_type: String,
    #[serde(default)]
    isolated: bool,
    #[serde(default)]
    is_auto_add_margin: String,
    position_side: String,
    notional: String,
    #[serde(default)]
    isolated_wallet: String,
    update_time: i64,
    #[serde(default)]
    initial_margin: String,
    #[serde(default)]
    maint_margin: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFundingRateResponse {
    symbol: String,
    mark_price: String,
    index_price: String,
    #[serde(default)]
    estimated_settle_price: String,
    last_funding_rate: String,
    next_funding_time: i64,
    #[serde(default)]
    interest_rate: String,
    time: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFundingRateHistoryItem {
    symbol: String,
    funding_rate: String,
    funding_time: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceLongShortRatio {
    symbol: String,
    long_short_ratio: String,
    #[serde(default)]
    long_account: String,
    #[serde(default)]
    short_account: String,
    timestamp: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinancePositionModeResponse {
    dual_side_position: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceMarginModResponse {
    code: i32,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    amount: Decimal,
    #[serde(rename = "type")]
    #[serde(default)]
    margin_type: i32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFundingHistoryItem {
    symbol: String,
    income_type: String,
    income: String,
    asset: String,
    tran_id: i64,
    time: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinancePositionHistoryItem {
    symbol: String,
    #[serde(default)]
    income_type: String,
    income: String,
    #[serde(default)]
    asset: String,
    tran_id: i64,
    time: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceFuturesLeverageResponse {
    leverage: i32,
    #[serde(default)]
    max_notional_value: Option<String>,
    symbol: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceDepositAddressResponse {
    coin: String,
    address: String,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    network: String,
    #[serde(default)]
    url: String,
}

// Convert API Response Types
type BinanceConvertExchangeInfo = Vec<BinanceConvertPairInfo>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceConvertPairInfo {
    from_asset: String,
    to_asset: String,
    #[serde(default)]
    from_asset_min_amount: String,
    #[serde(default)]
    from_asset_max_amount: String,
    #[serde(default)]
    to_asset_min_amount: String,
    #[serde(default)]
    to_asset_max_amount: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceConvertQuoteResponse {
    quote_id: String,
    ratio: String,
    #[serde(default)]
    inverse_ratio: String,
    #[serde(default)]
    valid_timestamp: i64,
    #[serde(default)]
    to_amount: String,
    #[serde(default)]
    from_amount: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceConvertAcceptResponse {
    order_id: String,
    #[serde(default)]
    create_time: i64,
    order_status: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceConvertOrderStatus {
    order_id: String,
    order_status: String,
    from_asset: String,
    #[serde(default)]
    from_amount: String,
    to_asset: String,
    #[serde(default)]
    to_amount: String,
    #[serde(default)]
    ratio: String,
    #[serde(default)]
    inverse_ratio: String,
    #[serde(default)]
    create_time: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceConvertTradeFlow {
    list: Vec<BinanceConvertTradeItem>,
    #[serde(default)]
    start_time: i64,
    #[serde(default)]
    end_time: i64,
    #[serde(default)]
    limit: i32,
    #[serde(default)]
    more_data: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceConvertTradeItem {
    quote_id: String,
    order_id: String,
    order_status: String,
    from_asset: String,
    #[serde(default)]
    from_amount: String,
    to_asset: String,
    #[serde(default)]
    to_amount: String,
    #[serde(default)]
    ratio: String,
    #[serde(default)]
    inverse_ratio: String,
    #[serde(default)]
    create_time: i64,
}

// Open Interest Response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceOpenInterest {
    symbol: String,
    open_interest: String,
    #[serde(default)]
    time: Option<i64>,
}

// Liquidation Response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceLiquidation {
    symbol: String,
    price: String,
    orig_qty: String,
    #[serde(default)]
    executed_qty: String,
    #[serde(default)]
    average_price: String,
    status: String,
    time_in_force: String,
    #[serde(rename = "type")]
    order_type: String,
    side: String,
    time: i64,
}

// Premium Index Response (for index price)
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinancePremiumIndex {
    symbol: String,
    mark_price: String,
    index_price: String,
    #[serde(default)]
    estimated_settle_price: String,
    #[serde(default)]
    last_funding_rate: String,
    #[serde(default)]
    next_funding_time: i64,
    #[serde(default)]
    interest_rate: String,
    time: i64,
}

// Transfer Response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceTransferResponse {
    tran_id: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceTransferHistory {
    #[serde(default)]
    total: i64,
    #[serde(default)]
    rows: Vec<BinanceTransferItem>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceTransferItem {
    asset: String,
    amount: String,
    #[serde(rename = "type")]
    transfer_type: String,
    status: String,
    tran_id: i64,
    timestamp: i64,
}

// Margin Loan Response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceMarginLoanResponse {
    tran_id: i64,
}

// Borrow Interest History Response
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceBorrowInterestHistory {
    #[serde(default)]
    total: i64,
    #[serde(default)]
    rows: Vec<BinanceBorrowInterestItem>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceBorrowInterestItem {
    asset: String,
    principal: String,
    interest: String,
    interest_rate: String,
    #[serde(default)]
    isolated_symbol: Option<String>,
    interest_accured_time: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let futures = BinanceFutures::new(config).unwrap();

        assert_eq!(futures.to_market_id("BTC/USDT:USDT"), "BTCUSDT");
        assert_eq!(futures.to_market_id("ETH/USDT:USDT"), "ETHUSDT");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let futures = BinanceFutures::new(config).unwrap();

        assert_eq!(futures.id(), ExchangeId::BinanceFutures);
        assert_eq!(futures.name(), "Binance USDⓈ-M Futures");
        assert!(!futures.has().spot);
        assert!(futures.has().swap);
        assert!(futures.has().future);
    }
}
