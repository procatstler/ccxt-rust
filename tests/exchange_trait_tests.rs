//! TDD Tests for Exchange trait
//! Red Phase: These tests should FAIL initially

use ccxt_rust::Exchange;
use shared_types::common::Exchange as ExchangeId;

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
    use ccxt_rust::ExchangeConfig;

    let config = ExchangeConfig::new(ExchangeId::Upbit)
        .with_api_key("test_key")
        .with_api_secret("test_secret");

    assert_eq!(config.exchange_id(), ExchangeId::Upbit);
    assert_eq!(config.api_key(), Some("test_key"));
    assert_eq!(config.api_secret(), Some("test_secret"));
}

/// Rate limiter 테스트
#[tokio::test]
async fn test_rate_limiter() {
    use ccxt_rust::RateLimiter;

    let limiter = RateLimiter::new(10); // 10 requests per second

    // 첫 번째 요청은 즉시 통과
    assert!(limiter.try_acquire().is_ok());
}
