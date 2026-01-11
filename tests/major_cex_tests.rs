//! Integration tests for major CEX (Centralized Exchange) implementations
//!
//! Tests for top-tier exchanges: Binance, Kraken, Coinbase, OKX, Bybit, etc.
//!
//! This test file requires the "cex" feature to be enabled.

#![cfg(feature = "cex")]

use ccxt_rust::{Exchange, ExchangeConfig, ExchangeId};

// === Binance Family Tests ===

#[tokio::test]
async fn test_binance_spot_creation() {
    use ccxt_rust::exchanges::Binance;

    let config = ExchangeConfig::new();
    let exchange = Binance::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Binance);
    assert_eq!(exchange.name(), "Binance");
    assert!(exchange.has().spot);
    assert!(exchange.has().margin);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_ticker);
    assert!(exchange.has().fetch_order_book);
    assert!(exchange.has().fetch_trades);
    assert!(exchange.has().fetch_ohlcv);
    assert!(exchange.has().create_order);
    assert!(exchange.has().cancel_order);
}

#[tokio::test]
async fn test_binance_coinm_creation() {
    use ccxt_rust::exchanges::BinanceCoinM;

    let config = ExchangeConfig::new();
    let exchange = BinanceCoinM::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::BinanceCoinM);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
    assert!(exchange.has().fetch_funding_rate);
}

#[tokio::test]
async fn test_binance_usdm_creation() {
    use ccxt_rust::exchanges::BinanceUsdm;

    let config = ExchangeConfig::new();
    let exchange = BinanceUsdm::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::BinanceUsdm);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
}

#[tokio::test]
async fn test_binanceus_creation() {
    use ccxt_rust::exchanges::BinanceUs;

    let config = ExchangeConfig::new();
    let exchange = BinanceUs::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::BinanceUs);
    assert_eq!(exchange.name(), "Binance US");
    assert!(exchange.has().spot);
}

// === Kraken Tests ===

#[tokio::test]
async fn test_kraken_creation() {
    use ccxt_rust::exchanges::Kraken;

    let config = ExchangeConfig::new();
    let exchange = Kraken::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Kraken);
    assert_eq!(exchange.name(), "Kraken");
    assert!(exchange.has().spot);
    assert!(exchange.has().margin);
    assert!(exchange.has().fetch_ticker);
    assert!(exchange.has().fetch_order_book);
    assert!(exchange.has().fetch_trades);
    assert!(exchange.has().fetch_ohlcv);
}

#[tokio::test]
async fn test_kraken_futures_creation() {
    use ccxt_rust::exchanges::KrakenFutures;

    let config = ExchangeConfig::new();
    let exchange = KrakenFutures::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::KrakenFutures);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
}

// === Coinbase Family Tests ===

#[tokio::test]
async fn test_coinbase_creation() {
    use ccxt_rust::exchanges::Coinbase;

    let config = ExchangeConfig::new();
    let exchange = Coinbase::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Coinbase);
    assert!(exchange.has().spot);
    assert!(exchange.has().fetch_ticker);
    assert!(exchange.has().fetch_order_book);
}

#[tokio::test]
async fn test_coinbase_advanced_creation() {
    use ccxt_rust::exchanges::CoinbaseAdvanced;

    let config = ExchangeConfig::new();
    let exchange = CoinbaseAdvanced::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::CoinbaseAdvanced);
    assert!(exchange.has().spot);
}

#[tokio::test]
async fn test_coinbase_exchange_creation() {
    use ccxt_rust::exchanges::CoinbaseExchange;

    let config = ExchangeConfig::new();
    let exchange = CoinbaseExchange::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::CoinbaseExchange);
    assert!(exchange.has().spot);
}

#[tokio::test]
async fn test_coinbase_international_creation() {
    use ccxt_rust::exchanges::CoinbaseInternational;

    let config = ExchangeConfig::new();
    let exchange = CoinbaseInternational::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::CoinbaseInternational);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
}

// === OKX Tests ===

#[tokio::test]
async fn test_okx_creation() {
    use ccxt_rust::exchanges::Okx;

    let config = ExchangeConfig::new();
    let exchange = Okx::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Okx);
    assert_eq!(exchange.name(), "OKX");
    assert!(exchange.has().spot);
    assert!(exchange.has().margin);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().option);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
    assert!(exchange.has().fetch_funding_rate);
    assert!(exchange.has().fetch_open_interest);
}

// === Bybit Tests ===

#[tokio::test]
async fn test_bybit_creation() {
    use ccxt_rust::exchanges::Bybit;

    let config = ExchangeConfig::new();
    let exchange = Bybit::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Bybit);
    assert_eq!(exchange.name(), "Bybit");
    assert!(exchange.has().spot);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().option);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
    assert!(exchange.has().fetch_funding_rate);
}

// === Gate.io Tests ===

#[tokio::test]
async fn test_gate_creation() {
    use ccxt_rust::exchanges::Gate;

    let config = ExchangeConfig::new();
    let exchange = Gate::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Gate);
    assert!(exchange.has().spot);
    assert!(exchange.has().margin);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
}

// === HTX (Huobi) Tests ===

#[tokio::test]
async fn test_htx_creation() {
    use ccxt_rust::exchanges::Htx;

    let config = ExchangeConfig::new();
    let exchange = Htx::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Htx);
    assert!(exchange.has().spot);
    assert!(exchange.has().margin);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
}

// === KuCoin Tests ===

