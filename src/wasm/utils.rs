//! WASM Utility Functions
//!
//! Cryptographic and encoding utilities for JavaScript.

use wasm_bindgen::prelude::*;

/// HMAC-SHA256 signing utility (for API authentication)
#[wasm_bindgen(js_name = hmacSha256)]
pub fn hmac_sha256(secret: &str, message: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// HMAC-SHA512 signing utility (for API authentication)
#[wasm_bindgen(js_name = hmacSha512)]
pub fn hmac_sha512(secret: &str, message: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha512;

    type HmacSha512 = Hmac<Sha512>;

    let mut mac = HmacSha512::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// SHA256 hash utility
#[wasm_bindgen(js_name = sha256)]
pub fn sha256_hash(data: &str) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

/// SHA512 hash utility
#[wasm_bindgen(js_name = sha512)]
pub fn sha512_hash(data: &str) -> String {
    use sha2::{Sha512, Digest};
    let mut hasher = Sha512::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

/// Base64 encode
#[wasm_bindgen(js_name = base64Encode)]
pub fn base64_encode(data: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data.as_bytes())
}

/// Base64 decode
#[wasm_bindgen(js_name = base64Decode)]
pub fn base64_decode(data: &str) -> Result<String, JsValue> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| JsValue::from_str(&e.to_string()))
        .and_then(|bytes| {
            String::from_utf8(bytes)
                .map_err(|e| JsValue::from_str(&e.to_string()))
        })
}

/// URL encode
#[wasm_bindgen(js_name = urlEncode)]
pub fn url_encode(data: &str) -> String {
    urlencoding::encode(data).to_string()
}

/// Generate UUID v4
#[wasm_bindgen(js_name = generateUuid)]
pub fn generate_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Get current timestamp in milliseconds
#[wasm_bindgen(js_name = getCurrentTimestamp)]
pub fn get_current_timestamp() -> i64 {
    js_sys::Date::now() as i64
}

/// Parse JSON safely
#[wasm_bindgen(js_name = parseJson)]
pub fn parse_json(json_str: &str) -> Result<JsValue, JsValue> {
    serde_json::from_str::<serde_json::Value>(json_str)
        .map(|v| JsValue::from_str(&v.to_string()))
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Helper to convert errors to JsValue
pub fn js_err(msg: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&msg.to_string())
}
