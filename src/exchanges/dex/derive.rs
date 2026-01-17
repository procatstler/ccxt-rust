//! Derive Exchange Implementation
//!
//! Derive DEX exchange implementation using Lyra Finance API
//! API Documentation: <https://docs.derive.xyz/docs/>

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketType,
    Order, OrderBook, OrderSide, OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade,
    OHLCV,
};

// Response structures
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveResponse<T> {
    result: Option<T>,
    error: Option<DeriveError>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveError {
    code: Option<String>,
    message: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveInstrument {
    instrument_name: Option<String>,
    instrument_type: Option<String>,
    quote_currency: Option<String>,
    base_currency: Option<String>,
    is_active: Option<bool>,
    tick_size: Option<String>,
    minimum_amount: Option<String>,
    amount_step: Option<String>,
    maker_fee_rate: Option<String>,
    taker_fee_rate: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveInstrumentsResult {
    instruments: Option<Vec<DeriveInstrument>>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveTicker {
    instrument_name: Option<String>,
    best_bid_price: Option<String>,
    best_bid_amount: Option<String>,
    best_ask_price: Option<String>,
    best_ask_amount: Option<String>,
    last_price: Option<String>,
    mark_price: Option<String>,
    index_price: Option<String>,
    open_interest: Option<String>,
    timestamp: Option<i64>,
    stats: Option<DeriveTickerStats>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveTickerStats {
    high: Option<String>,
    low: Option<String>,
    volume: Option<String>,
    volume_usd: Option<String>,
    price_change: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveTrade {
    trade_id: Option<String>,
    instrument_name: Option<String>,
    price: Option<String>,
    amount: Option<String>,
    direction: Option<String>,
    timestamp: Option<i64>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveTradesResult {
    trades: Option<Vec<DeriveTrade>>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveBalance {
    currency: Option<String>,
    amount: Option<String>,
    available: Option<String>,
    locked: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveBalanceResult {
    collaterals: Option<Vec<DeriveBalance>>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveOrder {
    order_id: Option<String>,
    instrument_name: Option<String>,
    direction: Option<String>,
    order_type: Option<String>,
    limit_price: Option<String>,
    amount: Option<String>,
    filled_amount: Option<String>,
    average_price: Option<String>,
    order_status: Option<String>,
    creation_timestamp: Option<i64>,
    last_update_timestamp: Option<i64>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveOrderResult {
    order: Option<DeriveOrder>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveOrdersResult {
    orders: Option<Vec<DeriveOrder>>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveMyTrade {
    trade_id: Option<String>,
    order_id: Option<String>,
    instrument_name: Option<String>,
    price: Option<String>,
    amount: Option<String>,
    direction: Option<String>,
    fee: Option<String>,
    fee_currency: Option<String>,
    timestamp: Option<i64>,
    liquidity: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DeriveMyTradesResult {
    trades: Option<Vec<DeriveMyTrade>>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DerivePosition {
    instrument_name: Option<String>,
    direction: Option<String>,
    amount: Option<String>,
    average_price: Option<String>,
    mark_price: Option<String>,
    unrealized_pnl: Option<String>,
    realized_pnl: Option<String>,
    maintenance_margin: Option<String>,
    initial_margin: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct DerivePositionsResult {
    positions: Option<Vec<DerivePosition>>,
}

pub struct Derive {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Derive {
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".to_string());
        timeframes.insert(Timeframe::Minute3, "3m".to_string());
        timeframes.insert(Timeframe::Minute5, "5m".to_string());
        timeframes.insert(Timeframe::Minute15, "15m".to_string());
        timeframes.insert(Timeframe::Minute30, "30m".to_string());
        timeframes.insert(Timeframe::Hour1, "1h".to_string());
        timeframes.insert(Timeframe::Hour2, "2h".to_string());
        timeframes.insert(Timeframe::Hour4, "4h".to_string());
        timeframes.insert(Timeframe::Hour12, "12h".to_string());
        timeframes.insert(Timeframe::Day1, "1d".to_string());
        timeframes.insert(Timeframe::Week1, "1w".to_string());
        timeframes.insert(Timeframe::Month1, "1M".to_string());

        let features = ExchangeFeatures {
            swap: true,
            option: true,
            fetch_markets: true,
            fetch_ticker: true,
            fetch_trades: true,
            fetch_balance: true,
            create_order: true,
            cancel_order: true,
            cancel_all_orders: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_positions: true,
            fetch_funding_rate: true,
            fetch_funding_rate_history: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            edit_order: true,
            ..Default::default()
        };

        let urls = ExchangeUrls {
            logo: Some(
                "https://github.com/user-attachments/assets/f835b95f-033a-43dd-b6bb-24e698fc498c"
                    .to_string(),
            ),
            api: HashMap::from([
                (
                    "public".to_string(),
                    "https://api.lyra.finance/public".to_string(),
                ),
                (
                    "private".to_string(),
                    "https://api.lyra.finance/private".to_string(),
                ),
            ]),
            www: Some("https://www.derive.xyz/".to_string()),
            doc: vec!["https://docs.derive.xyz/docs/".to_string()],
            fees: Some("https://docs.derive.xyz/reference/fees-1/".to_string()),
        };

        let client = HttpClient::new("https://api.lyra.finance", &config)?;
        let rate_limiter = RateLimiter::new(50);

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

    async fn public_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<&HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        let full_path = format!("/public/{path}");
        let body = params.map(|p| serde_json::to_value(p).unwrap_or(serde_json::json!({})));
        self.client.post(&full_path, body, None).await
    }

    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<&HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        // Derive uses wallet-based authentication
        // api_key = wallet address, api_secret = private key
        let wallet_address =
            self.config
                .api_key()
                .ok_or_else(|| CcxtError::AuthenticationError {
                    message: "Wallet address (api_key) required for private endpoints".to_string(),
                })?;

        let private_key = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Private key (api_secret) required for private endpoints".to_string(),
            })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let signature = self.sign_message(&timestamp, private_key)?;

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        headers.insert("X-LyraWallet".to_string(), wallet_address.to_string());
        headers.insert("X-LyraTimestamp".to_string(), timestamp);
        headers.insert("X-LyraSignature".to_string(), signature);

        let full_path = format!("/private/{path}");
        let body = params.map(|p| serde_json::to_value(p).unwrap_or(serde_json::json!({})));
        self.client.post(&full_path, body, Some(headers)).await
    }

    fn sign_message(&self, message: &str, private_key: &str) -> CcxtResult<String> {
        // Use EvmWallet for EIP-191 personal sign
        let wallet = crate::crypto::EvmWallet::from_private_key(private_key)?;
        let signature = wallet.sign_message_sync(message.as_bytes())?;
        Ok(format!("0x{}", hex::encode(signature.to_bytes())))
    }

    fn safe_symbol(&self, instrument_name: &str) -> String {
        // Convert BTC-PERP to BTC/USD:BTC
        let parts: Vec<&str> = instrument_name.split('-').collect();
        if parts.len() >= 2 {
            let base = parts[0];
            match parts.get(1) {
                Some(&"PERP") => format!("{base}/USD:{base}"),
                Some(quote) => format!("{base}/{quote}"),
                None => instrument_name.to_string(),
            }
        } else {
            instrument_name.to_string()
        }
    }

    fn safe_market_id(&self, symbol: &str) -> String {
        // Convert BTC/USD:BTC to BTC-PERP
        if symbol.contains(':') {
            let parts: Vec<&str> = symbol.split('/').collect();
            if let Some(base) = parts.first() {
                return format!("{base}-PERP");
            }
        }

        symbol.replace('/', "-")
    }

    fn decimal_places(value: &Decimal) -> i32 {
        let s = value.to_string();
        if let Some(pos) = s.find('.') {
            (s.len() - pos - 1) as i32
        } else {
            0
        }
    }

    fn parse_ticker(&self, ticker_data: &DeriveTicker) -> Ticker {
        let instrument_name = ticker_data.instrument_name.as_deref().unwrap_or("");
        let symbol = self.safe_symbol(instrument_name);

        let last = ticker_data
            .last_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let bid = ticker_data
            .best_bid_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let ask = ticker_data
            .best_ask_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let mark = ticker_data
            .mark_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let index = ticker_data
            .index_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let (high, low, base_volume, quote_volume, change) = if let Some(stats) = &ticker_data.stats
        {
            (
                stats.high.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                stats.low.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                stats
                    .volume
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok()),
                stats
                    .volume_usd
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok()),
                stats
                    .price_change
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok()),
            )
        } else {
            (None, None, None, None, None)
        };

        Ticker {
            symbol,
            timestamp: ticker_data.timestamp,
            datetime: ticker_data.timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            high,
            low,
            bid,
            bid_volume: ticker_data
                .best_bid_amount
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok()),
            ask,
            ask_volume: ticker_data
                .best_ask_amount
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok()),
            vwap: None,
            open: None,
            close: last,
            last,
            previous_close: None,
            change,
            percentage: None,
            average: None,
            base_volume,
            quote_volume,
            index_price: index,
            mark_price: mark,
            info: serde_json::to_value(ticker_data).unwrap_or_default(),
        }
    }

    fn parse_trade(&self, trade_data: &DeriveTrade) -> Trade {
        let instrument_name = trade_data.instrument_name.as_deref().unwrap_or("");
        let symbol = self.safe_symbol(instrument_name);

        let price = trade_data
            .price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let amount = trade_data
            .amount
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);

        let side = match trade_data.direction.as_deref() {
            Some("buy") => Some("buy".to_string()),
            Some("sell") => Some("sell".to_string()),
            _ => Some("buy".to_string()),
        };

        Trade {
            id: trade_data.trade_id.clone().unwrap_or_default(),
            order: None,
            timestamp: trade_data.timestamp,
            datetime: trade_data.timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            symbol,
            trade_type: None,
            side,
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: vec![],
            info: serde_json::to_value(trade_data).unwrap_or_default(),
        }
    }

    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status.to_lowercase().as_str() {
            "open" | "untriggered" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "cancelled" | "canceled" => OrderStatus::Canceled,
            "rejected" => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        }
    }

    fn parse_order(&self, order_data: &DeriveOrder) -> Order {
        let instrument_name = order_data.instrument_name.as_deref().unwrap_or("");
        let symbol = self.safe_symbol(instrument_name);

        let price = order_data
            .limit_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let amount = order_data
            .amount
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let filled = order_data
            .filled_amount
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let average = order_data
            .average_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let side = match order_data.direction.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match order_data.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let status = order_data
            .order_status
            .as_deref()
            .map(|s| self.parse_order_status(s))
            .unwrap_or(OrderStatus::Open);

        let remaining = amount - filled;
        let cost = if let Some(avg) = average {
            filled * avg
        } else {
            price.map(|p| filled * p).unwrap_or(Decimal::ZERO)
        };

        Order {
            id: order_data.order_id.clone().unwrap_or_default(),
            client_order_id: None,
            timestamp: order_data.creation_timestamp,
            datetime: order_data.creation_timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: order_data.last_update_timestamp,
            last_update_timestamp: order_data.last_update_timestamp,
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price,
            average,
            amount,
            filled,
            remaining: Some(remaining),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: Some(cost),
            trades: vec![],
            fee: None,
            fees: vec![],
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(order_data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Derive {
    fn id(&self) -> ExchangeId {
        ExchangeId::Derive
    }

    fn name(&self) -> &'static str {
        "Derive"
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
        Some(self.safe_market_id(symbol))
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        Some(self.safe_symbol(market_id))
    }

    fn sign(
        &self,
        path: &str,
        _api: &str,
        method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        // Derive uses POST for all endpoints with JSON body
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        SignedRequest {
            url: format!("https://api.lyra.finance{path}"),
            method: method.to_string(),
            headers,
            body: None,
        }
    }

    async fn load_markets(&self, _reload: bool) -> CcxtResult<HashMap<String, Market>> {
        let params = HashMap::new();
        let response: DeriveResponse<DeriveInstrumentsResult> = self
            .public_post("get_all_instruments", Some(&params))
            .await?;

        let instruments = response
            .result
            .and_then(|r| r.instruments)
            .unwrap_or_default();

        let mut markets = HashMap::new();

        for instrument in instruments {
            let instrument_name = instrument.instrument_name.clone().unwrap_or_default();
            let symbol = self.safe_symbol(&instrument_name);
            let base = instrument
                .base_currency
                .clone()
                .unwrap_or_else(|| instrument_name.split('-').next().unwrap_or("").to_string());
            let quote = instrument
                .quote_currency
                .clone()
                .unwrap_or_else(|| "USD".to_string());

            let tick_size = instrument
                .tick_size
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::new(1, 8));
            let step_size = instrument
                .amount_step
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::new(1, 8));
            let min_amount = instrument
                .minimum_amount
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok());

            let price_precision = Self::decimal_places(&tick_size);
            let amount_precision = Self::decimal_places(&step_size);

            let is_perp = instrument_name.contains("PERP");
            let is_option = instrument.instrument_type.as_deref() == Some("option");

            let taker_fee = instrument
                .taker_fee_rate
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok());
            let maker_fee = instrument
                .maker_fee_rate
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok());

            let market = Market {
                id: instrument_name.clone(),
                lowercase_id: Some(instrument_name.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                settle: Some(quote.clone()),
                settle_id: Some(quote.clone()),
                active: instrument.is_active.unwrap_or(true),
                market_type: if is_option {
                    MarketType::Option
                } else if is_perp {
                    MarketType::Swap
                } else {
                    MarketType::Spot
                },
                spot: false,
                margin: false,
                swap: is_perp,
                future: false,
                option: is_option,
                index: false,
                contract: is_perp || is_option,
                linear: Some(true),
                inverse: Some(false),
                sub_type: Some(if is_perp {
                    "linear".into()
                } else {
                    "option".into()
                }),
                contract_size: Some(Decimal::ONE),
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
            underlying: None,
            underlying_id: None,
                taker: taker_fee,
                maker: maker_fee,
                percentage: true,
                tier_based: false,
                precision: crate::types::MarketPrecision {
                    price: Some(price_precision),
                    amount: Some(amount_precision),
                    cost: None,
                    base: Some(amount_precision),
                    quote: Some(price_precision),
                },
                limits: crate::types::MarketLimits {
                    amount: crate::types::MinMax {
                        min: min_amount,
                        max: None,
                    },
                    price: crate::types::MinMax {
                        min: Some(tick_size),
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
                info: serde_json::to_value(&instrument).unwrap_or_default(),
            };

            markets.insert(symbol.clone(), market);
        }

        // Update internal markets cache
        if let Ok(mut markets_guard) = self.markets.write() {
            *markets_guard = markets.clone();
        }

        if let Ok(mut markets_by_id_guard) = self.markets_by_id.write() {
            for (symbol, market) in &markets {
                markets_by_id_guard.insert(market.id.clone(), symbol.clone());
            }
        }

        Ok(markets)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(false).await?;
        Ok(markets.into_values().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.safe_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("instrument_name".to_string(), market_id);

        let response: DeriveResponse<DeriveTicker> =
            self.public_post("get_ticker", Some(&params)).await?;

        let ticker_data = response.result.ok_or_else(|| CcxtError::BadResponse {
            message: "No ticker data in response".to_string(),
        })?;

        Ok(self.parse_ticker(&ticker_data))
    }

    async fn fetch_order_book(&self, _symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        // Derive does not support orderbook fetching via REST
        Err(CcxtError::NotSupported {
            feature: "fetch_order_book".to_string(),
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.safe_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("instrument_name".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }

        let response: DeriveResponse<DeriveTradesResult> =
            self.public_post("get_trade_history", Some(&params)).await?;

        let trades_data = response.result.and_then(|r| r.trades).unwrap_or_default();

        Ok(trades_data.iter().map(|t| self.parse_trade(t)).collect())
    }

    async fn fetch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        // Derive does not support OHLCV via REST
        Err(CcxtError::NotSupported {
            feature: "fetch_ohlcv".to_string(),
        })
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();
        let response: DeriveResponse<DeriveBalanceResult> =
            self.private_post("get_collaterals", Some(&params)).await?;

        let collaterals = response
            .result
            .and_then(|r| r.collaterals)
            .unwrap_or_default();

        let mut balances = HashMap::new();
        for balance_data in collaterals {
            let currency = balance_data.currency.unwrap_or_default();
            let balance_free = balance_data
                .available
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);
            let balance_locked = balance_data
                .locked
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);
            let balance_total = balance_data
                .amount
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(balance_free + balance_locked);

            balances.insert(
                currency,
                Balance {
                    free: Some(balance_free),
                    used: Some(balance_locked),
                    total: Some(balance_total),
                    debt: None,
                },
            );
        }

        Ok(Balances {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies: balances,
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
        let market_id = self.safe_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("instrument_name".to_string(), market_id);
        params.insert(
            "direction".to_string(),
            match side {
                OrderSide::Buy => "buy".to_string(),
                OrderSide::Sell => "sell".to_string(),
            },
        );
        params.insert(
            "order_type".to_string(),
            match order_type {
                OrderType::Limit => "limit".to_string(),
                OrderType::Market => "market".to_string(),
                _ => "limit".to_string(),
            },
        );
        params.insert("amount".to_string(), amount.to_string());

        if let Some(p) = price {
            params.insert("limit_price".to_string(), p.to_string());
        }

        let response: DeriveResponse<DeriveOrderResult> =
            self.private_post("order", Some(&params)).await?;

        let order_data =
            response
                .result
                .and_then(|r| r.order)
                .ok_or_else(|| CcxtError::BadResponse {
                    message: "No order data in response".to_string(),
                })?;

        Ok(self.parse_order(&order_data))
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("order_id".to_string(), id.to_string());

        let response: DeriveResponse<DeriveOrderResult> =
            self.private_post("cancel", Some(&params)).await?;

        let order_data =
            response
                .result
                .and_then(|r| r.order)
                .ok_or_else(|| CcxtError::OrderNotFound {
                    order_id: id.to_string(),
                })?;

        Ok(self.parse_order(&order_data))
    }

    async fn fetch_order(&self, _id: &str, _symbol: &str) -> CcxtResult<Order> {
        // Derive does not support fetching single order
        Err(CcxtError::NotSupported {
            feature: "fetch_order".to_string(),
        })
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("instrument_name".to_string(), self.safe_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }

        let response: DeriveResponse<DeriveOrdersResult> =
            self.private_post("get_orders", Some(&params)).await?;

        let orders_data = response.result.and_then(|r| r.orders).unwrap_or_default();

        Ok(orders_data.iter().map(|o| self.parse_order(o)).collect())
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("instrument_name".to_string(), self.safe_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }

        let response: DeriveResponse<DeriveOrdersResult> =
            self.private_post("get_open_orders", Some(&params)).await?;

        let orders_data = response.result.and_then(|r| r.orders).unwrap_or_default();

        Ok(orders_data.iter().map(|o| self.parse_order(o)).collect())
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("instrument_name".to_string(), self.safe_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }

        let response: DeriveResponse<DeriveMyTradesResult> = self
            .private_post("get_trade_history", Some(&params))
            .await?;

        let trades_data = response.result.and_then(|r| r.trades).unwrap_or_default();

        Ok(trades_data
            .iter()
            .map(|t| {
                let instrument_name = t.instrument_name.as_deref().unwrap_or("");
                let symbol = self.safe_symbol(instrument_name);

                let price = t
                    .price
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                let amount = t
                    .amount
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);

                let side = match t.direction.as_deref() {
                    Some("buy") => Some("buy".to_string()),
                    Some("sell") => Some("sell".to_string()),
                    _ => Some("buy".to_string()),
                };

                let taker_or_maker = match t.liquidity.as_deref() {
                    Some("maker") | Some("M") => Some(crate::types::TakerOrMaker::Maker),
                    Some("taker") | Some("T") => Some(crate::types::TakerOrMaker::Taker),
                    _ => None,
                };

                let fee_cost = t.fee.as_ref().and_then(|s| Decimal::from_str(s).ok());
                let fee = fee_cost.map(|cost| crate::types::Fee {
                    cost: Some(cost),
                    currency: t.fee_currency.clone(),
                    rate: None,
                });

                Trade {
                    id: t.trade_id.clone().unwrap_or_default(),
                    order: t.order_id.clone(),
                    timestamp: t.timestamp,
                    datetime: t.timestamp.map(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
                    symbol,
                    trade_type: None,
                    side,
                    taker_or_maker,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee,
                    fees: vec![],
                    info: serde_json::to_value(t).unwrap_or_default(),
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_exchange() -> Derive {
        // For Derive: api_key = wallet address, api_secret = private key
        let config = ExchangeConfig::default()
            .with_api_key("0x1234567890abcdef1234567890abcdef12345678")
            .with_api_secret("0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890");
        Derive::new(config).unwrap()
    }

    #[test]
    fn test_derive_creation() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.id(), ExchangeId::Derive);
        assert_eq!(exchange.name(), "Derive");
    }

    #[test]
    fn test_has() {
        let exchange = create_test_exchange();
        let features = exchange.has();
        assert!(features.fetch_markets);
        assert!(features.fetch_ticker);
        assert!(features.fetch_trades);
        assert!(features.fetch_balance);
        assert!(features.create_order);
        assert!(features.cancel_order);
        assert!(features.fetch_orders);
        assert!(features.fetch_open_orders);
        assert!(features.fetch_my_trades);
    }

    #[test]
    fn test_timeframes() {
        let exchange = create_test_exchange();
        let timeframes = exchange.timeframes();
        assert!(timeframes.contains_key(&Timeframe::Minute1));
        assert!(timeframes.contains_key(&Timeframe::Hour1));
        assert!(timeframes.contains_key(&Timeframe::Day1));
    }

    #[test]
    fn test_urls() {
        let exchange = create_test_exchange();
        let urls = exchange.urls();
        assert!(urls.api.contains_key("public"));
        assert!(urls.api.contains_key("private"));
        assert!(urls.www.is_some());
    }

    #[test]
    fn test_safe_symbol() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.safe_symbol("BTC-PERP"), "BTC/USD:BTC");
        assert_eq!(exchange.safe_symbol("ETH-PERP"), "ETH/USD:ETH");
        assert_eq!(exchange.safe_symbol("BTC-USD"), "BTC/USD");
    }

    #[test]
    fn test_safe_market_id() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.safe_market_id("BTC/USD:BTC"), "BTC-PERP");
        assert_eq!(exchange.safe_market_id("ETH/USD:ETH"), "ETH-PERP");
    }

    #[test]
    fn test_parse_order_status() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.parse_order_status("open"), OrderStatus::Open);
        assert_eq!(exchange.parse_order_status("filled"), OrderStatus::Closed);
        assert_eq!(
            exchange.parse_order_status("cancelled"),
            OrderStatus::Canceled
        );
        assert_eq!(
            exchange.parse_order_status("rejected"),
            OrderStatus::Rejected
        );
    }

    #[test]
    fn test_exchange_info() {
        let exchange = create_test_exchange();
        assert!(exchange.urls().logo.is_some());
        assert!(!exchange.urls().doc.is_empty());
    }
}
