//! MEXC Exchange Implementation
//!
//! MEXC API 구현

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
    Balance, Balances, DepositAddress, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Fee,
    FundingRate, Leverage, MarginMode, MarginModeInfo, MarginModification, MarginModificationType,
    Market, MarketLimits, MarketPrecision, MarketType, OpenInterest, Order, OrderBook,
    OrderBookEntry, OrderSide, OrderStatus, OrderType, Position, PositionMode, PositionModeInfo,
    SignedRequest, Ticker, Timeframe, Trade, Transaction, TransactionStatus, TransactionType,
    TransferEntry, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// MEXC 거래소
pub struct Mexc {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Mexc {
    const BASE_URL: &'static str = "https://api.mexc.com";
    const RATE_LIMIT_MS: u64 = 50; // 1200 requests per minute

    /// 새 MEXC 클라이언트 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".to_string());
        timeframes.insert(Timeframe::Minute5, "5m".to_string());
        timeframes.insert(Timeframe::Minute15, "15m".to_string());
        timeframes.insert(Timeframe::Minute30, "30m".to_string());
        timeframes.insert(Timeframe::Hour1, "1h".to_string());
        timeframes.insert(Timeframe::Hour4, "4h".to_string());
        timeframes.insert(Timeframe::Day1, "1d".to_string());
        timeframes.insert(Timeframe::Week1, "1W".to_string());
        timeframes.insert(Timeframe::Month1, "1M".to_string());

        let features = ExchangeFeatures {
            spot: true,
            margin: true,
            swap: true,
            future: true,
            option: false,
            fetch_markets: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            cancel_order: true,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/137283979-8b2a818d-8633-461b-bfca-de89e8c446b2.jpg".into()),
            api: api_urls,
            www: Some("https://www.mexc.com".into()),
            doc: vec!["https://mexcdevelop.github.io/apidocs/".into()],
            fees: Some("https://www.mexc.com/fee".into()),
        };

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

    /// 심볼을 MEXC market ID로 변환 (BTC/USDT -> BTCUSDT)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// MEXC market ID를 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_symbol(&self, market_id: &str) -> String {
        if let Some(symbol) = self.markets_by_id.read().unwrap().get(market_id) {
            return symbol.clone();
        }
        market_id.to_string()
    }

    /// Timeframe 문자열 반환
    fn get_timeframe_string(&self, timeframe: Timeframe) -> Option<&String> {
        self.timeframes.get(&timeframe)
    }

