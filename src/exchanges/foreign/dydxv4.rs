//! dYdX v4 Exchange Implementation
//!
//! dYdX v4 is a decentralized perpetual futures DEX built on Cosmos SDK + CometBFT.
//! This is the sovereign L1 blockchain version (not the StarkEx L2 version).
//!
//! # Architecture
//!
//! - **Indexer API**: Public REST API for market data, positions, orders
//! - **Node API**: Cosmos SDK transactions for trading (requires wallet)
//!
//! # References
//!
//! - [dYdX v4 Documentation](https://docs.dydx.xyz/)
//! - [dYdX v4 Clients](https://github.com/dydxprotocol/v4-clients)

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::crypto::cosmos::{CosmosWallet, DYDX_MAINNET, DYDX_TESTNET};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balances, Balance, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderStatus, OrderType, SignedRequest, Ticker, TimeInForce, Timeframe, Trade, OHLCV,
};

use super::dydxv4_order::{
    DydxOrder, DydxOrderSide, DydxTimeInForce, DydxMarketInfo, DydxTransactionBuilder,
    DydxNodeClient, OrderId, SubaccountId, GoodTilOneof, ConditionType,
    order_flags, generate_client_id, SHORT_BLOCK_WINDOW,
};

/// dYdX v4 거래소
///
/// Cosmos SDK 기반 탈중앙화 영구 선물 DEX
pub struct DydxV4 {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    /// 마켓 정보 캐시 (quantums/subticks 변환용)
    market_info: RwLock<HashMap<String, DydxMarketInfo>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    /// 테스트넷 모드
    testnet: bool,
    /// 지갑 주소 (dydx 접두사)
    address: Option<String>,
    /// 서브어카운트 번호 (기본: 0)
    subaccount_number: u32,
    /// Cosmos 지갑 (서명 기능)
    wallet: Option<CosmosWallet>,
    /// Node API 클라이언트
    node_client: Option<DydxNodeClient>,
    /// 트랜잭션 빌더
    tx_builder: Option<DydxTransactionBuilder>,
}

impl DydxV4 {
    /// Mainnet Indexer API base URL
    const MAINNET_INDEXER_URL: &'static str = "https://indexer.dydx.trade/v4";
    /// Testnet Indexer API base URL
    const TESTNET_INDEXER_URL: &'static str = "https://indexer.v4testnet.dydx.exchange/v4";
    /// Rate limit (10 requests per second)
    const RATE_LIMIT_MS: u64 = 100;

    /// 새 dYdX v4 인스턴스 생성 (Mainnet)
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        Self::with_options(config, false, None, None)
    }

    /// 테스트넷 인스턴스 생성
    pub fn testnet(config: ExchangeConfig) -> CcxtResult<Self> {
        Self::with_options(config, true, None, None)
    }

    /// 지갑 주소와 함께 인스턴스 생성 (읽기 전용)
    pub fn with_address(config: ExchangeConfig, address: &str, testnet: bool) -> CcxtResult<Self> {
        Self::with_options(config, testnet, Some(address.to_string()), None)
    }

    /// 니모닉으로 지갑 생성 (거래 가능)
    ///
    /// # Arguments
    ///
    /// * `config` - 거래소 설정
    /// * `mnemonic` - BIP-39 니모닉 문구 (12/24 단어)
    /// * `testnet` - 테스트넷 여부
    /// * `index` - 주소 인덱스 (기본: 0)
    pub fn with_mnemonic(
        config: ExchangeConfig,
        mnemonic: &str,
        testnet: bool,
        index: u32,
    ) -> CcxtResult<Self> {
        let chain_config = if testnet { &DYDX_TESTNET } else { &DYDX_MAINNET };
        let wallet = CosmosWallet::from_mnemonic(mnemonic, chain_config, index)?;
        let address = wallet.address().to_string();
        Self::with_options(config, testnet, Some(address), Some(wallet))
    }

    /// 개인키로 지갑 생성 (거래 가능)
    ///
    /// # Arguments
    ///
    /// * `config` - 거래소 설정
    /// * `private_key_hex` - Hex 인코딩된 개인키
    /// * `testnet` - 테스트넷 여부
    pub fn with_private_key(
        config: ExchangeConfig,
        private_key_hex: &str,
        testnet: bool,
    ) -> CcxtResult<Self> {
        let chain_config = if testnet { &DYDX_TESTNET } else { &DYDX_MAINNET };
        let wallet = CosmosWallet::from_private_key_hex(private_key_hex, chain_config)?;
        let address = wallet.address().to_string();
        Self::with_options(config, testnet, Some(address), Some(wallet))
    }

    /// 옵션과 함께 인스턴스 생성
    fn with_options(
        config: ExchangeConfig,
        testnet: bool,
        address: Option<String>,
        wallet: Option<CosmosWallet>,
    ) -> CcxtResult<Self> {
        let base_url = if testnet {
            Self::TESTNET_INDEXER_URL
        } else {
            Self::MAINNET_INDEXER_URL
        };

        let client = HttpClient::new(base_url, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let has_wallet = wallet.is_some();
        let has_address = address.is_some();

        let features = ExchangeFeatures {
            spot: false,
            margin: false,
            swap: true,
            future: true,
            option: false,
            fetch_markets: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: has_address,
            create_order: has_wallet, // Cosmos 지갑 있으면 거래 가능
            create_limit_order: has_wallet,
            create_market_order: has_wallet,
            cancel_order: has_wallet,
            fetch_order: has_address,
            fetch_orders: has_address,
            fetch_open_orders: has_address,
            fetch_my_trades: has_address,
            fetch_positions: has_address,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("indexer".into(), base_url.into());

        let urls = ExchangeUrls {
            logo: Some("https://dydx.exchange/favicon.ico".into()),
            api: api_urls,
            www: Some("https://dydx.exchange".into()),
            doc: vec![
                "https://docs.dydx.xyz/".into(),
            ],
            fees: Some("https://dydx.exchange/trading-rewards".into()),
        };

        // dYdX v4 Candle resolutions
        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1MIN".into());
        timeframes.insert(Timeframe::Minute5, "5MINS".into());
        timeframes.insert(Timeframe::Minute15, "15MINS".into());
        timeframes.insert(Timeframe::Minute30, "30MINS".into());
        timeframes.insert(Timeframe::Hour1, "1HOUR".into());
        timeframes.insert(Timeframe::Hour4, "4HOURS".into());
        timeframes.insert(Timeframe::Day1, "1DAY".into());

        // Node API 클라이언트 및 트랜잭션 빌더 생성 (지갑이 있는 경우)
        let (node_client, tx_builder) = if wallet.is_some() {
            let node = DydxNodeClient::new(testnet);
            let builder = DydxTransactionBuilder::new(testnet);
            (Some(node), Some(builder))
        } else {
            (None, None)
        };

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            market_info: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            testnet,
            address,
            subaccount_number: 0,
            wallet,
            node_client,
            tx_builder,
        })
    }

    /// 서브어카운트 번호 설정
    pub fn set_subaccount(&mut self, number: u32) {
        self.subaccount_number = number;
    }

    /// 테스트넷 여부 확인
    pub fn is_testnet(&self) -> bool {
        self.testnet
    }

    /// 지갑 주소 반환
    pub fn address(&self) -> Option<&str> {
        self.address.as_deref()
    }

    /// 지갑 반환 (읽기 전용)
    pub fn wallet(&self) -> Option<&CosmosWallet> {
        self.wallet.as_ref()
    }

    /// 서명 가능 여부 확인
    pub fn can_sign(&self) -> bool {
        self.wallet.is_some()
    }

    /// 지갑 필요 검사 (거래용)
    fn require_wallet(&self) -> CcxtResult<&CosmosWallet> {
        self.wallet.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "dYdX v4 requires wallet with signing capability for this operation. Use with_mnemonic() or with_private_key()".into(),
        })
    }

    /// Indexer API GET 요청
    async fn indexer_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, None, None).await
    }

    /// 마켓 ID 변환 (BTC/USD → BTC-USD-PERP)
    fn symbol_to_market_id(&self, symbol: &str) -> String {
        // BTC/USD → BTC-USD-PERP
        let base = symbol.replace("/", "-");
        if base.ends_with("-PERP") {
            base
        } else {
            format!("{}-PERP", base)
        }
    }

    /// 마켓 ID를 심볼로 변환 (BTC-USD-PERP → BTC/USD)
    fn market_id_to_symbol(&self, market_id: &str) -> String {
        // BTC-USD-PERP → BTC/USD
        let stripped = market_id.strip_suffix("-PERP").unwrap_or(market_id);
        stripped.replacen("-", "/", 1)
    }

    /// 주소 필요 검사
    fn require_address(&self) -> CcxtResult<&str> {
        self.address.as_deref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "dYdX v4 requires wallet address for this operation".into(),
        })
    }
}

