//! TOTP (Time-based One-Time Password) Utilities
//!
//! RFC 6238 TOTP 구현 (2FA 인증용)

// Note: base64 not needed for TOTP (uses Base32)
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::errors::{CcxtError, CcxtResult};

/// TOTP 알고리즘
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotpAlgorithm {
    /// SHA1 (기본, 대부분의 서비스)
    Sha1,
    /// SHA256
    Sha256,
    /// SHA512
    Sha512,
}

impl Default for TotpAlgorithm {
    fn default() -> Self {
        Self::Sha1
    }
}

/// TOTP 설정
#[derive(Debug, Clone)]
pub struct TotpConfig {
    /// 비밀키 (Base32 인코딩)
    pub secret: String,
    /// 코드 자릿수 (기본 6)
    pub digits: u32,
    /// 시간 간격 (초, 기본 30)
    pub period: u64,
    /// 알고리즘
    pub algorithm: TotpAlgorithm,
}

impl TotpConfig {
    /// 기본 설정으로 TOTP 생성
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            digits: 6,
            period: 30,
            algorithm: TotpAlgorithm::Sha1,
        }
    }

    /// 자릿수 설정
    pub fn with_digits(mut self, digits: u32) -> Self {
        self.digits = digits;
        self
    }

    /// 시간 간격 설정
    pub fn with_period(mut self, period: u64) -> Self {
        self.period = period;
        self
    }

    /// 알고리즘 설정
    pub fn with_algorithm(mut self, algorithm: TotpAlgorithm) -> Self {
        self.algorithm = algorithm;
        self
    }
}

/// TOTP 생성기
pub struct Totp {
    config: TotpConfig,
    secret_bytes: Vec<u8>,
}

impl Totp {
    /// 새 TOTP 생성기 생성
    pub fn new(config: TotpConfig) -> CcxtResult<Self> {
        let secret_bytes = decode_base32(&config.secret)?;
        Ok(Self {
            config,
            secret_bytes,
        })
    }

    /// 비밀키만으로 간단히 생성
    pub fn from_secret(secret: impl Into<String>) -> CcxtResult<Self> {
        Self::new(TotpConfig::new(secret))
    }

    /// 현재 시간 기준 TOTP 코드 생성
    pub fn generate(&self) -> CcxtResult<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("System time error: {e}"),
            })?
            .as_secs();

        self.generate_at(now)
    }

    /// 특정 시간 기준 TOTP 코드 생성
    pub fn generate_at(&self, timestamp: u64) -> CcxtResult<String> {
        let counter = timestamp / self.config.period;
        self.generate_hotp(counter)
    }

    /// HOTP 생성 (카운터 기반)
    fn generate_hotp(&self, counter: u64) -> CcxtResult<String> {
        // 카운터를 8바이트 빅엔디안으로 변환
        let counter_bytes = counter.to_be_bytes();

        // HMAC 계산
        let hash = match self.config.algorithm {
            TotpAlgorithm::Sha1 => {
                let mut mac =
                    Hmac::<Sha1>::new_from_slice(&self.secret_bytes).map_err(|e| {
                        CcxtError::AuthenticationError {
                            message: format!("HMAC error: {e}"),
                        }
                    })?;
                mac.update(&counter_bytes);
                mac.finalize().into_bytes().to_vec()
            }
            TotpAlgorithm::Sha256 => {
                let mut mac =
                    Hmac::<Sha256>::new_from_slice(&self.secret_bytes).map_err(|e| {
                        CcxtError::AuthenticationError {
                            message: format!("HMAC error: {e}"),
                        }
                    })?;
                mac.update(&counter_bytes);
                mac.finalize().into_bytes().to_vec()
            }
            TotpAlgorithm::Sha512 => {
                let mut mac =
                    Hmac::<Sha512>::new_from_slice(&self.secret_bytes).map_err(|e| {
                        CcxtError::AuthenticationError {
                            message: format!("HMAC error: {e}"),
                        }
                    })?;
                mac.update(&counter_bytes);
                mac.finalize().into_bytes().to_vec()
            }
        };

        // 동적 트렁케이션
        let offset = (hash.last().unwrap_or(&0) & 0x0f) as usize;
        let binary = ((hash[offset] & 0x7f) as u32) << 24
            | (hash[offset + 1] as u32) << 16
            | (hash[offset + 2] as u32) << 8
            | (hash[offset + 3] as u32);

        // 자릿수에 맞게 나머지 계산
        let otp = binary % 10u32.pow(self.config.digits);

        // 앞에 0을 채워서 자릿수 맞춤
        Ok(format!("{:0>width$}", otp, width = self.config.digits as usize))
    }

    /// TOTP 코드 검증 (현재 시간 기준, 앞뒤 1개 허용)
    pub fn verify(&self, code: &str) -> CcxtResult<bool> {
        self.verify_with_window(code, 1)
    }

    /// TOTP 코드 검증 (윈도우 크기 지정)
    pub fn verify_with_window(&self, code: &str, window: u64) -> CcxtResult<bool> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("System time error: {e}"),
            })?
            .as_secs();

        let current_counter = now / self.config.period;

        for i in 0..=window {
            // 현재
            if i == 0 {
                let generated = self.generate_hotp(current_counter)?;
                if constant_time_compare(code, &generated) {
                    return Ok(true);
                }
            } else {
                // 이전
                if current_counter >= i {
                    let generated = self.generate_hotp(current_counter - i)?;
                    if constant_time_compare(code, &generated) {
                        return Ok(true);
                    }
                }
                // 이후
                let generated = self.generate_hotp(current_counter + i)?;
                if constant_time_compare(code, &generated) {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// 남은 시간 (초)
    pub fn time_remaining(&self) -> CcxtResult<u64> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("System time error: {e}"),
            })?
            .as_secs();

        Ok(self.config.period - (now % self.config.period))
    }
}

