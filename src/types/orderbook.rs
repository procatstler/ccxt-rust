//! OrderBook type - 호가창

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// 체크섬 알고리즘 종류
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlgorithm {
    /// CRC32 (Binance, OKX)
    Crc32,
    /// CRC32 with specific format (Kraken)
    Crc32Kraken,
    /// FTX style (bid prices + ask prices concatenated)
    Ftx,
    /// Custom format
    Custom,
}

/// 체크섬 검증 결과
#[derive(Debug, Clone)]
pub struct ChecksumResult {
    /// 계산된 체크섬
    pub calculated: u32,
    /// 거래소에서 제공한 체크섬
    pub expected: Option<u32>,
    /// 검증 성공 여부
    pub valid: bool,
    /// 사용된 알고리즘
    pub algorithm: ChecksumAlgorithm,
}

impl ChecksumResult {
    /// 검증 성공 여부
    pub fn is_valid(&self) -> bool {
        self.valid
    }

    /// 체크섬 불일치 여부
    pub fn is_mismatch(&self) -> bool {
        self.expected.is_some() && !self.valid
    }
}

/// 호가창
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OrderBook {
    /// 심볼
    #[serde(default)]
    pub symbol: String,
    /// 타임스탬프 (밀리초)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub timestamp: Option<i64>,
    /// ISO 8601 datetime
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub datetime: Option<String>,
    /// 매수호가 (가격순 내림차순)
    #[serde(default)]
    pub bids: Vec<OrderBookEntry>,
    /// 매도호가 (가격순 오름차순)
    #[serde(default)]
    pub asks: Vec<OrderBookEntry>,
    /// 호가 시퀀스 번호
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub nonce: Option<i64>,
    /// 거래소에서 제공한 체크섬
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub checksum: Option<u32>,
}

/// 호가 항목
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBookEntry {
    /// 가격
    pub price: Decimal,
    /// 수량
    pub amount: Decimal,
}