#[async_trait]
impl Exchange for DydxV4 {
    fn id(&self) -> ExchangeId {
        ExchangeId::Dydx // 동일한 ID 사용 (v4는 내부적으로 구분)
    }

    fn name(&self) -> &str {
        "dYdX v4"
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
        // Indexer API는 인증이 필요 없음
        SignedRequest {
            url: path.to_string(),
            method: method.to_string(),
            headers: HashMap::new(),
            body: None,
        }
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response: DydxV4PerpetualMarketsResponse =
            self.indexer_get("/perpetualMarkets").await?;

        let mut markets = Vec::new();

        for (market_id, info) in response.markets {
            // BTC-USD-PERP → BTC, USD
            let parts: Vec<&str> = market_id.split('-').collect();
            let base = parts.first().unwrap_or(&"").to_string();
            let quote = parts.get(1).unwrap_or(&"USD").to_string();
            let symbol = format!("{}/{}", base, quote);

            let tick_size: Option<Decimal> = info.tick_size.as_ref().and_then(|s| s.parse().ok());
            let step_size: Option<Decimal> = info.step_size.as_ref().and_then(|s| s.parse().ok());

            // Calculate precision from tick/step size
            let price_precision = tick_size.map(|t| {
                let s = t.to_string();
                if let Some(pos) = s.find('.') {
                    (s.len() - pos - 1) as i32
                } else {
                    0
                }
            });

            let amount_precision = step_size.map(|t| {
                let s = t.to_string();
                if let Some(pos) = s.find('.') {
                    (s.len() - pos - 1) as i32
                } else {
                    0
                }
            });

            let market = Market {
                id: market_id.clone(),
                lowercase_id: Some(market_id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                market_type: MarketType::Swap,
                spot: false,
                margin: false,
                swap: true,
                future: false,
                option: false,
                index: false,
                active: matches!(info.status.as_deref(), Some("ACTIVE")),
                contract: true,
                linear: Some(true),
                inverse: Some(false),
                sub_type: Some("linear".to_string()),
                taker: None, // dYdX v4 uses dynamic fees
                maker: None,
                contract_size: Some(Decimal::ONE),
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                settle: Some(quote.clone()),
                settle_id: Some(quote.clone()),
                precision: MarketPrecision {
                    amount: amount_precision,
                    price: price_precision,
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&info).unwrap_or_default(),
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
        let market_id = self.symbol_to_market_id(symbol);
        let path = format!("/perpetualMarkets?ticker={}", market_id);
        let response: DydxV4PerpetualMarketsResponse = self.indexer_get(&path).await?;

        let info = response.markets.get(&market_id).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: info.price_change_24h.as_ref().and_then(|_| info.oracle_price.as_ref()).and_then(|s| s.parse().ok()),
            low: None,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: None,
            close: info.oracle_price.as_ref().and_then(|s| s.parse().ok()),
            last: info.oracle_price.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: info.price_change_24h.as_ref().and_then(|s| s.parse().ok()),
            percentage: None,
            average: None,
            base_volume: info.volume_24h.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: None,
            index_price: None,
            mark_price: info.oracle_price.as_ref().and_then(|s| s.parse().ok()),
            info: serde_json::to_value(&info).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.symbol_to_market_id(symbol);
        let path = format!("/orderbooks/perpetualMarket/{}", market_id);
        let response: DydxV4OrderBookResponse = self.indexer_get(&path).await?;

        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &[DydxV4OrderBookEntry]| -> Vec<OrderBookEntry> {
            let iter = entries.iter().filter_map(|e| {
                let price: Decimal = e.price.parse().ok()?;
                let amount: Decimal = e.size.parse().ok()?;
                Some(OrderBookEntry { price, amount })
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
            bids: parse_entries(&response.bids),
            asks: parse_entries(&response.asks),
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.symbol_to_market_id(symbol);
        let mut path = format!("/trades/perpetualMarket/{}", market_id);

        if let Some(l) = limit {
            path.push_str(&format!("?limit={}", l));
        }

        let response: DydxV4TradesResponse = self.indexer_get(&path).await?;

        let trades: Vec<Trade> = response.trades.iter()
            .filter_map(|t| {
                let timestamp = t.created_at.as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let price: Decimal = t.price.parse().ok()?;
                let amount: Decimal = t.size.parse().ok()?;

                Some(Trade {
                    id: t.id.clone(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: t.trade_type.clone(),
                    side: Some(t.side.to_uppercase()),
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                })
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
        let market_id = self.symbol_to_market_id(symbol);
        let resolution = self.timeframes.get(&timeframe)
            .ok_or_else(|| CcxtError::NotSupported {
                feature: format!("Timeframe {:?}", timeframe),
            })?;

        let mut path = format!("/candles/perpetualMarkets/{}?resolution={}", market_id, resolution);

        if let Some(l) = limit {
            path.push_str(&format!("&limit={}", l));
        }

        let response: DydxV4CandlesResponse = self.indexer_get(&path).await?;

        let ohlcv: Vec<OHLCV> = response.candles.iter()
            .filter_map(|c| {
                let timestamp = c.started_at.as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())?;

                let open: Decimal = c.open.parse().ok()?;
                let high: Decimal = c.high.parse().ok()?;
                let low: Decimal = c.low.parse().ok()?;
                let close: Decimal = c.close.parse().ok()?;
                let volume: Decimal = c.base_token_volume.as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(Decimal::ZERO);

                Some(OHLCV {
                    timestamp,
                    open,
                    high,
                    low,
                    close,
                    volume,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let address = self.require_address()?;
        let path = format!("/addresses/{}/subaccountNumber/{}", address, self.subaccount_number);
        let response: DydxV4SubaccountResponse = self.indexer_get(&path).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        // USDC 잔고 (equity)
        if let Some(equity) = response.subaccount.equity.as_ref().and_then(|s| s.parse::<Decimal>().ok()) {
            let free_collateral = response.subaccount.free_collateral
                .as_ref()
                .and_then(|s| s.parse::<Decimal>().ok())
                .unwrap_or(Decimal::ZERO);

            balances.currencies.insert("USDC".to_string(), Balance {
                free: Some(free_collateral),
                used: Some(equity - free_collateral),
                total: Some(equity),
                debt: None,
            });
        }

        // Asset positions (추가 자산)
        if let Some(positions) = &response.subaccount.asset_positions {
            for (asset, position) in positions {
                if let Some(size) = position.size.as_ref().and_then(|s| s.parse::<Decimal>().ok()) {
                    balances.currencies.insert(asset.clone(), Balance {
                        free: Some(size),
                        used: Some(Decimal::ZERO),
                        total: Some(size),
                        debt: None,
                    });
                }
            }
        }

        balances.info = serde_json::to_value(&response).unwrap_or_default();
        Ok(balances)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let address = self.require_address()?;
        let mut path = format!(
            "/orders?address={}&subaccountNumber={}&status=OPEN",
            address, self.subaccount_number
        );

        if let Some(s) = symbol {
            let market_id = self.symbol_to_market_id(s);
            path.push_str(&format!("&ticker={}", market_id));
        }

        if let Some(l) = limit {
            path.push_str(&format!("&limit={}", l));
        }

        let response: Vec<DydxV4Order> = self.indexer_get(&path).await?;
        self.parse_orders(&response)
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("/orders/{}", id);
        let response: DydxV4Order = self.indexer_get(&path).await?;

        let orders = self.parse_orders(&[response])?;
        orders.into_iter().next().ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
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
        // 지갑 및 관련 컴포넌트 필수
        let wallet = self.require_wallet()?;
        let node_client = self.node_client.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Node client not initialized".into(),
        })?;
        let tx_builder = self.tx_builder.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Transaction builder not initialized".into(),
        })?;

        // 마켓 정보 가져오기
        let market_info = self.get_or_fetch_market_info(symbol).await?;

        // 계정 정보 조회
        let account = node_client.get_account(wallet.address()).await?;
        let account_number: u64 = account.account_number.parse().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to parse account number".into(),
        })?;
        let sequence: u64 = account.sequence.parse().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to parse sequence".into(),
        })?;

        // 현재 블록 높이 조회 (short-term order용)
        let current_block = node_client.get_latest_block_height().await?;
        let good_til_block = current_block + SHORT_BLOCK_WINDOW;

        // 클라이언트 ID 생성
        let client_id = generate_client_id();

        // 주문 방향 변환
        let dydx_side: DydxOrderSide = side.into();

        // 주문 유형에 따른 TimeInForce 결정
        let time_in_force = match order_type {
            OrderType::Market => DydxTimeInForce::Ioc,
            OrderType::Limit => DydxTimeInForce::Unspecified, // GTC
            _ => DydxTimeInForce::Unspecified,
        };

        // 가격 처리 (마켓 주문은 슬리피지 허용 가격 사용)
        let order_price = match order_type {
            OrderType::Market => {
                // 마켓 주문은 슬리피지를 포함한 최악의 가격 사용
                // 매수: 매우 높은 가격, 매도: 매우 낮은 가격
                match side {
                    OrderSide::Buy => Decimal::from(1_000_000_000u64), // 높은 가격
                    OrderSide::Sell => Decimal::from(1u64), // 낮은 가격
                }
            }
            _ => price.ok_or_else(|| CcxtError::BadRequest {
                message: "Price is required for limit orders".into(),
            })?,
        };

        // Quantums 및 Subticks 변환
        let quantums = market_info.size_to_quantums(amount);
        let subticks = market_info.price_to_subticks(order_price);

        // dYdX 주문 생성 (short-term)
        let dydx_order = DydxOrder::new_short_term(
            wallet.address(),
            self.subaccount_number,
            client_id,
            market_info.clob_pair_id,
            dydx_side,
            quantums,
            subticks,
            good_til_block,
            time_in_force,
            false, // reduce_only
        );

        // 트랜잭션 빌드 및 서명
        let tx_bytes = tx_builder.build_place_order_tx(
            wallet,
            dydx_order,
            account_number,
            sequence,
            "", // memo
        )?;

        // 트랜잭션 브로드캐스트 (동기)
        let result = node_client.broadcast_tx_sync(&tx_bytes).await?;

        // 주문 응답 생성
        let timestamp = Utc::now().timestamp_millis();
        let order = Order {
            id: result.hash.clone(),
            client_order_id: Some(client_id.to_string()),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: Some(TimeInForce::GTC),
            post_only: Some(false),
            reduce_only: Some(false),
            side,
            price,
            trigger_price: None,
            amount,
            cost: price.map(|p| p * amount),
            average: None,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            status: OrderStatus::Open,
            fee: None,
            fees: Vec::new(),
            trades: Vec::new(),
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::json!({
                "tx_hash": result.hash,
                "client_id": client_id,
                "good_til_block": good_til_block,
                "quantums": quantums,
                "subticks": subticks,
            }),
        };

        Ok(order)
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        // 지갑 및 관련 컴포넌트 필수
        let wallet = self.require_wallet()?;
        let node_client = self.node_client.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Node client not initialized".into(),
        })?;
        let tx_builder = self.tx_builder.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Transaction builder not initialized".into(),
        })?;

        // 기존 주문 정보 조회
        let existing_order = self.fetch_order(id, symbol).await?;

        // 마켓 정보 가져오기
        let market_info = self.get_or_fetch_market_info(symbol).await?;

        // 계정 정보 조회
        let account = node_client.get_account(wallet.address()).await?;
        let account_number: u64 = account.account_number.parse().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to parse account number".into(),
        })?;
        let sequence: u64 = account.sequence.parse().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to parse sequence".into(),
        })?;

        // 현재 블록 높이 조회
        let current_block = node_client.get_latest_block_height().await?;
        let good_til_block = current_block + SHORT_BLOCK_WINDOW;

        // client_id 파싱
        let client_id: u32 = existing_order
            .client_order_id
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // OrderId 생성
        let order_id = OrderId::new(
            SubaccountId::new(wallet.address(), self.subaccount_number),
            client_id,
            order_flags::SHORT_TERM,
            market_info.clob_pair_id,
        );

        // 트랜잭션 빌드 및 서명
        let tx_bytes = tx_builder.build_cancel_order_tx(
            wallet,
            order_id,
            GoodTilOneof::GoodTilBlock(good_til_block),
            account_number,
            sequence,
            "", // memo
        )?;

        // 트랜잭션 브로드캐스트 (동기)
        let result = node_client.broadcast_tx_sync(&tx_bytes).await?;

        // 취소된 주문 반환
        let mut cancelled_order = existing_order;
        cancelled_order.status = OrderStatus::Canceled;
        cancelled_order.info = serde_json::json!({
            "cancel_tx_hash": result.hash,
            "original_order": cancelled_order.info,
        });

        Ok(cancelled_order)
    }
}

