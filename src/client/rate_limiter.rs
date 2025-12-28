//! Rate limiting for API requests
//!
//! CCXT의 Token Bucket 알고리즘 구현
//!
//! Enhanced features:
//! - Token bucket with burst capacity
//! - Exponential backoff after rate limit errors
//! - Request statistics and metrics

use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// 레이트 리미터
///
/// CCXT 방식의 토큰 버킷 알고리즘 구현
/// - cost: 요청당 소비되는 토큰 수
/// - rate_limit: 밀리초 단위 최소 요청 간격
pub struct RateLimiter {
    /// 밀리초 단위 레이트 리밋
    rate_limit_ms: u64,
    /// 마지막 요청 시간
    last_request: Mutex<Instant>,
    /// 현재 토큰 수 (1000배 스케일)
    tokens: AtomicU64,
    /// 최대 토큰 수 (1000배 스케일)
    max_tokens: u64,
    /// 초당 리필율 (1000배 스케일)
    refill_rate: u64,
    /// 통계: 총 요청 수
    total_requests: AtomicU64,
    /// 통계: 제한된 요청 수 (대기 발생)
    throttled_requests: AtomicU64,
    /// 백오프 상태
    backoff: Mutex<BackoffState>,
    /// 버스트 허용 여부
    allow_burst: AtomicBool,
}

/// 백오프 상태 관리
struct BackoffState {
    /// 현재 백오프 레벨 (0 = 정상)
    level: u32,
    /// 마지막 429 에러 시간
    last_rate_limit: Option<Instant>,
    /// 최대 백오프 레벨
    max_level: u32,
    /// 기본 백오프 시간 (ms)
    base_delay_ms: u64,
}

impl BackoffState {
    fn new() -> Self {
        Self {
            level: 0,
            last_rate_limit: None,
            max_level: 5,
            base_delay_ms: 1000,
        }
    }

    /// 백오프 지연 시간 계산 (지수 백오프)
    fn delay_ms(&self) -> u64 {
        if self.level == 0 {
            0
        } else {
            // 지수 백오프: base * 2^(level-1), 최대 30초
            let delay = self.base_delay_ms * (1 << (self.level - 1));
            delay.min(30000)
        }
    }

    /// 레이트 리밋 발생 기록
    fn record_rate_limit(&mut self) {
        self.level = (self.level + 1).min(self.max_level);
        self.last_rate_limit = Some(Instant::now());
    }

    /// 성공 요청 기록 (백오프 레벨 감소)
    fn record_success(&mut self) {
        if let Some(last) = self.last_rate_limit {
            // 30초 이상 지나면 레벨 감소
            if last.elapsed() > Duration::from_secs(30) {
                self.level = self.level.saturating_sub(1);
                if self.level == 0 {
                    self.last_rate_limit = None;
                }
            }
        }
    }
}

impl RateLimiter {
    /// 새로운 레이트 리미터 생성
    ///
    /// # Arguments
    /// * `rate_limit_ms` - 밀리초 단위 최소 요청 간격 (예: 50ms = 초당 20회)
    pub fn new(rate_limit_ms: u64) -> Self {
        // 초당 요청 수 계산: 1000ms / rate_limit_ms
        let requests_per_second = if rate_limit_ms > 0 {
            1000 / rate_limit_ms
        } else {
            1000
        };

        // 1000배 스케일로 정밀도 향상
        let scaled_rps = requests_per_second * 1000;

        Self {
            rate_limit_ms,
            last_request: Mutex::new(Instant::now()),
            tokens: AtomicU64::new(scaled_rps),
            max_tokens: scaled_rps,
            refill_rate: scaled_rps,
            total_requests: AtomicU64::new(0),
            throttled_requests: AtomicU64::new(0),
            backoff: Mutex::new(BackoffState::new()),
            allow_burst: AtomicBool::new(true),
        }
    }

    /// 버스트 모드 설정
    pub fn set_allow_burst(&self, allow: bool) {
        self.allow_burst.store(allow, Ordering::SeqCst);
    }

    /// 요청 전 대기 (cost 기반)
    ///
    /// CCXT의 throttle 메서드와 동일
    /// cost = 1000 / (rateLimit * RPS)
    pub async fn throttle(&self, cost: f64) {
        self.total_requests.fetch_add(1, Ordering::SeqCst);

        // 백오프 지연 적용
        {
            let backoff = self.backoff.lock().await;
            let delay = backoff.delay_ms();
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }

        // 토큰 리필
        self.refill_tokens().await;

        // 필요한 토큰 수 계산 (1000배 스케일)
        let required = (cost * 1000.0) as u64;
        let mut throttled = false;

        loop {
            let current = self.tokens.load(Ordering::SeqCst);

            if current >= required {
                // 토큰 소비
                if self.tokens.compare_exchange(
                    current,
                    current - required,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ).is_ok() {
                    break;
                }
            } else {
                // 토큰 부족 - 대기 후 재시도
                if !throttled {
                    self.throttled_requests.fetch_add(1, Ordering::SeqCst);
                    throttled = true;
                }
                let wait_ms = self.rate_limit_ms.max(10);
                tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                self.refill_tokens().await;
            }
        }

        // 성공 기록 (백오프 레벨 감소 기회)
        {
            let mut backoff = self.backoff.lock().await;
            backoff.record_success();
        }
    }