impl OrderBook {
    /// 새 OrderBook 생성
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            timestamp: None,
            datetime: None,
            bids: Vec::new(),
            asks: Vec::new(),
            nonce: None,
            checksum: None,
        }
    }

    /// 타임스탬프 설정
    pub fn with_timestamp(mut self, ts: i64) -> Self {
        self.timestamp = Some(ts);
        self.datetime = Some(
            DateTime::<Utc>::from_timestamp_millis(ts)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        );
        self
    }

    /// 체크섬 설정
    pub fn with_checksum(mut self, checksum: u32) -> Self {
        self.checksum = Some(checksum);
        self
    }

    /// 최고 매수호가
    pub fn best_bid(&self) -> Option<&OrderBookEntry> {
        self.bids.first()
    }

    /// 최저 매도호가
    pub fn best_ask(&self) -> Option<&OrderBookEntry> {
        self.asks.first()
    }

    /// 스프레드 (매도-매수 가격 차이)
    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask.price - bid.price),
            _ => None,
        }
    }

    /// 스프레드 비율 (%)
    pub fn spread_percentage(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) if bid.price > Decimal::ZERO => {
                Some((ask.price - bid.price) / bid.price * Decimal::ONE_HUNDRED)
            },
            _ => None,
        }
    }

    /// 매수호가 추가 (정렬됨)
    pub fn add_bid(&mut self, price: Decimal, amount: Decimal) {
        let entry = OrderBookEntry { price, amount };
        let pos = self
            .bids
            .binary_search_by(|e| price.cmp(&e.price))
            .unwrap_or_else(|pos| pos);
        self.bids.insert(pos, entry);
    }

    /// 매도호가 추가 (정렬됨)
    pub fn add_ask(&mut self, price: Decimal, amount: Decimal) {
        let entry = OrderBookEntry { price, amount };
        let pos = self
            .asks
            .binary_search_by(|e| e.price.cmp(&price))
            .unwrap_or_else(|pos| pos);
        self.asks.insert(pos, entry);
    }

    /// CRC32 체크섬 계산 (OKX 스타일)
    /// 형식: bid_price:bid_amount:ask_price:ask_amount:...
    pub fn calculate_crc32(&self, depth: usize) -> u32 {
        let mut data = String::new();

        let bid_depth = self.bids.len().min(depth);
        let ask_depth = self.asks.len().min(depth);

        for i in 0..bid_depth.max(ask_depth) {
            if i < bid_depth {
                let bid = &self.bids[i];
                if !data.is_empty() {
                    data.push(':');
                }
                data.push_str(&format!("{}:{}", bid.price, bid.amount));
            }
            if i < ask_depth {
                let ask = &self.asks[i];
                if !data.is_empty() {
                    data.push(':');
                }
                data.push_str(&format!("{}:{}", ask.price, ask.amount));
            }
        }

        crc32fast::hash(data.as_bytes())
    }

    /// CRC32 체크섬 계산 (Kraken 스타일)
    /// 형식: `ask[0][0] + ask[0][1] + bid[0][0] + bid[0][1] + ...`
    pub fn calculate_crc32_kraken(&self, depth: usize) -> u32 {
        let mut data = String::new();

        let max_depth = depth.min(10); // Kraken uses top 10

        // Asks first (price + amount, remove decimals)
        for i in 0..max_depth.min(self.asks.len()) {
            let ask = &self.asks[i];
            data.push_str(&Self::format_kraken_number(ask.price));
            data.push_str(&Self::format_kraken_number(ask.amount));
        }

        // Then bids
        for i in 0..max_depth.min(self.bids.len()) {
            let bid = &self.bids[i];
            data.push_str(&Self::format_kraken_number(bid.price));
            data.push_str(&Self::format_kraken_number(bid.amount));
        }

        crc32fast::hash(data.as_bytes())
    }

    /// Kraken 스타일 숫자 포맷 (소수점과 선행/후행 0 제거)
    fn format_kraken_number(num: Decimal) -> String {
        let s = num.to_string();
        s.replace('.', "")
            .trim_start_matches('0')
            .trim_end_matches('0')
            .to_string()
    }

    /// 체크섬 검증
    pub fn verify_checksum(&self, algorithm: ChecksumAlgorithm, depth: usize) -> ChecksumResult {
        let calculated = match algorithm {
            ChecksumAlgorithm::Crc32 => self.calculate_crc32(depth),
            ChecksumAlgorithm::Crc32Kraken => self.calculate_crc32_kraken(depth),
            ChecksumAlgorithm::Ftx => self.calculate_crc32(depth), // Same as CRC32 for now
            ChecksumAlgorithm::Custom => 0,
        };

        let valid = match self.checksum {
            Some(expected) => calculated == expected,
            None => true, // No checksum to verify
        };

        ChecksumResult {
            calculated,
            expected: self.checksum,
            valid,
            algorithm,
        }
    }

    /// 체크섬이 유효한지 확인 (체크섬이 있는 경우에만)
    pub fn is_checksum_valid(&self, algorithm: ChecksumAlgorithm, depth: usize) -> bool {
        self.verify_checksum(algorithm, depth).is_valid()
    }

    /// 호가 업데이트 적용 (델타 업데이트용)
    pub fn apply_update(&mut self, bids: &[(Decimal, Decimal)], asks: &[(Decimal, Decimal)]) {
        // 매수호가 업데이트
        for (price, amount) in bids {
            if amount.is_zero() {
                // 수량이 0이면 제거
                self.bids.retain(|e| e.price != *price);
            } else {
                // 기존 항목 업데이트 또는 추가
                if let Some(entry) = self.bids.iter_mut().find(|e| e.price == *price) {
                    entry.amount = *amount;
                } else {
                    self.add_bid(*price, *amount);
                }
            }
        }

        // 매도호가 업데이트
        for (price, amount) in asks {
            if amount.is_zero() {
                self.asks.retain(|e| e.price != *price);
            } else if let Some(entry) = self.asks.iter_mut().find(|e| e.price == *price) {
                entry.amount = *amount;
            } else {
                self.add_ask(*price, *amount);
            }
        }
    }

    /// 호가 깊이 제한
    pub fn limit_depth(&mut self, depth: usize) {
        self.bids.truncate(depth);
        self.asks.truncate(depth);
    }
}