impl DydxV4 {
    /// 마켓 정보 가져오기 (캐시 또는 API 조회)
    async fn get_or_fetch_market_info(&self, symbol: &str) -> CcxtResult<DydxMarketInfo> {
        let market_id = self.symbol_to_market_id(symbol);

        // 캐시 확인
        {
            let cache = self.market_info.read().unwrap();
            if let Some(info) = cache.get(&market_id) {
                return Ok(info.clone());
            }
        }

        // API에서 마켓 정보 조회
        let path = format!("/perpetualMarkets?ticker={}", market_id);
        let response: DydxV4PerpetualMarketsResponse = self.indexer_get(&path).await?;

        let market_info = response.markets.get(&market_id).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        // DydxMarketInfo로 변환
        let clob_pair_id = market_info.clob_pair_id
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let atomic_resolution = market_info.atomic_resolution.unwrap_or(-10);
        let step_base_quantums = market_info.step_base_quantums.unwrap_or(1) as u64;
        let subticks_per_tick = market_info.subticks_per_tick.unwrap_or(1) as u32;
        let quantum_conversion_exponent = market_info.quantum_conversion_exponent.unwrap_or(-9);

        let info = DydxMarketInfo {
            clob_pair_id,
            atomic_resolution,
            step_base_quantums,
            subticks_per_tick,
            quantum_conversion_exponent,
        };

        // 캐시에 저장
        {
            let mut cache = self.market_info.write().unwrap();
            cache.insert(market_id, info.clone());
        }

        Ok(info)
    }

