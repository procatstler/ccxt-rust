//! Time utilities

use chrono::{DateTime, Datelike, Utc};

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

/// 밀리초를 ISO 8601 문자열로 변환
pub fn ms_to_iso8601(ms: i64) -> String {
    ms_to_datetime(ms).to_rfc3339()
}

/// ISO 8601 문자열을 밀리초로 변환
pub fn iso8601_to_ms(datetime: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(datetime)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// 타임스탬프가 밀리초인지 초인지 판단하여 밀리초로 정규화
pub fn normalize_timestamp(ts: i64) -> i64 {
    // 13자리 이상이면 밀리초, 아니면 초로 판단
    if ts > 1_000_000_000_000 {
        ts
    } else {
        ts * 1000
    }
}

/// 밀리초 기준 시간 차이 계산
pub fn time_diff_ms(ts1: i64, ts2: i64) -> i64 {
    (ts1 - ts2).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now_ms() {
        let ts = now_ms();
        // 타임스탬프는 13자리 이상이어야 함 (밀리초)
        assert!(ts > 1_000_000_000_000);
        // 현재 시간과 합리적인 범위 내에 있어야 함
        let now = Utc::now().timestamp_millis();
        assert!((now - ts).abs() < 1000); // 1초 이내
    }

    #[test]
    fn test_now_secs() {
        let ts = now_secs();
        // 타임스탬프는 10자리여야 함 (초)
        assert!(ts > 1_000_000_000);
        assert!(ts < 10_000_000_000);
        // 현재 시간과 합리적인 범위 내에 있어야 함
        let now = Utc::now().timestamp();
        assert!((now - ts).abs() < 2); // 2초 이내
    }

    #[test]
    fn test_ms_to_datetime() {
        // 2024-01-01 00:00:00 UTC
        let ts = 1704067200000_i64;
        let dt = ms_to_datetime(ts);
        assert_eq!(dt.timestamp_millis(), ts);
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 1);
    }

    #[test]
    fn test_ms_to_datetime_invalid() {
        // 유효하지 않은 타임스탬프는 현재 시간 반환
        let dt = ms_to_datetime(i64::MAX);
        let now = Utc::now();
        // 현재 시간과 가까워야 함
        assert!((now.timestamp() - dt.timestamp()).abs() < 2);
    }

    #[test]
    fn test_secs_to_datetime() {
        // 2024-01-01 00:00:00 UTC
        let ts = 1704067200_i64;
        let dt = secs_to_datetime(ts);
        assert_eq!(dt.timestamp(), ts);
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 1);
    }

    #[test]
    fn test_secs_to_datetime_invalid() {
        // 유효하지 않은 타임스탬프는 현재 시간 반환
        let dt = secs_to_datetime(i64::MAX);
        let now = Utc::now();
        assert!((now.timestamp() - dt.timestamp()).abs() < 2);
    }

    #[test]
    fn test_ms_to_iso8601() {
        let ts = 1704067200000_i64;
        let iso = ms_to_iso8601(ts);
        assert!(iso.starts_with("2024-01-01"));
        assert!(iso.contains("T"));
    }

    #[test]
    fn test_iso8601_to_ms() {
        let iso = "2024-01-01T00:00:00+00:00";
        let ms = iso8601_to_ms(iso);
        assert_eq!(ms, Some(1704067200000));
    }

    #[test]
    fn test_iso8601_to_ms_invalid() {
        let invalid = "not-a-date";
        assert!(iso8601_to_ms(invalid).is_none());
    }

    #[test]
    fn test_normalize_timestamp_ms() {
        // 이미 밀리초
        let ts = 1704067200000_i64;
        assert_eq!(normalize_timestamp(ts), ts);
    }

    #[test]
    fn test_normalize_timestamp_secs() {
        // 초 -> 밀리초 변환
        let ts = 1704067200_i64;
        assert_eq!(normalize_timestamp(ts), ts * 1000);
    }

    #[test]
    fn test_time_diff_ms() {
        let ts1 = 1704067200000_i64;
        let ts2 = 1704067205000_i64; // 5초 후
        assert_eq!(time_diff_ms(ts1, ts2), 5000);
        assert_eq!(time_diff_ms(ts2, ts1), 5000); // 순서 무관
    }

    #[test]
    fn test_roundtrip_conversion() {
        let original_ms = 1704067200123_i64;
        let dt = ms_to_datetime(original_ms);
        let back_to_ms = dt.timestamp_millis();
        assert_eq!(original_ms, back_to_ms);
    }
}
