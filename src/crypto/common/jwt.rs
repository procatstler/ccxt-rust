//! JWT (JSON Web Token) Utilities
//!
//! JWT 생성 및 검증 기능 제공 (거래소 API 인증용)

use jsonwebtoken::{
    decode, encode, Algorithm, DecodingKey, EncodingKey, Header, TokenData, Validation,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::errors::{CcxtError, CcxtResult};

/// JWT 클레임 기본 구조
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// 발급자 (issuer)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    /// 대상자 (subject)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    /// 수신자 (audience)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,
    /// 만료 시간 (expiration time)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<u64>,
    /// 활성 시간 (not before)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nbf: Option<u64>,
    /// 발급 시간 (issued at)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iat: Option<u64>,
    /// JWT ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// 논스 (replay attack 방지)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
}

impl Default for JwtClaims {
    fn default() -> Self {
        Self {
            iss: None,
            sub: None,
            aud: None,
            exp: None,
            nbf: None,
            iat: None,
            jti: None,
            nonce: None,
        }
    }
}

impl JwtClaims {
    /// 새 클레임 생성
    pub fn new() -> Self {
        Self::default()
    }

    /// 발급자 설정
    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.iss = Some(issuer.into());
        self
    }

    /// 대상자 설정
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.sub = Some(subject.into());
        self
    }

    /// 수신자 설정
    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.aud = Some(audience.into());
        self
    }

    /// 만료 시간 설정 (초 단위 상대 시간)
    pub fn with_expiry(mut self, seconds_from_now: u64) -> CcxtResult<Self> {
        let now = current_timestamp()?;
        self.exp = Some(now + seconds_from_now);
        Ok(self)
    }

    /// 발급 시간 설정 (현재 시간)
    pub fn with_issued_at(mut self) -> CcxtResult<Self> {
        self.iat = Some(current_timestamp()?);
        Ok(self)
    }

    /// JWT ID 설정
    pub fn with_jti(mut self, jti: impl Into<String>) -> Self {
        self.jti = Some(jti.into());
        self
    }

    /// 논스 설정
    pub fn with_nonce(mut self, nonce: impl Into<String>) -> Self {
        self.nonce = Some(nonce.into());
        self
    }

    /// 만료 여부 확인
    pub fn is_expired(&self) -> CcxtResult<bool> {
        match self.exp {
            Some(exp) => {
                let now = current_timestamp()?;
                Ok(now >= exp)
            }
            None => Ok(false),
        }
    }
}

/// JWT 인코더
pub struct JwtEncoder {
    #[allow(dead_code)]
    algorithm: Algorithm,
    encoding_key: EncodingKey,
    header: Header,
}

impl JwtEncoder {
    /// HMAC-SHA256으로 인코더 생성 (대부분의 거래소)
    pub fn hs256(secret: &[u8]) -> Self {
        Self {
            algorithm: Algorithm::HS256,
            encoding_key: EncodingKey::from_secret(secret),
            header: Header::new(Algorithm::HS256),
        }
    }

    /// HMAC-SHA384로 인코더 생성
    pub fn hs384(secret: &[u8]) -> Self {
        Self {
            algorithm: Algorithm::HS384,
            encoding_key: EncodingKey::from_secret(secret),
            header: Header::new(Algorithm::HS384),
        }
    }

    /// HMAC-SHA512로 인코더 생성
    pub fn hs512(secret: &[u8]) -> Self {
        Self {
            algorithm: Algorithm::HS512,
            encoding_key: EncodingKey::from_secret(secret),
            header: Header::new(Algorithm::HS512),
        }
    }

    /// RSA-SHA256으로 인코더 생성 (PEM 형식 개인키)
    pub fn rs256(private_key_pem: &[u8]) -> CcxtResult<Self> {
        let encoding_key =
            EncodingKey::from_rsa_pem(private_key_pem).map_err(|e| CcxtError::AuthenticationError {
                message: format!("Invalid RSA private key: {e}"),
            })?;

        Ok(Self {
            algorithm: Algorithm::RS256,
            encoding_key,
            header: Header::new(Algorithm::RS256),
        })
    }

    /// RSA-SHA512으로 인코더 생성 (PEM 형식 개인키)
    pub fn rs512(private_key_pem: &[u8]) -> CcxtResult<Self> {
        let encoding_key =
            EncodingKey::from_rsa_pem(private_key_pem).map_err(|e| CcxtError::AuthenticationError {
                message: format!("Invalid RSA private key: {e}"),
            })?;

        Ok(Self {
            algorithm: Algorithm::RS512,
            encoding_key,
            header: Header::new(Algorithm::RS512),
        })
    }