    /// Public API 요청
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let mut url = format!("{}{}", Self::BASE_URL, path);
        if let Some(p) = params {
            let query: String = p
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}?{query}");
        }

        self.client.get(&url, None, None).await
    }

    /// Private API 요청
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        mut params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or(CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let api_secret = self
            .config
            .api_secret()
            .ok_or(CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?;

        let timestamp = Utc::now().timestamp_millis();
        params.insert("timestamp".to_string(), timestamp.to_string());
        params.insert("recvWindow".to_string(), "5000".to_string());

        // Create signature
        let query: String = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|e| {
            CcxtError::ExchangeError {
                message: e.to_string(),
            }
        })?;
        mac.update(query.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let url = format!(
            "{}{}?{}&signature={}",
            Self::BASE_URL,
            path,
            query,
            signature
        );

        let mut headers = HashMap::new();
        headers.insert("X-MEXC-APIKEY".to_string(), api_key.to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        if method == "GET" {
            self.client.get(&url, None, Some(headers)).await
        } else if method == "DELETE" {
            self.client.delete(&url, None, Some(headers)).await
        } else {
            self.client.post(&url, None, Some(headers)).await
        }
    }

    fn parse_order(&self, order_data: &MexcOrder, symbol: &str) -> CcxtResult<Order> {
        let timestamp = order_data.time;

        let order_type = match order_data.order_type.as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            "LIMIT_MAKER" => OrderType::Limit,
            _ => OrderType::Limit,
        };

        let side = match order_data.side.as_str() {
            "BUY" => OrderSide::Buy,
            _ => OrderSide::Sell,
        };

        let status = match order_data.status.as_str() {
            "NEW" | "PARTIALLY_FILLED" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" | "PENDING_CANCEL" | "REJECTED" | "EXPIRED" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let price: Decimal = order_data.price.parse().unwrap_or_default();
        let amount: Decimal = order_data.orig_qty.parse().unwrap_or_default();
        let filled: Decimal = order_data.executed_qty.parse().unwrap_or_default();
        let cost: Decimal = order_data.cummulative_quote_qty.parse().unwrap_or_default();

        Ok(Order {
            id: order_data.order_id.to_string(),
            client_order_id: order_data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: order_data.update_time,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price: Some(price),
            average: if filled > Decimal::ZERO {
                Some(cost / filled)
            } else {
                None
            },
            amount,
            filled,
            remaining: Some(amount - filled),
            cost: Some(cost),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(order_data).unwrap_or_default(),
        })
    }
}

#[async_trait]
impl Exchange for Mexc {
    fn id(&self) -> ExchangeId {
        ExchangeId::Mexc
    }

    fn name(&self) -> &str {
        "MEXC"
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

    async fn load_markets(&self, _reload: bool) -> CcxtResult<HashMap<String, Market>> {
        let response: MexcExchangeInfo = self.public_get("/api/v3/exchangeInfo", None).await?;

        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for symbol_data in response.symbols {
            if symbol_data.status != "ENABLED" {
                continue;
            }

            let base = symbol_data.base_asset.clone();
            let quote = symbol_data.quote_asset.clone();
            let symbol = format!("{base}/{quote}");
            let market_id = symbol_data.symbol.clone();

            let market = Market {
                id: market_id.clone(),
                lowercase_id: Some(market_id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: symbol_data.base_asset.clone(),
                quote_id: symbol_data.quote_asset.clone(),
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
                taker: Some(Decimal::new(2, 3)), // 0.2%
                maker: Some(Decimal::new(2, 3)), // 0.2%
                percentage: true,
                tier_based: false,
                precision: MarketPrecision {
                    amount: Some(symbol_data.base_asset_precision),
                    price: Some(symbol_data.quote_precision),
                    cost: None,
                    base: Some(symbol_data.base_asset_precision),
                    quote: Some(symbol_data.quote_precision),
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                    price: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                    cost: crate::types::MinMax {
                        min: symbol_data
                            .quote_amount_precision
                            .as_ref()
                            .and_then(|v| v.parse().ok()),
                        max: None,
                    },
                    leverage: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&symbol_data).unwrap_or_default(),
            };

            markets_by_id.insert(market_id, symbol.clone());
            markets.insert(symbol, market);
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

        let response: MexcTicker = self.public_get("/api/v3/ticker/24hr", Some(params)).await?;

        let timestamp = response
            .close_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: response.high_price.as_ref().and_then(|v| v.parse().ok()),
            low: response.low_price.as_ref().and_then(|v| v.parse().ok()),
            bid: response.bid_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: response.bid_qty.as_ref().and_then(|v| v.parse().ok()),
            ask: response.ask_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: response.ask_qty.as_ref().and_then(|v| v.parse().ok()),
            vwap: response
                .weighted_avg_price
                .as_ref()
                .and_then(|v| v.parse().ok()),
            open: response.open_price.as_ref().and_then(|v| v.parse().ok()),
            close: response.last_price.as_ref().and_then(|v| v.parse().ok()),
            last: response.last_price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: response
                .prev_close_price
                .as_ref()
                .and_then(|v| v.parse().ok()),
            change: response.price_change.as_ref().and_then(|v| v.parse().ok()),
            percentage: response
                .price_change_percent
                .as_ref()
                .and_then(|v| v.parse().ok()),
            average: None,
            base_volume: response.volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: response.quote_volume.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<MexcTicker> = self.public_get("/api/v3/ticker/24hr", None).await?;

        let symbol_set: Option<std::collections::HashSet<_>> = symbols.map(|s| s.iter().collect());

        let mut tickers = HashMap::new();
        for ticker_data in response {
            if let Some(symbol_str) = &ticker_data.symbol {
                let symbol = self.to_symbol(symbol_str);

                if let Some(ref set) = symbol_set {
                    if !set.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let timestamp = ticker_data
                    .close_time
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let ticker = Ticker {
                    symbol: symbol.clone(),
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    high: ticker_data.high_price.as_ref().and_then(|v| v.parse().ok()),
                    low: ticker_data.low_price.as_ref().and_then(|v| v.parse().ok()),
                    bid: ticker_data.bid_price.as_ref().and_then(|v| v.parse().ok()),
                    bid_volume: ticker_data.bid_qty.as_ref().and_then(|v| v.parse().ok()),
                    ask: ticker_data.ask_price.as_ref().and_then(|v| v.parse().ok()),
                    ask_volume: ticker_data.ask_qty.as_ref().and_then(|v| v.parse().ok()),
                    vwap: ticker_data
                        .weighted_avg_price
                        .as_ref()
                        .and_then(|v| v.parse().ok()),
                    open: ticker_data.open_price.as_ref().and_then(|v| v.parse().ok()),
                    close: ticker_data.last_price.as_ref().and_then(|v| v.parse().ok()),
                    last: ticker_data.last_price.as_ref().and_then(|v| v.parse().ok()),
                    previous_close: ticker_data
                        .prev_close_price
                        .as_ref()
                        .and_then(|v| v.parse().ok()),
                    change: ticker_data
                        .price_change
                        .as_ref()
                        .and_then(|v| v.parse().ok()),
                    percentage: ticker_data
                        .price_change_percent
                        .as_ref()
                        .and_then(|v| v.parse().ok()),
                    average: None,
                    base_volume: ticker_data.volume.as_ref().and_then(|v| v.parse().ok()),
                    quote_volume: ticker_data
                        .quote_volume
                        .as_ref()
                        .and_then(|v| v.parse().ok()),
                    index_price: None,
                    mark_price: None,
                    info: serde_json::to_value(&ticker_data).unwrap_or_default(),
                };

                tickers.insert(symbol, ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("limit".to_string(), limit.unwrap_or(100).to_string());

        let response: MexcOrderBook = self.public_get("/api/v3/depth", Some(params)).await?;

        let timestamp = response
            .last_update_id
            .unwrap_or_else(|| Utc::now().timestamp_millis());

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

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: response.last_update_id,
            bids,
            asks,
            checksum: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("limit".to_string(), limit.unwrap_or(500).to_string());

        let response: Vec<MexcTrade> = self.public_get("/api/v3/trades", Some(params)).await?;

        let mut trades = Vec::new();
        for trade_data in response {
            if let Some(since_ts) = since {
                if trade_data.time < since_ts {
                    continue;
                }
            }

            let price: Decimal = trade_data.price.parse().unwrap_or_default();
            let amount: Decimal = trade_data.qty.parse().unwrap_or_default();

            trades.push(Trade {
                id: trade_data.id.to_string(),
                order: None,
                timestamp: Some(trade_data.time),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(trade_data.time)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(if trade_data.is_buyer_maker {
                    "sell".to_string()
                } else {
                    "buy".to_string()
                }),
                taker_or_maker: None,
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(&trade_data).unwrap_or_default(),
            });
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
        let market_id = self.to_market_id(symbol);
        let interval =
            self.get_timeframe_string(timeframe)
                .ok_or_else(|| CcxtError::BadRequest {
                    message: "Invalid timeframe".into(),
                })?;

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("interval".to_string(), interval.clone());
        params.insert("limit".to_string(), limit.unwrap_or(500).to_string());

        if let Some(since_ts) = since {
            params.insert("startTime".to_string(), since_ts.to_string());
        }

        let response: Vec<Vec<serde_json::Value>> =
            self.public_get("/api/v3/klines", Some(params)).await?;

        let mut ohlcv_list = Vec::new();
        for kline in response {
            if kline.len() >= 6 {
                ohlcv_list.push(OHLCV {
                    timestamp: kline[0].as_i64().unwrap_or_default(),
                    open: kline[1]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default(),
                    high: kline[2]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default(),
                    low: kline[3]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default(),
                    close: kline[4]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default(),
                    volume: kline[5]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default(),
                });
            }
        }

        Ok(ohlcv_list)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: MexcAccountInfo = self
            .private_request("GET", "/api/v3/account", HashMap::new())
            .await?;

        let timestamp = response
            .update_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let mut currencies: HashMap<String, Balance> = HashMap::new();

        for balance in response.balances {
            let free: Decimal = balance.free.parse().unwrap_or_default();
            let locked: Decimal = balance.locked.parse().unwrap_or_default();

            if free > Decimal::ZERO || locked > Decimal::ZERO {
                currencies.insert(
                    balance.asset.clone(),
                    Balance {
                        free: Some(free),
                        used: Some(locked),
                        total: Some(free + locked),
                        debt: None,
                    },
                );
            }
        }

        Ok(Balances {
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            currencies,
            info: serde_json::json!({}),
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

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert(
            "side".to_string(),
            match side {
                OrderSide::Buy => "BUY".to_string(),
                OrderSide::Sell => "SELL".to_string(),
            },
        );
        params.insert(
            "type".to_string(),
            match order_type {
                OrderType::Limit => "LIMIT".to_string(),
                OrderType::Market => "MARKET".to_string(),
                _ => "LIMIT".to_string(),
            },
        );
        params.insert("quantity".to_string(), amount.to_string());

        if let Some(p) = price {
            params.insert("price".to_string(), p.to_string());
        }

        let response: MexcOrderResponse = self
            .private_request("POST", "/api/v3/order", params)
            .await?;

        self.fetch_order(&response.order_id.to_string(), symbol)
            .await
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("orderId".to_string(), id.to_string());

        let _response: MexcOrderResponse = self
            .private_request("DELETE", "/api/v3/order", params)
            .await?;

        self.fetch_order(id, symbol).await
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("orderId".to_string(), id.to_string());

        let response: MexcOrder = self.private_request("GET", "/api/v3/order", params).await?;

        self.parse_order(&response, symbol)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(sym) = symbol {
            params.insert("symbol".to_string(), self.to_market_id(sym));
        }

        let response: Vec<MexcOrder> = self
            .private_request("GET", "/api/v3/openOrders", params)
            .await?;

        let mut orders = Vec::new();
        for order_data in response {
            let default_symbol = self.to_symbol(&order_data.symbol);
            let sym = symbol.unwrap_or(&default_symbol);
            orders.push(self.parse_order(&order_data, sym)?);
        }

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol_str = symbol.ok_or_else(|| CcxtError::BadRequest {
            message: "Symbol is required".into(),
        })?;

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), self.to_market_id(symbol_str));
        params.insert("limit".to_string(), limit.unwrap_or(500).to_string());

        let response: Vec<MexcOrder> = self
            .private_request("GET", "/api/v3/allOrders", params)
            .await?;

        let mut orders = Vec::new();
        for order_data in response {
            if order_data.status == "FILLED" || order_data.status == "CANCELED" {
                orders.push(self.parse_order(&order_data, symbol_str)?);
            }
        }

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol_str = symbol.ok_or_else(|| CcxtError::BadRequest {
            message: "Symbol is required".into(),
        })?;

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), self.to_market_id(symbol_str));
        params.insert("limit".to_string(), limit.unwrap_or(500).to_string());

        if let Some(since_ts) = since {
            params.insert("startTime".to_string(), since_ts.to_string());
        }

        let response: Vec<MexcMyTrade> = self
            .private_request("GET", "/api/v3/myTrades", params)
            .await?;

        let mut trades = Vec::new();
        for trade_data in response {
            let price: Decimal = trade_data.price.parse().unwrap_or_default();
            let amount: Decimal = trade_data.qty.parse().unwrap_or_default();
            let commission: Decimal = trade_data.commission.parse().unwrap_or_default();

            trades.push(Trade {
                id: trade_data.id.to_string(),
                order: Some(trade_data.order_id.to_string()),
                timestamp: Some(trade_data.time),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(trade_data.time)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol_str.to_string(),
                trade_type: None,
                side: Some(if trade_data.is_buyer {
                    "buy".to_string()
                } else {
                    "sell".to_string()
                }),
                taker_or_maker: Some(if trade_data.is_maker {
                    crate::types::TakerOrMaker::Maker
                } else {
                    crate::types::TakerOrMaker::Taker
                }),
                price,
                amount,
                cost: Some(price * amount),
                fee: Some(Fee {
                    cost: Some(commission),
                    currency: Some(trade_data.commission_asset.clone()),
                    rate: None,
                }),
                fees: Vec::new(),
                info: serde_json::to_value(&trade_data).unwrap_or_default(),
            });
        }

        Ok(trades)
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("coin".to_string(), c.to_string());
        }
        if let Some(since_ts) = since {
            params.insert("startTime".to_string(), since_ts.to_string());
        }
        params.insert("limit".to_string(), limit.unwrap_or(1000).to_string());

        let response: Vec<MexcDeposit> = self
            .private_request("GET", "/api/v3/capital/deposit/hisrec", params)
            .await?;

        let deposits: Vec<Transaction> = response
            .iter()
            .map(|d| {
                let status = match d.status {
                    0 => TransactionStatus::Pending,
                    6 => TransactionStatus::Ok,
                    _ => TransactionStatus::Pending,
                };

                Transaction {
                    id: d.id.clone().unwrap_or_default(),
                    txid: d.tx_id.clone(),
                    timestamp: d.insert_time,
                    datetime: d.insert_time.map(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
                    network: d.network.clone(),
                    address: d.address.clone(),
                    tag: d.address_tag.clone(),
                    tx_type: TransactionType::Deposit,
                    amount: d.amount.parse().unwrap_or_default(),
                    currency: d.coin.clone(),
                    status,
                    updated: None,
                    internal: None,
                    confirmations: d.confirm_times,
                    fee: None,
                    info: serde_json::to_value(d).unwrap_or_default(),
                }
            })
            .collect();

        Ok(deposits)
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("coin".to_string(), c.to_string());
        }
        if let Some(since_ts) = since {
            params.insert("startTime".to_string(), since_ts.to_string());
        }
        params.insert("limit".to_string(), limit.unwrap_or(1000).to_string());

        let response: Vec<MexcWithdrawal> = self
            .private_request("GET", "/api/v3/capital/withdraw/history", params)
            .await?;

        let withdrawals: Vec<Transaction> = response
            .iter()
            .map(|w| {
                let status = match w.status.as_str() {
                    "completed" => TransactionStatus::Ok,
                    "cancel" | "rejected" => TransactionStatus::Canceled,
                    _ => TransactionStatus::Pending,
                };

                Transaction {
                    id: w.id.clone(),
                    txid: w.tx_id.clone(),
                    timestamp: w.apply_time,
                    datetime: w.apply_time.map(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
                    network: w.network.clone(),
                    address: Some(w.address.clone()),
                    tag: None,
                    tx_type: TransactionType::Withdrawal,
                    amount: w.amount.parse().unwrap_or_default(),
                    currency: w.coin.clone(),
                    status,
                    updated: None,
                    internal: None,
                    confirmations: w.confirm_no,
                    fee: w
                        .transaction_fee
                        .as_ref()
                        .and_then(|f| f.parse().ok())
                        .map(|cost| Fee {
                            cost: Some(cost),
                            currency: Some(w.coin.clone()),
                            rate: None,
                        }),
                    info: serde_json::to_value(w).unwrap_or_default(),
                }
            })
            .collect();

        Ok(withdrawals)
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

        if let Some(t) = tag {
            params.insert("addressTag".into(), t.to_string());
        }
        if let Some(n) = network {
            params.insert("network".into(), n.to_string());
        }

        let response: MexcWithdrawResult = self
            .private_request("POST", "/api/v3/capital/withdraw/apply", params)
            .await?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Transaction {
            id: response.id.clone(),
            txid: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            address: Some(address.to_string()),
            tag: tag.map(|t| t.to_string()),
            tx_type: TransactionType::Withdrawal,
            amount,
            currency: code.to_string(),
            status: TransactionStatus::Pending,
            fee: None,
            network: network.map(|n| n.to_string()),
            info: serde_json::json!({"id": response.id}),
            updated: None,
            internal: None,
            confirmations: None,
        })
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<DepositAddress> {
        let mut params = HashMap::new();
        params.insert("coin".into(), code.to_string());
        if let Some(n) = network {
            params.insert("network".into(), n.to_string());
        }

        let response: Vec<MexcDepositAddress> = self
            .private_request("GET", "/api/v3/capital/deposit/address", params)
            .await?;

        let addr = response.first().ok_or_else(|| CcxtError::BadResponse {
            message: format!("No deposit address found for {code}"),
        })?;

        Ok(DepositAddress {
            currency: code.to_string(),
            address: addr.address.clone(),
            tag: addr.tag.clone(),
            network: Some(addr.network.clone()),
            info: serde_json::to_value(addr).unwrap_or_default(),
        })
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        Some(self.to_market_id(symbol))
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        self.markets_by_id.read().unwrap().get(market_id).cloned()
    }

    async fn edit_order(
        &self,
        id: &str,
        symbol: &str,
        _order_type: Option<OrderType>,
        _side: Option<OrderSide>,
        _amount: Option<Decimal>,
        _price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        // MEXC doesn't support order modification
        Err(CcxtError::NotSupported {
            feature: format!("Order editing is not supported. Order: {id}, Symbol: {symbol}"),
        })
    }

    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        let sym = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "symbol is required for cancel_all_orders".into(),
        })?;

        let market_id = self.to_market_id(sym);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let _response: serde_json::Value = self
            .private_request("DELETE", "/api/v3/openOrders", params)
            .await?;

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
        params.insert("asset".into(), code.to_string());
        params.insert("amount".into(), amount.to_string());
        params.insert("fromAccountType".into(), from_account.to_uppercase());
        params.insert("toAccountType".into(), to_account.to_uppercase());

        let response: MexcTransferResult = self
            .private_request("POST", "/api/v3/capital/transfer", params)
            .await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(TransferEntry::new()
            .with_currency(code)
            .with_amount(amount)
            .with_id(response.tran_id.to_string())
            .with_from_account(from_account)
            .with_to_account(to_account)
            .with_timestamp(timestamp)
            .with_status("ok"))
    }

    async fn fetch_transfers(
        &self,
        _code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<TransferEntry>> {
        let mut params = HashMap::new();
        if let Some(ts) = since {
            params.insert("startTime".into(), ts.to_string());
        }
        if let Some(lim) = limit {
            params.insert("size".into(), lim.to_string());
        }

        let response: MexcTransferList = self
            .private_request("GET", "/api/v3/capital/transfer", params)
            .await?;

        let entries = response.rows.unwrap_or_default();
        let mut transfers = Vec::new();

        for item in entries {
            let amount: Decimal = item.amount.parse().unwrap_or_default();
            let timestamp: i64 = item.timestamp.unwrap_or(0);

            transfers.push(
                TransferEntry::new()
                    .with_currency(&item.asset)
                    .with_amount(amount)
                    .with_id(item.tran_id.to_string())
                    .with_timestamp(timestamp)
                    .with_status(&item.status),
            );
        }

        Ok(transfers)
    }

    async fn fetch_positions(&self, _symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>> {
        // MEXC Futures positions - returns empty for spot-only implementation
        Ok(Vec::new())
    }

    async fn set_leverage(&self, leverage: Decimal, symbol: &str) -> CcxtResult<Leverage> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("leverage".into(), leverage.to_string());

        let _response: serde_json::Value = self
            .private_request("POST", "/api/v1/private/position/change-leverage", params)
            .await?;

        Ok(Leverage {
            info: serde_json::Value::Null,
            symbol: symbol.to_string(),
            margin_mode: MarginMode::Cross,
            long_leverage: leverage,
            short_leverage: leverage,
        })
    }

    async fn fetch_leverage(&self, symbol: &str) -> CcxtResult<Leverage> {
        Ok(Leverage {
            info: serde_json::Value::Null,
            symbol: symbol.to_string(),
            margin_mode: MarginMode::Cross,
            long_leverage: Decimal::from(20), // Default
            short_leverage: Decimal::from(20),
        })
    }

    async fn set_margin_mode(
        &self,
        margin_mode: MarginMode,
        symbol: &str,
    ) -> CcxtResult<MarginModeInfo> {
        let market_id = self.to_market_id(symbol);
        let mode_str = match margin_mode {
            MarginMode::Cross => "cross",
            MarginMode::Isolated => "isolated",
            MarginMode::Unknown => "cross", // Default to cross for unknown
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("marginMode".into(), mode_str.to_string());

        let _response: serde_json::Value = self
            .private_request(
                "POST",
                "/api/v1/private/position/change-margin-type",
                params,
            )
            .await?;

        Ok(MarginModeInfo::new(symbol, margin_mode))
    }

    async fn set_position_mode(
        &self,
        _hedged: bool,
        _symbol: Option<&str>,
    ) -> CcxtResult<PositionModeInfo> {
        // MEXC uses one-way mode only
        Ok(PositionModeInfo::new(PositionMode::OneWay))
    }

    async fn fetch_position_mode(&self, _symbol: Option<&str>) -> CcxtResult<PositionModeInfo> {
        Ok(PositionModeInfo::new(PositionMode::OneWay))
    }

    async fn add_margin(&self, symbol: &str, amount: Decimal) -> CcxtResult<MarginModification> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("amount".into(), amount.to_string());
        params.insert("type".into(), "1".to_string()); // 1 = add margin

        let response: serde_json::Value = self
            .private_request("POST", "/api/v1/private/position/change-margin", params)
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
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("amount".into(), amount.to_string());
        params.insert("type".into(), "2".to_string()); // 2 = reduce margin

        let response: serde_json::Value = self
            .private_request("POST", "/api/v1/private/position/change-margin", params)
            .await?;

        Ok(MarginModification {
            info: response,
            symbol: symbol.to_string(),
            modification_type: Some(MarginModificationType::Reduce),
            amount: Some(amount),
            ..Default::default()
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> CcxtResult<FundingRate> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: MexcFundingRateResponse = self
            .public_get("/api/v1/contract/funding_rate", Some(params))
            .await?;

        let timestamp = Utc::now().timestamp_millis();
        let rate: Decimal = response.data.funding_rate.parse().unwrap_or_default();

        Ok(FundingRate::new(symbol)
            .with_funding_rate(rate)
            .with_timestamp(timestamp))
    }

    async fn fetch_funding_rates(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, FundingRate>> {
        let mut rates = HashMap::new();
        if let Some(syms) = symbols {
            for sym in syms {
                if let Ok(rate) = self.fetch_funding_rate(sym).await {
                    rates.insert(sym.to_string(), rate);
                }
            }
        }
        Ok(rates)
    }

    async fn fetch_open_interest(&self, symbol: &str) -> CcxtResult<OpenInterest> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: MexcOpenInterestResponse = self
            .public_get("/api/v1/contract/open_interest", Some(params))
            .await?;

        let oi: Decimal = response.data.open_interest.parse().unwrap_or_default();
        let timestamp = Utc::now().timestamp_millis();

        Ok(OpenInterest::new(symbol)
            .with_amount(oi)
            .with_timestamp(timestamp))
    }

    async fn fetch_index_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: MexcPremiumIndexResponse = self
            .public_get("/api/v1/contract/premiumIndex", Some(params))
            .await?;

        let data = response.data;
        let timestamp = Utc::now().timestamp_millis();
        let mark_price: Decimal = data.mark_price.parse().unwrap_or_default();
        let index_price: Decimal = data.index_price.parse().unwrap_or_default();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last: Some(index_price),
            mark_price: Some(mark_price),
            index_price: Some(index_price),
            info: serde_json::to_value(&data).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn fetch_mark_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: MexcPremiumIndexResponse = self
            .public_get("/api/v1/contract/premiumIndex", Some(params))
            .await?;

        let data = response.data;
        let timestamp = Utc::now().timestamp_millis();
        let mark_price: Decimal = data.mark_price.parse().unwrap_or_default();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            mark_price: Some(mark_price),
            info: serde_json::to_value(&data).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn fetch_mark_prices(
        &self,
        _symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        // MEXC doesn't have bulk premium index endpoint
        // Return NotSupported as we'd need to iterate over all symbols
        Err(CcxtError::NotSupported {
            feature: "fetch_mark_prices requires iterating over symbols on MEXC".into(),
        })
    }

    async fn fetch_mark_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        // MEXC uses /api/v1/contract/kline for futures OHLCV
        // Mark price data is typically embedded in premiumIndex, not separate candles
        // Return NotSupported
        let _ = (symbol, timeframe, since, limit);
        Err(CcxtError::NotSupported {
            feature: "fetch_mark_ohlcv is not supported by MEXC".into(),
        })
    }

    async fn fetch_index_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        // MEXC doesn't have dedicated index OHLCV endpoint
        let _ = (symbol, timeframe, since, limit);
        Err(CcxtError::NotSupported {
            feature: "fetch_index_ohlcv is not supported by MEXC".into(),
        })
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

// === MEXC API Response Types ===

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MexcExchangeInfo {
    #[serde(default)]
    symbols: Vec<MexcSymbol>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MexcSymbol {
    symbol: String,
    status: String,
    base_asset: String,
    #[serde(default)]
    base_asset_precision: i32,
    quote_asset: String,
    #[serde(default)]
    quote_precision: i32,
    #[serde(default)]
    quote_amount_precision: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MexcTicker {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    price_change: Option<String>,
    #[serde(default)]
    price_change_percent: Option<String>,
    #[serde(default)]
    weighted_avg_price: Option<String>,
    #[serde(default)]
    prev_close_price: Option<String>,
    #[serde(default)]
    last_price: Option<String>,
    #[serde(default)]
    bid_price: Option<String>,
    #[serde(default)]
    bid_qty: Option<String>,
    #[serde(default)]
    ask_price: Option<String>,
    #[serde(default)]
    ask_qty: Option<String>,
    #[serde(default)]
    open_price: Option<String>,
    #[serde(default)]
    high_price: Option<String>,
    #[serde(default)]
    low_price: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    quote_volume: Option<String>,
    #[serde(default)]
    open_time: Option<i64>,
    #[serde(default)]
    close_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MexcOrderBook {
    #[serde(default)]
    last_update_id: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MexcTrade {
    id: i64,
    price: String,
    qty: String,
    time: i64,
    is_buyer_maker: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MexcAccountInfo {
    #[serde(default)]
    balances: Vec<MexcBalance>,
    #[serde(default)]
    update_time: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MexcBalance {
    asset: String,
    free: String,
    locked: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MexcOrder {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    order_id: i64,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    price: String,
    #[serde(default)]
    orig_qty: String,
    #[serde(default)]
    executed_qty: String,
    #[serde(default)]
    cummulative_quote_qty: String,
    #[serde(default)]
    status: String,
    #[serde(rename = "type", default)]
    order_type: String,
    #[serde(default)]
    side: String,
    #[serde(default)]
    time: i64,
    #[serde(default)]
    update_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MexcOrderResponse {
    #[serde(default)]
    order_id: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MexcMyTrade {
    id: i64,
    order_id: i64,
    price: String,
    qty: String,
    commission: String,
    commission_asset: String,
    time: i64,
    is_buyer: bool,
    is_maker: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MexcDeposit {
    #[serde(default)]
    id: Option<String>,
    coin: String,
    #[serde(default)]
    network: Option<String>,
    amount: String,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    address_tag: Option<String>,
    #[serde(default)]
    tx_id: Option<String>,
    status: i32,
    #[serde(default)]
    insert_time: Option<i64>,
    #[serde(default)]
    confirm_times: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MexcWithdrawal {
    id: String,
    coin: String,
    #[serde(default)]
    network: Option<String>,
    amount: String,
    address: String,
    #[serde(default)]
    tx_id: Option<String>,
    status: String,
    #[serde(default)]
    transaction_fee: Option<String>,
    #[serde(default)]
    apply_time: Option<i64>,
    #[serde(default)]
    confirm_no: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MexcTransferResult {
    tran_id: i64,
}

#[derive(Debug, Default, Deserialize)]
struct MexcTransferList {
    #[serde(default)]
    rows: Option<Vec<MexcTransferRecord>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MexcTransferRecord {
    tran_id: i64,
    asset: String,
    amount: String,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Default, Deserialize)]
struct MexcFundingRateResponse {
    #[serde(default)]
    data: MexcFundingRateData,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MexcFundingRateData {
    #[serde(default)]
    funding_rate: String,
}

#[derive(Debug, Default, Deserialize)]
struct MexcOpenInterestResponse {
    #[serde(default)]
    data: MexcOpenInterestData,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MexcOpenInterestData {
    #[serde(default)]
    open_interest: String,
}

#[derive(Debug, Default, Deserialize)]
struct MexcPremiumIndexResponse {
    #[serde(default)]
    data: MexcPremiumIndexData,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MexcPremiumIndexData {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    index_price: String,
    #[serde(default)]
    mark_price: String,
    #[serde(default)]
    last_funding_rate: String,
}

#[derive(Debug, Default, Deserialize)]
struct MexcWithdrawResult {
    #[serde(default)]
    id: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MexcDepositAddress {
    #[serde(default)]
    address: String,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    network: String,
    #[serde(default)]
    coin: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Mexc::new(config).unwrap();

        assert_eq!(exchange.name(), "MEXC");
        assert!(exchange.has().spot);
        assert!(exchange.has().fetch_ticker);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::default();
        let exchange = Mexc::new(config).unwrap();

        assert_eq!(exchange.to_market_id("BTC/USDT"), "BTCUSDT");
        assert_eq!(exchange.to_market_id("ETH/BTC"), "ETHBTC");
    }
}
