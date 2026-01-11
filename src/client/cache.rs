//! Market data caching system
//!
//! TTL-based cache for reducing API calls and improving performance

use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// 캐시 항목
#[derive(Debug, Clone)]
struct CacheEntry<T> {
    value: T,
    created_at: Instant,
    ttl: Duration,
}

impl<T> CacheEntry<T> {
    fn new(value: T, ttl: Duration) -> Self {
        Self {
            value,
            created_at: Instant::now(),
            ttl,
        }
    }

    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }
}

/// 캐시 설정
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Markets 캐시 TTL (밀리초)
    pub markets_ttl_ms: u64,
    /// Currencies 캐시 TTL (밀리초)
    pub currencies_ttl_ms: u64,
    /// Ticker 캐시 TTL (밀리초)
    pub ticker_ttl_ms: u64,
    /// OrderBook 캐시 TTL (밀리초)
    pub orderbook_ttl_ms: u64,
    /// 캐시 활성화 여부
    pub enabled: bool,
    /// 최대 캐시 항목 수
    pub max_entries: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            markets_ttl_ms: 3600000,    // 1시간
            currencies_ttl_ms: 3600000, // 1시간
            ticker_ttl_ms: 1000,        // 1초
            orderbook_ttl_ms: 100,      // 100ms
            enabled: true,
            max_entries: 10000,
        }
    }
}

impl CacheConfig {
    /// 새로운 캐시 설정 생성
    pub fn new() -> Self {
        Self::default()
    }

    /// 캐시 비활성화
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Markets TTL 설정
    pub fn with_markets_ttl(mut self, ttl_ms: u64) -> Self {
        self.markets_ttl_ms = ttl_ms;
        self
    }

    /// Ticker TTL 설정
    pub fn with_ticker_ttl(mut self, ttl_ms: u64) -> Self {
        self.ticker_ttl_ms = ttl_ms;
        self
    }

    /// OrderBook TTL 설정
    pub fn with_orderbook_ttl(mut self, ttl_ms: u64) -> Self {
        self.orderbook_ttl_ms = ttl_ms;
        self
    }
}

/// 캐시 통계
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// 캐시 히트 수
    pub hits: u64,
    /// 캐시 미스 수
    pub misses: u64,
    /// 현재 캐시 항목 수
    pub entries: usize,
    /// 만료된 항목 수
    pub expired: u64,
}

impl CacheStats {
    /// 캐시 히트율 계산
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// 범용 캐시 저장소
pub struct Cache<T> {
    entries: RwLock<HashMap<String, CacheEntry<T>>>,
    config: CacheConfig,
    stats: RwLock<CacheStats>,
}

impl<T: Clone> Cache<T> {
    /// 새로운 캐시 생성
    pub fn new(config: CacheConfig) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            config,
            stats: RwLock::new(CacheStats::default()),
        }
    }

    /// 캐시에서 값 조회
    pub async fn get(&self, key: &str) -> Option<T> {
        if !self.config.enabled {
            return None;
        }

        let entries = self.entries.read().await;
        if let Some(entry) = entries.get(key) {
            if !entry.is_expired() {
                let mut stats = self.stats.write().await;
                stats.hits += 1;
                return Some(entry.value.clone());
            } else {
                let mut stats = self.stats.write().await;
                stats.expired += 1;
            }
        }

        let mut stats = self.stats.write().await;
        stats.misses += 1;
        None
    }

    /// 캐시에 값 저장
    pub async fn set(&self, key: impl Into<String>, value: T, ttl: Duration) {
        if !self.config.enabled {
            return;
        }

        let key = key.into();
        let mut entries = self.entries.write().await;

        // 최대 항목 수 초과 시 만료된 항목 정리
        if entries.len() >= self.config.max_entries {
            entries.retain(|_, entry| !entry.is_expired());
        }

        entries.insert(key, CacheEntry::new(value, ttl));

        let mut stats = self.stats.write().await;
        stats.entries = entries.len();
    }

    /// 캐시에서 값 삭제
    pub async fn remove(&self, key: &str) {
        let mut entries = self.entries.write().await;
        entries.remove(key);

        let mut stats = self.stats.write().await;
        stats.entries = entries.len();
    }

    /// 캐시 전체 삭제
    pub async fn clear(&self) {
        let mut entries = self.entries.write().await;
        entries.clear();

        let mut stats = self.stats.write().await;
        stats.entries = 0;
    }

    /// 만료된 항목 정리
    pub async fn cleanup(&self) {
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|_, entry| !entry.is_expired());
        let removed = before - entries.len();

        let mut stats = self.stats.write().await;
        stats.expired += removed as u64;
        stats.entries = entries.len();
    }

    /// 캐시 통계 조회
    pub async fn stats(&self) -> CacheStats {
        let stats = self.stats.read().await;
        stats.clone()
    }

    /// 캐시 항목 수 조회
    pub async fn len(&self) -> usize {
        let entries = self.entries.read().await;
        entries.len()
    }

    /// 캐시가 비어있는지 확인
    pub async fn is_empty(&self) -> bool {
        let entries = self.entries.read().await;
        entries.is_empty()
    }
}

