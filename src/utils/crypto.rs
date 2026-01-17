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
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
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
    let mut mac = HmacSha384::new_from_slice(secret).expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// HMAC-SHA384 서명을 hex 문자열로 반환
pub fn hmac_sha384_hex(secret: &[u8], message: &str) -> String {
    hex::encode(hmac_sha384(secret, message))
}

/// HMAC-SHA512 서명 생성
pub fn hmac_sha512(secret: &str, message: &str) -> Vec<u8> {
    let mut mac =
        HmacSha512::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
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
pub fn create_jwt_hs256(secret: &str, payload: serde_json::Value, expiry_seconds: u64) -> String {
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
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};

    let key = EncodingKey::from_rsa_pem(private_key_pem.as_bytes())
        .map_err(|e| JwtError::KeyError(e.to_string()))?;

    let header = Header::new(Algorithm::RS256);

    encode(&header, claims, &key).map_err(|e| JwtError::EncodingError(e.to_string()))
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
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};

    let key = EncodingKey::from_ec_pem(private_key_pem.as_bytes())
        .map_err(|e| JwtError::KeyError(e.to_string()))?;

    let header = Header::new(Algorithm::ES256);

    encode(&header, claims, &key).map_err(|e| JwtError::EncodingError(e.to_string()))
}

/// JWT 검증 (RS256)
pub fn verify_jwt_rs256<T: serde::de::DeserializeOwned>(
    token: &str,
    public_key_pem: &str,
) -> Result<T, JwtError> {
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

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

// === ECDSA (secp256k1) signatures ===
// Requires the "advanced-crypto" or "dex" feature

/// ECDSA secp256k1 signing error
#[cfg(feature = "advanced-crypto")]
#[derive(Debug, Clone)]
pub enum EcdsaError {
    InvalidPrivateKey(String),
    SigningFailed(String),
    InvalidSignature(String),
    VerificationFailed(String),
}

#[cfg(feature = "advanced-crypto")]
impl std::fmt::Display for EcdsaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EcdsaError::InvalidPrivateKey(msg) => write!(f, "Invalid private key: {msg}"),
            EcdsaError::SigningFailed(msg) => write!(f, "Signing failed: {msg}"),
            EcdsaError::InvalidSignature(msg) => write!(f, "Invalid signature: {msg}"),
            EcdsaError::VerificationFailed(msg) => write!(f, "Verification failed: {msg}"),
        }
    }
}

#[cfg(feature = "advanced-crypto")]
impl std::error::Error for EcdsaError {}

/// Sign a message using ECDSA secp256k1
///
/// # Arguments
/// * `private_key` - 32-byte private key (hex string without 0x prefix)
/// * `message` - Message bytes to sign
///
/// # Returns
/// * Signature as 64-byte (r || s) hex string
#[cfg(feature = "advanced-crypto")]
pub fn ecdsa_sign(private_key: &str, message: &[u8]) -> Result<String, EcdsaError> {
    use k256::ecdsa::{signature::Signer, Signature, SigningKey};

    let key_bytes = hex::decode(private_key.trim_start_matches("0x"))
        .map_err(|e| EcdsaError::InvalidPrivateKey(e.to_string()))?;

    let signing_key = SigningKey::from_bytes(key_bytes.as_slice().into())
        .map_err(|e| EcdsaError::InvalidPrivateKey(e.to_string()))?;

    let signature: Signature = signing_key.sign(message);
    Ok(hex::encode(signature.to_bytes()))
}

/// Sign a message hash using ECDSA secp256k1 (pre-hashed message)
///
/// # Arguments
/// * `private_key` - 32-byte private key (hex string without 0x prefix)
/// * `message_hash` - 32-byte message hash
///
/// # Returns
/// * Signature as 64-byte (r || s) hex string
#[cfg(feature = "advanced-crypto")]
pub fn ecdsa_sign_hash(private_key: &str, message_hash: &[u8]) -> Result<String, EcdsaError> {
    use k256::ecdsa::{signature::hazmat::PrehashSigner, Signature, SigningKey};

    if message_hash.len() != 32 {
        return Err(EcdsaError::SigningFailed("Message hash must be 32 bytes".to_string()));
    }

    let key_bytes = hex::decode(private_key.trim_start_matches("0x"))
        .map_err(|e| EcdsaError::InvalidPrivateKey(e.to_string()))?;

    let signing_key = SigningKey::from_bytes(key_bytes.as_slice().into())
        .map_err(|e| EcdsaError::InvalidPrivateKey(e.to_string()))?;

    let signature: Signature = signing_key
        .sign_prehash(message_hash)
        .map_err(|e| EcdsaError::SigningFailed(e.to_string()))?;

    Ok(hex::encode(signature.to_bytes()))
}