#[tokio::test]
async fn test_kucoin_creation() {
    use ccxt_rust::exchanges::Kucoin;

    let config = ExchangeConfig::new();
    let exchange = Kucoin::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Kucoin);
    assert_eq!(exchange.name(), "Kucoin");
    assert!(exchange.has().spot);
    assert!(exchange.has().margin);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
}

#[tokio::test]
async fn test_kucoin_futures_creation() {
    use ccxt_rust::exchanges::KucoinFutures;

    let config = ExchangeConfig::new();
    let exchange = KucoinFutures::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::KucoinFutures);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().fetch_funding_rate);
}

// === Bitget Tests ===

#[tokio::test]
async fn test_bitget_creation() {
    use ccxt_rust::exchanges::Bitget;

    let config = ExchangeConfig::new();
    let exchange = Bitget::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Bitget);
    assert_eq!(exchange.name(), "Bitget");
    assert!(exchange.has().spot);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
}

// === MEXC Tests ===

#[tokio::test]
async fn test_mexc_creation() {
    use ccxt_rust::exchanges::Mexc;

    let config = ExchangeConfig::new();
    let exchange = Mexc::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Mexc);
    assert_eq!(exchange.name(), "MEXC");
    assert!(exchange.has().spot);
    assert!(exchange.has().swap);
}

// === Bitfinex Tests ===

#[tokio::test]
async fn test_bitfinex_creation() {
    use ccxt_rust::exchanges::Bitfinex;

    let config = ExchangeConfig::new();
    let exchange = Bitfinex::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Bitfinex);
    assert_eq!(exchange.name(), "Bitfinex");
    assert!(exchange.has().spot);
    assert!(exchange.has().margin);
}

// === Bitstamp Tests ===

#[tokio::test]
async fn test_bitstamp_creation() {
    use ccxt_rust::exchanges::Bitstamp;

    let config = ExchangeConfig::new();
    let exchange = Bitstamp::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Bitstamp);
    assert_eq!(exchange.name(), "Bitstamp");
    assert!(exchange.has().spot);
}

// === Gemini Tests ===

#[tokio::test]
async fn test_gemini_creation() {
    use ccxt_rust::exchanges::Gemini;

    let config = ExchangeConfig::new();
    let exchange = Gemini::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Gemini);
    assert_eq!(exchange.name(), "Gemini");
    assert!(exchange.has().spot);
}

// === Crypto.com Tests ===

#[tokio::test]
async fn test_cryptocom_creation() {
    use ccxt_rust::exchanges::CryptoCom;

    let config = ExchangeConfig::new();
    let exchange = CryptoCom::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::CryptoCom);
    assert!(exchange.has().spot);
    assert!(exchange.has().swap);
}

// === BitMEX Tests ===

#[tokio::test]
async fn test_bitmex_creation() {
    use ccxt_rust::exchanges::Bitmex;

    let config = ExchangeConfig::new();
    let exchange = Bitmex::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Bitmex);
    assert_eq!(exchange.name(), "BitMEX");
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
}

// === Deribit Tests ===

#[tokio::test]
async fn test_deribit_creation() {
    use ccxt_rust::exchanges::Deribit;

    let config = ExchangeConfig::new();
    let exchange = Deribit::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Deribit);
    assert_eq!(exchange.name(), "Deribit");
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().option);
}

#[tokio::test]
async fn test_deribit_options_features() {
    use ccxt_rust::exchanges::Deribit;

    let config = ExchangeConfig::new();
    let exchange = Deribit::new(config).unwrap();

    // Deribit should support options trading features
    assert!(exchange.has().option);
    assert!(exchange.has().fetch_option);
    assert!(exchange.has().fetch_option_chain);
    assert!(exchange.has().fetch_greeks);
    assert!(exchange.has().fetch_underlying_assets);
}

// === Exchange Config with Credentials Tests ===

#[tokio::test]
async fn test_binance_with_credentials() {
    use ccxt_rust::exchanges::Binance;

    let config = ExchangeConfig::new()
        .with_api_key("test_api_key")
        .with_api_secret("test_api_secret");

    let exchange = Binance::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Binance);
    // Private API features should be available when credentials are provided
    assert!(exchange.has().fetch_balance);
    assert!(exchange.has().create_order);
    assert!(exchange.has().cancel_order);
}

// === Exchange URL Tests ===

#[tokio::test]
async fn test_exchange_urls() {
    use ccxt_rust::exchanges::Binance;

    let config = ExchangeConfig::new();
    let exchange = Binance::new(config).unwrap();

    let urls = exchange.urls();
    assert!(urls.api.contains_key("public") || urls.api.contains_key("rest"));
}

// === Exchange Timeframes Tests ===

#[tokio::test]
async fn test_exchange_timeframes() {
    use ccxt_rust::exchanges::Binance;
    use ccxt_rust::Timeframe;

    let config = ExchangeConfig::new();
    let exchange = Binance::new(config).unwrap();

    let timeframes = exchange.timeframes();
    // Binance should support common timeframes
    assert!(timeframes.contains_key(&Timeframe::Minute1));
    assert!(timeframes.contains_key(&Timeframe::Hour1));
    assert!(timeframes.contains_key(&Timeframe::Day1));
}

// === Feature Detection Tests ===

#[tokio::test]
async fn test_has_feature_detection() {
    use ccxt_rust::exchanges::Okx;

    let config = ExchangeConfig::new();
    let exchange = Okx::new(config).unwrap();

    // Test has_feature method
    assert!(exchange.has_feature("fetchTicker"));
    assert!(exchange.has_feature("fetchOrderBook"));
    assert!(exchange.has_feature("createOrder"));
    assert!(!exchange.has_feature("nonExistentFeature"));
}