/// 거래소 데이터 캐시 관리자
pub struct ExchangeCache {
    config: CacheConfig,
    /// Markets 캐시 (JSON 문자열로 저장)
    markets: Cache<String>,
    /// Currencies 캐시
    currencies: Cache<String>,
    /// Ticker 캐시
    tickers: Cache<String>,
    /// OrderBook 캐시
    orderbooks: Cache<String>,
    /// 범용 캐시
    generic: Cache<String>,
}

impl ExchangeCache {
    /// 새로운 거래소 캐시 생성
    pub fn new(config: CacheConfig) -> Self {
        Self {
            markets: Cache::new(config.clone()),
            currencies: Cache::new(config.clone()),
            tickers: Cache::new(config.clone()),
            orderbooks: Cache::new(config.clone()),
            generic: Cache::new(config.clone()),
            config,
        }
    }

    /// 기본 설정으로 캐시 생성
    pub fn default_config() -> Self {
        Self::new(CacheConfig::default())
    }

    /// Markets 캐시 가져오기
    pub async fn get_markets<T: DeserializeOwned>(&self) -> Option<T> {
        if let Some(json) = self.markets.get("markets").await {
            serde_json::from_str(&json).ok()
        } else {
            None
        }
    }

    /// Markets 캐시 저장
    pub async fn set_markets<T: Serialize>(&self, markets: &T) {
        if let Ok(json) = serde_json::to_string(markets) {
            let ttl = Duration::from_millis(self.config.markets_ttl_ms);
            self.markets.set("markets", json, ttl).await;
        }
    }

    /// Currencies 캐시 가져오기
    pub async fn get_currencies<T: DeserializeOwned>(&self) -> Option<T> {
        if let Some(json) = self.currencies.get("currencies").await {
            serde_json::from_str(&json).ok()
        } else {
            None
        }
    }

    /// Currencies 캐시 저장
    pub async fn set_currencies<T: Serialize>(&self, currencies: &T) {
        if let Ok(json) = serde_json::to_string(currencies) {
            let ttl = Duration::from_millis(self.config.currencies_ttl_ms);
            self.currencies.set("currencies", json, ttl).await;
        }
    }

    /// Ticker 캐시 가져오기
    pub async fn get_ticker<T: DeserializeOwned>(&self, symbol: &str) -> Option<T> {
        let key = format!("ticker:{symbol}");
        if let Some(json) = self.tickers.get(&key).await {
            serde_json::from_str(&json).ok()
        } else {
            None
        }
    }

    /// Ticker 캐시 저장
    pub async fn set_ticker<T: Serialize>(&self, symbol: &str, ticker: &T) {
        if let Ok(json) = serde_json::to_string(ticker) {
            let key = format!("ticker:{symbol}");
            let ttl = Duration::from_millis(self.config.ticker_ttl_ms);
            self.tickers.set(key, json, ttl).await;
        }
    }

    /// OrderBook 캐시 가져오기
    pub async fn get_orderbook<T: DeserializeOwned>(&self, symbol: &str) -> Option<T> {
        let key = format!("orderbook:{symbol}");
        if let Some(json) = self.orderbooks.get(&key).await {
            serde_json::from_str(&json).ok()
        } else {
            None
        }
    }