impl OrderBookEntry {
    pub fn new(price: Decimal, amount: Decimal) -> Self {
        Self { price, amount }
    }

    /// 총 가치 (price * amount)
    pub fn cost(&self) -> Decimal {
        self.price * self.amount
    }
}

// Array 형태로 변환 지원 [price, amount]
impl From<[Decimal; 2]> for OrderBookEntry {
    fn from(arr: [Decimal; 2]) -> Self {
        Self {
            price: arr[0],
            amount: arr[1],
        }
    }
}

impl From<(Decimal, Decimal)> for OrderBookEntry {
    fn from((price, amount): (Decimal, Decimal)) -> Self {
        Self { price, amount }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_orderbook() {
        let mut ob = OrderBook::new("BTC/KRW".into());
        ob.add_bid(dec!(50000000), dec!(1.0));
        ob.add_bid(dec!(49900000), dec!(2.0));
        ob.add_ask(dec!(50100000), dec!(1.5));
        ob.add_ask(dec!(50200000), dec!(0.5));

        assert_eq!(ob.best_bid().unwrap().price, dec!(50000000));
        assert_eq!(ob.best_ask().unwrap().price, dec!(50100000));
        assert_eq!(ob.spread(), Some(dec!(100000)));
    }

    #[test]
    fn test_orderbook_checksum() {
        let mut ob = OrderBook::new("BTC/USDT".into());
        ob.add_bid(dec!(50000), dec!(1.0));
        ob.add_bid(dec!(49900), dec!(2.0));
        ob.add_ask(dec!(50100), dec!(1.5));
        ob.add_ask(dec!(50200), dec!(0.5));

        // Calculate checksum
        let checksum = ob.calculate_crc32(10);
        assert!(checksum > 0);

        // Set checksum and verify
        ob.checksum = Some(checksum);
        let result = ob.verify_checksum(ChecksumAlgorithm::Crc32, 10);
        assert!(result.is_valid());
        assert!(!result.is_mismatch());

        // Invalid checksum
        ob.checksum = Some(12345);
        let result = ob.verify_checksum(ChecksumAlgorithm::Crc32, 10);
        assert!(!result.is_valid());
        assert!(result.is_mismatch());
    }

    #[test]
    fn test_orderbook_apply_update() {
        let mut ob = OrderBook::new("BTC/USDT".into());
        ob.add_bid(dec!(50000), dec!(1.0));
        ob.add_ask(dec!(50100), dec!(1.5));

        // Update existing and add new
        ob.apply_update(
            &[(dec!(50000), dec!(2.0)), (dec!(49900), dec!(1.0))],
            &[(dec!(50100), dec!(0)), (dec!(50200), dec!(0.5))], // 50100 removed (amount=0)
        );

        assert_eq!(ob.best_bid().unwrap().amount, dec!(2.0));
        assert_eq!(ob.bids.len(), 2);
        assert_eq!(ob.best_ask().unwrap().price, dec!(50200));
        assert_eq!(ob.asks.len(), 1); // 50100 was removed
    }

    #[test]
    fn test_orderbook_limit_depth() {
        let mut ob = OrderBook::new("BTC/USDT".into());
        for i in 0..20 {
            ob.add_bid(Decimal::from(50000 - i), Decimal::ONE);
            ob.add_ask(Decimal::from(50100 + i), Decimal::ONE);
        }

        ob.limit_depth(5);
        assert_eq!(ob.bids.len(), 5);
        assert_eq!(ob.asks.len(), 5);
    }
}