    /// ES256 (ECDSA-SHA256)으로 인코더 생성 (PEM 형식)
    pub fn es256(private_key_pem: &[u8]) -> CcxtResult<Self> {
        let encoding_key =
            EncodingKey::from_ec_pem(private_key_pem).map_err(|e| CcxtError::AuthenticationError {
                message: format!("Invalid EC private key: {e}"),
            })?;

        Ok(Self {
            algorithm: Algorithm::ES256,
            encoding_key,
            header: Header::new(Algorithm::ES256),
        })
    }

    /// 커스텀 헤더 설정
    pub fn with_header(mut self, header: Header) -> Self {
        self.header = header;
        self
    }

    /// 헤더에 키 ID (kid) 설정
    pub fn with_key_id(mut self, kid: impl Into<String>) -> Self {
        self.header.kid = Some(kid.into());
        self
    }

    /// JWT 토큰 생성
    pub fn encode<T: Serialize>(&self, claims: &T) -> CcxtResult<String> {
        encode(&self.header, claims, &self.encoding_key).map_err(|e| CcxtError::AuthenticationError {
            message: format!("JWT encoding failed: {e}"),
        })
    }
}

/// JWT 디코더
pub struct JwtDecoder {
    decoding_key: DecodingKey,
    validation: Validation,
}

impl JwtDecoder {
    /// HMAC-SHA256으로 디코더 생성
    pub fn hs256(secret: &[u8]) -> Self {
        Self {
            decoding_key: DecodingKey::from_secret(secret),
            validation: Validation::new(Algorithm::HS256),
        }
    }

    /// HMAC-SHA384로 디코더 생성
    pub fn hs384(secret: &[u8]) -> Self {
        Self {
            decoding_key: DecodingKey::from_secret(secret),
            validation: Validation::new(Algorithm::HS384),
        }
    }

    /// HMAC-SHA512로 디코더 생성
    pub fn hs512(secret: &[u8]) -> Self {
        Self {
            decoding_key: DecodingKey::from_secret(secret),
            validation: Validation::new(Algorithm::HS512),
        }
    }

    /// RSA-SHA256으로 디코더 생성 (PEM 형식 공개키)
    pub fn rs256(public_key_pem: &[u8]) -> CcxtResult<Self> {
        let decoding_key =
            DecodingKey::from_rsa_pem(public_key_pem).map_err(|e| CcxtError::AuthenticationError {
                message: format!("Invalid RSA public key: {e}"),
            })?;

        Ok(Self {
            decoding_key,
            validation: Validation::new(Algorithm::RS256),
        })
    }

    /// RSA-SHA512으로 디코더 생성 (PEM 형식 공개키)
    pub fn rs512(public_key_pem: &[u8]) -> CcxtResult<Self> {
        let decoding_key =
            DecodingKey::from_rsa_pem(public_key_pem).map_err(|e| CcxtError::AuthenticationError {
                message: format!("Invalid RSA public key: {e}"),
            })?;

        Ok(Self {
            decoding_key,
            validation: Validation::new(Algorithm::RS512),
        })
    }

    /// ES256으로 디코더 생성 (PEM 형식 공개키)
    pub fn es256(public_key_pem: &[u8]) -> CcxtResult<Self> {
        let decoding_key =
            DecodingKey::from_ec_pem(public_key_pem).map_err(|e| CcxtError::AuthenticationError {
                message: format!("Invalid EC public key: {e}"),
            })?;

        Ok(Self {
            decoding_key,
            validation: Validation::new(Algorithm::ES256),
        })
    }

    /// 검증 옵션: 만료 시간 검증 비활성화
    pub fn without_exp_validation(mut self) -> Self {
        self.validation.validate_exp = false;
        self
    }

    /// 검증 옵션: 필수 클레임 설정
    pub fn require_claims(mut self, claims: &[&str]) -> Self {
        self.validation.required_spec_claims = claims.iter().map(|s| s.to_string()).collect();
        self
    }

    /// 검증 옵션: 수신자 설정
    pub fn with_audience(mut self, audience: &[&str]) -> Self {
        self.validation.aud = Some(audience.iter().map(|s| s.to_string()).collect());
        self
    }

    /// 검증 옵션: 발급자 설정
    pub fn with_issuer(mut self, issuers: &[&str]) -> Self {
        self.validation.iss = Some(issuers.iter().map(|s| s.to_string()).collect());
        self
    }

    /// 검증 옵션: 시간 여유 설정 (초)
    pub fn with_leeway(mut self, seconds: u64) -> Self {
        self.validation.leeway = seconds;
        self
    }

    /// JWT 토큰 디코딩 및 검증
    pub fn decode<T: DeserializeOwned>(&self, token: &str) -> CcxtResult<TokenData<T>> {
        decode::<T>(token, &self.decoding_key, &self.validation).map_err(|e| {
            CcxtError::AuthenticationError {
                message: format!("JWT decoding failed: {e}"),
            }
        })
    }

