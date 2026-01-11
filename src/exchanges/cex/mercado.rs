//! Mercado Bitcoin Exchange Implementation
//!
//! Brazilian cryptocurrency exchange supporting BRL trading pairs

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

#[allow(dead_code)]
type HmacSha512 = Hmac<Sha512>;

/// Mercado Bitcoin exchange
pub struct Mercado {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Mercado {
    const BASE_URL: &'static str = "https://api.mercadobitcoin.net/api/v4";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

    /// Create new Mercado instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            fetch_markets: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: false,
            cancel_order: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), format!("{}/tapi", Self::BASE_URL));

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27837060-e7c58714-60ea-11e7-9192-f05e86adb83f.jpg".into()),
            api: api_urls,
            www: Some("https://www.mercadobitcoin.com.br/".into()),
            doc: vec!["https://www.mercadobitcoin.com.br/api-doc/".into()],
            fees: Some("https://www.mercadobitcoin.com.br/taxas/".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());

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

    /// Public API call
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Private API call
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Option<HashMap<String, String>>,
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
                message: "Secret required".into(),
            })?;

        let nonce = Utc::now().timestamp_millis().to_string();

        let mut post_params = params.unwrap_or_default();
        post_params.insert("tapi_method".into(), method.to_string());
        post_params.insert("tapi_nonce".into(), nonce);

        let query_string: String = post_params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let path = "/tapi/v3/";
        let message = format!("{path}?{query_string}");

        let mut mac = HmacSha512::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(message.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("TAPI-ID".into(), api_key.to_string());
        headers.insert("TAPI-MAC".into(), signature);
        headers.insert(
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into(),
        );

        self.client
            .post_form("/tapi/v3/", &post_params, Some(headers))
            .await
    }

    /// Convert symbol to market ID (BTC/BRL -> BTC)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('/').collect();
        if !parts.is_empty() {
            parts[0].to_uppercase()
        } else {
            symbol.to_uppercase()
        }
    }
}

#[async_trait]
impl Exchange for Mercado {
    fn id(&self) -> ExchangeId {
        ExchangeId::Mercado
    }

    fn name(&self) -> &str {
        "Mercado Bitcoin"
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
            let cache = self.markets.read().unwrap();
            if !reload && !cache.is_empty() {
                return Ok(cache.clone());
            }
        }

        let markets = self.fetch_markets().await?;
        let mut result = HashMap::new();

        let mut cache = self.markets.write().unwrap();
        let mut by_id = self.markets_by_id.write().unwrap();

        for market in markets {
            result.insert(market.symbol.clone(), market.clone());
            cache.insert(market.symbol.clone(), market.clone());
            by_id.insert(market.id.clone(), market.symbol.clone());
        }

        Ok(result)
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        let cache = self.markets.read().unwrap();
        cache.get(symbol).map(|m| m.id.clone())
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let by_id = self.markets_by_id.read().unwrap();
        by_id.get(market_id).cloned()
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
        SignedRequest {
            url: path.to_string(),
            method: method.to_string(),
            headers: HashMap::new(),
            body: None,
        }
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        // Mercado Bitcoin supports BRL pairs
        let pairs = vec![
            ("BTC", "BTC", "BRL"),
            ("ETH", "ETH", "BRL"),
            ("LTC", "LTC", "BRL"),
            ("XRP", "XRP", "BRL"),
            ("BCH", "BCH", "BRL"),
            ("USDC", "USDC", "BRL"),
            ("USDT", "USDT", "BRL"),
            ("ADA", "ADA", "BRL"),
            ("SOL", "SOL", "BRL"),
            ("DOT", "DOT", "BRL"),
            ("AVAX", "AVAX", "BRL"),
            ("LINK", "LINK", "BRL"),
            ("UNI", "UNI", "BRL"),
            ("MATIC", "MATIC", "BRL"),
            ("DOGE", "DOGE", "BRL"),
        ];