    /// 레이트 리밋 에러 발생 시 호출 (백오프 레벨 증가)
    pub async fn on_rate_limit_error(&self) {
        let mut backoff = self.backoff.lock().await;
        backoff.record_rate_limit();
    }

    /// 특정 시간만큼 대기 후 재시도를 위한 메서드
    pub async fn wait_for_retry(&self, retry_after_ms: u64) {
        let mut backoff = self.backoff.lock().await;
        backoff.record_rate_limit();
        drop(backoff);

        tokio::time::sleep(Duration::from_millis(retry_after_ms)).await;
    }

    /// 토큰 리필
    async fn refill_tokens(&self) {
        let mut last = self.last_request.lock().await;
        let now = Instant::now();
        let elapsed = now.duration_since(*last);

        // 경과 시간에 따라 토큰 추가
        let elapsed_secs = elapsed.as_secs_f64();
        let new_tokens = (elapsed_secs * self.refill_rate as f64) as u64;

        if new_tokens > 0 {
            let current = self.tokens.load(Ordering::SeqCst);
            let updated = (current + new_tokens).min(self.max_tokens);
            self.tokens.store(updated, Ordering::SeqCst);
            *last = now;
        }
    }

    /// 토큰 획득 시도 (블로킹 없음)
    pub fn try_acquire(&self, cost: f64) -> bool {
        let required = (cost * 1000.0) as u64;
        let current = self.tokens.load(Ordering::SeqCst);

        if current >= required {
            self.tokens.compare_exchange(
                current,
                current - required,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ).is_ok()
        } else {
            false
        }
    }

    /// 현재 사용 가능한 토큰 수
    pub fn available_tokens(&self) -> f64 {
        self.tokens.load(Ordering::SeqCst) as f64 / 1000.0
    }

    /// 레이트 리밋 (밀리초)
    pub fn rate_limit_ms(&self) -> u64 {
        self.rate_limit_ms
    }

    /// 통계: 총 요청 수
    pub fn total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::SeqCst)
    }

    /// 통계: 제한된 요청 수
    pub fn throttled_requests(&self) -> u64 {
        self.throttled_requests.load(Ordering::SeqCst)
    }

    /// 통계: 제한 비율 (0.0 ~ 1.0)
    pub fn throttle_rate(&self) -> f64 {
        let total = self.total_requests() as f64;
        if total == 0.0 {
            0.0
        } else {
            self.throttled_requests() as f64 / total
        }
    }

    /// 통계: 현재 백오프 레벨
    pub async fn current_backoff_level(&self) -> u32 {
        let backoff = self.backoff.lock().await;
        backoff.level
    }

    /// 통계 초기화
    pub fn reset_stats(&self) {
        self.total_requests.store(0, Ordering::SeqCst);
        self.throttled_requests.store(0, Ordering::SeqCst);
    }

    /// 레이트 리미터 통계 요약
    pub fn stats_summary(&self) -> RateLimiterStats {
        RateLimiterStats {
            total_requests: self.total_requests(),
            throttled_requests: self.throttled_requests(),
            throttle_rate: self.throttle_rate(),
            available_tokens: self.available_tokens(),
            rate_limit_ms: self.rate_limit_ms,
        }
    }
}

/// 레이트 리미터 통계
#[derive(Debug, Clone)]
pub struct RateLimiterStats {
    /// 총 요청 수
    pub total_requests: u64,
    /// 제한된 요청 수
    pub throttled_requests: u64,
    /// 제한 비율
    pub throttle_rate: f64,
    /// 사용 가능한 토큰 수
    pub available_tokens: f64,
    /// 레이트 리밋 (ms)
    pub rate_limit_ms: u64,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(1000) // 초당 1회
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter() {
        let limiter = RateLimiter::new(100); // 초당 10회

        // 초기 토큰 확인
        assert!(limiter.available_tokens() > 0.0);

        // 토큰 소비
        limiter.throttle(1.0).await;
        let after = limiter.available_tokens();

        // 토큰이 감소했는지 확인
        assert!(after < 10.0);
    }

    #[tokio::test]
    async fn test_try_acquire() {
        let limiter = RateLimiter::new(100);

        // 첫 번째 시도는 성공해야 함
        assert!(limiter.try_acquire(1.0));
    }
}