    /// OrderBook 캐시 저장
    pub async fn set_orderbook<T: Serialize>(&self, symbol: &str, orderbook: &T) {
        if let Ok(json) = serde_json::to_string(orderbook) {
            let key = format!("orderbook:{symbol}");
            let ttl = Duration::from_millis(self.config.orderbook_ttl_ms);
            self.orderbooks.set(key, json, ttl).await;
        }
    }

    /// 범용 캐시 가져오기
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        if let Some(json) = self.generic.get(key).await {
            serde_json::from_str(&json).ok()
        } else {
            None
        }
    }

    /// 범용 캐시 저장
    pub async fn set<T: Serialize>(&self, key: &str, value: &T, ttl_ms: u64) {
        if let Ok(json) = serde_json::to_string(value) {
            let ttl = Duration::from_millis(ttl_ms);
            self.generic.set(key, json, ttl).await;
        }
    }

    /// 모든 캐시 삭제
    pub async fn clear_all(&self) {
        self.markets.clear().await;
        self.currencies.clear().await;
        self.tickers.clear().await;
        self.orderbooks.clear().await;
        self.generic.clear().await;
    }

    /// 만료된 항목 정리
    pub async fn cleanup_all(&self) {
        self.markets.cleanup().await;
        self.currencies.cleanup().await;
        self.tickers.cleanup().await;
        self.orderbooks.cleanup().await;
        self.generic.cleanup().await;
    }

    /// 전체 캐시 통계
    pub async fn total_stats(&self) -> CacheStats {
        let markets = self.markets.stats().await;
        let currencies = self.currencies.stats().await;
        let tickers = self.tickers.stats().await;
        let orderbooks = self.orderbooks.stats().await;
        let generic = self.generic.stats().await;

        CacheStats {
            hits: markets.hits + currencies.hits + tickers.hits + orderbooks.hits + generic.hits,
            misses: markets.misses
                + currencies.misses
                + tickers.misses
                + orderbooks.misses
                + generic.misses,
            entries: markets.entries
                + currencies.entries
                + tickers.entries
                + orderbooks.entries
                + generic.entries,
            expired: markets.expired
                + currencies.expired
                + tickers.expired
                + orderbooks.expired
                + generic.expired,
        }
    }
}

impl Default for ExchangeCache {
    fn default() -> Self {
        Self::default_config()
    }
}

/// 스레드 안전한 공유 캐시
pub type SharedCache = Arc<ExchangeCache>;

/// 공유 캐시 생성
pub fn create_shared_cache(config: CacheConfig) -> SharedCache {
    Arc::new(ExchangeCache::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_basic_operations() {
        let cache: Cache<String> = Cache::new(CacheConfig::default());

        // 캐시 저장 및 조회
        cache
            .set("key1", "value1".to_string(), Duration::from_secs(60))
            .await;
        let result = cache.get("key1").await;
        assert_eq!(result, Some("value1".to_string()));

        // 존재하지 않는 키
        let result = cache.get("key2").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_cache_expiration() {
        let cache: Cache<String> = Cache::new(CacheConfig::default());

        // 매우 짧은 TTL로 저장
        cache
            .set("key1", "value1".to_string(), Duration::from_millis(1))
            .await;

        // 잠시 대기
        tokio::time::sleep(Duration::from_millis(10)).await;

        // 만료됨
        let result = cache.get("key1").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_cache_stats() {
        let cache: Cache<String> = Cache::new(CacheConfig::default());

        cache
            .set("key1", "value1".to_string(), Duration::from_secs(60))
            .await;

        // 히트
        let _ = cache.get("key1").await;
        // 미스
        let _ = cache.get("key2").await;

        let stats = cache.stats().await;
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hit_rate(), 0.5);
    }

    #[tokio::test]
    async fn test_exchange_cache() {
        let cache = ExchangeCache::default_config();

        // Ticker 캐시 테스트
        let ticker = serde_json::json!({
            "symbol": "BTC/USDT",
            "last": 50000.0
        });

        cache.set_ticker("BTC/USDT", &ticker).await;
        let result: Option<serde_json::Value> = cache.get_ticker("BTC/USDT").await;
        assert!(result.is_some());
    }

    #[test]
    fn test_cache_config() {
        let config = CacheConfig::new()
            .with_ticker_ttl(5000)
            .with_orderbook_ttl(500);

        assert_eq!(config.ticker_ttl_ms, 5000);
        assert_eq!(config.orderbook_ttl_ms, 500);
    }
}
