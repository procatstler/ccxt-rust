//! Hyperliquid DEX Exchange Implementation
//!
//! Hyperliquid DEX 거래소 API 구현
//! A high-performance L1 blockchain optimized for perpetual futures trading

#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::crypto::evm::{EvmWallet, Eip712Domain, Eip712TypedData, TypedDataField, keccak256};
use crate::crypto::common::Signer;
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, TimeInForce, OHLCV,
    Position, PositionSide, MarginMode, FundingRate, FundingRateHistory, Trade,
};

const BASE_URL: &str = "https://api.hyperliquid.xyz";
const TESTNET_URL: &str = "https://api.hyperliquid-testnet.xyz";
const RATE_LIMIT_MS: u64 = 50; // 1200 requests per minute = 20 per second

/// Hyperliquid DEX exchange structure
pub struct Hyperliquid {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    /// Maps base asset index to symbol (e.g., 0 -> "BTC/USDC:USDC")
    asset_index_to_symbol: RwLock<HashMap<i64, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    sandbox: bool,
    /// EVM wallet for EIP-712 signing (optional - set via from_private_key)
    wallet: Option<EvmWallet>,
}

// ============ Response Types ============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidMetaResponse {
    universe: Vec<HyperliquidAsset>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidAsset {
    name: String,
    sz_decimals: i32,
    #[serde(default)]
    max_leverage: Option<i32>,
    #[serde(default)]
    only_isolated: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidAssetCtx {
    #[serde(default)]
    funding: Option<String>,
    #[serde(default)]
    open_interest: Option<String>,
    #[serde(default)]
    prev_day_px: Option<String>,
    #[serde(default)]
    day_ntl_vlm: Option<String>,
    #[serde(default)]
    premium: Option<String>,
    #[serde(default)]
    oracle_px: Option<String>,
    #[serde(default)]
    mark_px: Option<String>,
    #[serde(default)]
    mid_px: Option<String>,
    #[serde(default)]
    impact_pxs: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidSpotMeta {
    tokens: Vec<HyperliquidToken>,
    universe: Vec<HyperliquidSpotMarket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidToken {
    name: String,
    sz_decimals: i32,
    #[serde(default)]
    wei_decimals: Option<i32>,
    index: i32,
    #[serde(default)]
    token_id: Option<String>,
    #[serde(default)]
    full_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidSpotMarket {
    name: String,
    tokens: Vec<i32>,
    index: i32,
    #[serde(default)]
    is_canonical: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidL2Book {
    coin: String,
    levels: Vec<Vec<HyperliquidLevel>>,
    time: String,
}

#[derive(Debug, Deserialize)]
struct HyperliquidLevel {
    px: String,
    sz: String,
    n: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidOhlcv {
    #[serde(rename = "t")]
    timestamp: i64,
    #[serde(rename = "o")]
    open: String,
    #[serde(rename = "h")]
    high: String,
    #[serde(rename = "l")]
    low: String,
    #[serde(rename = "c")]
    close: String,
    #[serde(rename = "v")]
    volume: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidClearinghouseState {
    margin_summary: HyperliquidMarginSummary,
    cross_margin_summary: Option<HyperliquidMarginSummary>,
    asset_positions: Vec<HyperliquidAssetPosition>,
    withdrawable: String,
    time: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidMarginSummary {
    account_value: String,
    total_margin_used: String,
    total_ntl_pos: String,
    total_raw_usd: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidAssetPosition {
    position: HyperliquidPosition,
    #[serde(rename = "type")]
    position_type: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidPosition {
    coin: String,
    entry_px: Option<String>,
    leverage: Option<HyperliquidLeverage>,
    liquidation_px: Option<String>,
    margin_used: String,
    position_value: String,
    return_on_equity: String,
    szi: String,
    unrealized_pnl: String,
    cum_funding: Option<HyperliquidCumFunding>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidLeverage {
    #[serde(rename = "type")]
    leverage_type: String,
    value: i32,
    raw_usd: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidCumFunding {
    all_time: String,
    since_open: String,
    since_change: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidSpotBalance {
    balances: Vec<HyperliquidSpotBalanceEntry>,
}

#[derive(Debug, Deserialize)]
struct HyperliquidSpotBalanceEntry {
    coin: String,
    hold: String,
    total: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidOrderResponse {
    status: String,
    response: Option<HyperliquidOrderResponseData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidOrderResponseData {
    #[serde(rename = "type")]
    response_type: String,
    data: Option<HyperliquidOrderData>,
}

#[derive(Debug, Deserialize)]
struct HyperliquidOrderData {
    statuses: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidOrderStatus {
    order: Option<HyperliquidOrderInfo>,
    status: String,
    status_timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidOrderInfo {
    coin: String,
    side: String,
    #[serde(rename = "limitPx")]
    limit_px: String,
    sz: String,
    oid: i64,
    timestamp: String,
    #[serde(default)]
    orig_sz: Option<String>,
    #[serde(default)]
    cloid: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    tif: Option<String>,
    #[serde(default)]
    reduce_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidOpenOrder {
    coin: String,
    #[serde(rename = "limitPx")]
    limit_px: String,
    oid: i64,
    #[serde(default)]
    orig_sz: Option<String>,
    side: String,
    sz: String,
    timestamp: i64,
    #[serde(default)]
    cloid: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidFundingHistory {
    coin: String,
    funding_rate: String,
    premium: String,
    time: i64,
}

impl Hyperliquid {
    /// Create a new Hyperliquid instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let sandbox = config.is_sandbox();
        let base_url = if sandbox { TESTNET_URL } else { BASE_URL };

        let client = HttpClient::new(base_url, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), base_url.into());
        api_urls.insert("private".into(), base_url.into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/ccxt/ccxt/assets/43336371/b371bc6c-4a8c-489f-87f4-20a913dd8d4b".into()),
            api: api_urls,
            www: Some("https://hyperliquid.xyz".into()),
            doc: vec![
                "https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api".into(),
            ],
            fees: Some("https://hyperliquid.gitbook.io/hyperliquid-docs/trading/fees".into()),
        };

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
            swap: true,
            future: true,
            option: false,
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
            cancel_all_orders: false,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            fetch_deposit_address: false,
            ws: true,
            watch_ticker: true,
            watch_tickers: true,
            watch_order_book: true,
            watch_trades: true,
            watch_ohlcv: true,
            watch_balance: true,
            watch_orders: true,
            watch_my_trades: true,
            fetch_funding_rate: false,
            fetch_funding_rates: true,
            fetch_funding_rate_history: true,
            fetch_positions: true,
            fetch_position: true,
            set_leverage: true,
            set_margin_mode: true,
            add_margin: true,
            reduce_margin: true,
            transfer: true,
            edit_order: true,
            create_orders: true,
            cancel_orders: true,
            ..Default::default()
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".to_string());
        timeframes.insert(Timeframe::Minute3, "3m".to_string());
        timeframes.insert(Timeframe::Minute5, "5m".to_string());
        timeframes.insert(Timeframe::Minute15, "15m".to_string());
        timeframes.insert(Timeframe::Minute30, "30m".to_string());
        timeframes.insert(Timeframe::Hour1, "1h".to_string());
        timeframes.insert(Timeframe::Hour2, "2h".to_string());
        timeframes.insert(Timeframe::Hour4, "4h".to_string());
        timeframes.insert(Timeframe::Hour8, "8h".to_string());
        timeframes.insert(Timeframe::Hour12, "12h".to_string());
        timeframes.insert(Timeframe::Day1, "1d".to_string());
        timeframes.insert(Timeframe::Day3, "3d".to_string());
        timeframes.insert(Timeframe::Week1, "1w".to_string());
        timeframes.insert(Timeframe::Month1, "1M".to_string());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            asset_index_to_symbol: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            sandbox,
            wallet: None,
        })
    }

    /// Create a new Hyperliquid instance with a private key for signing
    ///
    /// # Arguments
    ///
    /// * `config` - Exchange configuration (api_key should be wallet address)
    /// * `private_key` - Hex-encoded private key (with or without 0x prefix)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let config = ExchangeConfig::default()
    ///     .with_api_key("0x...your_wallet_address...")
    ///     .with_sandbox(true);
    /// let exchange = Hyperliquid::from_private_key(config, "0x...your_private_key...")?;
    /// ```
    pub fn from_private_key(config: ExchangeConfig, private_key: &str) -> CcxtResult<Self> {
        let wallet = EvmWallet::from_private_key(private_key)?;

        let mut instance = Self::new(config)?;
        instance.wallet = Some(wallet);

        Ok(instance)
    }

    /// Set the wallet for signing (allows setting wallet after creation)
    pub fn set_wallet(&mut self, wallet: EvmWallet) {
        self.wallet = Some(wallet);
    }

    /// Get the wallet address (from wallet if available, otherwise from config)
    fn get_wallet_address_from_wallet(&self) -> CcxtResult<String> {
        if let Some(ref wallet) = self.wallet {
            Ok(wallet.address().to_string())
        } else {
            self.get_wallet_address()
        }
    }

    /// Compute action hash for L1 signing
    ///
    /// keccak256(abi.encodePacked(action_hash, vault_or_zero, nonce))
    fn action_hash(&self, action: &Value, vault_address: Option<&str>, nonce: u64) -> [u8; 32] {
        // Serialize action to msgpack
        let action_bytes = rmp_serde::to_vec_named(action)
            .unwrap_or_else(|_| serde_json::to_vec(action).unwrap_or_default());
        let action_hash = keccak256(&action_bytes);

        // Encode: action_hash (32 bytes) + vault_or_zero (20 bytes) + nonce (8 bytes)
        let mut data = Vec::with_capacity(60);
        data.extend_from_slice(&action_hash);

        // Vault address or zero address (20 bytes)
        if let Some(vault) = vault_address {
            let vault_hex = vault.strip_prefix("0x").unwrap_or(vault);
            if let Ok(vault_bytes) = hex::decode(vault_hex) {
                if vault_bytes.len() == 20 {
                    data.extend_from_slice(&vault_bytes);
                } else {
                    data.extend_from_slice(&[0u8; 20]);
                }
            } else {
                data.extend_from_slice(&[0u8; 20]);
            }
        } else {
            data.extend_from_slice(&[0u8; 20]);
        }

        // Nonce (8 bytes big endian)
        data.extend_from_slice(&nonce.to_be_bytes());

        keccak256(&data)
    }

    /// Construct phantom agent for EIP-712 signing
    ///
    /// Returns (source, connectionId) where:
    /// - source: "a" for testnet (Arbitrum Sepolia), "b" for mainnet (Arbitrum One)
    /// - connectionId: bytes32 hash
    fn construct_phantom_agent(&self, hash: &[u8; 32]) -> (String, [u8; 32]) {
        let source = if self.sandbox { "a" } else { "b" };
        (source.to_string(), *hash)
    }

    /// Get chain ID for EIP-712 domain
    fn get_chain_id(&self) -> u64 {
        if self.sandbox {
            421614 // Arbitrum Sepolia
        } else {
            42161 // Arbitrum One
        }
    }

    /// Sign L1 action using EIP-712
    ///
    /// # Arguments
    ///
    /// * `action` - The action to sign
    /// * `nonce` - Unique nonce for the action
    /// * `vault_address` - Optional vault address
    ///
    /// # Returns
    ///
    /// Object with signature components (r, s, v) and typed data
    async fn sign_l1_action(
        &self,
        action: &Value,
        nonce: u64,
        vault_address: Option<&str>,
    ) -> CcxtResult<Value> {
        let wallet = self.wallet.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Wallet not set. Use from_private_key() or set_wallet() before signing.".to_string(),
        })?;

        // Compute action hash
        let hash = self.action_hash(action, vault_address, nonce);
        let (source, connection_id) = self.construct_phantom_agent(&hash);

        // Build EIP-712 domain
        let chain_id = self.get_chain_id();
        let domain = Eip712Domain::new("HyperliquidSignTransaction", "1", chain_id)
            .with_verifying_contract("0x0000000000000000000000000000000000000000");

        // Build types for Agent
        let mut types = HashMap::new();
        types.insert(
            "Agent".to_string(),
            vec![
                TypedDataField::new("source", "string"),
                TypedDataField::new("connectionId", "bytes32"),
            ],
        );

        // Build message
        let connection_id_hex = format!("0x{}", hex::encode(connection_id));
        let message = json!({
            "source": source,
            "connectionId": connection_id_hex
        });

        // Create typed data
        let typed_data = Eip712TypedData::new(domain, "Agent", types, message.clone());

        // Sign
        let signature = wallet.sign_typed_data(&typed_data).await?;

        // Return signature in format expected by Hyperliquid
        Ok(json!({
            "r": format!("0x{}", hex::encode(signature.r)),
            "s": format!("0x{}", hex::encode(signature.s)),
            "v": signature.v
        }))
    }

    /// Make a signed POST request to /exchange endpoint
    async fn private_post_exchange(&self, action: Value, nonce: u64, vault_address: Option<&str>) -> CcxtResult<Value> {
        self.rate_limiter.throttle(1.0).await;

        // Sign the action
        let signature = self.sign_l1_action(&action, nonce, vault_address).await?;

        // Build request payload
        let payload = json!({
            "action": action,
            "nonce": nonce,
            "signature": signature,
            "vaultAddress": vault_address
        });

        self.client.post("/exchange", Some(payload), None).await
    }

    /// Generate nonce based on current timestamp
    fn generate_nonce(&self) -> u64 {
        chrono::Utc::now().timestamp_millis() as u64
    }

    /// Make a public POST request to /info endpoint
    async fn public_post_info(&self, body: Value) -> CcxtResult<Value> {
        self.rate_limiter.throttle(1.0).await;

        self.client.post("/info", Some(body), None).await
    }

    /// Get wallet address from config (uses api_key as wallet address for Hyperliquid)
    fn get_wallet_address(&self) -> CcxtResult<String> {
        self.config.api_key()
            .map(|s| s.to_string())
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "walletAddress (api_key) is required for Hyperliquid".to_string()
            })
    }

    /// Parse a swap market from the meta response
    fn parse_swap_market(&self, index: i64, asset: &HyperliquidAsset, _ctx: Option<&HyperliquidAssetCtx>) -> Market {
        let base = asset.name.clone();
        let quote = "USDC".to_string();
        let settle = "USDC".to_string();
        let symbol = format!("{base}/{quote}:{settle}");

        let sz_decimals = asset.sz_decimals;
        let max_leverage = asset.max_leverage.unwrap_or(50);

        // Precision as number of decimal places
        let amount_precision = sz_decimals;
        let price_precision = 5; // 5 significant digits for price

        // Min amount as Decimal for limits
        let min_amount = Decimal::from_str(&format!("0.{:0>width$}1", "", width = sz_decimals as usize))
            .unwrap_or_else(|_| Decimal::new(1, 2));

        Market {
            id: base.clone(),
            lowercase_id: Some(base.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: index.to_string(),
            quote_id: "0".to_string(),
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
            sub_type: None,
            settle: Some(settle),
            settle_id: Some("0".to_string()),
            contract_size: Some(Decimal::ONE),
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            taker: Some(Decimal::new(45, 5)), // 0.00045 = 0.045%
            maker: Some(Decimal::new(15, 5)), // 0.00015 = 0.015%
            precision: MarketPrecision {
                amount: Some(amount_precision),
                price: Some(price_precision),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: MinMax {
                    min: Some(min_amount),
                    max: None,
                },
                price: MinMax {
                    min: Some(Decimal::new(1, price_precision as u32)),
                    max: None,
                },
                cost: MinMax {
                    min: Some(Decimal::new(10, 0)), // Minimum $10 order
                    max: None,
                },
                leverage: MinMax {
                    min: Some(Decimal::ONE),
                    max: Some(Decimal::from(max_leverage)),
                },
            },
            margin_modes: None,
            created: None,
            tier_based: false,
            percentage: true,
            info: json!({
                "name": asset.name,
                "szDecimals": asset.sz_decimals,
                "maxLeverage": max_leverage,
                "onlyIsolated": asset.only_isolated,
            }),
        }
    }

    /// Parse a spot market
    fn parse_spot_market(
        &self,
        market: &HyperliquidSpotMarket,
        tokens: &[HyperliquidToken],
        _ctx: Option<&Value>,
    ) -> Option<Market> {
        if market.tokens.len() < 2 {
            return None;
        }

        let base_idx = market.tokens[0] as usize;
        let quote_idx = market.tokens[1] as usize;

        let base_token = tokens.get(base_idx)?;
        let quote_token = tokens.get(quote_idx)?;

        let base = base_token.name.clone();
        let quote = quote_token.name.clone();
        let symbol = format!("{base}/{quote}");

        let sz_decimals = base_token.sz_decimals;
        let min_amount = Decimal::from_str(&format!("0.{:0>width$}1", "", width = sz_decimals as usize))
            .unwrap_or_else(|_| Decimal::new(1, 2));

        // Spot market ID starts at 10000
        let market_id = 10000 + market.index;

        Some(Market {
            id: market.name.clone(),
            lowercase_id: Some(market.name.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: market_id.to_string(),
            quote_id: quote_idx.to_string(),
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
            settle: None,
            settle_id: None,
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            taker: Some(Decimal::new(7, 4)), // 0.0007 = 0.07%
            maker: Some(Decimal::new(4, 4)), // 0.0004 = 0.04%
            precision: MarketPrecision {
                amount: Some(sz_decimals),
                price: Some(8), // 8 decimal places for spot
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: MinMax {
                    min: Some(min_amount),
                    max: None,
                },
                price: MinMax {
                    min: Some(Decimal::new(1, 8)), // 0.00000001
                    max: None,
                },
                cost: MinMax {
                    min: Some(Decimal::new(10, 0)), // $10
                    max: None,
                },
                leverage: MinMax::default(),
            },
            margin_modes: None,
            created: None,
            tier_based: false,
            percentage: true,
            info: json!({
                "name": market.name,
                "tokens": market.tokens,
                "index": market.index,
                "isCanonical": market.is_canonical,
            }),
        })
    }

    /// Calculate price precision based on significant digits rule
    fn calculate_price_precision(&self, price_str: &str, amount_decimals: i32, max_decimals: i32) -> Decimal {
        let price: f64 = price_str.parse().unwrap_or(0.0);

        if price == 0.0 {
            let sig_digits = 5;
            let precision = (max_decimals - amount_decimals).min(sig_digits);
            let precision_str = format!("0.{:0>width$}1", "", width = precision.max(0) as usize);
            return Decimal::from_str(&precision_str).unwrap_or_else(|_| Decimal::new(1, 5)); // 0.00001
        }

        let integer_part = price.abs() as i64;
        let integer_digits = if integer_part == 0 { 0 } else { integer_part.to_string().len() as i32 };
        let sig_digits = 5.max(integer_digits);
        let price_precision = (max_decimals - amount_decimals).min(sig_digits - integer_digits);

        let precision_str = format!("0.{:0>width$}1", "", width = price_precision.max(0) as usize);
        Decimal::from_str(&precision_str).unwrap_or_else(|_| Decimal::new(1, 5)) // 0.00001
    }

    /// Convert coin name to market ID format
    fn coin_to_market_id(&self, coin: &str) -> String {
        format!("{coin}/USDC:USDC")
    }

    /// Parse order side
    fn parse_order_side(&self, side: &str) -> OrderSide {
        match side.to_uppercase().as_str() {
            "B" | "BUY" => OrderSide::Buy,
            "A" | "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        }
    }

    /// Parse order status
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status.to_lowercase().as_str() {
            "open" | "resting" => OrderStatus::Open,
            "filled" | "closed" => OrderStatus::Closed,
            "canceled" | "cancelled" => OrderStatus::Canceled,
            "rejected" => OrderStatus::Rejected,
            "expired" => OrderStatus::Expired,
            _ => OrderStatus::Open,
        }
    }

    /// Parse time in force string to TimeInForce enum
    fn parse_time_in_force(&self, tif: Option<&str>) -> Option<TimeInForce> {
        match tif?.to_uppercase().as_str() {
            "GTC" => Some(TimeInForce::GTC),
            "IOC" => Some(TimeInForce::IOC),
            "FOK" => Some(TimeInForce::FOK),
            "PO" => Some(TimeInForce::PO),
            _ => Some(TimeInForce::GTC),
        }
    }

    /// Parse an order from Hyperliquid response
    fn parse_order(&self, order_data: &Value, market: Option<&Market>) -> Option<Order> {
        // Handle different order response formats
        let order = if let Some(resting) = order_data.get("resting") {
            // createOrder response
            let oid = resting.get("oid")?.as_i64()?;
            return Some(Order {
                id: oid.to_string(),
                client_order_id: None,
                timestamp: Some(chrono::Utc::now().timestamp_millis()),
                datetime: Some(chrono::Utc::now().to_rfc3339()),
                last_trade_timestamp: None,
                last_update_timestamp: None,
                status: OrderStatus::Open,
                symbol: market.map(|m| m.symbol.clone()).unwrap_or_default(),
                order_type: OrderType::Limit,
                time_in_force: Some(TimeInForce::GTC),
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
                trades: vec![],
                fee: None,
                fees: vec![],
                reduce_only: None,
                post_only: None,
                info: order_data.clone(),
            });
        } else if let Some(filled) = order_data.get("filled") {
            // Immediately filled order response
            let oid = filled.get("oid")?.as_i64()?;
            let total_sz = filled.get("totalSz").and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default();
            let avg_px = filled.get("avgPx").and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str(s).ok());

            return Some(Order {
                id: oid.to_string(),
                client_order_id: None,
                timestamp: Some(chrono::Utc::now().timestamp_millis()),
                datetime: Some(chrono::Utc::now().to_rfc3339()),
                last_trade_timestamp: None,
                last_update_timestamp: None,
                status: OrderStatus::Closed,
                symbol: market.map(|m| m.symbol.clone()).unwrap_or_default(),
                order_type: OrderType::Limit,
                time_in_force: Some(TimeInForce::GTC),
                side: OrderSide::Buy,
                price: avg_px,
                average: avg_px,
                amount: total_sz,
                filled: total_sz,
                remaining: Some(Decimal::ZERO),
                stop_price: None,
                trigger_price: None,
                take_profit_price: None,
                stop_loss_price: None,
                cost: None,
                trades: vec![],
                fee: None,
                fees: vec![],
                reduce_only: None,
                post_only: None,
                info: order_data.clone(),
            });
        } else if order_data.get("order").is_some() {
            // fetchOrder response with nested order
            order_data.get("order")?
        } else {
            order_data
        };

        let coin = order.get("coin").and_then(|v| v.as_str())?;
        let side_str = order.get("side").and_then(|v| v.as_str()).unwrap_or("B");
        let limit_px = order.get("limitPx").and_then(|v| v.as_str());
        let sz = order.get("sz").and_then(|v| v.as_str());
        let orig_sz = order.get("origSz").and_then(|v| v.as_str());
        let oid = order.get("oid").and_then(|v| {
            v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })?;
        let timestamp = order.get("timestamp").and_then(|v| {
            v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        });
        let cloid = order.get("cloid").and_then(|v| v.as_str()).map(|s| s.to_string());
        let tif_str = order.get("tif").and_then(|v| v.as_str());

        let status_str = order_data.get("status").and_then(|v| v.as_str())
            .or_else(|| order_data.get("ccxtStatus").and_then(|v| v.as_str()))
            .unwrap_or("open");

        let symbol = self.coin_to_market_id(coin);
        let side = self.parse_order_side(side_str);
        let status = self.parse_order_status(status_str);

        let amount = orig_sz.or(sz).and_then(|s| Decimal::from_str(s).ok()).unwrap_or_default();
        let filled_amount = if status == OrderStatus::Closed {
            amount
        } else {
            sz.and_then(|s| Decimal::from_str(s).ok()).unwrap_or_default()
        };
        let remaining = if amount > filled_amount {
            Some(amount - filled_amount)
        } else {
            Some(Decimal::ZERO)
        };

        Some(Order {
            id: oid.to_string(),
            client_order_id: cloid,
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol,
            order_type: OrderType::Limit,
            time_in_force: self.parse_time_in_force(tif_str),
            side,
            price: limit_px.and_then(|s| Decimal::from_str(s).ok()),
            average: None,
            amount,
            filled: filled_amount,
            remaining,
            stop_price: None,
            trigger_price: order.get("triggerPx").and_then(|v| v.as_str()).and_then(|s| Decimal::from_str(s).ok()),
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: vec![],
            fee: None,
            fees: vec![],
            reduce_only: order.get("reduceOnly").and_then(|v| v.as_bool()),
            post_only: None,
            info: order_data.clone(),
        })
    }
}

#[async_trait]
impl Exchange for Hyperliquid {
    fn id(&self) -> ExchangeId {
        ExchangeId::Hyperliquid
    }

    fn name(&self) -> &str {
        "Hyperliquid"
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
        let markets = self.markets.read().ok()?;
        markets.get(symbol).map(|m| m.id.clone())
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let markets_by_id = self.markets_by_id.read().ok()?;
        markets_by_id.get(market_id).cloned()
    }

    fn sign(
        &self,
        path: &str,
        _api: &str,
        method: &str,
        _params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        // Hyperliquid uses wallet-based signing (EIP-712)
        // For now, return a basic request - actual signing requires ethers-rs
        SignedRequest {
            url: format!("{BASE_URL}{path}"),
            method: method.to_string(),
            headers: headers.unwrap_or_default(),
            body: body.map(|s| s.to_string()),
        }
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        if !reload {
            let markets = self.markets.read().unwrap();
            if !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        // Fetch swap markets (perpetuals)
        let swap_request = json!({
            "type": "metaAndAssetCtxs"
        });
        let swap_response = self.public_post_info(swap_request).await?;

        let mut all_markets = HashMap::new();
        let mut markets_by_id = HashMap::new();
        let mut asset_index_to_symbol = HashMap::new();

        // Parse swap markets
        if let Some(meta_array) = swap_response.as_array() {
            if meta_array.len() >= 2 {
                if let Some(meta) = meta_array.first() {
                    if let Some(universe) = meta.get("universe").and_then(|u| u.as_array()) {
                        let ctx_array = meta_array.get(1).and_then(|c| c.as_array());

                        for (i, asset_val) in universe.iter().enumerate() {
                            if let Ok(asset) = serde_json::from_value::<HyperliquidAsset>(asset_val.clone()) {
                                let ctx = ctx_array.and_then(|arr| arr.get(i))
                                    .and_then(|v| serde_json::from_value::<HyperliquidAssetCtx>(v.clone()).ok());

                                let market = self.parse_swap_market(i as i64, &asset, ctx.as_ref());
                                asset_index_to_symbol.insert(i as i64, market.symbol.clone());
                                markets_by_id.insert(market.id.clone(), market.symbol.clone());
                                all_markets.insert(market.symbol.clone(), market);
                            }
                        }
                    }
                }
            }
        }

        // Fetch spot markets
        let spot_request = json!({
            "type": "spotMetaAndAssetCtxs"
        });

        if let Ok(spot_response) = self.public_post_info(spot_request).await {
            if let Some(spot_array) = spot_response.as_array() {
                if spot_array.len() >= 2 {
                    if let Some(meta) = spot_array.first() {
                        if let (Some(tokens), Some(universe)) = (
                            meta.get("tokens").and_then(|t| t.as_array()),
                            meta.get("universe").and_then(|u| u.as_array()),
                        ) {
                            let parsed_tokens: Vec<HyperliquidToken> = tokens.iter()
                                .filter_map(|t| serde_json::from_value(t.clone()).ok())
                                .collect();

                            let ctx_array = spot_array.get(1).and_then(|c| c.as_array());

                            for (i, market_val) in universe.iter().enumerate() {
                                if let Ok(market_data) = serde_json::from_value::<HyperliquidSpotMarket>(market_val.clone()) {
                                    let ctx = ctx_array.and_then(|arr| arr.get(i));

                                    if let Some(market) = self.parse_spot_market(&market_data, &parsed_tokens, ctx) {
                                        let spot_index = 10000 + i as i64;
                                        asset_index_to_symbol.insert(spot_index, market.symbol.clone());
                                        markets_by_id.insert(market.id.clone(), market.symbol.clone());
                                        all_markets.insert(market.symbol.clone(), market);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Update caches
        *self.markets.write().unwrap() = all_markets.clone();
        *self.markets_by_id.write().unwrap() = markets_by_id;
        *self.asset_index_to_symbol.write().unwrap() = asset_index_to_symbol;

        Ok(all_markets)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(true).await?;
        Ok(markets.into_values().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let tickers = self.fetch_tickers(Some(&[symbol])).await?;
        tickers.get(symbol)
            .cloned()
            .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        // Fetch swap tickers
        let swap_request = json!({
            "type": "metaAndAssetCtxs"
        });
        let swap_response = self.public_post_info(swap_request).await?;

        let mut result = HashMap::new();
        let _markets = self.markets.read().unwrap();

        if let Some(meta_array) = swap_response.as_array() {
            if meta_array.len() >= 2 {
                if let (Some(meta), Some(ctx_array)) = (
                    meta_array.first(),
                    meta_array.get(1).and_then(|c| c.as_array()),
                ) {
                    if let Some(universe) = meta.get("universe").and_then(|u| u.as_array()) {
                        for (i, asset_val) in universe.iter().enumerate() {
                            let name = asset_val.get("name").and_then(|n| n.as_str());
                            let ctx = ctx_array.get(i);

                            if let (Some(name), Some(ctx)) = (name, ctx) {
                                let symbol = self.coin_to_market_id(name);

                                if let Some(filter_symbols) = symbols {
                                    if !filter_symbols.iter().any(|s| *s == symbol) {
                                        continue;
                                    }
                                }

                                let mid_px = ctx.get("midPx").and_then(|v| v.as_str())
                                    .and_then(|s| Decimal::from_str(s).ok());
                                let prev_day_px = ctx.get("prevDayPx").and_then(|v| v.as_str())
                                    .and_then(|s| Decimal::from_str(s).ok());
                                let day_ntl_vlm = ctx.get("dayNtlVlm").and_then(|v| v.as_str())
                                    .and_then(|s| Decimal::from_str(s).ok());
                                let mark_px = ctx.get("markPx").and_then(|v| v.as_str())
                                    .and_then(|s| Decimal::from_str(s).ok());
                                let impact_pxs = ctx.get("impactPxs").and_then(|v| v.as_array());

                                let bid = impact_pxs.and_then(|arr| arr.first())
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| Decimal::from_str(s).ok());
                                let ask = impact_pxs.and_then(|arr| arr.get(1))
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| Decimal::from_str(s).ok());

                                let ticker = Ticker {
                                    symbol: symbol.clone(),
                                    timestamp: None,
                                    datetime: None,
                                    high: None,
                                    low: None,
                                    bid,
                                    bid_volume: None,
                                    ask,
                                    ask_volume: None,
                                    vwap: None,
                                    open: None,
                                    close: mid_px,
                                    last: mid_px,
                                    previous_close: prev_day_px,
                                    change: None,
                                    percentage: None,
                                    average: None,
                                    base_volume: None,
                                    quote_volume: day_ntl_vlm,
                                    index_price: None,
                                    mark_price: mark_px,
                                    info: ctx.clone(),
                                };

                                result.insert(symbol, ticker);
                            }
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;

        let coin = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol)
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?;

            if market.swap {
                market.base.clone()
            } else {
                market.id.clone()
            }
        };

        let request = json!({
            "type": "l2Book",
            "coin": coin
        });

        let response = self.public_post_info(request).await?;
        let book: HyperliquidL2Book = serde_json::from_value(response.clone())
            .map_err(|e| CcxtError::ParseError {
                data_type: "order_book".to_string(),
                message: format!("Failed to parse order book: {e}")
            })?;

        let timestamp = book.time.parse::<i64>().ok();
        let datetime = timestamp.map(|t| {
            chrono::DateTime::from_timestamp_millis(t)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        });

        let bids = book.levels.first().map(|levels| {
            levels.iter().map(|level| {
                OrderBookEntry {
                    price: Decimal::from_str(&level.px).unwrap_or_default(),
                    amount: Decimal::from_str(&level.sz).unwrap_or_default(),
                }
            }).collect()
        }).unwrap_or_default();

        let asks = book.levels.get(1).map(|levels| {
            levels.iter().map(|level| {
                OrderBookEntry {
                    price: Decimal::from_str(&level.px).unwrap_or_default(),
                    amount: Decimal::from_str(&level.sz).unwrap_or_default(),
                }
            }).collect()
        }).unwrap_or_default();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp,
            datetime,
            nonce: None,
        })
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        self.load_markets(false).await?;

        let coin = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol)
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?;

            if market.swap {
                market.base.clone()
            } else {
                market.id.clone()
            }
        };

        let interval = self.timeframes.get(&timeframe)
            .cloned()
            .unwrap_or_else(|| "1h".to_string());

        let end_time = chrono::Utc::now().timestamp_millis();
        let start_time = since.unwrap_or_else(|| {
            // Default to 500 candles back
            let limit_val = limit.unwrap_or(500) as i64;
            let timeframe_ms = match timeframe {
                Timeframe::Second1 => 1_000,
                Timeframe::Minute1 => 60_000,
                Timeframe::Minute3 => 180_000,
                Timeframe::Minute5 => 300_000,
                Timeframe::Minute15 => 900_000,
                Timeframe::Minute30 => 1_800_000,
                Timeframe::Hour1 => 3_600_000,
                Timeframe::Hour2 => 7_200_000,
                Timeframe::Hour3 => 10_800_000,
                Timeframe::Hour4 => 14_400_000,
                Timeframe::Hour6 => 21_600_000,
                Timeframe::Hour8 => 28_800_000,
                Timeframe::Hour12 => 43_200_000,
                Timeframe::Day1 => 86_400_000,
                Timeframe::Day3 => 259_200_000,
                Timeframe::Week1 => 604_800_000,
                Timeframe::Month1 => 2_592_000_000,
            };
            end_time - (limit_val * timeframe_ms)
        });

        let request = json!({
            "type": "candleSnapshot",
            "req": {
                "coin": coin,
                "interval": interval,
                "startTime": start_time,
                "endTime": end_time
            }
        });

        let response = self.public_post_info(request).await?;

        let candles: Vec<HyperliquidOhlcv> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "OHLCV".to_string(),
                message: format!("Failed to parse OHLCV: {e}")
            })?;

        let result: Vec<OHLCV> = candles.iter().map(|c| {
            OHLCV {
                timestamp: c.timestamp,
                open: Decimal::from_str(&c.open).unwrap_or_default(),
                high: Decimal::from_str(&c.high).unwrap_or_default(),
                low: Decimal::from_str(&c.low).unwrap_or_default(),
                close: Decimal::from_str(&c.close).unwrap_or_default(),
                volume: Decimal::from_str(&c.volume).unwrap_or_default(),
            }
        }).collect();

        // Apply limit if specified
        if let Some(limit_val) = limit {
            let limit_usize = limit_val as usize;
            if result.len() > limit_usize {
                return Ok(result.into_iter().rev().take(limit_usize).rev().collect());
            }
        }

        Ok(result)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let wallet_address = self.get_wallet_address()?;

        // Fetch perpetual balance
        let perp_request = json!({
            "type": "clearinghouseState",
            "user": wallet_address
        });
        let perp_response = self.public_post_info(perp_request).await?;

        let mut result = Balances::default();
        result.info = perp_response.clone();

        // Check if this is a spot response (has balances array)
        if let Some(balances) = perp_response.get("balances").and_then(|b| b.as_array()) {
            for balance in balances {
                if let (Some(coin), Some(total), Some(hold)) = (
                    balance.get("coin").and_then(|c| c.as_str()),
                    balance.get("total").and_then(|t| t.as_str()),
                    balance.get("hold").and_then(|h| h.as_str()),
                ) {
                    let total_dec = Decimal::from_str(total).unwrap_or_default();
                    let used_dec = Decimal::from_str(hold).unwrap_or_default();
                    let free_dec = total_dec - used_dec;

                    result.currencies.insert(coin.to_string(), Balance {
                        free: Some(free_dec),
                        used: Some(used_dec),
                        total: Some(total_dec),
                        debt: None,
                    });
                }
            }
        } else {
            // Perpetual balance response
            if let Some(margin_summary) = perp_response.get("marginSummary") {
                let account_value = margin_summary.get("accountValue")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or_default();
                let margin_used = margin_summary.get("totalMarginUsed")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or_default();
                let withdrawable = perp_response.get("withdrawable")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or_default();

                result.currencies.insert("USDC".to_string(), Balance {
                    free: Some(withdrawable),
                    used: Some(margin_used),
                    total: Some(account_value),
                    debt: None,
                });
            }

            // Also fetch spot balance
            let spot_request = json!({
                "type": "spotClearinghouseState",
                "user": wallet_address
            });

            if let Ok(spot_response) = self.public_post_info(spot_request).await {
                if let Some(balances) = spot_response.get("balances").and_then(|b| b.as_array()) {
                    for balance in balances {
                        if let (Some(coin), Some(total), Some(hold)) = (
                            balance.get("coin").and_then(|c| c.as_str()),
                            balance.get("total").and_then(|t| t.as_str()),
                            balance.get("hold").and_then(|h| h.as_str()),
                        ) {
                            let total_dec = Decimal::from_str(total).unwrap_or_default();
                            let used_dec = Decimal::from_str(hold).unwrap_or_default();
                            let free_dec = total_dec - used_dec;

                            result.currencies.insert(coin.to_string(), Balance {
                                free: Some(free_dec),
                                used: Some(used_dec),
                                total: Some(total_dec),
                                debt: None,
                            });
                        }
                    }
                }
            }
        }

        let timestamp = perp_response.get("time")
            .and_then(|t| t.as_str())
            .and_then(|s| s.parse::<i64>().ok());
        result.timestamp = timestamp;
        result.datetime = timestamp.map(|t| {
            chrono::DateTime::from_timestamp_millis(t)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        });

        Ok(result)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let wallet_address = self.get_wallet_address()?;
        self.load_markets(false).await?;

        let request = json!({
            "type": "frontendOpenOrders",
            "user": wallet_address
        });

        let response = self.public_post_info(request).await?;

        let orders: Vec<Value> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "orders".to_string(),
                message: format!("Failed to parse orders: {e}")
            })?;

        let _markets = self.markets.read().unwrap();
        let result: Vec<Order> = orders.iter()
            .filter_map(|o| {
                let mut order_with_status = o.clone();
                if order_with_status.get("status").is_none() {
                    order_with_status.as_object_mut()?.insert("ccxtStatus".to_string(), json!("open"));
                }
                self.parse_order(&order_with_status, None)
            })
            .filter(|order| {
                if let Some(sym) = symbol {
                    order.symbol == sym
                } else {
                    true
                }
            })
            .collect();

        Ok(result)
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let wallet_address = self.get_wallet_address()?;
        self.load_markets(false).await?;

        // Determine if id is a client order id (longer than 34 chars)
        let oid: Value = if id.len() >= 34 {
            json!(id)
        } else {
            json!(id.parse::<i64>().map_err(|_| CcxtError::BadRequest { message: "Invalid order ID".to_string() })?)
        };

        let request = json!({
            "type": "orderStatus",
            "user": wallet_address,
            "oid": oid
        });

        let response = self.public_post_info(request).await?;

        let markets = self.markets.read().unwrap();
        let market = markets.get(symbol);

        self.parse_order(&response, market)
            .ok_or_else(|| CcxtError::OrderNotFound { order_id: id.to_string() })
    }

    async fn fetch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>> {
        let wallet_address = self.get_wallet_address()?;
        self.load_markets(false).await?;

        let request = json!({
            "type": "clearinghouseState",
            "user": wallet_address
        });

        let response = self.public_post_info(request).await?;

        let mut positions = Vec::new();

        if let Some(asset_positions) = response.get("assetPositions").and_then(|p| p.as_array()) {
            for pos_data in asset_positions {
                if let Some(position) = pos_data.get("position") {
                    let coin = position.get("coin").and_then(|c| c.as_str()).unwrap_or_default();
                    let symbol = self.coin_to_market_id(coin);

                    // Filter by symbols if specified
                    if let Some(filter_symbols) = symbols {
                        if !filter_symbols.iter().any(|s| *s == symbol) {
                            continue;
                        }
                    }

                    let szi = position.get("szi").and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str(s).ok())
                        .unwrap_or_default();

                    if szi == Decimal::ZERO {
                        continue; // Skip empty positions
                    }

                    let entry_px = position.get("entryPx").and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str(s).ok());
                    let liquidation_px = position.get("liquidationPx").and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str(s).ok());
                    let margin_used = position.get("marginUsed").and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str(s).ok());
                    let position_value = position.get("positionValue").and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str(s).ok());
                    let unrealized_pnl = position.get("unrealizedPnl").and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str(s).ok());
                    let return_on_equity = position.get("returnOnEquity").and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str(s).ok());

                    let leverage_data = position.get("leverage");
                    let leverage = leverage_data.and_then(|l| l.get("value"))
                        .and_then(|v| v.as_i64())
                        .map(Decimal::from);
                    let margin_mode = leverage_data.and_then(|l| l.get("type"))
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_lowercase());

                    let side = if szi > Decimal::ZERO { PositionSide::Long } else { PositionSide::Short };
                    let contracts = szi.abs();
                    let margin_mode_enum = margin_mode.as_deref().map(|m| {
                        match m {
                            "isolated" => MarginMode::Isolated,
                            "cross" => MarginMode::Cross,
                            _ => MarginMode::Unknown,
                        }
                    });

                    positions.push(Position {
                        symbol: symbol.clone(),
                        id: None,
                        info: pos_data.clone(),
                        timestamp: None,
                        datetime: None,
                        contracts: Some(contracts),
                        contract_size: Some(Decimal::ONE),
                        side: Some(side),
                        notional: position_value,
                        leverage,
                        unrealized_pnl,
                        realized_pnl: None,
                        collateral: margin_used,
                        entry_price: entry_px,
                        mark_price: None,
                        liquidation_price: liquidation_px,
                        margin_mode: margin_mode_enum,
                        hedged: Some(false),
                        maintenance_margin: None,
                        maintenance_margin_percentage: None,
                        initial_margin: margin_used,
                        initial_margin_percentage: None,
                        margin_ratio: None,
                        last_update_timestamp: None,
                        last_price: None,
                        stop_loss_price: None,
                        take_profit_price: None,
                        percentage: return_on_equity,
                    });
                }
            }
        }

        Ok(positions)
    }

    async fn fetch_funding_rates(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, FundingRate>> {
        self.load_markets(false).await?;

        let request = json!({
            "type": "metaAndAssetCtxs"
        });
        let response = self.public_post_info(request).await?;

        let mut result = HashMap::new();

        if let Some(meta_array) = response.as_array() {
            if meta_array.len() >= 2 {
                if let (Some(meta), Some(ctx_array)) = (
                    meta_array.first(),
                    meta_array.get(1).and_then(|c| c.as_array()),
                ) {
                    if let Some(universe) = meta.get("universe").and_then(|u| u.as_array()) {
                        for (i, asset_val) in universe.iter().enumerate() {
                            if let Some(name) = asset_val.get("name").and_then(|n| n.as_str()) {
                                let symbol = self.coin_to_market_id(name);

                                if let Some(filter_symbols) = symbols {
                                    if !filter_symbols.iter().any(|s| *s == symbol) {
                                        continue;
                                    }
                                }

                                if let Some(ctx) = ctx_array.get(i) {
                                    let funding_rate = ctx.get("funding")
                                        .and_then(|f| f.as_str())
                                        .and_then(|s| Decimal::from_str(s).ok());
                                    let mark_px = ctx.get("markPx")
                                        .and_then(|m| m.as_str())
                                        .and_then(|s| Decimal::from_str(s).ok());
                                    let oracle_px = ctx.get("oraclePx")
                                        .and_then(|o| o.as_str())
                                        .and_then(|s| Decimal::from_str(s).ok());

                                    // Funding timestamp is next hour
                                    let now = chrono::Utc::now();
                                    let funding_timestamp = ((now.timestamp_millis() / 3600000) + 1) * 3600000;

                                    result.insert(symbol.clone(), FundingRate {
                                        symbol: symbol.clone(),
                                        funding_rate,
                                        timestamp: None,
                                        datetime: None,
                                        funding_timestamp: Some(funding_timestamp),
                                        funding_datetime: Some(
                                            chrono::DateTime::from_timestamp_millis(funding_timestamp)
                                                .map(|dt| dt.to_rfc3339())
                                                .unwrap_or_default()
                                        ),
                                        mark_price: mark_px,
                                        index_price: oracle_px,
                                        interest_rate: None,
                                        estimated_settle_price: None,
                                        next_funding_rate: None,
                                        next_funding_timestamp: None,
                                        next_funding_datetime: None,
                                        previous_funding_rate: None,
                                        previous_funding_timestamp: None,
                                        previous_funding_datetime: None,
                                        interval: Some("1h".to_string()),
                                        info: ctx.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    async fn fetch_funding_rate_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<FundingRateHistory>> {
        self.load_markets(false).await?;

        let base_currency = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol)
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?;
            market.base.clone()
        };

        let limit_val = limit.unwrap_or(500) as i64;
        let end_time = chrono::Utc::now().timestamp_millis();
        let start_time = since.unwrap_or_else(|| end_time - (limit_val * 3600000));

        let request = json!({
            "type": "fundingHistory",
            "coin": base_currency,
            "startTime": start_time,
            "endTime": end_time
        });

        let response = self.public_post_info(request).await?;

        let history: Vec<HyperliquidFundingHistory> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "funding_history".to_string(),
                message: format!("Failed to parse funding history: {e}")
            })?;

        let mut result: Vec<FundingRateHistory> = history.iter().map(|h| {
            let timestamp = h.time;
            FundingRateHistory {
                info: json!({
                    "coin": h.coin,
                    "fundingRate": h.funding_rate,
                    "premium": h.premium,
                    "time": h.time
                }),
                symbol: symbol.to_string(),
                funding_rate: Decimal::from_str(&h.funding_rate).unwrap_or_default(),
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                ),
            }
        }).collect();

        result.sort_by_key(|f| f.timestamp);

        if let Some(limit_val) = limit {
            let limit_usize = limit_val as usize;
            if result.len() > limit_usize {
                result = result.into_iter().rev().take(limit_usize).rev().collect();
            }
        }

        Ok(result)
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.load_markets(false).await?;

        let coin = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol)
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?;

            if market.swap {
                market.base.clone()
            } else {
                market.id.clone()
            }
        };

        let request = json!({
            "type": "recentTrades",
            "coin": coin
        });

        let response = self.public_post_info(request).await?;

        let trades: Vec<Value> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "trades".to_string(),
                message: format!("Failed to parse trades: {e}")
            })?;

        let result: Vec<Trade> = trades.iter()
            .filter_map(|t| {
                let price = t.get("px").and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok())?;
                let amount = t.get("sz").and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok())?;
                let side_str = t.get("side").and_then(|v| v.as_str()).unwrap_or("B");
                let timestamp = t.get("time").and_then(|v| v.as_i64())?;
                let tid = t.get("tid").and_then(|v| v.as_i64()).map(|v| v.to_string());

                let side_string = match side_str.to_uppercase().as_str() {
                    "B" | "BUY" => "buy",
                    "A" | "SELL" => "sell",
                    _ => "buy",
                };

                Some(Trade {
                    id: tid.unwrap_or_else(|| timestamp.to_string()),
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    ),
                    symbol: symbol.to_string(),
                    order: None,
                    trade_type: None,
                    side: Some(side_string.to_string()),
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: Vec::new(),
                    info: t.clone(),
                })
            })
            .collect();

        Ok(result)
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

        // Validate wallet is set
        if self.wallet.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "Wallet not set. Use from_private_key() or set_wallet() before creating orders.".to_string(),
            });
        }

        // Get market info
        let (asset_index, _is_spot) = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol)
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?;

            let is_spot = market.spot;
            let asset_index = if is_spot {
                // Spot market IDs are stored as "10000+index"
                market.base_id.parse::<i64>().unwrap_or(0)
            } else {
                // Swap market - base_id is the asset index
                market.base_id.parse::<i64>().unwrap_or(0)
            };

            (asset_index, is_spot)
        };

        // Determine order type and time in force
        let (order_type_json, limit_px) = match order_type {
            OrderType::Market => {
                // Market orders use "ioc" (immediate or cancel) with aggressive price
                // For buy: use very high price, for sell: use very low price
                let aggressive_price = match side {
                    OrderSide::Buy => "999999999.0".to_string(),
                    OrderSide::Sell => "0.00001".to_string(),
                };
                (json!({"limit": {"tif": "Ioc"}}), aggressive_price)
            }
            OrderType::Limit => {
                let px = price.ok_or_else(|| CcxtError::BadRequest {
                    message: "Limit order requires price".to_string(),
                })?;
                (json!({"limit": {"tif": "Gtc"}}), px.to_string())
            }
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type {order_type:?} not yet supported"),
                });
            }
        };

        let is_buy = match side {
            OrderSide::Buy => true,
            OrderSide::Sell => false,
        };

        // Build order structure
        let order_spec = json!({
            "a": asset_index,
            "b": is_buy,
            "p": limit_px,
            "s": amount.to_string(),
            "r": false,  // reduce_only
            "t": order_type_json
        });

        let action = json!({
            "type": "order",
            "orders": [order_spec],
            "grouping": "na",
            "builder": null
        });

        // Generate nonce and sign
        let nonce = self.generate_nonce();
        let response = self.private_post_exchange(action, nonce, None).await?;

        // Parse response
        let status = response.get("status").and_then(|s| s.as_str()).unwrap_or("error");
        if status != "ok" {
            let error_msg = response.get("response")
                .and_then(|r| r.as_str())
                .unwrap_or("Unknown error");
            return Err(CcxtError::ExchangeError {
                message: format!("Order creation failed: {error_msg}"),
            });
        }

        // Extract order from response
        if let Some(data) = response.get("response").and_then(|r| r.get("data")) {
            if let Some(statuses) = data.get("statuses").and_then(|s| s.as_array()) {
                if let Some(order_status) = statuses.first() {
                    let markets = self.markets.read().unwrap();
                    let market = markets.get(symbol);
                    if let Some(order) = self.parse_order(order_status, market) {
                        return Ok(order);
                    }
                }
            }
        }

        // If we couldn't parse the order, return a minimal order object
        Ok(Order {
            id: "".to_string(),
            client_order_id: None,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            datetime: Some(chrono::Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: Some(TimeInForce::GTC),
            side,
            price,
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: vec![],
            fee: None,
            fees: vec![],
            reduce_only: None,
            post_only: None,
            info: response,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.load_markets(false).await?;

        // Validate wallet is set
        if self.wallet.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "Wallet not set. Use from_private_key() or set_wallet() before canceling orders.".to_string(),
            });
        }

        // Get asset index from symbol
        let asset_index = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol)
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?;

            market.base_id.parse::<i64>().unwrap_or(0)
        };

        // Parse order ID
        let oid: i64 = id.parse().map_err(|_| CcxtError::BadRequest {
            message: format!("Invalid order ID: {id}"),
        })?;

        // Build cancel action
        let cancel_spec = json!({
            "a": asset_index,
            "o": oid
        });

        let action = json!({
            "type": "cancel",
            "cancels": [cancel_spec]
        });

        // Generate nonce and sign
        let nonce = self.generate_nonce();
        let response = self.private_post_exchange(action, nonce, None).await?;

        // Check response
        let status = response.get("status").and_then(|s| s.as_str()).unwrap_or("error");
        if status != "ok" {
            let error_msg = response.get("response")
                .and_then(|r| r.as_str())
                .unwrap_or("Unknown error");
            return Err(CcxtError::ExchangeError {
                message: format!("Order cancellation failed: {error_msg}"),
            });
        }

        // Fetch the order to return its current state
        self.fetch_order(id, symbol).await
    }
}
