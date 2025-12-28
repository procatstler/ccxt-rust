//! Alpaca Exchange Implementation
//!
//! Stock and crypto broker with dual API endpoints (Trading + Market Data)

#![allow(dead_code)]

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, TimeInForce, Trade,
    OHLCV,
};

/// Alpaca exchange
pub struct Alpaca {
    config: ExchangeConfig,
    trader_client: HttpClient,
    market_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    default_loc: String,
}

impl Alpaca {
    const TRADER_URL: &'static str = "https://api.alpaca.markets";
    const TRADER_URL_PAPER: &'static str = "https://paper-api.alpaca.markets";
    const MARKET_URL: &'static str = "https://data.alpaca.markets";
    const RATE_LIMIT_MS: u64 = 333; // 3 req/s for free tier

    /// Create new Alpaca instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let trader_url = if config.is_sandbox() {
            Self::TRADER_URL_PAPER
        } else {
            Self::TRADER_URL
        };

        let trader_client = HttpClient::new(trader_url, &config)?;
        let market_client = HttpClient::new(Self::MARKET_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
            swap: false,
            future: false,
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
            withdraw: true,
            fetch_deposit_address: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("trader".into(), trader_url.into());
        api_urls.insert("market".into(), Self::MARKET_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/user-attachments/assets/e9476df8-a450-4c3e-ab9a-1a7794219e1b".into()),
            api: api_urls,
            www: Some("https://alpaca.markets".into()),
            doc: vec![
                "https://alpaca.markets/docs/".into(),
            ],
            fees: Some("https://docs.alpaca.markets/docs/crypto-fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1min".into());
        timeframes.insert(Timeframe::Minute3, "3min".into());
        timeframes.insert(Timeframe::Minute5, "5min".into());
        timeframes.insert(Timeframe::Minute15, "15min".into());
        timeframes.insert(Timeframe::Minute30, "30min".into());
        timeframes.insert(Timeframe::Hour1, "1H".into());
        timeframes.insert(Timeframe::Hour2, "2H".into());
        timeframes.insert(Timeframe::Hour4, "4H".into());
        timeframes.insert(Timeframe::Hour6, "6H".into());
        timeframes.insert(Timeframe::Hour8, "8H".into());
        timeframes.insert(Timeframe::Hour12, "12H".into());
        timeframes.insert(Timeframe::Day1, "1D".into());
        timeframes.insert(Timeframe::Day3, "3D".into());
        timeframes.insert(Timeframe::Week1, "1W".into());
        timeframes.insert(Timeframe::Month1, "1M".into());

        Ok(Self {
            config,
            trader_client,
            market_client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            default_loc: "us".into(),
        })
    }

    /// Trading API private call
    async fn trader_private<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let mut headers = HashMap::new();
        headers.insert("APCA-API-KEY-ID".into(), self.config.api_key().unwrap_or_default().to_string());
        headers.insert("APCA-API-SECRET-KEY".into(), self.config.api_secret().unwrap_or_default().to_string());
        headers.insert("APCA-PARTNER-ID".into(), "ccxt".into());

        let response = match method {
            "GET" => self.trader_client.get(path, params, Some(headers)).await?,
            "POST" => {
                headers.insert("Content-Type".into(), "application/json".into());
                let body = params.map(|p| serde_json::to_value(&p).unwrap_or_default());
                self.trader_client.post(path, body, Some(headers)).await?
            }
            "DELETE" => self.trader_client.delete(path, params, Some(headers)).await?,
            "PATCH" => {
                headers.insert("Content-Type".into(), "application/json".into());
                self.trader_client.patch(path, params.unwrap_or_default(), Some(headers)).await?
            }
            _ => return Err(CcxtError::NotSupported { feature: format!("Unsupported method: {}", method) }),
        };

        Ok(response)
    }

    /// Market Data API public call
    async fn market_public<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        let response = self.market_client.get(path, params, None).await?;
        Ok(response)
    }

    /// Convert unified symbol to Alpaca market ID (BTC/USD -> BTCUSD)
    fn symbol_to_market_id(&self, symbol: &str) -> String {
        symbol.replace('/', "")
    }

    /// Convert Alpaca market ID to unified symbol (BTCUSD -> BTC/USD, BTC/USDT -> BTC/USDT)
    fn market_id_to_symbol(&self, market_id: &str) -> String {
        if market_id.contains('/') {
            market_id.to_string()
        } else {
            // Simple heuristic: if ends with USD/USDT, split there
            if market_id.ends_with("USD") {
                let base = &market_id[..market_id.len() - 3];
                format!("{}/USD", base)
            } else if market_id.ends_with("USDT") {
                let base = &market_id[..market_id.len() - 4];
                format!("{}/USDT", base)
            } else {
                market_id.to_string()
            }
        }
    }

    /// Count decimal places in a string representation of a number
    fn count_decimals(value: &str) -> Option<i32> {
        if let Some(pos) = value.find('.') {
            Some((value.len() - pos - 1) as i32)
        } else {
            Some(0)
        }
    }

    /// Get market by symbol
    fn market(&self, symbol: &str) -> CcxtResult<Market> {
        let markets = self.markets.read().unwrap();
        markets.get(symbol).cloned().ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })
    }

    /// Get market by market ID
    fn market_by_id(&self, market_id: &str) -> CcxtResult<Market> {
        let markets_by_id = self.markets_by_id.read().unwrap();
        if let Some(symbol) = markets_by_id.get(market_id) {
            let markets = self.markets.read().unwrap();
            markets.get(symbol).cloned().ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })
        } else {
            Err(CcxtError::BadSymbol {
                symbol: market_id.to_string(),
            })
        }
    }

    /// Parse Alpaca asset to Market
    fn parse_market(&self, asset: &AlpacaAsset) -> Market {
        let market_id = asset.symbol.clone();
        let parts: Vec<&str> = market_id.split('/').collect();

        let (base, quote) = if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            // For symbols without /, try to parse
            if market_id.ends_with("USD") && market_id != "USD" {
                let base = market_id[..market_id.len() - 3].to_string();
                (base, "USD".to_string())
            } else if market_id.ends_with("USDT") {
                let base = market_id[..market_id.len() - 4].to_string();
                (base, "USDT".to_string())
            } else {
                (market_id.clone(), "USD".to_string())
            }
        };

        let symbol = format!("{}/{}", base, quote);
        let active = asset.status == "active" && asset.tradable;

        let min_amount = asset.min_order_size.parse::<Decimal>().ok();
        let amount_precision = Self::count_decimals(&asset.min_trade_increment);
        let price_precision = Self::count_decimals(&asset.price_increment);

        Market {
            id: market_id.clone(),
            lowercase_id: Some(market_id.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: base,
            quote_id: quote,
            active,
            market_type: MarketType::Spot,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            index: false,
            contract: false,
            settle: None,
            settle_id: None,
            contract_size: None,
            linear: None,
            inverse: None,
            sub_type: None,
            taker: None,
            maker: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            precision: MarketPrecision {
                amount: amount_precision,
                price: price_precision,
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: crate::types::MinMax {
                    min: min_amount,
                    max: None,
                },
                price: crate::types::MinMax {
                    min: None,
                    max: None,
                },
                cost: crate::types::MinMax {
                    min: None,
                    max: None,
                },
                leverage: crate::types::MinMax {
                    min: None,
                    max: None,
                },
            },
            margin_modes: None,
            created: None,
            info: serde_json::to_value(asset).unwrap_or_default(),
            tier_based: false,
            percentage: false,
        }
    }

    /// Parse Alpaca order book entry
    fn parse_order_book_entry(&self, entry: &AlpacaOrderBookEntry) -> OrderBookEntry {
        OrderBookEntry {
            price: Decimal::from_f64_retain(entry.p).unwrap_or_default(),
            amount: Decimal::from_f64_retain(entry.s).unwrap_or_default(),
        }
    }

    /// Parse Alpaca trade
    fn parse_trade(&self, trade: &AlpacaTrade, market: Option<&Market>) -> Trade {
        let symbol = market.map(|m| m.symbol.clone()).unwrap_or_default();

        let side = match trade.tks.as_deref() {
            Some("B") => Some("buy".to_string()),
            Some("S") => Some("sell".to_string()),
            _ => trade.side.clone(),
        };

        let timestamp = chrono::DateTime::parse_from_rfc3339(&trade.t)
            .map(|dt| dt.timestamp_millis())
            .ok();

        let price = Decimal::from_f64_retain(trade.p).unwrap_or_default();
        let amount = Decimal::from_f64_retain(trade.s).unwrap_or_default();
        let cost = Some(price * amount);

        Trade {
            id: trade.i.clone().unwrap_or_default(),
            order: trade.order_id.clone(),
            info: serde_json::to_value(trade).unwrap_or_default(),
            timestamp,
            datetime: Some(trade.t.clone()),
            symbol,
            trade_type: None,
            side,
            taker_or_maker: Some(TakerOrMaker::Taker),
            price,
            amount,
            cost,
            fee: None,
            fees: Vec::new(),
        }
    }

    /// Parse Alpaca OHLCV bar
    fn parse_ohlcv(&self, bar: &AlpacaBar) -> OHLCV {
        let timestamp = chrono::DateTime::parse_from_rfc3339(&bar.t)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(0);

        OHLCV {
            timestamp,
            open: Decimal::from_f64_retain(bar.o).unwrap_or_default(),
            high: Decimal::from_f64_retain(bar.h).unwrap_or_default(),
            low: Decimal::from_f64_retain(bar.l).unwrap_or_default(),
            close: Decimal::from_f64_retain(bar.c).unwrap_or_default(),
            volume: Decimal::from_f64_retain(bar.v).unwrap_or_default(),
        }
    }

    /// Parse Alpaca order status
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status {
            "pending_new" | "accepted" | "new" | "partially_filled" | "activated" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "canceled" => OrderStatus::Canceled,
            "expired" => OrderStatus::Expired,
            "rejected" | "failed" => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        }
    }

    /// Parse Alpaca order
    fn parse_order(&self, order: &AlpacaOrder, market: Option<&Market>) -> Order {
        let market_id = &order.symbol;
        let symbol = market.map(|m| m.symbol.clone())
            .unwrap_or_else(|| self.market_id_to_symbol(market_id));

        let timestamp = chrono::DateTime::parse_from_rfc3339(&order.submitted_at)
            .map(|dt| dt.timestamp_millis())
            .ok();

        let status = self.parse_order_status(&order.status);

        let order_type = if order.order_type.contains("limit") {
            OrderType::Limit
        } else if order.order_type == "market" {
            OrderType::Market
        } else {
            OrderType::Limit
        };

        let side = match order.side.as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = match order.time_in_force.as_str() {
            "gtc" => Some(TimeInForce::GTC),
            "ioc" => Some(TimeInForce::IOC),
            "fok" => Some(TimeInForce::FOK),
            "day" => Some(TimeInForce::GTC), // Map day to GTC as Day is not in enum
            _ => None,
        };

        let fee = order.commission.map(|cost| crate::types::Fee {
            cost: Decimal::from_f64_retain(cost),
            currency: Some("USD".to_string()),
            rate: None,
        });

        let amount = order.qty.and_then(Decimal::from_f64_retain).unwrap_or_default();
        let filled = order.filled_qty.and_then(Decimal::from_f64_retain).unwrap_or_default();
        let remaining = if amount > filled { Some(amount - filled) } else { None };

        Order {
            id: order.id.clone(),
            client_order_id: Some(order.client_order_id.clone()),
            datetime: Some(order.submitted_at.clone()),
            timestamp,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force,
            side,
            price: order.limit_price.and_then(Decimal::from_f64_retain),
            average: order.filled_avg_price.and_then(Decimal::from_f64_retain),
            amount,
            filled,
            remaining,
            cost: None,
            trades: Vec::new(),
            fee,
            fees: Vec::new(),
            info: serde_json::json!(order),
            stop_price: order.stop_price.and_then(Decimal::from_f64_retain),
            trigger_price: order.stop_price.and_then(Decimal::from_f64_retain),
            take_profit_price: None,
            stop_loss_price: None,
            post_only: None,
            reduce_only: None,
        }
    }

    /// Parse Alpaca ticker from snapshot
    fn parse_ticker(&self, snapshot: &AlpacaSnapshot, market: &Market) -> Ticker {
        let timestamp = snapshot.latest_quote.as_ref()
            .and_then(|q| chrono::DateTime::parse_from_rfc3339(&q.t).ok())
            .map(|dt| dt.timestamp_millis());

        let bid = snapshot.latest_quote.as_ref().and_then(|q| q.bp.and_then(Decimal::from_f64_retain));
        let bid_volume = snapshot.latest_quote.as_ref().and_then(|q| q.bs.and_then(Decimal::from_f64_retain));
        let ask = snapshot.latest_quote.as_ref().and_then(|q| q.ap.and_then(Decimal::from_f64_retain));
        let ask_volume = snapshot.latest_quote.as_ref().and_then(|q| q.as_.and_then(Decimal::from_f64_retain));

        let last = snapshot.latest_trade.as_ref().and_then(|t| Decimal::from_f64_retain(t.p));
        let open = snapshot.daily_bar.as_ref().and_then(|b| Decimal::from_f64_retain(b.o));
        let high = snapshot.daily_bar.as_ref().and_then(|b| Decimal::from_f64_retain(b.h));
        let low = snapshot.daily_bar.as_ref().and_then(|b| Decimal::from_f64_retain(b.l));
        let close = snapshot.daily_bar.as_ref().and_then(|b| Decimal::from_f64_retain(b.c));
        let volume = snapshot.daily_bar.as_ref().and_then(|b| Decimal::from_f64_retain(b.v));
        let vwap = snapshot.daily_bar.as_ref().and_then(|b| Decimal::from_f64_retain(b.vw));
        let previous_close = snapshot.prev_daily_bar.as_ref().and_then(|b| Decimal::from_f64_retain(b.c));

        let (change, percentage) = if let (Some(c), Some(pc)) = (close, previous_close) {
            let change_val = c - pc;
            let percentage_val = if pc != Decimal::ZERO { (change_val / pc) * Decimal::from(100) } else { Decimal::ZERO };
            (Some(change_val), Some(percentage_val))
        } else {
            (None, None)
        };

        Ticker {
            symbol: market.symbol.clone(),
            timestamp,
            datetime: timestamp.map(|ts| {
                chrono::DateTime::from_timestamp(ts / 1000, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            high,
            low,
            bid,
            bid_volume,
            ask,
            ask_volume,
            vwap,
            open,
            close,
            last,
            previous_close,
            change,
            percentage,
            average: None,
            base_volume: volume,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::json!(snapshot),
        }
    }
}

#[async_trait]
impl Exchange for Alpaca {
    fn id(&self) -> ExchangeId {
        ExchangeId::Alpaca
    }

    fn name(&self) -> &'static str {
        "Alpaca"
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
        Some(self.symbol_to_market_id(symbol))
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
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{}{}", Self::TRADER_URL, path);
        let mut headers = HashMap::new();

        headers.insert(
            "APCA-API-KEY-ID".into(),
            self.config.api_key().unwrap_or_default().to_string(),
        );
        headers.insert(
            "APCA-API-SECRET-KEY".into(),
            self.config.api_secret().unwrap_or_default().to_string(),
        );
        headers.insert("APCA-PARTNER-ID".into(), "ccxt".into());

        let body = if method == "GET" {
            if !params.is_empty() {
                let query: String = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join("&");
                url = format!("{}?{}", url, query);
            }
            None
        } else {
            if !params.is_empty() {
                headers.insert("Content-Type".into(), "application/json".into());
                Some(serde_json::to_string(&params).unwrap_or_default())
            } else {
                None
            }
        };

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body,
        }
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        if !reload {
            let markets = self.markets.read().unwrap();
            if !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let markets = self.fetch_markets().await?;

        let mut markets_map = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for market in markets {
            markets_by_id.insert(market.id.clone(), market.symbol.clone());
            markets_map.insert(market.symbol.clone(), market);
        }

        *self.markets.write().unwrap() = markets_map.clone();
        *self.markets_by_id.write().unwrap() = markets_by_id;

        Ok(markets_map)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let mut params = HashMap::new();
        params.insert("asset_class".into(), "crypto".into());
        params.insert("status".into(), "active".into());

        let assets: Vec<AlpacaAsset> = self.trader_private("GET", "/v2/assets", Some(params)).await?;

        Ok(assets.iter().map(|asset| self.parse_market(asset)).collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.load_markets(false).await?;

        let market = self.market(symbol)?;
        let market_id = &market.id;

        let mut params = HashMap::new();
        params.insert("symbols".into(), market_id.clone());
        params.insert("loc".into(), self.default_loc.clone());

        let path = format!("/v1beta3/crypto/{}/snapshots", self.default_loc);
        let response: AlpacaSnapshotResponse = self.market_public(&path, Some(params)).await?;

        let snapshot = response.snapshots.get(market_id)
            .ok_or_else(|| CcxtError::BadResponse { message: "No snapshot data".into() })?;

        Ok(self.parse_ticker(snapshot, &market))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        let symbols_str = if let Some(syms) = symbols {
            let market_ids: Vec<String> = syms.iter()
                .filter_map(|s| self.market(s).ok())
                .map(|m| m.id.clone())
                .collect();
            market_ids.join(",")
        } else {
            // Fetch all crypto markets
            let markets = self.markets.read().unwrap();
            let market_ids: Vec<String> = markets.values()
                .map(|m| m.id.clone())
                .collect();
            market_ids.join(",")
        };

        let mut params = HashMap::new();
        params.insert("symbols".into(), symbols_str);
        params.insert("loc".into(), self.default_loc.clone());

        let path = format!("/v1beta3/crypto/{}/snapshots", self.default_loc);
        let response: AlpacaSnapshotResponse = self.market_public(&path, Some(params)).await?;

        let mut tickers = HashMap::new();
        for (market_id, snapshot) in response.snapshots {
            if let Ok(market) = self.market_by_id(&market_id) {
                let ticker = self.parse_ticker(&snapshot, &market);
                tickers.insert(market.symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;

        let market = self.market(symbol)?;
        let market_id = &market.id;

        let mut params = HashMap::new();
        params.insert("symbols".into(), market_id.clone());
        params.insert("loc".into(), self.default_loc.clone());

        let path = format!("/v1beta3/crypto/{}/latest/orderbooks", self.default_loc);
        let response: AlpacaOrderBookResponse = self.market_public(&path, Some(params)).await?;

        let orderbook = response.orderbooks.get(market_id)
            .ok_or_else(|| CcxtError::BadResponse { message: "No orderbook data".into() })?;

        let timestamp = chrono::DateTime::parse_from_rfc3339(&orderbook.t)
            .map(|dt| dt.timestamp_millis())
            .ok();

        let mut bids: Vec<OrderBookEntry> = orderbook.b.iter()
            .map(|entry| self.parse_order_book_entry(entry))
            .collect();

        let mut asks: Vec<OrderBookEntry> = orderbook.a.iter()
            .map(|entry| self.parse_order_book_entry(entry))
            .collect();

        if let Some(lim) = limit {
            bids.truncate(lim as usize);
            asks.truncate(lim as usize);
        }

        Ok(OrderBook {
            symbol: market.symbol.clone(),
            bids,
            asks,
            timestamp,
            datetime: timestamp.map(|ts| {
                chrono::DateTime::from_timestamp(ts as i64 / 1000, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            nonce: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.load_markets(false).await?;

        let market = self.market(symbol)?;
        let market_id = &market.id;

        let mut params = HashMap::new();
        params.insert("symbols".into(), market_id.clone());
        params.insert("loc".into(), self.default_loc.clone());

        if let Some(s) = since {
            let dt = chrono::DateTime::from_timestamp(s / 1000, 0)
                .ok_or_else(|| CcxtError::BadRequest { message: "Invalid timestamp".into() })?;
            params.insert("start".into(), dt.to_rfc3339());
        }

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let path = format!("/v1beta3/crypto/{}/trades", self.default_loc);
        let response: AlpacaTradesResponse = self.market_public(&path, Some(params)).await?;

        let trades = response.trades.get(market_id)
            .ok_or_else(|| CcxtError::BadResponse { message: "No trades data".into() })?;

        Ok(trades.iter().map(|t| self.parse_trade(t, Some(&market))).collect())
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        self.load_markets(false).await?;

        let market = self.market(symbol)?;
        let market_id = &market.id;

        let tf = self.timeframes.get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest { message: format!("Unsupported timeframe: {:?}", timeframe) })?;

        let mut params = HashMap::new();
        params.insert("symbols".into(), market_id.clone());
        params.insert("loc".into(), self.default_loc.clone());
        params.insert("timeframe".into(), tf.clone());

        if let Some(s) = since {
            let dt = chrono::DateTime::from_timestamp(s / 1000, 0)
                .ok_or_else(|| CcxtError::BadRequest { message: "Invalid timestamp".into() })?;
            params.insert("start".into(), dt.format("%Y-%m-%d").to_string());
        }

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let path = format!("/v1beta3/crypto/{}/bars", self.default_loc);
        let response: AlpacaBarsResponse = self.market_public(&path, Some(params)).await?;

        let bars = response.bars.get(market_id)
            .ok_or_else(|| CcxtError::BadResponse { message: "No OHLCV data".into() })?;

        Ok(bars.iter().map(|b| self.parse_ohlcv(b)).collect())
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: AlpacaAccount = self.trader_private("GET", "/v2/account", None).await?;

        let mut balances = Balances {
            info: serde_json::json!(response),
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            datetime: Some(chrono::Utc::now().to_rfc3339()),
            currencies: HashMap::new(),
        };

        let currency = response.currency.clone();
        let cash = Decimal::from_f64_retain(response.cash);
        let equity = Decimal::from_f64_retain(response.equity);

        let balance = Balance {
            free: cash,
            used: if let (Some(eq), Some(c)) = (equity, cash) {
                Some(eq - c)
            } else {
                None
            },
            total: equity,
            debt: None,
        };

        balances.currencies.insert(currency, balance);

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
        self.load_markets(false).await?;

        let market = self.market(symbol)?;
        let market_id = &market.id;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id.clone());
        let side_str = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };
        params.insert("side".into(), side_str.to_string());

        let type_str = match order_type {
            OrderType::Market => "market",
            OrderType::Limit => "limit",
            OrderType::StopLoss => "stop_limit",
            OrderType::TakeProfit => "stop_limit",
            _ => "limit",
        };
        params.insert("type".into(), type_str.to_string());

        if matches!(order_type, OrderType::Limit | OrderType::StopLoss | OrderType::TakeProfit) {
            if let Some(p) = price {
                params.insert("limit_price".into(), p.to_string());
            }
        }

        params.insert("qty".into(), amount.to_string());
        params.insert("time_in_force".into(), "gtc".to_string());

        // Generate client order ID
        let client_id = format!("ccxt_{}", uuid::Uuid::new_v4().simple());
        params.insert("client_order_id".into(), client_id);

        let order: AlpacaOrder = self.trader_private("POST", "/v2/orders", Some(params)).await?;

        Ok(self.parse_order(&order, Some(&market)))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.load_markets(false).await?;

        let path = format!("/v2/orders/{}", id);
        let order: AlpacaOrder = self.trader_private("DELETE", &path, None).await?;

        let market = self.market(symbol).ok();

        Ok(self.parse_order(&order, market.as_ref()))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.load_markets(false).await?;

        let path = format!("/v2/orders/{}", id);
        let order: AlpacaOrder = self.trader_private("GET", &path, None).await?;

        let market = self.market(symbol).ok();

        Ok(self.parse_order(&order, market.as_ref()))
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        params.insert("status".into(), "all".into());

        if let Some(sym) = symbol {
            let market = self.market(sym)?;
            params.insert("symbols".into(), market.id.clone());
        }

        if let Some(s) = since {
            let dt = chrono::DateTime::from_timestamp(s / 1000, 0)
                .ok_or_else(|| CcxtError::BadRequest { message: "Invalid timestamp".into() })?;
            params.insert("after".into(), dt.to_rfc3339());
        }

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let orders: Vec<AlpacaOrder> = self.trader_private("GET", "/v2/orders", Some(params)).await?;

        Ok(orders.iter().map(|o| {
            let market = self.market_by_id(&o.symbol).ok();
            self.parse_order(o, market.as_ref())
        }).collect())
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        params.insert("status".into(), "open".into());

        if let Some(sym) = symbol {
            let market = self.market(sym)?;
            params.insert("symbols".into(), market.id.clone());
        }

        if let Some(s) = since {
            let dt = chrono::DateTime::from_timestamp(s / 1000, 0)
                .ok_or_else(|| CcxtError::BadRequest { message: "Invalid timestamp".into() })?;
            params.insert("after".into(), dt.to_rfc3339());
        }

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let orders: Vec<AlpacaOrder> = self.trader_private("GET", "/v2/orders", Some(params)).await?;

        Ok(orders.iter().map(|o| {
            let market = self.market_by_id(&o.symbol).ok();
            self.parse_order(o, market.as_ref())
        }).collect())
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        params.insert("status".into(), "closed".into());

        if let Some(sym) = symbol {
            let market = self.market(sym)?;
            params.insert("symbols".into(), market.id.clone());
        }

        if let Some(s) = since {
            let dt = chrono::DateTime::from_timestamp(s / 1000, 0)
                .ok_or_else(|| CcxtError::BadRequest { message: "Invalid timestamp".into() })?;
            params.insert("after".into(), dt.to_rfc3339());
        }

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let orders: Vec<AlpacaOrder> = self.trader_private("GET", "/v2/orders", Some(params)).await?;

        Ok(orders.iter().map(|o| {
            let market = self.market_by_id(&o.symbol).ok();
            self.parse_order(o, market.as_ref())
        }).collect())
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        params.insert("activity_type".into(), "FILL".into());

        if let Some(s) = since {
            let dt = chrono::DateTime::from_timestamp(s / 1000, 0)
                .ok_or_else(|| CcxtError::BadRequest { message: "Invalid timestamp".into() })?;
            params.insert("after".into(), dt.to_rfc3339());
        }

        if let Some(l) = limit {
            params.insert("page_size".into(), l.to_string());
        }

        let trades: Vec<AlpacaTrade> = self.trader_private("GET", "/v2/account/activities/FILL", Some(params)).await?;

        let market = if let Some(sym) = symbol {
            Some(self.market(sym)?)
        } else {
            None
        };

        Ok(trades.iter()
            .filter(|t| {
                if let Some(ref m) = market {
                    t.symbol.as_ref().map_or(false, |s| s == &m.id)
                } else {
                    true
                }
            })
            .map(|t| self.parse_trade(t, market.as_ref()))
            .collect())
    }
}

// Alpaca API response structures

#[derive(Debug, Serialize, Deserialize)]
struct AlpacaAsset {
    id: String,
    #[serde(rename = "class")]
    class_type: String,
    exchange: String,
    symbol: String,
    name: String,
    status: String,
    tradable: bool,
    marginable: bool,
    shortable: bool,
    easy_to_borrow: bool,
    fractionable: bool,
    min_order_size: String,
    min_trade_increment: String,
    price_increment: String,
}

#[derive(Debug, Deserialize)]
struct AlpacaSnapshotResponse {
    snapshots: HashMap<String, AlpacaSnapshot>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AlpacaSnapshot {
    #[serde(rename = "latestQuote")]
    latest_quote: Option<AlpacaQuote>,
    #[serde(rename = "latestTrade")]
    latest_trade: Option<AlpacaTradeTick>,
    #[serde(rename = "dailyBar")]
    daily_bar: Option<AlpacaBar>,
    #[serde(rename = "prevDailyBar")]
    prev_daily_bar: Option<AlpacaBar>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AlpacaQuote {
    t: String,
    bp: Option<f64>,
    bs: Option<f64>,
    ap: Option<f64>,
    #[serde(rename = "as")]
    as_: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AlpacaTradeTick {
    t: String,
    p: f64,
    s: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct AlpacaBar {
    t: String,
    o: f64,
    h: f64,
    l: f64,
    c: f64,
    v: f64,
    #[serde(default)]
    vw: f64,
    #[serde(default)]
    n: i64,
}

#[derive(Debug, Deserialize)]
struct AlpacaOrderBookResponse {
    orderbooks: HashMap<String, AlpacaOrderBookData>,
}

#[derive(Debug, Deserialize)]
struct AlpacaOrderBookData {
    t: String,
    b: Vec<AlpacaOrderBookEntry>,
    a: Vec<AlpacaOrderBookEntry>,
}

#[derive(Debug, Deserialize)]
struct AlpacaOrderBookEntry {
    p: f64,
    s: f64,
}

#[derive(Debug, Deserialize)]
struct AlpacaTradesResponse {
    trades: HashMap<String, Vec<AlpacaTrade>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AlpacaTrade {
    t: String,
    #[serde(default)]
    i: Option<String>,
    p: f64,
    s: f64,
    #[serde(default)]
    tks: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AlpacaBarsResponse {
    bars: HashMap<String, Vec<AlpacaBar>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AlpacaAccount {
    id: String,
    account_number: String,
    status: String,
    crypto_status: Option<String>,
    currency: String,
    buying_power: f64,
    cash: f64,
    portfolio_value: f64,
    equity: f64,
    last_equity: f64,
    long_market_value: f64,
    short_market_value: f64,
    initial_margin: f64,
    maintenance_margin: f64,
}

#[derive(Debug, Deserialize, Serialize)]
struct AlpacaOrder {
    id: String,
    client_order_id: String,
    created_at: String,
    updated_at: String,
    submitted_at: String,
    filled_at: Option<String>,
    expired_at: Option<String>,
    canceled_at: Option<String>,
    failed_at: Option<String>,
    asset_id: String,
    symbol: String,
    asset_class: String,
    notional: Option<String>,
    qty: Option<f64>,
    filled_qty: Option<f64>,
    filled_avg_price: Option<f64>,
    order_class: String,
    order_type: String,
    #[serde(rename = "type")]
    type_: String,
    side: String,
    time_in_force: String,
    limit_price: Option<f64>,
    stop_price: Option<f64>,
    status: String,
    extended_hours: bool,
    commission: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alpaca_creation() {
        let config = ExchangeConfig::default();
        let exchange = Alpaca::new(config);
        assert!(exchange.is_ok());
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::default();
        let alpaca = Alpaca::new(config).unwrap();

        assert_eq!(alpaca.symbol_to_market_id("BTC/USD"), "BTCUSD");
        assert_eq!(alpaca.symbol_to_market_id("ETH/USDT"), "ETHUSDT");

        assert_eq!(alpaca.market_id_to_symbol("BTCUSD"), "BTC/USD");
        assert_eq!(alpaca.market_id_to_symbol("BTC/USDT"), "BTC/USDT");
    }
}