/// Get public key from private key (ECDSA secp256k1)
///
/// # Arguments
/// * `private_key` - 32-byte private key (hex string without 0x prefix)
///
/// # Returns
/// * Public key as 33-byte compressed hex string
#[cfg(feature = "advanced-crypto")]
pub fn ecdsa_get_public_key(private_key: &str) -> Result<String, EcdsaError> {
    use k256::ecdsa::SigningKey;

    let key_bytes = hex::decode(private_key.trim_start_matches("0x"))
        .map_err(|e| EcdsaError::InvalidPrivateKey(e.to_string()))?;

    let signing_key = SigningKey::from_bytes(key_bytes.as_slice().into())
        .map_err(|e| EcdsaError::InvalidPrivateKey(e.to_string()))?;

    let verifying_key = signing_key.verifying_key();
    let public_key_bytes = verifying_key.to_encoded_point(true);

    Ok(hex::encode(public_key_bytes.as_bytes()))
}

/// Verify ECDSA signature
///
/// # Arguments
/// * `public_key` - 33-byte compressed public key (hex string)
/// * `message` - Original message bytes
/// * `signature` - 64-byte signature (hex string)
#[cfg(feature = "advanced-crypto")]
pub fn ecdsa_verify(
    public_key: &str,
    message: &[u8],
    signature: &str,
) -> Result<bool, EcdsaError> {
    use k256::ecdsa::{signature::Verifier, Signature, VerifyingKey};

    let pubkey_bytes = hex::decode(public_key.trim_start_matches("0x"))
        .map_err(|e| EcdsaError::InvalidSignature(e.to_string()))?;

    let verifying_key = VerifyingKey::from_sec1_bytes(&pubkey_bytes)
        .map_err(|e| EcdsaError::InvalidSignature(e.to_string()))?;

    let sig_bytes = hex::decode(signature.trim_start_matches("0x"))
        .map_err(|e| EcdsaError::InvalidSignature(e.to_string()))?;

    let sig = Signature::from_slice(&sig_bytes)
        .map_err(|e| EcdsaError::InvalidSignature(e.to_string()))?;

    Ok(verifying_key.verify(message, &sig).is_ok())
}

// === Ed25519 signatures ===
// Requires the "advanced-crypto" feature

/// Ed25519 signing error
#[cfg(feature = "advanced-crypto")]
#[derive(Debug, Clone)]
pub enum Ed25519Error {
    InvalidPrivateKey(String),
    InvalidPublicKey(String),
    SigningFailed(String),
    InvalidSignature(String),
    VerificationFailed(String),
}

#[cfg(feature = "advanced-crypto")]
impl std::fmt::Display for Ed25519Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ed25519Error::InvalidPrivateKey(msg) => write!(f, "Invalid private key: {msg}"),
            Ed25519Error::InvalidPublicKey(msg) => write!(f, "Invalid public key: {msg}"),
            Ed25519Error::SigningFailed(msg) => write!(f, "Signing failed: {msg}"),
            Ed25519Error::InvalidSignature(msg) => write!(f, "Invalid signature: {msg}"),
            Ed25519Error::VerificationFailed(msg) => write!(f, "Verification failed: {msg}"),
        }
    }
}

#[cfg(feature = "advanced-crypto")]
impl std::error::Error for Ed25519Error {}

/// Sign a message using Ed25519
///
/// # Arguments
/// * `private_key` - 32-byte private key (hex string or base64)
/// * `message` - Message bytes to sign
///
/// # Returns
/// * 64-byte signature as hex string
#[cfg(feature = "advanced-crypto")]
pub fn ed25519_sign(private_key: &str, message: &[u8]) -> Result<String, Ed25519Error> {
    use ed25519_dalek::{Signer, SigningKey};

    let key_bytes = decode_key(private_key)
        .map_err(|e| Ed25519Error::InvalidPrivateKey(e.to_string()))?;

    if key_bytes.len() != 32 {
        return Err(Ed25519Error::InvalidPrivateKey(format!(
            "Expected 32 bytes, got {}",
            key_bytes.len()
        )));
    }

    let mut key_array = [0u8; 32];
    key_array.copy_from_slice(&key_bytes);

    let signing_key = SigningKey::from_bytes(&key_array);
    let signature = signing_key.sign(message);

    Ok(hex::encode(signature.to_bytes()))
}