/// Base32 디코딩 (RFC 4648)
fn decode_base32(input: &str) -> CcxtResult<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

    let input = input
        .to_uppercase()
        .replace([' ', '-'], "")
        .trim_end_matches('=')
        .to_string();

    if input.is_empty() {
        return Err(CcxtError::AuthenticationError {
            message: "Empty Base32 input".into(),
        });
    }

    let mut result = Vec::new();
    let mut buffer: u64 = 0;
    let mut bits: u32 = 0;

    for c in input.bytes() {
        let value = ALPHABET
            .iter()
            .position(|&x| x == c)
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: format!("Invalid Base32 character: {}", c as char),
            })? as u64;

        buffer = (buffer << 5) | value;
        bits += 5;

        if bits >= 8 {
            bits -= 8;
            result.push((buffer >> bits) as u8);
        }
    }

    Ok(result)
}

/// 상수 시간 비교 (타이밍 공격 방지)
fn constant_time_compare(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut result = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        result |= x ^ y;
    }
    result == 0
}

/// TOTP 코드 생성 (단일 함수)
pub fn generate_totp(secret: &str) -> CcxtResult<String> {
    Totp::from_secret(secret)?.generate()
}

/// TOTP 코드 검증 (단일 함수)
pub fn verify_totp(secret: &str, code: &str) -> CcxtResult<bool> {
    Totp::from_secret(secret)?.verify(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 테스트 벡터 (SHA1)
    // Base32("12345678901234567890") = "GEZDGNBVGY3TQOJQ"
    const TEST_SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn test_totp_generation() {
        let totp = Totp::new(TotpConfig::new(TEST_SECRET).with_digits(8)).unwrap();

        // RFC 6238 테스트 벡터: 시간 59초 -> 94287082
        let code = totp.generate_at(59).unwrap();
        assert_eq!(code, "94287082");
    }

    #[test]
    fn test_totp_6_digits() {
        let totp = Totp::from_secret(TEST_SECRET).unwrap();
        let code = totp.generate().unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_totp_verification() {
        let totp = Totp::from_secret(TEST_SECRET).unwrap();
        let code = totp.generate().unwrap();
        assert!(totp.verify(&code).unwrap());
        assert!(!totp.verify("000000").unwrap());
    }

    #[test]
    fn test_base32_decode() {
        // "test" -> "ORSXG5A="
        let decoded = decode_base32("ORSXG5A").unwrap();
        assert_eq!(decoded, b"test");

        // "1234567890" -> "GEZDGNBVGY3TQOJQ"
        let decoded = decode_base32("GEZDGNBVGY3TQOJQ").unwrap();
        assert_eq!(decoded, b"1234567890");
    }

    #[test]
    fn test_time_remaining() {
        let totp = Totp::from_secret(TEST_SECRET).unwrap();
        let remaining = totp.time_remaining().unwrap();
        assert!(remaining <= 30);
    }
}