    /// 주문 목록 파싱
    fn parse_orders(&self, orders: &[DydxV4Order]) -> CcxtResult<Vec<Order>> {
        let mut result = Vec::new();

        for o in orders {
            let symbol = self.market_id_to_symbol(&o.ticker);
            let timestamp = o.created_at.as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis());

            let price = o.price.parse().ok();
            let amount: Decimal = o.size.parse().unwrap_or(Decimal::ZERO);
            let filled: Decimal = o.total_filled.as_ref()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::ZERO);
            let remaining = amount - filled;

            let status = match o.status.as_str() {
                "OPEN" => OrderStatus::Open,
                "FILLED" => OrderStatus::Closed,
                "CANCELED" | "CANCELLED" => OrderStatus::Canceled,
                "BEST_EFFORT_CANCELED" => OrderStatus::Canceled,
                "UNTRIGGERED" => OrderStatus::Open,
                _ => OrderStatus::Open,
            };

            let order_type = match o.order_type.as_str() {
                "LIMIT" => OrderType::Limit,
                "MARKET" => OrderType::Market,
                "STOP_LIMIT" => OrderType::StopLimit,
                "STOP_MARKET" => OrderType::StopMarket,
                "TAKE_PROFIT_LIMIT" => OrderType::TakeProfitLimit,
                "TAKE_PROFIT_MARKET" => OrderType::TakeProfitMarket,
                _ => OrderType::Limit,
            };

            let time_in_force = o.time_in_force.as_ref().and_then(|tif| {
                match tif.to_uppercase().as_str() {
                    "GTC" | "GTT" => Some(TimeInForce::GTC),
                    "IOC" => Some(TimeInForce::IOC),
                    "FOK" => Some(TimeInForce::FOK),
                    _ => None,
                }
            });

            let side = match o.side.to_uppercase().as_str() {
                "BUY" => OrderSide::Buy,
                _ => OrderSide::Sell,
            };

