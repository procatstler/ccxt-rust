//! Rate limiting for API requests

use shared_types::error::{TradingError, TradingResult};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// 레이트 리미터
///
/// 초당 요청 수를 제한하는 토큰 버킷 알고리즘 구현
pub struct RateLimiter {
    requests_per_second: u32,
    last_request: Mutex<Instant>,
    tokens: AtomicU64,
    max_tokens: u64,
}

impl RateLimiter {
    /// 새로운 레이트 리미터 생성
    ///
    /// # Arguments
    /// * `requests_per_second` - 초당 허용 요청 수
    pub fn new(requests_per_second: u32) -> Self {
        Self {
            requests_per_second,
            last_request: Mutex::new(Instant::now()),
            tokens: AtomicU64::new(requests_per_second as u64),
            max_tokens: requests_per_second as u64,
        }
    }

    /// 토큰 획득 시도 (블로킹 없음)
    pub fn try_acquire(&self) -> TradingResult<()> {
        let current = self.tokens.load(Ordering::SeqCst);
        if current > 0 {
            self.tokens.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        } else {
            Err(TradingError::RateLimitError {
                exchange: shared_types::common::Exchange::Upbit, // TODO: 동적으로 설정
                retry_after_secs: Some(1),
            })
        }
    }

    /// 토큰 획득 (필요시 대기)
    pub async fn acquire(&self) -> TradingResult<()> {
        loop {
            // 토큰 리필
            self.refill_tokens().await;

            // 토큰 획득 시도
            if self.try_acquire().is_ok() {
                return Ok(());
            }

            // 토큰이 없으면 잠시 대기
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// 토큰 리필
    async fn refill_tokens(&self) {
        let mut last = self.last_request.lock().await;
        let now = Instant::now();
        let elapsed = now.duration_since(*last);

        // 경과 시간에 따라 토큰 추가
        let new_tokens = (elapsed.as_secs_f64() * self.requests_per_second as f64) as u64;
        if new_tokens > 0 {
            let current = self.tokens.load(Ordering::SeqCst);
            let updated = (current + new_tokens).min(self.max_tokens);
            self.tokens.store(updated, Ordering::SeqCst);
            *last = now;
        }
    }

    /// 현재 사용 가능한 토큰 수
    pub fn available_tokens(&self) -> u64 {
        self.tokens.load(Ordering::SeqCst)
    }
}
