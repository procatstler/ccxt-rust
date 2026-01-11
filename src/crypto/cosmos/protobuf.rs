//! Protobuf Encoding Utilities for Cosmos SDK
//!
//! Low-level protobuf encoding functions used by Cosmos SDK transactions.
//! These utilities implement the Protocol Buffers wire format encoding.
//!
//! # Wire Types
//!
//! | Type | Meaning | Used For |
//! |------|---------|----------|
//! | 0 | Varint | int32, int64, uint32, uint64, sint32, sint64, bool, enum |
//! | 2 | Length-delimited | string, bytes, embedded messages |
//! | 5 | 32-bit | fixed32, sfixed32, float |
//!
//! # Example
//!
//! ```ignore
//! use ccxt_rust::crypto::cosmos::protobuf::*;
//!
//! let mut buf = Vec::new();
//! encode_string(&mut buf, 1, "hello");
//! encode_uint64(&mut buf, 2, 12345);
//! ```
//!
//! # References
//!
//! - [Protocol Buffers Encoding](https://protobuf.dev/programming-guides/encoding/)
//! - [Cosmos SDK Proto Definitions](https://github.com/cosmos/cosmos-sdk/tree/main/proto)

/// Wire type constants
pub mod wire_type {
    /// Varint: int32, int64, uint32, uint64, sint32, sint64, bool, enum
    pub const VARINT: u8 = 0;
    /// Length-delimited: string, bytes, embedded messages, packed repeated fields
    pub const LENGTH_DELIMITED: u8 = 2;
    /// 32-bit: fixed32, sfixed32, float
    pub const FIXED32: u8 = 5;
}

/// Encode a variable-length integer (varint)
///
/// Varints are a method of serializing integers using one or more bytes.
/// Smaller numbers take a smaller number of bytes.
#[inline]
pub fn encode_varint(buf: &mut Vec<u8>, value: u64) {
    let mut v = value;
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

/// Encode a field tag (field number + wire type)
#[inline]
pub fn encode_tag(buf: &mut Vec<u8>, field_number: u32, wire_type: u8) {
    encode_varint(buf, ((field_number as u64) << 3) | (wire_type as u64));
}

/// Encode a uint32 field
///
/// Skips encoding if value is 0 (protobuf default behavior).
#[inline]
pub fn encode_uint32(buf: &mut Vec<u8>, field_number: u32, value: u32) {
    if value == 0 {
        return;
    }
    encode_tag(buf, field_number, wire_type::VARINT);
    encode_varint(buf, value as u64);
}

/// Encode a uint64 field
///
/// Skips encoding if value is 0 (protobuf default behavior).
#[inline]
pub fn encode_uint64(buf: &mut Vec<u8>, field_number: u32, value: u64) {
    if value == 0 {
        return;
    }
    encode_tag(buf, field_number, wire_type::VARINT);
    encode_varint(buf, value);
}

/// Encode a fixed32 field (little-endian 4 bytes)
#[inline]
pub fn encode_fixed32(buf: &mut Vec<u8>, field_number: u32, value: u32) {
    encode_tag(buf, field_number, wire_type::FIXED32);
    buf.extend_from_slice(&value.to_le_bytes());
}

/// Encode a boolean field
///
/// Skips encoding if value is false (protobuf default behavior).
#[inline]
pub fn encode_bool(buf: &mut Vec<u8>, field_number: u32, value: bool) {
    if !value {
        return;
    }
    encode_tag(buf, field_number, wire_type::VARINT);
    buf.push(1);
}

/// Encode a string field
///
/// Skips encoding if value is empty (protobuf default behavior).
#[inline]
pub fn encode_string(buf: &mut Vec<u8>, field_number: u32, value: &str) {
    if value.is_empty() {
        return;
    }
    encode_tag(buf, field_number, wire_type::LENGTH_DELIMITED);
    encode_varint(buf, value.len() as u64);
    buf.extend_from_slice(value.as_bytes());
}

/// Encode a bytes field
///
/// Skips encoding if value is empty (protobuf default behavior).
#[inline]
pub fn encode_bytes(buf: &mut Vec<u8>, field_number: u32, value: &[u8]) {
    if value.is_empty() {
        return;
    }
    encode_tag(buf, field_number, wire_type::LENGTH_DELIMITED);
    encode_varint(buf, value.len() as u64);
    buf.extend_from_slice(value);
}

/// Encode a length-delimited field (for embedded messages)
///
/// Unlike `encode_bytes`, this always encodes even if value is empty,
/// which is needed for embedded message fields.
#[inline]
pub fn encode_length_delimited(buf: &mut Vec<u8>, field_number: u32, value: &[u8]) {
    encode_tag(buf, field_number, wire_type::LENGTH_DELIMITED);
    encode_varint(buf, value.len() as u64);
    buf.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_varint_small() {
        let mut buf = Vec::new();
        encode_varint(&mut buf, 1);
        assert_eq!(buf, vec![1]);
    }

    #[test]
    fn test_encode_varint_large() {
        let mut buf = Vec::new();
        encode_varint(&mut buf, 300);
        assert_eq!(buf, vec![0xAC, 0x02]);
    }

    #[test]
    fn test_encode_string() {
        let mut buf = Vec::new();
        encode_string(&mut buf, 1, "testing");
        // Field 1, wire type 2, length 7, "testing"
        assert_eq!(
            buf,
            vec![0x0A, 0x07, b't', b'e', b's', b't', b'i', b'n', b'g']
        );
    }

    #[test]
    fn test_encode_string_empty() {
        let mut buf = Vec::new();
        encode_string(&mut buf, 1, "");
        assert!(buf.is_empty());
    }

    #[test]
    fn test_encode_uint32() {
        let mut buf = Vec::new();
        encode_uint32(&mut buf, 2, 150);
        assert_eq!(buf, vec![0x10, 0x96, 0x01]);
    }

    #[test]
    fn test_encode_uint32_zero() {
        let mut buf = Vec::new();
        encode_uint32(&mut buf, 2, 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_encode_fixed32() {
        let mut buf = Vec::new();
        encode_fixed32(&mut buf, 1, 0x12345678);
        assert_eq!(buf, vec![0x0D, 0x78, 0x56, 0x34, 0x12]);
    }

    #[test]
    fn test_encode_bool_true() {
        let mut buf = Vec::new();
        encode_bool(&mut buf, 1, true);
        assert_eq!(buf, vec![0x08, 0x01]);
    }

    #[test]
    fn test_encode_bool_false() {
        let mut buf = Vec::new();
        encode_bool(&mut buf, 1, false);
        assert!(buf.is_empty());
    }
}
