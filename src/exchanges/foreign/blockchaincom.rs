//! Blockchain.com Exchange Implementation
//!
//! CCXT blockchaincom.ts를 Rust로 포팅

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, Ticker, Trade, Transaction, TransactionStatus, TransactionType,
};

/// Blockchain.com 거래소
pub struct BlockchainCom {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
}

impl BlockchainCom {
    const BASE_URL: &'static str = "https://api.blockchain.com/v3/exchange";
    const RATE_LIMIT_MS: u64 = 500;

    /// 새 Blockchain.com 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
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
            fetch_trades: false,
            fetch_ohlcv: false,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            create_stop_order: true,
            create_stop_limit_order: true,
            create_stop_market_order: true,
            cancel_order: true,
            cancel_all_orders: true,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_canceled_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            fetch_deposit_address: true,
            withdraw: true,
            fetch_trading_fees: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/user-attachments/assets/975e3054-3399-4363-bcee-ec3c6d63d4e8".into()),
            api: api_urls,
            www: Some("https://blockchain.com".into()),
            doc: vec!["https://api.blockchain.com/v3".into()],
            fees: Some("https://exchange.blockchain.com/fees".into()),
        };

        Ok(Self {
            config,
            public_client,
            private_client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
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

        self.public_client.get(&url, None, None).await
    }

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "X-API-Token required".into(),
        })?;

        let mut headers = HashMap::new();
        headers.insert("X-API-Token".into(), secret.to_string());

        match method {
            "GET" => {
                let url = if !params.is_empty() {
                    let query: String = params
                        .iter()
                        .map(|(k, v)| {
                            let val = match v {
                                serde_json::Value::String(s) => s.clone(),
                                _ => v.to_string(),
                            };
                            format!("{}={}", k, urlencoding::encode(&val))
                        })
                        .collect::<Vec<_>>()
                        .join("&");
                    format!("{path}?{query}")
                } else {
                    path.to_string()
                };
                self.private_client.get(&url, None, Some(headers)).await
            }
            "POST" => {
                headers.insert("Content-Type".into(), "application/json".into());
                let body = if !params.is_empty() {
                    Some(serde_json::to_value(&params).unwrap_or_default())
                } else {
                    None
                };
                self.private_client.post(path, body, Some(headers)).await
            }
            "DELETE" => {
                let url = if !params.is_empty() {
                    let query: String = params
                        .iter()
                        .map(|(k, v)| {
                            let val = match v {
                                serde_json::Value::String(s) => s.clone(),
                                _ => v.to_string(),
                            };
                            format!("{}={}", k, urlencoding::encode(&val))
                        })
                        .collect::<Vec<_>>()
                        .join("&");
                    format!("{path}?{query}")
                } else {
                    path.to_string()
                };
                self.private_client.delete(&url, None, Some(headers)).await
            }
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USD → BTC-USD)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// 마켓 ID → 심볼 변환 (BTC-USD → BTC/USD)
    fn to_symbol(&self, market_id: &str) -> String {
        market_id.replace("-", "/")
    }

    /// precision 계산 (scale에서 실제 값으로)
    fn parse_precision(&self, scale_str: &str) -> Decimal {
        if let Ok(scale) = scale_str.parse::<i32>() {
            Decimal::new(1, scale as u32)
        } else {
            Decimal::new(1, 8)
        }
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &BlockchainComTicker, _market: Option<&Market>) -> Ticker {
        let symbol = self.to_symbol(&data.symbol);

        Ticker {
            symbol,
            timestamp: None,
            datetime: None,
            high: None,
            low: None,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: data.price_24h,
            close: None,
            last: data.last_trade_price,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume_24h,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 상태 파싱
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status {
            "OPEN" => OrderStatus::Open,
            "PART_FILLED" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" => OrderStatus::Canceled,
            "REJECTED" => OrderStatus::Rejected,
            "EXPIRED" => OrderStatus::Expired,
            _ => OrderStatus::Open,
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &BlockchainComOrder, _market: Option<&Market>) -> Order {
        let symbol = self.to_symbol(&data.symbol);
        let status = self.parse_order_status(&data.ord_status);

        let order_type = match data.ord_type.to_uppercase().as_str() {
            "MARKET" => OrderType::Market,
            "LIMIT" => OrderType::Limit,
            "STOP" => OrderType::StopMarket,
            "STOPLIMIT" => OrderType::StopLimit,
            _ => OrderType::Limit,
        };

        let side = match data.side.to_uppercase().as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price = if order_type != OrderType::Market {
            data.price.as_ref().and_then(|p| p.parse().ok())
        } else {
            None
        };

        let average: Option<Decimal> = data.avg_px.as_ref().and_then(|a| a.parse().ok());
        let filled: Decimal = data.cum_qty.as_ref().and_then(|f| f.parse().ok()).unwrap_or_default();
        let remaining: Decimal = data.leaves_qty.as_ref().and_then(|r| r.parse().ok()).unwrap_or_default();
        let amount = filled + remaining;

        let cost = average.and_then(|avg| {
            if filled > Decimal::ZERO {
                Some(avg * filled)
            } else {
                None
            }
        });

        Order {
            id: data.ex_ord_id.to_string(),
            client_order_id: Some(data.cl_ord_id.clone()),
            timestamp: Some(data.timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
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
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: Some(false),
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 거래 내역 파싱
    fn parse_trade(&self, data: &BlockchainComTrade, _market: Option<&Market>) -> Trade {
        let symbol = self.to_symbol(&data.symbol);
        let side = match data.side.to_uppercase().as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.qty.parse().unwrap_or_default();
        let cost = price * amount;

        let fee_cost: Option<Decimal> = data.fee.as_ref().and_then(|f| f.parse().ok());
        let fee = fee_cost.map(|cost| crate::types::Fee {
            cost: Some(cost),
            currency: None, // Quote currency, determined by caller
            rate: None,
        });

        Trade {
            id: data.trade_id.to_string(),
            order: Some(data.ex_ord_id.to_string()),
            timestamp: Some(data.timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol,
            trade_type: None,
            side: Some(format!("{:?}", side).to_lowercase()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(cost),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 거래소 상태 파싱
    fn parse_transaction_status(&self, state: &str) -> TransactionStatus {
        match state {
            "COMPLETED" => TransactionStatus::Ok,
            "REJECTED" => TransactionStatus::Failed,
            "PENDING" => TransactionStatus::Pending,
            "FAILED" => TransactionStatus::Failed,
            "REFUNDED" => TransactionStatus::Canceled,
            _ => TransactionStatus::Pending,
        }
    }

    /// 입출금 내역 파싱
    fn parse_transaction(&self, data: &BlockchainComTransaction) -> Transaction {
        let (tx_type, id) = if let Some(deposit_id) = &data.deposit_id {
            (TransactionType::Deposit, deposit_id.clone())
        } else if let Some(withdrawal_id) = &data.withdrawal_id {
            (TransactionType::Withdrawal, withdrawal_id.clone())
        } else {
            (TransactionType::Deposit, String::new())
        };

        let status = self.parse_transaction_status(&data.state);
        let amount: Decimal = data.amount.parse().unwrap_or_default();

        let fee = if tx_type == TransactionType::Withdrawal {
            data.fee.as_ref().and_then(|f| f.parse::<Decimal>().ok()).map(|cost| {
                crate::types::Fee {
                    cost: Some(cost),
                    currency: Some(data.currency.clone()),
                    rate: None,
                }
            })
        } else {
            None
        };

        Transaction {
            id,
            timestamp: Some(data.timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            updated: None,
            tx_type,
            currency: data.currency.clone(),
            network: None,
            amount,
            status,
            address: data.address.clone(),
            tag: None,
            txid: data.tx_hash.clone(),
            fee,
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[BlockchainComBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free: Option<Decimal> = b.available.parse().ok();
            let total: Option<Decimal> = b.balance.parse().ok();
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
            result.add(&b.currency, balance);
        }

        result
    }
}

#[async_trait]
impl Exchange for BlockchainCom {
    fn id(&self) -> ExchangeId {
        ExchangeId::BlockchainCom
    }

    fn name(&self) -> &str {
        "Blockchain.com"
    }

    fn version(&self) -> &str {
        "v3"
    }

    fn countries(&self) -> &[&str] {
        &["LX"] // Luxembourg
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

    fn timeframes(&self) -> &HashMap<crate::types::Timeframe, String> {
        // OHLCV not supported, return empty static map
        static EMPTY: std::sync::OnceLock<HashMap<crate::types::Timeframe, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
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
        #[derive(Deserialize, Serialize)]
        struct MarketInfo {
            base_currency: String,
            #[serde(default)]
            base_currency_scale: i32,
            counter_currency: String,
            #[serde(default)]
            counter_currency_scale: i32,
            #[serde(default)]
            min_price_increment: String,
            #[serde(default)]
            min_price_increment_scale: i32,
            #[serde(default)]
            min_order_size: String,
            #[serde(default)]
            min_order_size_scale: i32,
            #[serde(default)]
            max_order_size: String,
            #[serde(default)]
            max_order_size_scale: i32,
            #[serde(default)]
            lot_size: String,
            #[serde(default)]
            lot_size_scale: i32,
            status: String,
            #[serde(default)]
            id: i64,
        }

        let response: HashMap<String, MarketInfo> = self.public_get("/symbols", None).await?;

        let mut markets = Vec::new();
        for (market_id, info) in response {
            let base = info.base_currency.clone();
            let quote = info.counter_currency.clone();
            let symbol = format!("{}/{}", base, quote);
            let active = info.status == "open";

            // Calculate precisions (not used in current implementation, but available for future use)
            let min_price_increment: Decimal = info.min_price_increment.parse().unwrap_or(Decimal::new(1, 8));
            let min_price_scale = self.parse_precision(&info.min_price_increment_scale.to_string());
            let _price_precision = min_price_increment * min_price_scale;

            let lot_size: Decimal = info.lot_size.parse().unwrap_or(Decimal::new(1, 8));
            let lot_size_scale = self.parse_precision(&info.lot_size_scale.to_string());
            let _amount_precision = lot_size * lot_size_scale;

            let min_order_size: Decimal = info.min_order_size.parse().unwrap_or_default();
            let min_order_scale = self.parse_precision(&info.min_order_size_scale.to_string());
            let min_amount = min_order_size * min_order_scale;

            let max_order_size: Decimal = info.max_order_size.parse().unwrap_or_default();
            let max_amount = if max_order_size > Decimal::ZERO {
                let max_order_scale = self.parse_precision(&info.max_order_size_scale.to_string());
                Some(max_order_size * max_order_scale)
            } else {
                None
            };

            let market = Market {
                id: market_id,
                lowercase_id: None,
                symbol,
                base,
                quote,
                settle: None,
                base_id: info.base_currency.clone(),
                quote_id: info.counter_currency.clone(),
                settle_id: None,
                market_type: MarketType::Spot,
                spot: true,
                margin: false,
                swap: false,
                future: false,
                option: false,
                index: false,
                active,
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: Some(Decimal::new(4, 3)), // 0.004
                maker: Some(Decimal::new(4, 3)), // 0.004
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(info.base_currency_scale),
                    price: Some(info.counter_currency_scale),
                    cost: None,
                    base: Some(info.base_currency_scale),
                    quote: Some(info.counter_currency_scale),
                },
                limits: MarketLimits {
                    leverage: crate::types::MinMax::default(),
                    amount: crate::types::MinMax {
                        min: Some(min_amount),
                        max: max_amount,
                    },
                    price: crate::types::MinMax::default(),
                    cost: crate::types::MinMax::default(),
                },
                margin_modes: None,
                created: None,
                info: serde_json::json!(info),
                tier_based: false,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let path = format!("/tickers/{}", market_id);
        let response: BlockchainComTicker = self.public_get(&path, None).await?;

        Ok(self.parse_ticker(&response, None))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        let response: Vec<BlockchainComTicker> = self.public_get("/tickers", None).await?;

        let mut tickers = HashMap::new();
        for ticker_data in response {
            let ticker = self.parse_ticker(&ticker_data, None);
            if let Some(syms) = symbols {
                if syms.iter().any(|&s| s == ticker.symbol) {
                    tickers.insert(ticker.symbol.clone(), ticker);
                }
            } else {
                tickers.insert(ticker.symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let mut params = HashMap::new();
        if let Some(lim) = limit {
            params.insert("depth".into(), lim.to_string());
        }

        let path = format!("/l3/{}", market_id);
        let response: BlockchainComOrderBook = self.public_get(&path, Some(params)).await?;

        let mut bids = Vec::new();
        for bid in response.bids {
            if let (Ok(price), Ok(amount)) = (bid.px.parse::<Decimal>(), bid.qty.parse::<Decimal>()) {
                bids.push(OrderBookEntry { price, amount });
            }
        }

        let mut asks = Vec::new();
        for ask in response.asks {
            if let (Ok(price), Ok(amount)) = (ask.px.parse::<Decimal>(), ask.qty.parse::<Decimal>()) {
                asks.push(OrderBookEntry { price, amount });
            }
        }

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp: None,
            datetime: None,
            nonce: None,
        })
    }

    async fn fetch_trades(&self, _symbol: &str, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        Err(CcxtError::NotSupported {
            feature: "fetch_trades not supported by Blockchain.com".into(),
        })
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        self.load_markets(false).await?;

        #[derive(Deserialize)]
        struct BalanceResponse {
            primary: Vec<BlockchainComBalance>,
        }

        let response: BalanceResponse = self.private_request("GET", "/accounts", HashMap::new()).await?;

        Ok(self.parse_balance(&response.primary))
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

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let ord_type = match order_type {
            OrderType::Market => "MARKET",
            OrderType::Limit => "LIMIT",
            OrderType::StopMarket => "STOP",
            OrderType::StopLimit => "STOPLIMIT",
            _ => "LIMIT",
        };

        let side_str = match side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };

        let client_order_id = format!("{}", Utc::now().timestamp_nanos_opt().unwrap_or(0) % 1_000_000_000_000_000);

        let mut params = HashMap::new();
        params.insert("ordType".into(), serde_json::Value::String(ord_type.into()));
        params.insert("symbol".into(), serde_json::Value::String(market_id));
        params.insert("side".into(), serde_json::Value::String(side_str.into()));
        params.insert("orderQty".into(), serde_json::Value::String(amount.to_string()));
        params.insert("clOrdId".into(), serde_json::Value::String(client_order_id));

        if ord_type == "LIMIT" || ord_type == "STOPLIMIT" {
            if let Some(p) = price {
                params.insert("price".into(), serde_json::Value::String(p.to_string()));
            } else {
                return Err(CcxtError::ArgumentsRequired {
                    message: "price required for limit orders".into(),
                });
            }
        }

        let response: BlockchainComOrder = self.private_request("POST", "/orders", params).await?;

        Ok(self.parse_order(&response, None))
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("/orders/{}", id);
        let response: serde_json::Value = self.private_request("DELETE", &path, HashMap::new()).await?;

        Ok(Order {
            id: id.to_string(),
            status: OrderStatus::Canceled,
            info: response,
            ..Default::default()
        })
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        self.load_markets(false).await?;

        let path = format!("/orders/{}", id);
        let response: BlockchainComOrder = self.private_request("GET", &path, HashMap::new()).await?;

        Ok(self.parse_order(&response, None))
    }

    async fn fetch_orders(&self, _symbol: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        Err(CcxtError::NotSupported {
            feature: "fetch_orders not supported, use fetch_open_orders or fetch_closed_orders".into(),
        })
    }

    async fn fetch_open_orders(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        params.insert("status".into(), serde_json::Value::String("OPEN".into()));
        if let Some(lim) = limit {
            params.insert("limit".into(), serde_json::Value::Number(lim.into()));
        } else {
            params.insert("limit".into(), serde_json::Value::Number(100.into()));
        }

        if let Some(sym) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                markets
                    .get(sym)
                    .map(|m| m.id.clone())
                    .ok_or_else(|| CcxtError::BadSymbol {
                        symbol: sym.to_string(),
                    })?
            };
            params.insert("symbol".into(), serde_json::Value::String(market_id));
        }

        let response: Vec<BlockchainComOrder> = self.private_request("GET", "/orders", params).await?;

        Ok(response.iter().map(|o| self.parse_order(o, None)).collect())
    }

    async fn fetch_closed_orders(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        params.insert("status".into(), serde_json::Value::String("FILLED".into()));
        if let Some(lim) = limit {
            params.insert("limit".into(), serde_json::Value::Number(lim.into()));
        } else {
            params.insert("limit".into(), serde_json::Value::Number(100.into()));
        }

        if let Some(sym) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                markets
                    .get(sym)
                    .map(|m| m.id.clone())
                    .ok_or_else(|| CcxtError::BadSymbol {
                        symbol: sym.to_string(),
                    })?
            };
            params.insert("symbol".into(), serde_json::Value::String(market_id));
        }

        let response: Vec<BlockchainComOrder> = self.private_request("GET", "/orders", params).await?;

        Ok(response.iter().map(|o| self.parse_order(o, None)).collect())
    }

    async fn fetch_my_trades(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        if let Some(lim) = limit {
            params.insert("limit".into(), serde_json::Value::Number(lim.into()));
        }

        if let Some(sym) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                markets
                    .get(sym)
                    .map(|m| m.id.clone())
                    .ok_or_else(|| CcxtError::BadSymbol {
                        symbol: sym.to_string(),
                    })?
            };
            params.insert("symbol".into(), serde_json::Value::String(market_id));
        }

        let response: Vec<BlockchainComTrade> = self.private_request("GET", "/fills", params).await?;

        Ok(response.iter().map(|t| self.parse_trade(t, None)).collect())
    }

    async fn fetch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: crate::types::Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::OHLCV>> {
        Err(CcxtError::NotSupported {
            feature: "fetch_ohlcv not supported by Blockchain.com".into(),
        })
    }

    async fn fetch_deposits(&self, _code: Option<&str>, since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Transaction>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        if let Some(s) = since {
            params.insert("from".into(), serde_json::Value::Number(s.into()));
        }

        let response: Vec<BlockchainComTransaction> = self.private_request("GET", "/deposits", params).await?;

        Ok(response.iter().map(|t| self.parse_transaction(t)).collect())
    }

    async fn fetch_withdrawals(&self, _code: Option<&str>, since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Transaction>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        if let Some(s) = since {
            params.insert("from".into(), serde_json::Value::Number(s.into()));
        }

        let response: Vec<BlockchainComTransaction> = self.private_request("GET", "/withdrawals", params).await?;

        Ok(response.iter().map(|t| self.parse_transaction(t)).collect())
    }

    async fn withdraw(
        &self,
        _code: &str,
        _amount: Decimal,
        _address: &str,
        _tag: Option<&str>,
        _network: Option<&str>,
    ) -> CcxtResult<Transaction> {
        Err(CcxtError::NotSupported {
            feature: "withdraw requires beneficiary ID - use exchange API directly".into(),
        })
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        let markets = self.markets.read().unwrap();
        markets.get(symbol).map(|m| m.id.clone())
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
    ) -> crate::types::SignedRequest {
        // Simple implementation - actual signing is done in private_request
        let mut url = format!("{}{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();

        if let Some(api_key) = self.config.api_key() {
            headers.insert("X-API-Token".into(), api_key.to_string());
        }

        if method == "GET" && !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{}?{}", url, query);
        }

        crate::types::SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        }
    }
}

// Response structures
#[derive(Debug, Deserialize, Serialize)]
struct BlockchainComTicker {
    symbol: String,
    #[serde(default)]
    price_24h: Option<Decimal>,
    #[serde(default)]
    volume_24h: Option<Decimal>,
    #[serde(default)]
    last_trade_price: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BlockchainComOrderBook {
    bids: Vec<BlockchainComOrderBookEntry>,
    asks: Vec<BlockchainComOrderBookEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BlockchainComOrderBookEntry {
    px: String,
    qty: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlockchainComOrder {
    #[serde(default)]
    ex_ord_id: i64,
    cl_ord_id: String,
    ord_type: String,
    ord_status: String,
    side: String,
    symbol: String,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    avg_px: Option<String>,
    timestamp: i64,
    #[serde(default)]
    cum_qty: Option<String>,
    #[serde(default)]
    leaves_qty: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlockchainComTrade {
    #[serde(default)]
    ex_ord_id: i64,
    #[serde(default)]
    trade_id: i64,
    side: String,
    symbol: String,
    price: String,
    qty: String,
    #[serde(default)]
    fee: Option<String>,
    timestamp: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlockchainComBalance {
    currency: String,
    balance: String,
    available: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlockchainComTransaction {
    #[serde(default)]
    deposit_id: Option<String>,
    #[serde(default)]
    withdrawal_id: Option<String>,
    amount: String,
    currency: String,
    state: String,
    timestamp: i64,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    tx_hash: Option<String>,
    #[serde(default)]
    fee: Option<String>,
}