/// Sign a message using Ed25519 and return base64 signature
#[cfg(feature = "advanced-crypto")]
pub fn ed25519_sign_base64(private_key: &str, message: &[u8]) -> Result<String, Ed25519Error> {
    use ed25519_dalek::{Signer, SigningKey};

    let key_bytes = decode_key(private_key)
        .map_err(|e| Ed25519Error::InvalidPrivateKey(e.to_string()))?;

    if key_bytes.len() != 32 {
        return Err(Ed25519Error::InvalidPrivateKey(format!(
            "Expected 32 bytes, got {}",
            key_bytes.len()
        )));
    }

    let mut key_array = [0u8; 32];
    key_array.copy_from_slice(&key_bytes);

    let signing_key = SigningKey::from_bytes(&key_array);
    let signature = signing_key.sign(message);

    Ok(base64_encode(&signature.to_bytes()))
}

/// Get public key from Ed25519 private key
///
/// # Arguments
/// * `private_key` - 32-byte private key (hex string or base64)
///
/// # Returns
/// * 32-byte public key as hex string
#[cfg(feature = "advanced-crypto")]
pub fn ed25519_get_public_key(private_key: &str) -> Result<String, Ed25519Error> {
    use ed25519_dalek::SigningKey;

    let key_bytes = decode_key(private_key)
        .map_err(|e| Ed25519Error::InvalidPrivateKey(e.to_string()))?;

    if key_bytes.len() != 32 {
        return Err(Ed25519Error::InvalidPrivateKey(format!(
            "Expected 32 bytes, got {}",
            key_bytes.len()
        )));
    }

    let mut key_array = [0u8; 32];
    key_array.copy_from_slice(&key_bytes);

    let signing_key = SigningKey::from_bytes(&key_array);
    let public_key = signing_key.verifying_key();

    Ok(hex::encode(public_key.as_bytes()))
}

/// Verify Ed25519 signature
///
/// # Arguments
/// * `public_key` - 32-byte public key (hex string or base64)
/// * `message` - Original message bytes
/// * `signature` - 64-byte signature (hex string or base64)
#[cfg(feature = "advanced-crypto")]
pub fn ed25519_verify(
    public_key: &str,
    message: &[u8],
    signature: &str,
) -> Result<bool, Ed25519Error> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let pubkey_bytes = decode_key(public_key)
        .map_err(|e| Ed25519Error::InvalidPublicKey(e.to_string()))?;

    if pubkey_bytes.len() != 32 {
        return Err(Ed25519Error::InvalidPublicKey(format!(
            "Expected 32 bytes, got {}",
            pubkey_bytes.len()
        )));
    }

    let mut pubkey_array = [0u8; 32];
    pubkey_array.copy_from_slice(&pubkey_bytes);

    let verifying_key = VerifyingKey::from_bytes(&pubkey_array)
        .map_err(|e| Ed25519Error::InvalidPublicKey(e.to_string()))?;

    let sig_bytes = decode_key(signature)
        .map_err(|e| Ed25519Error::InvalidSignature(e.to_string()))?;

    if sig_bytes.len() != 64 {
        return Err(Ed25519Error::InvalidSignature(format!(
            "Expected 64 bytes, got {}",
            sig_bytes.len()
        )));
    }

    let mut sig_array = [0u8; 64];
    sig_array.copy_from_slice(&sig_bytes);

    let sig = Signature::from_bytes(&sig_array);

    Ok(verifying_key.verify(message, &sig).is_ok())
}

