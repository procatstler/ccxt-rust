//! Cryptographic utilities for API signing
//!
//! Provides HMAC, RSA, and JWT signing utilities for exchange API authentication

use hmac::{Hmac, Mac};
use sha2::{Sha256, Sha384, Sha512};
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;
type HmacSha384 = Hmac<Sha384>;
type HmacSha512 = Hmac<Sha512>;

/// HMAC-SHA256 서명 생성
pub fn hmac_sha256(secret: &str, message: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// HMAC-SHA256 서명을 hex 문자열로 반환
pub fn hmac_sha256_hex(secret: &str, message: &str) -> String {
    hex::encode(hmac_sha256(secret, message))
}

/// HMAC-SHA256 서명을 base64로 반환
pub fn hmac_sha256_base64(secret: &str, message: &str) -> String {
    base64_encode(&hmac_sha256(secret, message))
}

/// HMAC-SHA384 서명 생성
pub fn hmac_sha384(secret: &[u8], message: &str) -> Vec<u8> {
    let mut mac = HmacSha384::new_from_slice(secret)
        .expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// HMAC-SHA384 서명을 hex 문자열로 반환
pub fn hmac_sha384_hex(secret: &[u8], message: &str) -> String {
    hex::encode(hmac_sha384(secret, message))
}

/// HMAC-SHA512 서명 생성
pub fn hmac_sha512(secret: &str, message: &str) -> Vec<u8> {
    let mut mac = HmacSha512::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// HMAC-SHA512 서명을 hex 문자열로 반환
pub fn hmac_sha512_hex(secret: &str, message: &str) -> String {
    hex::encode(hmac_sha512(secret, message))
}

/// Base64 인코딩
pub fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Base64 디코딩
pub fn base64_decode(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(data)
}

/// URL-safe Base64 인코딩 (JWT용)
pub fn base64_url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// URL-safe Base64 디코딩
pub fn base64_url_decode(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data)
}

/// JWT 생성 (HS256)
///
/// # Arguments
/// * `secret` - HMAC secret key
/// * `payload` - JWT payload as JSON value
/// * `expiry_seconds` - Token expiry in seconds from now
pub fn create_jwt_hs256(
    secret: &str,
    payload: serde_json::Value,
    expiry_seconds: u64,
) -> String {
    let header = serde_json::json!({
        "alg": "HS256",
        "typ": "JWT"
    });

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut payload = payload;
    if let serde_json::Value::Object(ref mut map) = payload {
        map.insert("iat".to_string(), serde_json::json!(now));
        map.insert("exp".to_string(), serde_json::json!(now + expiry_seconds));
    }

    let header_b64 = base64_url_encode(header.to_string().as_bytes());
    let payload_b64 = base64_url_encode(payload.to_string().as_bytes());
    let message = format!("{header_b64}.{payload_b64}");

    let signature = hmac_sha256(secret, &message);
    let signature_b64 = base64_url_encode(&signature);

    format!("{message}.{signature_b64}")
}

/// JWT 생성 using jsonwebtoken crate (RS256)
///
/// # Arguments
/// * `private_key_pem` - RSA private key in PEM format
/// * `payload` - JWT claims
pub fn create_jwt_rs256(
    private_key_pem: &str,
    claims: &impl serde::Serialize,
) -> Result<String, JwtError> {
    use jsonwebtoken::{encode, EncodingKey, Header, Algorithm};

    let key = EncodingKey::from_rsa_pem(private_key_pem.as_bytes())
        .map_err(|e| JwtError::KeyError(e.to_string()))?;

    let header = Header::new(Algorithm::RS256);

    encode(&header, claims, &key)
        .map_err(|e| JwtError::EncodingError(e.to_string()))
}

/// JWT 생성 (ES256)
///
/// # Arguments
/// * `private_key_pem` - EC private key in PEM format
/// * `claims` - JWT claims
pub fn create_jwt_es256(
    private_key_pem: &str,
    claims: &impl serde::Serialize,
) -> Result<String, JwtError> {
    use jsonwebtoken::{encode, EncodingKey, Header, Algorithm};

    let key = EncodingKey::from_ec_pem(private_key_pem.as_bytes())
        .map_err(|e| JwtError::KeyError(e.to_string()))?;

    let header = Header::new(Algorithm::ES256);

    encode(&header, claims, &key)
        .map_err(|e| JwtError::EncodingError(e.to_string()))
}

/// JWT 검증 (RS256)
pub fn verify_jwt_rs256<T: serde::de::DeserializeOwned>(
    token: &str,
    public_key_pem: &str,
) -> Result<T, JwtError> {
    use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm};

    let key = DecodingKey::from_rsa_pem(public_key_pem.as_bytes())
        .map_err(|e| JwtError::KeyError(e.to_string()))?;

    let validation = Validation::new(Algorithm::RS256);

    decode::<T>(token, &key, &validation)
        .map(|data| data.claims)
        .map_err(|e| JwtError::DecodingError(e.to_string()))
}

/// JWT Error type
#[derive(Debug, Clone)]
pub enum JwtError {
    KeyError(String),
    EncodingError(String),
    DecodingError(String),
}

impl std::fmt::Display for JwtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JwtError::KeyError(msg) => write!(f, "JWT key error: {msg}"),
            JwtError::EncodingError(msg) => write!(f, "JWT encoding error: {msg}"),
            JwtError::DecodingError(msg) => write!(f, "JWT decoding error: {msg}"),
        }
    }
}

impl std::error::Error for JwtError {}

/// 현재 UTC timestamp (밀리초)
pub fn current_timestamp_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// 현재 UTC timestamp (초)
pub fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hmac_sha256() {
        let signature = hmac_sha256_hex("secret", "message");
        assert!(!signature.is_empty());
    }

    #[test]
    fn test_base64() {
        let data = b"hello world";
        let encoded = base64_encode(data);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(data.to_vec(), decoded);
    }

    #[test]
    fn test_jwt_hs256() {
        let payload = serde_json::json!({
            "sub": "1234567890",
            "name": "Test User"
        });

        let token = create_jwt_hs256("secret", payload, 3600);
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
    }
}