    /// JWT 토큰에서 클레임만 추출 (검증 없이)
    pub fn decode_without_validation<T: DeserializeOwned>(token: &str) -> CcxtResult<T> {
        let mut validation = Validation::default();
        validation.insecure_disable_signature_validation();
        validation.validate_exp = false;

        // 더미 키로 디코딩 (서명 검증 안함)
        let token_data = decode::<T>(token, &DecodingKey::from_secret(&[]), &validation)
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("JWT parsing failed: {e}"),
            })?;

        Ok(token_data.claims)
    }
}

/// 현재 Unix 타임스탬프 (초)
fn current_timestamp() -> CcxtResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| CcxtError::AuthenticationError {
            message: format!("System time error: {e}"),
        })
}

// === 편의 함수 ===

/// HMAC-SHA256 JWT 생성 (가장 일반적)
pub fn encode_jwt_hs256<T: Serialize>(claims: &T, secret: &[u8]) -> CcxtResult<String> {
    JwtEncoder::hs256(secret).encode(claims)
}

/// HMAC-SHA256 JWT 디코딩
pub fn decode_jwt_hs256<T: DeserializeOwned>(token: &str, secret: &[u8]) -> CcxtResult<T> {
    Ok(JwtDecoder::hs256(secret).decode::<T>(token)?.claims)
}

/// RSA-SHA256 JWT 생성
pub fn encode_jwt_rs256<T: Serialize>(claims: &T, private_key_pem: &[u8]) -> CcxtResult<String> {
    JwtEncoder::rs256(private_key_pem)?.encode(claims)
}

/// RSA-SHA256 JWT 디코딩
pub fn decode_jwt_rs256<T: DeserializeOwned>(
    token: &str,
    public_key_pem: &[u8],
) -> CcxtResult<T> {
    Ok(JwtDecoder::rs256(public_key_pem)?.decode::<T>(token)?.claims)
}

/// JWT 토큰에서 클레임 추출 (서명 검증 없이)
pub fn parse_jwt_claims<T: DeserializeOwned>(token: &str) -> CcxtResult<T> {
    JwtDecoder::decode_without_validation(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestClaims {
        sub: String,
        exp: u64,
        custom: String,
    }

    #[test]
    fn test_jwt_hs256_roundtrip() {
        let secret = b"test_secret_key_12345";
        let claims = TestClaims {
            sub: "user123".into(),
            exp: current_timestamp().unwrap() + 3600,
            custom: "test_value".into(),
        };

        // 인코딩
        let token = encode_jwt_hs256(&claims, secret).unwrap();
        assert!(!token.is_empty());
        assert!(token.contains('.'));

        // 디코딩
        let decoded: TestClaims = decode_jwt_hs256(&token, secret).unwrap();
        assert_eq!(decoded, claims);
    }

    #[test]
    fn test_jwt_claims_builder() {
        let claims = JwtClaims::new()
            .with_issuer("test_issuer")
            .with_subject("test_subject")
            .with_audience("test_audience")
            .with_jti("unique_id");

        assert_eq!(claims.iss, Some("test_issuer".into()));
        assert_eq!(claims.sub, Some("test_subject".into()));
        assert_eq!(claims.aud, Some("test_audience".into()));
        assert_eq!(claims.jti, Some("unique_id".into()));
    }

    #[test]
    fn test_jwt_expiry() {
        let claims = JwtClaims::new().with_expiry(3600).unwrap();

        assert!(claims.exp.is_some());
        let exp = claims.exp.unwrap();
        let now = current_timestamp().unwrap();
        assert!(exp > now);
        assert!(exp <= now + 3601);
    }

    #[test]
    fn test_parse_jwt_without_validation() {
        let secret = b"any_secret";
        let claims = TestClaims {
            sub: "user456".into(),
            exp: current_timestamp().unwrap() + 3600,
            custom: "value".into(),
        };

        let token = encode_jwt_hs256(&claims, secret).unwrap();

        // 검증 없이 파싱
        let parsed: TestClaims = parse_jwt_claims(&token).unwrap();
        assert_eq!(parsed.sub, "user456");
    }

    #[test]
    fn test_jwt_encoder_with_key_id() {
        let secret = b"test_secret";
        let encoder = JwtEncoder::hs256(secret).with_key_id("key_001");

        let claims = JwtClaims::new().with_subject("test");
        let token = encoder.encode(&claims).unwrap();

        assert!(!token.is_empty());
    }

    #[test]
    fn test_jwt_decoder_with_leeway() {
        let secret = b"test_secret";
        let claims = TestClaims {
            sub: "user".into(),
            exp: current_timestamp().unwrap() + 3600,
            custom: "data".into(),
        };

        let token = encode_jwt_hs256(&claims, secret).unwrap();

        let decoder = JwtDecoder::hs256(secret).with_leeway(60);
        let decoded = decoder.decode::<TestClaims>(&token).unwrap();

        assert_eq!(decoded.claims.sub, "user");
    }
}
