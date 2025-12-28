//! OHLCV type - 캔들 데이터

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// OHLCV 캔들 데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OHLCV {
    /// 타임스탬프 (밀리초)
    pub timestamp: i64,
    /// 시가
    pub open: Decimal,
    /// 고가
    pub high: Decimal,
    /// 저가
    pub low: Decimal,
    /// 종가
    pub close: Decimal,
    /// 거래량
    pub volume: Decimal,
}

impl OHLCV {
    /// 새 OHLCV 생성
    pub fn new(
        timestamp: i64,
        open: Decimal,
        high: Decimal,
        low: Decimal,
        close: Decimal,
        volume: Decimal,
    ) -> Self {
        Self {
            timestamp,
            open,
            high,
            low,
            close,
            volume,
        }
    }

    /// 배열에서 생성 [timestamp, open, high, low, close, volume]
    pub fn from_array(arr: [f64; 6]) -> Self {
        Self {
            timestamp: arr[0] as i64,
            open: Decimal::try_from(arr[1]).unwrap_or_default(),
            high: Decimal::try_from(arr[2]).unwrap_or_default(),
            low: Decimal::try_from(arr[3]).unwrap_or_default(),
            close: Decimal::try_from(arr[4]).unwrap_or_default(),
            volume: Decimal::try_from(arr[5]).unwrap_or_default(),
        }
    }

    /// 배열로 변환
    pub fn to_array(&self) -> [f64; 6] {
        [
            self.timestamp as f64,
            self.open.try_into().unwrap_or(0.0),
            self.high.try_into().unwrap_or(0.0),
            self.low.try_into().unwrap_or(0.0),
            self.close.try_into().unwrap_or(0.0),
            self.volume.try_into().unwrap_or(0.0),
        ]
    }

    /// 상승 캔들 여부
    pub fn is_bullish(&self) -> bool {
        self.close > self.open
    }

    /// 하락 캔들 여부
    pub fn is_bearish(&self) -> bool {
        self.close < self.open
    }

    /// 도지 캔들 여부
    pub fn is_doji(&self) -> bool {
        self.close == self.open
    }

    /// 캔들 몸통 크기
    pub fn body_size(&self) -> Decimal {
        (self.close - self.open).abs()
    }

    /// 캔들 전체 범위
    pub fn range(&self) -> Decimal {
        self.high - self.low
    }

    /// 윗꼬리 크기
    pub fn upper_shadow(&self) -> Decimal {
        self.high - self.close.max(self.open)
    }

    /// 아랫꼬리 크기
    pub fn lower_shadow(&self) -> Decimal {
        self.close.min(self.open) - self.low
    }

    /// 거래 금액 (대략적 추정: close * volume)
    pub fn turnover(&self) -> Decimal {
        self.close * self.volume
    }

    /// datetime 문자열 반환
    pub fn datetime(&self) -> String {
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(self.timestamp)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default()
    }
}

/// OHLCV 컬럼 인덱스
pub mod ohlcv_columns {
    pub const TIMESTAMP: usize = 0;
    pub const OPEN: usize = 1;
    pub const HIGH: usize = 2;
    pub const LOW: usize = 3;
    pub const CLOSE: usize = 4;
    pub const VOLUME: usize = 5;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_ohlcv() {
        let candle = OHLCV::new(
            1700000000000,
            dec!(100),
            dec!(110),
            dec!(95),
            dec!(105),
            dec!(1000),
        );

        assert!(candle.is_bullish());
        assert!(!candle.is_bearish());
        assert_eq!(candle.body_size(), dec!(5));
        assert_eq!(candle.range(), dec!(15));
    }

    #[test]
    fn test_ohlcv_from_array() {
        let arr = [1700000000000.0, 100.0, 110.0, 95.0, 105.0, 1000.0];
        let candle = OHLCV::from_array(arr);

        assert_eq!(candle.timestamp, 1700000000000);
        assert!(candle.is_bullish());
    }

    #[test]
    fn test_shadows() {
        let candle = OHLCV::new(
            1700000000000,
            dec!(100),
            dec!(120),
            dec!(90),
            dec!(110),
            dec!(1000),
        );

        // 상승 캔들: upper_shadow = high - close = 120 - 110 = 10
        // lower_shadow = open - low = 100 - 90 = 10
        assert_eq!(candle.upper_shadow(), dec!(10));
        assert_eq!(candle.lower_shadow(), dec!(10));
    }
}