            let order = Order {
                id: o.id.clone().unwrap_or_default(),
                client_order_id: o.client_id.clone(),
                timestamp,
                datetime: timestamp.map(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                last_trade_timestamp: None,
                last_update_timestamp: o.updated_at.as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis()),
                symbol,
                order_type,
                time_in_force,
                post_only: o.post_only,
                reduce_only: o.reduce_only,
                side,
                price,
                trigger_price: o.trigger_price.as_ref().and_then(|s| s.parse().ok()),
                amount,
                cost: price.map(|p| p * filled),
                average: None,
                filled,
                remaining: Some(remaining),
                status,
                fee: None,
                fees: Vec::new(),
                trades: Vec::new(),
                stop_price: o.trigger_price.as_ref().and_then(|s| s.parse().ok()),
                take_profit_price: None,
                stop_loss_price: None,
                info: serde_json::to_value(o).unwrap_or_default(),
            };
            result.push(order);
        }

        Ok(result)
    }

    /// 포지션 조회
    pub async fn fetch_positions(&self, symbol: Option<&str>) -> CcxtResult<Vec<DydxV4Position>> {
        let address = self.require_address()?;
        let mut path = format!(
            "/perpetualPositions?address={}&subaccountNumber={}&status=OPEN",
            address, self.subaccount_number
        );

        if let Some(s) = symbol {
            let market_id = self.symbol_to_market_id(s);
            path.push_str(&format!("&ticker={}", market_id));
        }

        let response: DydxV4PositionsResponse = self.indexer_get(&path).await?;
        Ok(response.positions)
    }

    /// 체결 내역 조회
    pub async fn fetch_my_fills(&self, symbol: Option<&str>, limit: Option<u32>) -> CcxtResult<Vec<DydxV4Fill>> {
        let address = self.require_address()?;
        let mut path = format!(
            "/fills?address={}&subaccountNumber={}",
            address, self.subaccount_number
        );

        if let Some(s) = symbol {
            let market_id = self.symbol_to_market_id(s);
            path.push_str(&format!("&market={}", market_id));
        }

        if let Some(l) = limit {
            path.push_str(&format!("&limit={}", l));
        }

        let response: DydxV4FillsResponse = self.indexer_get(&path).await?;
        Ok(response.fills)
    }

    /// Historical PnL 조회
    pub async fn fetch_historical_pnl(&self) -> CcxtResult<Vec<DydxV4PnlTick>> {
        let address = self.require_address()?;
        let path = format!(
            "/historical-pnl?address={}&subaccountNumber={}",
            address, self.subaccount_number
        );

        let response: DydxV4HistoricalPnlResponse = self.indexer_get(&path).await?;
        Ok(response.historical_pnl)
    }

    /// Funding payments 조회
    pub async fn fetch_funding_payments(&self, symbol: Option<&str>, limit: Option<u32>) -> CcxtResult<Vec<DydxV4FundingPayment>> {
        let address = self.require_address()?;
        let mut path = format!(
            "/fundingPayments?address={}&subaccountNumber={}",
            address, self.subaccount_number
        );

        if let Some(s) = symbol {
            let market_id = self.symbol_to_market_id(s);
            path.push_str(&format!("&ticker={}", market_id));
        }

        if let Some(l) = limit {
            path.push_str(&format!("&limit={}", l));
        }

        let response: DydxV4FundingPaymentsResponse = self.indexer_get(&path).await?;
        Ok(response.funding_payments)
    }

    /// Historical funding rate 조회
    pub async fn fetch_funding_rate_history(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<Vec<DydxV4HistoricalFunding>> {
        let market_id = self.symbol_to_market_id(symbol);
        let mut path = format!("/historicalFunding/{}", market_id);

        if let Some(l) = limit {
            path.push_str(&format!("?limit={}", l));
        }

        let response: DydxV4HistoricalFundingResponse = self.indexer_get(&path).await?;
        Ok(response.historical_funding)
    }

    // ============================================================================
    // Long-term Orders
    // ============================================================================

    /// Long-term 주문 생성
    ///
    /// Short-term 주문과 달리 블록 시간(Unix timestamp) 기준으로 만료됩니다.
    /// GTC(Good Til Cancel) 주문에 적합합니다.
    ///
    /// # Arguments
    ///
    /// * `symbol` - 거래 심볼 (예: "BTC/USD")
    /// * `order_type` - 주문 유형 (Limit, Market)
    /// * `side` - 주문 방향 (Buy, Sell)
    /// * `amount` - 주문 수량
    /// * `price` - 가격 (Limit 주문시 필수)
    /// * `good_til_time` - 만료 시간 (Unix timestamp in seconds)
    /// * `time_in_force` - 주문 유효 조건 (GTC, IOC, FOK, PostOnly)
    /// * `reduce_only` - 포지션 축소 전용 여부
    ///
    /// # Returns
    ///
    /// 생성된 주문 정보
    pub async fn create_long_term_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
        good_til_time: u32,
        time_in_force: Option<TimeInForce>,
        reduce_only: bool,
    ) -> CcxtResult<Order> {
        // 지갑 및 관련 컴포넌트 필수
        let wallet = self.require_wallet()?;
        let node_client = self.node_client.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Node client not initialized".into(),
        })?;
        let tx_builder = self.tx_builder.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Transaction builder not initialized".into(),
        })?;

        // 마켓 정보 가져오기
        let market_info = self.get_or_fetch_market_info(symbol).await?;

        // 계정 정보 조회
        let account = node_client.get_account(wallet.address()).await?;
        let account_number: u64 = account.account_number.parse().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to parse account number".into(),
        })?;
        let sequence: u64 = account.sequence.parse().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to parse sequence".into(),
        })?;

        // 클라이언트 ID 생성
        let client_id = generate_client_id();

        // 주문 방향 변환
        let dydx_side: DydxOrderSide = side.into();

        // TimeInForce 변환
        let dydx_time_in_force = match time_in_force {
            Some(tif) => tif.into(),
            None => match order_type {
                OrderType::Market => DydxTimeInForce::Ioc,
                _ => DydxTimeInForce::Unspecified, // GTC
            },
        };

        // 가격 처리
        let order_price = match order_type {
            OrderType::Market => {
                match side {
                    OrderSide::Buy => Decimal::from(1_000_000_000u64),
                    OrderSide::Sell => Decimal::from(1u64),
                }
            }
            _ => price.ok_or_else(|| CcxtError::BadRequest {
                message: "Price is required for limit orders".into(),
            })?,
        };

        // Quantums 및 Subticks 변환
        let quantums = market_info.size_to_quantums(amount);
        let subticks = market_info.price_to_subticks(order_price);

        // dYdX Long-term 주문 생성
        let dydx_order = DydxOrder::new_long_term(
            wallet.address(),
            self.subaccount_number,
            client_id,
            market_info.clob_pair_id,
            dydx_side,
            quantums,
            subticks,
            good_til_time,
            dydx_time_in_force,
            reduce_only,
        );

        // 트랜잭션 빌드 및 서명
        let tx_bytes = tx_builder.build_place_order_tx(
            wallet,
            dydx_order,
            account_number,
            sequence,
            "", // memo
        )?;

        // 트랜잭션 브로드캐스트 (동기)
        let result = node_client.broadcast_tx_sync(&tx_bytes).await?;

        // 주문 응답 생성
        let timestamp = Utc::now().timestamp_millis();
        let tif = time_in_force.unwrap_or(TimeInForce::GTC);

        let order = Order {
            id: result.hash.clone(),
            client_order_id: Some(client_id.to_string()),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: Some(tif),
            post_only: Some(matches!(dydx_time_in_force, DydxTimeInForce::PostOnly)),
            reduce_only: Some(reduce_only),
            side,
            price,
            trigger_price: None,
            amount,
            cost: price.map(|p| p * amount),
            average: None,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            status: OrderStatus::Open,
            fee: None,
            fees: Vec::new(),
            trades: Vec::new(),
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            info: serde_json::json!({
                "tx_hash": result.hash,
                "client_id": client_id,
                "order_flags": order_flags::LONG_TERM,
                "good_til_block_time": good_til_time,
                "quantums": quantums,
                "subticks": subticks,
            }),
        };

        Ok(order)
    }

    /// Long-term 주문 취소
    ///
    /// # Arguments
    ///
    /// * `id` - 주문 ID
    /// * `symbol` - 거래 심볼
    /// * `client_id` - 클라이언트 ID (주문 생성시 반환된 값)
    /// * `good_til_block_time` - 원래 주문의 만료 시간
    pub async fn cancel_long_term_order(
        &self,
        id: &str,
        symbol: &str,
        client_id: u32,
        good_til_block_time: u32,
    ) -> CcxtResult<Order> {
        // 지갑 및 관련 컴포넌트 필수
        let wallet = self.require_wallet()?;
        let node_client = self.node_client.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Node client not initialized".into(),
        })?;
        let tx_builder = self.tx_builder.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Transaction builder not initialized".into(),
        })?;

        // 기존 주문 정보 조회
        let existing_order = self.fetch_order(id, symbol).await?;

        // 마켓 정보 가져오기
        let market_info = self.get_or_fetch_market_info(symbol).await?;

        // 계정 정보 조회
        let account = node_client.get_account(wallet.address()).await?;
        let account_number: u64 = account.account_number.parse().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to parse account number".into(),
        })?;
        let sequence: u64 = account.sequence.parse().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to parse sequence".into(),
        })?;

        // OrderId 생성 (Long-term 플래그 사용)
        let order_id = OrderId::new(
            SubaccountId::new(wallet.address(), self.subaccount_number),
            client_id,
            order_flags::LONG_TERM,
            market_info.clob_pair_id,
        );

        // 트랜잭션 빌드 및 서명
        let tx_bytes = tx_builder.build_cancel_order_tx(
            wallet,
            order_id,
            GoodTilOneof::GoodTilBlockTime(good_til_block_time),
            account_number,
            sequence,
            "", // memo
        )?;

        // 트랜잭션 브로드캐스트 (동기)
        let result = node_client.broadcast_tx_sync(&tx_bytes).await?;

        // 취소된 주문 반환
        let mut cancelled_order = existing_order;
        cancelled_order.status = OrderStatus::Canceled;
        cancelled_order.info = serde_json::json!({
            "cancel_tx_hash": result.hash,
            "order_flags": order_flags::LONG_TERM,
            "original_order": cancelled_order.info,
        });

        Ok(cancelled_order)
    }

    /// GTC 주문 생성 (Long-term order 간편 버전)
    ///
    /// 기본 90일 만료로 Long-term 주문을 생성합니다.
    ///
    /// # Arguments
    ///
    /// * `symbol` - 거래 심볼
    /// * `side` - 주문 방향
    /// * `amount` - 주문 수량
    /// * `price` - 가격
    /// * `post_only` - Post-Only 여부
    pub async fn create_gtc_order(
        &self,
        symbol: &str,
        side: OrderSide,
        amount: Decimal,
        price: Decimal,
        post_only: bool,
    ) -> CcxtResult<Order> {
        // 90일 후 만료 (기본값)
        let now = Utc::now().timestamp() as u32;
        let good_til_time = now + (90 * 24 * 60 * 60); // 90 days

        let time_in_force = if post_only {
            Some(TimeInForce::PO)
        } else {
            Some(TimeInForce::GTC)
        };

        self.create_long_term_order(
            symbol,
            OrderType::Limit,
            side,
            amount,
            Some(price),
            good_til_time,
            time_in_force,
            false, // reduce_only
        ).await
    }

    // ============================================================================
    // Conditional Orders (Stop Loss / Take Profit)
    // ============================================================================

    /// 조건부 주문 생성 (Stop Loss)
    ///
    /// # Arguments
    ///
    /// * `symbol` - 거래 심볼
    /// * `side` - 주문 방향
    /// * `amount` - 주문 수량
    /// * `trigger_price` - 트리거 가격 (이 가격에 도달하면 주문 실행)
    /// * `limit_price` - 지정가 (None이면 시장가로 실행)
    /// * `good_til_time` - 만료 시간
    /// * `reduce_only` - 포지션 축소 전용
    pub async fn create_stop_loss_order(
        &self,
        symbol: &str,
        side: OrderSide,
        amount: Decimal,
        trigger_price: Decimal,
        limit_price: Option<Decimal>,
        good_til_time: u32,
        reduce_only: bool,
    ) -> CcxtResult<Order> {
        self.create_conditional_order(
            symbol,
            side,
            amount,
            trigger_price,
            limit_price,
            good_til_time,
            ConditionType::StopLoss,
            reduce_only,
        ).await
    }

    /// 조건부 주문 생성 (Take Profit)
    ///
    /// # Arguments
    ///
    /// * `symbol` - 거래 심볼
    /// * `side` - 주문 방향
    /// * `amount` - 주문 수량
    /// * `trigger_price` - 트리거 가격
    /// * `limit_price` - 지정가 (None이면 시장가로 실행)
    /// * `good_til_time` - 만료 시간
    /// * `reduce_only` - 포지션 축소 전용
    pub async fn create_take_profit_order(
        &self,
        symbol: &str,
        side: OrderSide,
        amount: Decimal,
        trigger_price: Decimal,
        limit_price: Option<Decimal>,
        good_til_time: u32,
        reduce_only: bool,
    ) -> CcxtResult<Order> {
        self.create_conditional_order(
            symbol,
            side,
            amount,
            trigger_price,
            limit_price,
            good_til_time,
            ConditionType::TakeProfit,
            reduce_only,
        ).await
    }

    /// 조건부 주문 생성 (내부 구현)
    async fn create_conditional_order(
        &self,
        symbol: &str,
        side: OrderSide,
        amount: Decimal,
        trigger_price: Decimal,
        limit_price: Option<Decimal>,
        good_til_time: u32,
        condition_type: ConditionType,
        reduce_only: bool,
    ) -> CcxtResult<Order> {
        // 지갑 및 관련 컴포넌트 필수
        let wallet = self.require_wallet()?;
        let node_client = self.node_client.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Node client not initialized".into(),
        })?;
        let tx_builder = self.tx_builder.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Transaction builder not initialized".into(),
        })?;

        // 마켓 정보 가져오기
        let market_info = self.get_or_fetch_market_info(symbol).await?;

        // 계정 정보 조회
        let account = node_client.get_account(wallet.address()).await?;
        let account_number: u64 = account.account_number.parse().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to parse account number".into(),
        })?;
        let sequence: u64 = account.sequence.parse().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to parse sequence".into(),
        })?;

        // 클라이언트 ID 생성
        let client_id = generate_client_id();

        // 주문 방향 변환
        let dydx_side: DydxOrderSide = side.into();

        // 가격 처리 (limit_price가 없으면 시장가 스타일)
        let order_price = limit_price.unwrap_or_else(|| {
            match side {
                OrderSide::Buy => Decimal::from(1_000_000_000u64),
                OrderSide::Sell => Decimal::from(1u64),
            }
        });

        // TimeInForce
        let time_in_force = if limit_price.is_some() {
            DydxTimeInForce::Unspecified // GTC for limit
        } else {
            DydxTimeInForce::Ioc // IOC for market-like
        };

        // Quantums 및 Subticks 변환
        let quantums = market_info.size_to_quantums(amount);
        let subticks = market_info.price_to_subticks(order_price);
        let trigger_subticks = market_info.price_to_subticks(trigger_price);

        // dYdX Conditional 주문 생성
        let dydx_order = DydxOrder {
            order_id: OrderId::new(
                SubaccountId::new(wallet.address(), self.subaccount_number),
                client_id,
                order_flags::CONDITIONAL,
                market_info.clob_pair_id,
            ),
            side: dydx_side,
            quantums,
            subticks,
            good_til_oneof: GoodTilOneof::GoodTilBlockTime(good_til_time),
            time_in_force,
            reduce_only,
            client_metadata: 0,
            condition_type,
            conditional_order_trigger_subticks: trigger_subticks,
        };

        // 트랜잭션 빌드 및 서명
        let tx_bytes = tx_builder.build_place_order_tx(
            wallet,
            dydx_order,
            account_number,
            sequence,
            "", // memo
        )?;

        // 트랜잭션 브로드캐스트 (동기)
        let result = node_client.broadcast_tx_sync(&tx_bytes).await?;

        // 주문 유형 결정
        let order_type = match condition_type {
            ConditionType::StopLoss => {
                if limit_price.is_some() { OrderType::StopLimit } else { OrderType::StopMarket }
            }
            ConditionType::TakeProfit => {
                if limit_price.is_some() { OrderType::TakeProfitLimit } else { OrderType::TakeProfitMarket }
            }
            _ => OrderType::Limit,
        };

        // 주문 응답 생성
        let timestamp = Utc::now().timestamp_millis();
        let order = Order {
            id: result.hash.clone(),
            client_order_id: Some(client_id.to_string()),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: Some(if limit_price.is_some() { TimeInForce::GTC } else { TimeInForce::IOC }),
            post_only: Some(false),
            reduce_only: Some(reduce_only),
            side,
            price: limit_price,
            trigger_price: Some(trigger_price),
            amount,
            cost: limit_price.map(|p| p * amount),
            average: None,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            status: OrderStatus::Open,
            fee: None,
            fees: Vec::new(),
            trades: Vec::new(),
            stop_price: Some(trigger_price),
            take_profit_price: if condition_type == ConditionType::TakeProfit { Some(trigger_price) } else { None },
            stop_loss_price: if condition_type == ConditionType::StopLoss { Some(trigger_price) } else { None },
            info: serde_json::json!({
                "tx_hash": result.hash,
                "client_id": client_id,
                "order_flags": order_flags::CONDITIONAL,
                "condition_type": format!("{:?}", condition_type),
                "good_til_block_time": good_til_time,
                "trigger_subticks": trigger_subticks,
                "quantums": quantums,
                "subticks": subticks,
            }),
        };

        Ok(order)
    }

    /// 조건부 주문 취소
    ///
    /// # Arguments
    ///
    /// * `id` - 주문 ID
    /// * `symbol` - 거래 심볼
    /// * `client_id` - 클라이언트 ID
    /// * `good_til_block_time` - 원래 주문의 만료 시간
    pub async fn cancel_conditional_order(
        &self,
        id: &str,
        symbol: &str,
        client_id: u32,
        good_til_block_time: u32,
    ) -> CcxtResult<Order> {
        // 지갑 및 관련 컴포넌트 필수
        let wallet = self.require_wallet()?;
        let node_client = self.node_client.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Node client not initialized".into(),
        })?;
        let tx_builder = self.tx_builder.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Transaction builder not initialized".into(),
        })?;

        // 기존 주문 정보 조회
        let existing_order = self.fetch_order(id, symbol).await?;

        // 마켓 정보 가져오기
        let market_info = self.get_or_fetch_market_info(symbol).await?;

        // 계정 정보 조회
        let account = node_client.get_account(wallet.address()).await?;
        let account_number: u64 = account.account_number.parse().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to parse account number".into(),
        })?;
        let sequence: u64 = account.sequence.parse().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to parse sequence".into(),
        })?;

        // OrderId 생성 (Conditional 플래그 사용)
        let order_id = OrderId::new(
            SubaccountId::new(wallet.address(), self.subaccount_number),
            client_id,
            order_flags::CONDITIONAL,
            market_info.clob_pair_id,
        );

        // 트랜잭션 빌드 및 서명
        let tx_bytes = tx_builder.build_cancel_order_tx(
            wallet,
            order_id,
            GoodTilOneof::GoodTilBlockTime(good_til_block_time),
            account_number,
            sequence,
            "", // memo
        )?;

        // 트랜잭션 브로드캐스트 (동기)
        let result = node_client.broadcast_tx_sync(&tx_bytes).await?;

        // 취소된 주문 반환
        let mut cancelled_order = existing_order;
        cancelled_order.status = OrderStatus::Canceled;
        cancelled_order.info = serde_json::json!({
            "cancel_tx_hash": result.hash,
            "order_flags": order_flags::CONDITIONAL,
            "original_order": cancelled_order.info,
        });

        Ok(cancelled_order)
    }
}