/// Helper function to decode a key from hex or base64
#[cfg(feature = "advanced-crypto")]
fn decode_key(key: &str) -> Result<Vec<u8>, String> {
    let trimmed = key.trim().trim_start_matches("0x");

    // Try hex first
    if let Ok(bytes) = hex::decode(trimmed) {
        return Ok(bytes);
    }

    // Try base64
    if let Ok(bytes) = base64_decode(trimmed) {
        return Ok(bytes);
    }

    Err("Failed to decode key as hex or base64".to_string())
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

    #[test]
    fn test_hmac_sha384() {
        let secret = b"secret";
        let signature = hmac_sha384_hex(secret, "message");
        assert!(!signature.is_empty());
        // SHA384 hex output is 96 characters
        assert_eq!(signature.len(), 96);
    }

    #[test]
    fn test_hmac_sha512() {
        let signature = hmac_sha512_hex("secret", "message");
        assert!(!signature.is_empty());
        // SHA512 hex output is 128 characters
        assert_eq!(signature.len(), 128);
    }

    #[test]
    fn test_hmac_sha256_base64() {
        let signature = hmac_sha256_base64("secret", "message");
        assert!(!signature.is_empty());
    }

    #[test]
    fn test_base64_url_encode() {
        let data = b"test data with special chars";
        let encoded = base64_url_encode(data);
        // URL safe base64는 +/= 대신 -_를 사용
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
    }

    #[test]
    fn test_base64_url_decode() {
        let data = b"hello world";
        let encoded = base64_url_encode(data);
        let decoded = base64_url_decode(&encoded).unwrap();
        assert_eq!(data.to_vec(), decoded);
    }

    #[test]
    fn test_current_timestamp_ms() {
        let ts = current_timestamp_ms();
        // 13자리 이상 (밀리초)
        assert!(ts > 1_000_000_000_000);
    }

    #[test]
    fn test_current_timestamp() {
        let ts = current_timestamp();
        // 10자리 (초)
        assert!(ts > 1_000_000_000);
        assert!(ts < 10_000_000_000);
    }
}

/// ECDSA/Ed25519 테스트 (advanced-crypto feature 필요)
#[cfg(all(test, feature = "advanced-crypto"))]
mod advanced_crypto_tests {
    use super::*;

    #[test]
    fn test_ecdsa_sign_and_verify() {
        // 테스트용 개인키 (32 bytes hex)
        let private_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let message = b"Hello, ECDSA!";

        // 서명 생성
        let signature = ecdsa_sign(private_key, message).expect("Failed to create signature");
        assert!(!signature.is_empty());

        // 공개키 추출
        let public_key = ecdsa_get_public_key(private_key).expect("Failed to get public key");
        assert!(!public_key.is_empty());

        // 서명 검증
        let is_valid = ecdsa_verify(&public_key, message, &signature).expect("Failed to verify");
        assert!(is_valid, "Signature verification failed");
    }

    #[test]
    fn test_ecdsa_sign_hash() {
        let private_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        // 32 bytes hash
        let message_hash = hex::decode("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap();

        let signature = ecdsa_sign_hash(private_key, &message_hash).expect("Failed to sign hash");
        assert!(!signature.is_empty());
    }

    #[test]
    fn test_ecdsa_get_public_key() {
        let private_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let public_key = ecdsa_get_public_key(private_key).expect("Failed to get public key");

        // 압축된 공개키는 33 bytes (66 hex chars)
        assert_eq!(public_key.len(), 66);
    }

    #[test]
    fn test_ecdsa_invalid_key() {
        let invalid_key = "not_a_valid_key";
        let result = ecdsa_sign(invalid_key, b"message");
        assert!(result.is_err());
    }

    #[test]
    fn test_ed25519_sign_and_verify() {
        // 테스트용 개인키 (32 bytes hex)
        let private_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let message = b"Hello, Ed25519!";

        // 서명 생성
        let signature = ed25519_sign(private_key, message).expect("Failed to create signature");
        assert!(!signature.is_empty());

        // 공개키 추출
        let public_key = ed25519_get_public_key(private_key).expect("Failed to get public key");
        assert!(!public_key.is_empty());

        // 서명 검증
        let is_valid = ed25519_verify(&public_key, message, &signature).expect("Failed to verify");
        assert!(is_valid, "Signature verification failed");
    }

    #[test]
    fn test_ed25519_sign_base64() {
        let private_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let message = b"Hello, Ed25519!";

        let signature = ed25519_sign_base64(private_key, message).expect("Failed to create base64 signature");
        // Base64 인코딩된 서명
        assert!(!signature.is_empty());
    }

    #[test]
    fn test_ed25519_get_public_key() {
        let private_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let public_key = ed25519_get_public_key(private_key).expect("Failed to get public key");

        // Ed25519 공개키는 32 bytes (64 hex chars)
        assert_eq!(public_key.len(), 64);
    }

    #[test]
    fn test_ed25519_invalid_key() {
        let invalid_key = "not_a_valid_key";
        let result = ed25519_sign(invalid_key, b"message");
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_key_hex() {
        let hex_key = "0123456789abcdef";
        let decoded = decode_key(hex_key);
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap().len(), 8);
    }

    #[test]
    fn test_decode_key_base64() {
        let base64_key = "SGVsbG8gV29ybGQ="; // "Hello World"
        let decoded = decode_key(base64_key);
        assert!(decoded.is_ok());
    }
}