        let mut markets = Vec::new();
        for (id, base, quote) in pairs {
            let symbol = format!("{base}/{quote}");
            let market = Market {
                id: id.to_string(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.to_string(),
                quote: quote.to_string(),
                base_id: id.to_string(),
                quote_id: quote.to_string(),
                market_type: MarketType::Spot,
                spot: true,
                margin: false,
                swap: false,
                future: false,
                option: false,
                index: false,
                active: true,
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: Some(Decimal::new(7, 3)), // 0.007
                maker: Some(Decimal::new(3, 3)), // 0.003
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                settle: None,
                settle_id: None,
                precision: MarketPrecision {
                    amount: Some(8),
                    price: Some(2),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::json!({"id": id, "base": base, "quote": quote}),
                tier_based: false,
                percentage: true,
            };
            markets.push(market);
        }

        // Cache markets
        let mut cache = self.markets.write().unwrap();
        let mut by_id = self.markets_by_id.write().unwrap();
        for market in &markets {
            cache.insert(market.symbol.clone(), market.clone());
            by_id.insert(market.id.clone(), market.symbol.clone());
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/{market_id}/ticker");

        let response: MercadoTicker = self.public_get(&path, None).await?;
        let timestamp = response
            .date
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: response.high,
            low: response.low,
            bid: response.buy,
            bid_volume: None,
            ask: response.sell,
            ask_volume: None,
            vwap: None,
            open: None,
            close: response.last,
            last: response.last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: response.vol,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/{market_id}/orderbook");

        let response: MercadoOrderBook = self.public_get(&path, None).await?;
        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<Vec<Decimal>>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().filter_map(|e| {
                if e.len() >= 2 {
                    Some(OrderBookEntry {
                        price: e[0],
                        amount: e[1],
                    })
                } else {
                    None
                }
            });
            if let Some(l) = limit {
                iter.take(l as usize).collect()
            } else {
                iter.collect()
            }
        };

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            checksum: None,
            bids: parse_entries(&response.bids),
            asks: parse_entries(&response.asks),
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/{market_id}/trades");

        let response: Vec<MercadoTrade> = self.public_get(&path, None).await?;

        let limit = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = response
            .iter()
            .take(limit)
            .map(|t| {
                let timestamp = t.date * 1000;
                Trade {
                    id: t.tid.to_string(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(t.trade_type.clone()),
                    taker_or_maker: None,
                    price: t.price,
                    amount: t.amount,
                    cost: None,
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
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.convert_to_market_id(symbol);
        let timeframe_str =
            self.timeframes
                .get(&timeframe)
                .ok_or_else(|| CcxtError::NotSupported {
                    feature: format!("Timeframe {timeframe:?}"),
                })?;

        let path = format!("/{market_id}/candles");
        let mut params = HashMap::new();
        params.insert("resolution".into(), timeframe_str.clone());
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<MercadoCandle> = self.public_get(&path, Some(params)).await?;

        let candles: Vec<OHLCV> = response
            .iter()
            .map(|c| OHLCV {
                timestamp: c.timestamp,
                open: c.open,
                high: c.high,
                low: c.low,
                close: c.close,
                volume: c.volume,
            })
            .collect();

        Ok(candles)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: MercadoBalance = self.private_request("get_account_info", None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        if let Some(balance_map) = response.balance {
            for (currency, info) in balance_map {
                let free = info.available.unwrap_or_default();
                let total = info.total.unwrap_or_default();
                let used = total - free;

                balances.currencies.insert(
                    currency.to_uppercase(),
                    Balance {
                        free: Some(free),
                        used: Some(used),
                        total: Some(total),
                        debt: None,
                    },
                );
            }
        }

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
        if order_type == OrderType::Market {
            return Err(CcxtError::NotSupported {
                feature: "Market orders".to_string(),
            });
        }

        let market_id = self.convert_to_market_id(symbol);
        let price = price.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Price is required for limit orders".into(),
        })?;

        let method = match side {
            OrderSide::Buy => "place_buy_order",
            OrderSide::Sell => "place_sell_order",
        };

        let mut params = HashMap::new();
        params.insert("coin_pair".into(), format!("BRL{market_id}"));
        params.insert("quantity".into(), amount.to_string());
        params.insert("limit_price".into(), price.to_string());

        let response: MercadoOrderResponse = self.private_request(method, Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: response.order_id.map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price: Some(price),
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&response).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("coin_pair".into(), format!("BRL{market_id}"));
        params.insert("order_id".into(), id.to_string());

        let _response: MercadoOrderResponse =
            self.private_request("cancel_order", Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: Some(timestamp),
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
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::Value::Null,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("coin_pair".into(), format!("BRL{market_id}"));
        params.insert("order_id".into(), id.to_string());

        let response: MercadoOrderDetail = self.private_request("get_order", Some(params)).await?;

        let timestamp = response
            .created_timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let status = match response.status {
            Some(2) => OrderStatus::Open,
            Some(3) => OrderStatus::Canceled,
            Some(4) => OrderStatus::Closed,
            _ => OrderStatus::Open,
        };

        let side = match response.order_type {
            Some(1) => OrderSide::Buy,
            Some(2) => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let amount = response.quantity.unwrap_or_default();
        let filled = response.executed_quantity.unwrap_or_default();

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side,
            price: response.limit_price,
            average: None,
            amount,
            filled,
            remaining: Some(amount - filled),
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&response).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let market_id = symbol
            .map(|s| self.convert_to_market_id(s))
            .unwrap_or("BTC".into());
        let symbol_str = symbol.unwrap_or("BTC/BRL");

        let mut params = HashMap::new();
        params.insert("coin_pair".into(), format!("BRL{market_id}"));
        params.insert("status_list".into(), "[2]".into()); // Open orders

        let response: MercadoOrderList = self.private_request("list_orders", Some(params)).await?;

        let orders: Vec<Order> = response
            .orders
            .unwrap_or_default()
            .iter()
            .map(|o| {
                let timestamp = o
                    .created_timestamp
                    .unwrap_or_else(|| Utc::now().timestamp_millis());
                let side = match o.order_type {
                    Some(1) => OrderSide::Buy,
                    Some(2) => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                let amount = o.quantity.unwrap_or_default();
                let filled = o.executed_quantity.unwrap_or_default();

                Order {
                    id: o.order_id.map(|i| i.to_string()).unwrap_or_default(),
                    client_order_id: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    last_trade_timestamp: None,
                    last_update_timestamp: None,
                    status: OrderStatus::Open,
                    symbol: symbol_str.to_string(),
                    order_type: OrderType::Limit,
                    time_in_force: None,
                    side,
                    price: o.limit_price,
                    average: None,
                    amount,
                    filled,
                    remaining: Some(amount - filled),
                    cost: None,
                    trades: Vec::new(),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(o).unwrap_or_default(),
                    stop_price: None,
                    trigger_price: None,
                    take_profit_price: None,
                    stop_loss_price: None,
                    reduce_only: None,
                    post_only: None,
                }
            })
            .collect();

        Ok(orders)
    }
}

// === Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct MercadoTicker {
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    buy: Option<Decimal>,
    #[serde(default)]
    sell: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    vol: Option<Decimal>,
    #[serde(default)]
    date: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MercadoOrderBook {
    #[serde(default)]
    bids: Vec<Vec<Decimal>>,
    #[serde(default)]
    asks: Vec<Vec<Decimal>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct MercadoTrade {
    #[serde(default)]
    tid: i64,
    #[serde(default)]
    date: i64,
    #[serde(default)]
    price: Decimal,
    #[serde(default)]
    amount: Decimal,
    #[serde(default, rename = "type")]
    trade_type: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct MercadoCandle {
    #[serde(default)]
    timestamp: i64,
    #[serde(default)]
    open: Decimal,
    #[serde(default)]
    high: Decimal,
    #[serde(default)]
    low: Decimal,
    #[serde(default)]
    close: Decimal,
    #[serde(default)]
    volume: Decimal,
}

#[derive(Debug, Deserialize)]
struct MercadoBalance {
    #[serde(default)]
    balance: Option<HashMap<String, MercadoBalanceInfo>>,
}

#[derive(Debug, Deserialize)]
struct MercadoBalanceInfo {
    #[serde(default)]
    available: Option<Decimal>,
    #[serde(default)]
    total: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct MercadoOrderResponse {
    #[serde(default)]
    order_id: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct MercadoOrderDetail {
    #[serde(default)]
    order_id: Option<i64>,
    #[serde(default)]
    created_timestamp: Option<i64>,
    #[serde(default)]
    order_type: Option<i32>,
    #[serde(default)]
    limit_price: Option<Decimal>,
    #[serde(default)]
    quantity: Option<Decimal>,
    #[serde(default)]
    executed_quantity: Option<Decimal>,
    #[serde(default)]
    status: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct MercadoOrderList {
    #[serde(default)]
    orders: Option<Vec<MercadoOrderDetail>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Mercado::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Mercado);
        assert_eq!(exchange.name(), "Mercado Bitcoin");
        assert!(exchange.has().spot);
        assert!(exchange.has().fetch_ohlcv);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Mercado::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/BRL"), "BTC");
        assert_eq!(exchange.convert_to_market_id("ETH/BRL"), "ETH");
    }
}