// === Response Types ===

#[derive(Debug, Deserialize)]
struct DydxV4PerpetualMarketsResponse {
    #[serde(default)]
    markets: HashMap<String, DydxV4MarketInfo>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DydxV4MarketInfo {
    ticker: Option<String>,
    status: Option<String>,
    clob_pair_id: Option<String>,
    oracle_price: Option<String>,
    price_change_24h: Option<String>,
    volume_24h: Option<String>,
    trades_24h: Option<i64>,
    next_funding_rate: Option<String>,
    initial_margin_fraction: Option<String>,
    maintenance_margin_fraction: Option<String>,
    open_interest: Option<String>,
    atomic_resolution: Option<i32>,
    quantum_conversion_exponent: Option<i32>,
    tick_size: Option<String>,
    step_size: Option<String>,
    step_base_quantums: Option<i64>,
    subticks_per_tick: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DydxV4OrderBookResponse {
    #[serde(default)]
    bids: Vec<DydxV4OrderBookEntry>,
    #[serde(default)]
    asks: Vec<DydxV4OrderBookEntry>,
}

#[derive(Debug, Deserialize)]
struct DydxV4OrderBookEntry {
    price: String,
    size: String,
}

#[derive(Debug, Deserialize)]
struct DydxV4TradesResponse {
    #[serde(default)]
    trades: Vec<DydxV4Trade>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DydxV4Trade {
    id: String,
    side: String,
    size: String,
    price: String,
    #[serde(rename = "type")]
    trade_type: Option<String>,
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DydxV4CandlesResponse {
    #[serde(default)]
    candles: Vec<DydxV4Candle>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DydxV4Candle {
    started_at: Option<String>,
    open: String,
    high: String,
    low: String,
    close: String,
    base_token_volume: Option<String>,
    usd_volume: Option<String>,
    trades: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DydxV4SubaccountResponse {
    subaccount: DydxV4Subaccount,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DydxV4Subaccount {
    address: Option<String>,
    subaccount_number: Option<u32>,
    equity: Option<String>,
    free_collateral: Option<String>,
    pending_perpetual_positions: Option<serde_json::Value>,
    open_perpetual_positions: Option<HashMap<String, DydxV4PerpetualPosition>>,
    asset_positions: Option<HashMap<String, DydxV4AssetPosition>>,
    margin_enabled: Option<bool>,
    updated_at_height: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DydxV4PerpetualPosition {
    market: Option<String>,
    status: Option<String>,
    side: Option<String>,
    size: Option<String>,
    max_size: Option<String>,
    entry_price: Option<String>,
    exit_price: Option<String>,
    realized_pnl: Option<String>,
    unrealized_pnl: Option<String>,
    sum_open: Option<String>,
    sum_close: Option<String>,
    net_funding: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DydxV4AssetPosition {
    symbol: Option<String>,
    side: Option<String>,
    size: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DydxV4Order {
    pub id: Option<String>,
    pub subaccount_id: Option<String>,
    pub client_id: Option<String>,
    pub clob_pair_id: Option<String>,
    pub side: String,
    pub size: String,
    pub price: String,
    pub status: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub time_in_force: Option<String>,
    pub post_only: Option<bool>,
    pub reduce_only: Option<bool>,
    pub order_flags: Option<String>,
    pub good_til_block: Option<String>,
    pub good_til_block_time: Option<String>,
    pub ticker: String,
    pub total_filled: Option<String>,
    pub trigger_price: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub created_at_height: Option<String>,
    pub updated_at_height: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DydxV4PositionsResponse {
    #[serde(default)]
    positions: Vec<DydxV4Position>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DydxV4Position {
    pub market: String,
    pub status: String,
    pub side: String,
    pub size: String,
    pub max_size: Option<String>,
    pub entry_price: String,
    pub exit_price: Option<String>,
    pub realized_pnl: Option<String>,
    pub unrealized_pnl: String,
    pub created_at: Option<String>,
    pub closed_at: Option<String>,
    pub sum_open: Option<String>,
    pub sum_close: Option<String>,
    pub net_funding: Option<String>,
    pub subaccount_number: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct DydxV4FillsResponse {
    #[serde(default)]
    fills: Vec<DydxV4Fill>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DydxV4Fill {
    pub id: String,
    pub side: String,
    pub liquidity: Option<String>,
    #[serde(rename = "type")]
    pub fill_type: Option<String>,
    pub market: String,
    pub market_type: Option<String>,
    pub price: String,
    pub size: String,
    pub fee: Option<String>,
    pub created_at: String,
    pub created_at_height: Option<String>,
    pub order_id: Option<String>,
    pub client_metadata: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DydxV4HistoricalPnlResponse {
    #[serde(default)]
    historical_pnl: Vec<DydxV4PnlTick>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DydxV4PnlTick {
    pub id: String,
    pub subaccount_id: String,
    pub equity: String,
    pub total_pnl: String,
    pub net_transfers: String,
    pub created_at: String,
    pub block_height: String,
    pub block_time: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DydxV4FundingPaymentsResponse {
    #[serde(default)]
    funding_payments: Vec<DydxV4FundingPayment>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DydxV4FundingPayment {
    pub id: String,
    pub subaccount_id: Option<String>,
    pub market: String,
    pub payment: String,
    pub rate: String,
    pub position_size: String,
    pub effective_at: String,
    pub effective_at_height: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DydxV4HistoricalFundingResponse {
    #[serde(default)]
    historical_funding: Vec<DydxV4HistoricalFunding>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DydxV4HistoricalFunding {
    pub ticker: String,
    pub rate: String,
    pub price: String,
    pub effective_at: String,
    pub effective_at_height: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = DydxV4::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Dydx);
        assert_eq!(exchange.name(), "dYdX v4");
        assert!(exchange.has().swap);
        assert!(!exchange.has().spot);
    }

    #[test]
    fn test_testnet_creation() {
        let config = ExchangeConfig::new();
        let exchange = DydxV4::testnet(config).unwrap();
        assert!(exchange.testnet);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = DydxV4::new(config).unwrap();

        // Symbol to market ID
        assert_eq!(exchange.symbol_to_market_id("BTC/USD"), "BTC-USD-PERP");
        assert_eq!(exchange.symbol_to_market_id("ETH/USD"), "ETH-USD-PERP");
        assert_eq!(exchange.symbol_to_market_id("BTC-USD-PERP"), "BTC-USD-PERP");

        // Market ID to symbol
        assert_eq!(exchange.market_id_to_symbol("BTC-USD-PERP"), "BTC/USD");
        assert_eq!(exchange.market_id_to_symbol("ETH-USD-PERP"), "ETH/USD");
    }

    #[test]
    fn test_timeframes() {
        let config = ExchangeConfig::new();
        let exchange = DydxV4::new(config).unwrap();
        assert_eq!(exchange.timeframes().get(&Timeframe::Minute1), Some(&"1MIN".to_string()));
        assert_eq!(exchange.timeframes().get(&Timeframe::Hour1), Some(&"1HOUR".to_string()));
        assert_eq!(exchange.timeframes().get(&Timeframe::Day1), Some(&"1DAY".to_string()));
    }

    #[test]
    fn test_with_address() {
        let config = ExchangeConfig::new();
        let exchange = DydxV4::with_address(
            config,
            "dydx1abc123...",
            true, // testnet
        ).unwrap();

        assert!(exchange.testnet);
        assert_eq!(exchange.address, Some("dydx1abc123...".to_string()));
        assert!(exchange.has().fetch_balance);
        assert!(exchange.has().fetch_open_orders);
    }

    #[test]
    fn test_subaccount_setting() {
        let config = ExchangeConfig::new();
        let mut exchange = DydxV4::new(config).unwrap();

        assert_eq!(exchange.subaccount_number, 0);
        exchange.set_subaccount(1);
        assert_eq!(exchange.subaccount_number, 1);
    }
}
