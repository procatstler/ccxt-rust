//! Integration tests for DEX (Decentralized Exchange) implementations
//!
//! Tests for DEX platforms: Hyperliquid, dYdX, Paradex, etc.
//!
//! This test file requires the "dex" feature to be enabled.

#![cfg(feature = "dex")]

use ccxt_rust::{Exchange, ExchangeConfig, ExchangeId};

// === Hyperliquid Tests ===

#[tokio::test]
async fn test_hyperliquid_creation() {
    use ccxt_rust::exchanges::Hyperliquid;

    let config = ExchangeConfig::new();
    let exchange = Hyperliquid::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Hyperliquid);
    assert_eq!(exchange.name(), "Hyperliquid");
    assert!(exchange.has().swap);
    assert!(exchange.has().fetch_ticker);
    assert!(exchange.has().fetch_order_book);
    assert!(exchange.has().fetch_trades);
    assert!(exchange.has().fetch_ohlcv);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
}

#[tokio::test]
async fn test_hyperliquid_futures_features() {
    use ccxt_rust::exchanges::Hyperliquid;

    let config = ExchangeConfig::new();
    let exchange = Hyperliquid::new(config).unwrap();

    // Hyperliquid is primarily a perpetuals DEX
    assert!(exchange.has().swap);
    // Hyperliquid supports fetch_funding_rates (plural) and fetch_funding_rate_history
    assert!(exchange.has().fetch_funding_rates);
    assert!(exchange.has().fetch_funding_rate_history);
}

// === dYdX Tests ===

#[tokio::test]
async fn test_dydx_creation() {
    use ccxt_rust::exchanges::Dydx;

    let config = ExchangeConfig::new();
    let exchange = Dydx::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Dydx);
    assert_eq!(exchange.name(), "dYdX");
    assert!(exchange.has().swap);
    assert!(exchange.has().fetch_ticker);
    assert!(exchange.has().fetch_order_book);
    assert!(exchange.has().fetch_trades);
}

#[tokio::test]
async fn test_dydx_v4_creation() {
    use ccxt_rust::exchanges::DydxV4;

    let config = ExchangeConfig::new();
    let exchange = DydxV4::new(config).unwrap();

    // dYdX v4 is the Cosmos-based version
    assert_eq!(exchange.name(), "dYdX v4");
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_ticker);
    assert!(exchange.has().fetch_order_book);
    // Note: fetch_positions requires address configuration
}

// === Paradex Tests ===

#[tokio::test]
async fn test_paradex_creation() {
    use ccxt_rust::exchanges::Paradex;

    let config = ExchangeConfig::new();
    let exchange = Paradex::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Paradex);
    assert_eq!(exchange.name(), "Paradex");
    assert!(exchange.has().swap);
    assert!(exchange.has().fetch_ticker);
    assert!(exchange.has().fetch_order_book);
}

// === Apex Tests ===

#[tokio::test]
async fn test_apex_creation() {
    use ccxt_rust::exchanges::Apex;

    let config = ExchangeConfig::new();
    let exchange = Apex::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Apex);
    assert!(exchange.has().swap);
    assert!(exchange.has().fetch_ticker);
    assert!(exchange.has().fetch_order_book);
}

// === Defx Tests ===

#[tokio::test]
async fn test_defx_creation() {
    use ccxt_rust::exchanges::Defx;

    let config = ExchangeConfig::new();
    let exchange = Defx::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Defx);
    assert!(exchange.has().swap);
}

// === Derive Tests ===

#[tokio::test]
async fn test_derive_creation() {
    use ccxt_rust::exchanges::Derive;

    let config = ExchangeConfig::new();
    let exchange = Derive::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Derive);
    assert!(exchange.has().swap);
}

// === Wavesexchange Tests ===

#[tokio::test]
async fn test_wavesexchange_creation() {
    use ccxt_rust::exchanges::Wavesexchange;

    let config = ExchangeConfig::new();
    let exchange = Wavesexchange::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Wavesexchange);
    assert_eq!(exchange.name(), "Waves.Exchange");
    assert!(exchange.has().spot);
}

// === DEX Common Features Tests ===

#[tokio::test]
async fn test_dex_perpetuals_features() {
    use ccxt_rust::exchanges::Hyperliquid;

    let config = ExchangeConfig::new();
    let exchange = Hyperliquid::new(config).unwrap();

    // Common DEX perpetuals features
    let features = exchange.has();
    assert!(features.swap, "DEX perpetuals should support swap");
    assert!(
        features.fetch_order_book,
        "Should support order book fetching"
    );
    assert!(features.fetch_trades, "Should support trade fetching");
}

// === DEX Config Tests ===

#[tokio::test]
async fn test_dex_with_wallet_config() {
    use ccxt_rust::exchanges::Hyperliquid;

    // DEX exchanges typically use wallet-based authentication
    let config = ExchangeConfig::new()
        .with_api_key("0x1234567890abcdef1234567890abcdef12345678")
        .with_api_secret("private_key_placeholder");

    let exchange = Hyperliquid::new(config).unwrap();
    assert_eq!(exchange.id(), ExchangeId::Hyperliquid);
}

// === Backpack (Solana-based Exchange) Tests ===
// NOTE: Backpack is classified as CEX in this library

#[cfg(feature = "cex")]
#[tokio::test]
async fn test_backpack_creation() {
    use ccxt_rust::exchanges::Backpack;

    let config = ExchangeConfig::new();
    let exchange = Backpack::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Backpack);
    assert_eq!(exchange.name(), "Backpack");
    assert!(exchange.has().spot);
    assert!(exchange.has().fetch_ticker);
    assert!(exchange.has().fetch_order_book);
}

// === DEX Specific API Tests ===

#[tokio::test]
async fn test_dex_markets_structure() {
    use ccxt_rust::exchanges::Hyperliquid;

    let config = ExchangeConfig::new();
    let exchange = Hyperliquid::new(config).unwrap();

    // DEX should support fetch_markets
    assert!(exchange.has().fetch_markets);
}

// === Cross-chain DEX Feature Tests ===

#[tokio::test]
async fn test_starknet_based_dex() {
    use ccxt_rust::exchanges::Paradex;

    // Paradex is StarkNet-based
    let config = ExchangeConfig::new();
    let exchange = Paradex::new(config).unwrap();

    assert!(exchange.has().swap);
    assert!(exchange.has().fetch_balance);
    assert!(exchange.has().create_order);
}

#[tokio::test]
async fn test_cosmos_based_dex() {
    use ccxt_rust::exchanges::DydxV4;

    // dYdX v4 is Cosmos-based
    let config = ExchangeConfig::new();
    let exchange = DydxV4::new(config).unwrap();

    assert!(exchange.has().swap);
}

// === DEX Leverage Tests ===

#[tokio::test]
async fn test_dex_leverage_support() {
    use ccxt_rust::exchanges::Hyperliquid;

    let config = ExchangeConfig::new();
    let exchange = Hyperliquid::new(config).unwrap();

    // Hyperliquid supports leverage operations
    assert!(exchange.has().set_leverage);
    assert!(exchange.has().fetch_positions);
}
