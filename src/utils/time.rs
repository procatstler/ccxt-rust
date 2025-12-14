//! Time utilities

use chrono::{DateTime, Utc};

/// 현재 UTC 타임스탬프 (밀리초)
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// 현재 UTC 타임스탬프 (초)
pub fn now_secs() -> i64 {
    Utc::now().timestamp()
}

/// 밀리초를 DateTime으로 변환
pub fn ms_to_datetime(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_else(Utc::now)
}

/// 초를 DateTime으로 변환
pub fn secs_to_datetime(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).unwrap_or_else(Utc::now)
}
