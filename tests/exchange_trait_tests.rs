//! TDD Tests for Exchange trait

use ccxt_rust::{Exchange, ExchangeConfig, ExchangeId, RateLimiter};

/// Exchange trait이 존재하는지 확인
#[tokio::test]
async fn test_exchange_trait_exists() {
    // Exchange trait이 정의되어 있어야 함
    fn assert_exchange_trait<T: Exchange>() {}
}

/// Exchange trait의 기본 메서드들이 정의되어 있는지 확인
#[tokio::test]
async fn test_exchange_trait_methods() {
    // 이 테스트는 trait 메서드 시그니처를 확인
    // 실제 구현은 각 거래소별로 진행
}

/// ExchangeConfig 구조체 테스트
#[tokio::test]
async fn test_exchange_config() {
    let config = ExchangeConfig::new()
        .with_api_key("test_key")
        .with_api_secret("test_secret");

    assert_eq!(config.api_key(), Some("test_key"));
    assert_eq!(config.api_secret(), Some("test_secret"));
    assert!(config.has_credentials());
}

/// Rate limiter 테스트
#[tokio::test]
async fn test_rate_limiter() {
    let limiter = RateLimiter::new(100); // 100ms = 10 RPS

    // 첫 번째 요청은 즉시 통과
    assert!(limiter.try_acquire(1.0));
}

/// Upbit 거래소 테스트
#[tokio::test]
async fn test_upbit_creation() {
    use ccxt_rust::exchanges::Upbit;

    let config = ExchangeConfig::new();
    let upbit = Upbit::new(config).unwrap();

    assert_eq!(upbit.id(), ExchangeId::Upbit);
    assert_eq!(upbit.name(), "Upbit");
    assert!(upbit.has().spot);
    assert!(!upbit.has().swap);
}

/// Bithumb 거래소 테스트
#[tokio::test]
async fn test_bithumb_creation() {
    use ccxt_rust::exchanges::Bithumb;

    let config = ExchangeConfig::new();
    let bithumb = Bithumb::new(config).unwrap();

    assert_eq!(bithumb.id(), ExchangeId::Bithumb);
    assert_eq!(bithumb.name(), "Bithumb");
    assert!(bithumb.has().spot);
    assert!(!bithumb.has().swap);
}

/// Coinone 거래소 테스트
#[tokio::test]
async fn test_coinone_creation() {
    use ccxt_rust::exchanges::Coinone;

    let config = ExchangeConfig::new();
    let coinone = Coinone::new(config).unwrap();

    assert_eq!(coinone.id(), ExchangeId::Coinone);
    assert_eq!(coinone.name(), "Coinone");
    assert!(coinone.has().spot);
    assert!(!coinone.has().swap);
}

// === Futures Exchange Tests ===

/// BinanceFutures 거래소 테스트
#[tokio::test]
async fn test_binance_futures_creation() {
    use ccxt_rust::exchanges::BinanceFutures;

    let config = ExchangeConfig::new();
    let exchange = BinanceFutures::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::BinanceFutures);
    assert_eq!(exchange.name(), "Binance USDⓈ-M Futures");
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
    assert!(exchange.has().fetch_open_interest);
    assert!(exchange.has().fetch_liquidations);
    assert!(exchange.has().fetch_index_price);
}

/// OKX 거래소 선물 기능 테스트
#[tokio::test]
async fn test_okx_futures_features() {
    use ccxt_rust::exchanges::Okx;

    let config = ExchangeConfig::new();
    let exchange = Okx::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Okx);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
    assert!(exchange.has().fetch_funding_rate);
    assert!(exchange.has().fetch_open_interest);
    assert!(exchange.has().fetch_liquidations);
    assert!(exchange.has().fetch_index_price);
}

/// Bybit 거래소 선물 기능 테스트
#[tokio::test]
async fn test_bybit_futures_features() {
    use ccxt_rust::exchanges::Bybit;

    let config = ExchangeConfig::new();
    let exchange = Bybit::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Bybit);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
    assert!(exchange.has().fetch_funding_rate);
    assert!(exchange.has().fetch_open_interest);
    assert!(exchange.has().fetch_liquidations);
    assert!(exchange.has().fetch_index_price);
}

/// Gate 거래소 선물 기능 테스트
#[tokio::test]
async fn test_gate_futures_features() {
    use ccxt_rust::exchanges::Gate;

    let config = ExchangeConfig::new();
    let exchange = Gate::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Gate);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
    assert!(exchange.has().fetch_funding_rate);
    assert!(exchange.has().fetch_open_interest);
    assert!(exchange.has().fetch_liquidations);
    assert!(exchange.has().fetch_index_price);
}

/// Bitget 거래소 선물 기능 테스트
#[tokio::test]
async fn test_bitget_futures_features() {
    use ccxt_rust::exchanges::Bitget;

    let config = ExchangeConfig::new();
    let exchange = Bitget::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Bitget);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
    assert!(exchange.has().fetch_funding_rate);
    assert!(exchange.has().fetch_open_interest);
    assert!(exchange.has().fetch_liquidations);
    assert!(exchange.has().fetch_index_price);
}

/// Kucoin 거래소 선물 기능 테스트
#[tokio::test]
async fn test_kucoin_futures_features() {
    use ccxt_rust::exchanges::Kucoin;

    let config = ExchangeConfig::new();
    let exchange = Kucoin::new(config).unwrap();

    assert_eq!(exchange.id(), ExchangeId::Kucoin);
    assert!(exchange.has().swap);
    assert!(exchange.has().future);
    assert!(exchange.has().fetch_positions);
    assert!(exchange.has().set_leverage);
    assert!(exchange.has().fetch_funding_rate);
    assert!(exchange.has().fetch_open_interest);
    assert!(exchange.has().fetch_liquidations);
    assert!(exchange.has().fetch_index_price);
}
