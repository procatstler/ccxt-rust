//! Cosmos SDK Transaction Types
//!
//! Common transaction types used by Cosmos SDK-based blockchains.
//! These types implement protobuf encoding for building and signing transactions.
//!
//! # Supported Types
//!
//! - `CosmosAny` - google.protobuf.Any wrapper
//! - `CosmosPubKey` - Secp256k1 public key
//! - `CosmosFee` - Transaction fee
//! - `CosmosCoin` - Coin amount
//! - `CosmosTxBody` - Transaction body with messages
//! - `CosmosModeInfo` - Signing mode info
//! - `CosmosSignerInfo` - Signer information
//! - `CosmosAuthInfo` - Authentication info
//! - `CosmosSignDoc` - Document to sign
//! - `CosmosTxRaw` - Raw signed transaction
//!
//! # Example
//!
//! ```ignore
//! use ccxt_rust::crypto::cosmos::transaction::*;
//!
//! // Build a transaction
//! let tx_body = CosmosTxBody::new(vec![message], "memo");
//! let signer_info = CosmosSignerInfo::new(&public_key, sequence);
//! let auth_info = CosmosAuthInfo::new(vec![signer_info], CosmosFee::zero());
//! ```
//!
//! # References
//!
//! - [Cosmos SDK Tx](https://docs.cosmos.network/main/core/transactions)
//! - [Cosmos Proto Definitions](https://github.com/cosmos/cosmos-sdk/tree/main/proto/cosmos/tx/v1beta1)

use super::protobuf::*;

// ============================================================================
// google.protobuf.Any
// ============================================================================

/// google.protobuf.Any - Universal message wrapper
///
/// Used to wrap arbitrary protobuf messages with their type URL.
#[derive(Clone, Debug, PartialEq)]
pub struct CosmosAny {
    /// Type URL (e.g., "/cosmos.bank.v1beta1.MsgSend")
    pub type_url: String,
    /// Encoded message bytes
    pub value: Vec<u8>,
}

impl CosmosAny {
    /// Create a new Any message
    pub fn new(type_url: impl Into<String>, value: Vec<u8>) -> Self {
        Self {
            type_url: type_url.into(),
            value,
        }
    }

    /// Encode to protobuf bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Field 1: type_url (string)
        encode_string(&mut buf, 1, &self.type_url);
        // Field 2: value (bytes)
        encode_bytes(&mut buf, 2, &self.value);
        buf
    }
}

// ============================================================================
// Public Key
// ============================================================================

/// Cosmos SDK Secp256k1 public key
#[derive(Clone, Debug)]
pub struct CosmosPubKey {
    /// Compressed public key bytes (33 bytes)
    pub key: Vec<u8>,
}

impl CosmosPubKey {
    /// Type URL for secp256k1 public key
    pub const TYPE_URL: &'static str = "/cosmos.crypto.secp256k1.PubKey";

    /// Create a new public key
    pub fn new(key: &[u8]) -> Self {
        Self { key: key.to_vec() }
    }

    /// Encode to protobuf bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Field 1: key (bytes)
        encode_bytes(&mut buf, 1, &self.key);
        buf
    }

    /// Wrap as Any message
    pub fn to_any(&self) -> CosmosAny {
        CosmosAny {
            type_url: Self::TYPE_URL.to_string(),
            value: self.encode(),
        }
    }
}

// ============================================================================
// Fee and Coin
// ============================================================================

/// Transaction fee
#[derive(Clone, Debug)]
pub struct CosmosFee {
    /// Fee amount
    pub amount: Vec<CosmosCoin>,
    /// Gas limit
    pub gas_limit: u64,
    /// Fee payer address (optional)
    pub payer: String,
    /// Fee granter address (optional)
    pub granter: String,
}

impl CosmosFee {
    /// Create a zero fee (used by some chains like dYdX v4)
    pub fn zero() -> Self {
        Self {
            amount: vec![],
            gas_limit: 0,
            payer: String::new(),
            granter: String::new(),
        }
    }

    /// Create a fee with specified amount and gas
    pub fn new(amount: Vec<CosmosCoin>, gas_limit: u64) -> Self {
        Self {
            amount,
            gas_limit,
            payer: String::new(),
            granter: String::new(),
        }
    }

    /// Encode to protobuf bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Field 1: amount (repeated Coin)
        for coin in &self.amount {
            let coin_bytes = coin.encode();
            encode_length_delimited(&mut buf, 1, &coin_bytes);
        }
        // Field 2: gas_limit (uint64)
        if self.gas_limit > 0 {
            encode_uint64(&mut buf, 2, self.gas_limit);
        }
        // Field 3: payer (string)
        if !self.payer.is_empty() {
            encode_string(&mut buf, 3, &self.payer);
        }
        // Field 4: granter (string)
        if !self.granter.is_empty() {
            encode_string(&mut buf, 4, &self.granter);
        }
        buf
    }
}

/// Coin amount
#[derive(Clone, Debug)]
pub struct CosmosCoin {
    /// Denomination (e.g., "uatom", "adydx")
    pub denom: String,
    /// Amount as string
    pub amount: String,
}

impl CosmosCoin {
    /// Create a new coin
    pub fn new(denom: impl Into<String>, amount: impl Into<String>) -> Self {
        Self {
            denom: denom.into(),
            amount: amount.into(),
        }
    }

    /// Encode to protobuf bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        encode_string(&mut buf, 1, &self.denom);
        encode_string(&mut buf, 2, &self.amount);
        buf
    }
}

// ============================================================================
// Transaction Body
// ============================================================================

/// Transaction body containing messages
#[derive(Clone, Debug)]
pub struct CosmosTxBody {
    /// Messages to execute
    pub messages: Vec<CosmosAny>,
    /// Transaction memo
    pub memo: String,
    /// Timeout block height (0 = no timeout)
    pub timeout_height: u64,
    /// Extension options
    pub extension_options: Vec<CosmosAny>,
    /// Non-critical extension options
    pub non_critical_extension_options: Vec<CosmosAny>,
}

impl CosmosTxBody {
    /// Create a new transaction body
    pub fn new(messages: Vec<CosmosAny>, memo: &str) -> Self {
        Self {
            messages,
            memo: memo.to_string(),
            timeout_height: 0,
            extension_options: vec![],
            non_critical_extension_options: vec![],
        }
    }

    /// Create with timeout height
    pub fn with_timeout(mut self, timeout_height: u64) -> Self {
        self.timeout_height = timeout_height;
        self
    }

    /// Encode to protobuf bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Field 1: messages (repeated Any)
        for msg in &self.messages {
            let msg_bytes = msg.encode();
            encode_length_delimited(&mut buf, 1, &msg_bytes);
        }
        // Field 2: memo (string)
        if !self.memo.is_empty() {
            encode_string(&mut buf, 2, &self.memo);
        }
        // Field 3: timeout_height (uint64)
        if self.timeout_height > 0 {
            encode_uint64(&mut buf, 3, self.timeout_height);
        }
        buf
    }
}

// ============================================================================
// Signing Mode
// ============================================================================

/// Signing mode
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SignMode {
    /// SIGN_MODE_DIRECT - Sign raw bytes directly
    Direct = 1,
    /// SIGN_MODE_TEXTUAL - Sign human-readable text
    Textual = 2,
    /// SIGN_MODE_LEGACY_AMINO_JSON - Legacy Amino JSON signing
    LegacyAminoJson = 127,
}

/// Mode info for signing
#[derive(Clone, Debug)]
pub struct CosmosModeInfo {
    /// Signing mode
    pub mode: SignMode,
}

impl CosmosModeInfo {
    /// Create DIRECT signing mode
    pub fn direct() -> Self {
        Self {
            mode: SignMode::Direct,
        }
    }

    /// Encode to protobuf bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // single.mode (nested message)
        let mut single_buf = Vec::new();
        encode_uint32(&mut single_buf, 1, self.mode as u32);
        encode_length_delimited(&mut buf, 1, &single_buf);
        buf
    }
}

// ============================================================================
// Signer Info
// ============================================================================

/// Signer information
#[derive(Clone, Debug)]
pub struct CosmosSignerInfo {
    /// Public key wrapped as Any
    pub public_key: Option<CosmosAny>,
    /// Signing mode info
    pub mode_info: CosmosModeInfo,
    /// Account sequence number
    pub sequence: u64,
}

impl CosmosSignerInfo {
    /// Create a new signer info with secp256k1 public key
    pub fn new(public_key: &[u8], sequence: u64) -> Self {
        let pubkey = CosmosPubKey::new(public_key);
        Self {
            public_key: Some(pubkey.to_any()),
            mode_info: CosmosModeInfo::direct(),
            sequence,
        }
    }

    /// Create without public key (for simulation)
    pub fn without_pubkey(sequence: u64) -> Self {
        Self {
            public_key: None,
            mode_info: CosmosModeInfo::direct(),
            sequence,
        }
    }

    /// Encode to protobuf bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Field 1: public_key (Any)
        if let Some(pk) = &self.public_key {
            let pk_bytes = pk.encode();
            encode_length_delimited(&mut buf, 1, &pk_bytes);
        }
        // Field 2: mode_info (ModeInfo)
        let mode_bytes = self.mode_info.encode();
        encode_length_delimited(&mut buf, 2, &mode_bytes);
        // Field 3: sequence (uint64)
        encode_uint64(&mut buf, 3, self.sequence);
        buf
    }
}

// ============================================================================
// Auth Info
// ============================================================================

/// Authentication info
#[derive(Clone, Debug)]
pub struct CosmosAuthInfo {
    /// Signer information
    pub signer_infos: Vec<CosmosSignerInfo>,
    /// Transaction fee
    pub fee: CosmosFee,
}

impl CosmosAuthInfo {
    /// Create new auth info
    pub fn new(signer_infos: Vec<CosmosSignerInfo>, fee: CosmosFee) -> Self {
        Self { signer_infos, fee }
    }

    /// Encode to protobuf bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Field 1: signer_infos (repeated SignerInfo)
        for signer in &self.signer_infos {
            let signer_bytes = signer.encode();
            encode_length_delimited(&mut buf, 1, &signer_bytes);
        }
        // Field 2: fee (Fee)
        let fee_bytes = self.fee.encode();
        encode_length_delimited(&mut buf, 2, &fee_bytes);
        buf
    }
}

// ============================================================================
// Sign Document
// ============================================================================

/// Document to sign (SignDoc)
///
/// This is what gets signed by the private key.
#[derive(Clone, Debug)]
pub struct CosmosSignDoc {
    /// Encoded TxBody bytes
    pub body_bytes: Vec<u8>,
    /// Encoded AuthInfo bytes
    pub auth_info_bytes: Vec<u8>,
    /// Chain ID
    pub chain_id: String,
    /// Account number
    pub account_number: u64,
}

impl CosmosSignDoc {
    /// Create a new sign document
    pub fn new(
        body_bytes: Vec<u8>,
        auth_info_bytes: Vec<u8>,
        chain_id: impl Into<String>,
        account_number: u64,
    ) -> Self {
        Self {
            body_bytes,
            auth_info_bytes,
            chain_id: chain_id.into(),
            account_number,
        }
    }

    /// Encode to protobuf bytes (this is what gets signed)
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Field 1: body_bytes (bytes)
        encode_bytes(&mut buf, 1, &self.body_bytes);
        // Field 2: auth_info_bytes (bytes)
        encode_bytes(&mut buf, 2, &self.auth_info_bytes);
        // Field 3: chain_id (string)
        encode_string(&mut buf, 3, &self.chain_id);
        // Field 4: account_number (uint64)
        encode_uint64(&mut buf, 4, self.account_number);
        buf
    }
}

// ============================================================================
// Raw Transaction
// ============================================================================

/// Raw signed transaction (TxRaw)
///
/// This is the final format broadcast to the network.
#[derive(Clone, Debug)]
pub struct CosmosTxRaw {
    /// Encoded TxBody bytes
    pub body_bytes: Vec<u8>,
    /// Encoded AuthInfo bytes
    pub auth_info_bytes: Vec<u8>,
    /// Signatures
    pub signatures: Vec<Vec<u8>>,
}

impl CosmosTxRaw {
    /// Create a new raw transaction
    pub fn new(body_bytes: Vec<u8>, auth_info_bytes: Vec<u8>, signatures: Vec<Vec<u8>>) -> Self {
        Self {
            body_bytes,
            auth_info_bytes,
            signatures,
        }
    }

    /// Encode to protobuf bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Field 1: body_bytes (bytes)
        encode_bytes(&mut buf, 1, &self.body_bytes);
        // Field 2: auth_info_bytes (bytes)
        encode_bytes(&mut buf, 2, &self.auth_info_bytes);
        // Field 3: signatures (repeated bytes)
        for sig in &self.signatures {
            encode_bytes(&mut buf, 3, sig);
        }
        buf
    }

    /// Encode to base64 string for broadcasting
    pub fn to_base64(&self) -> String {
        use base64::{engine::general_purpose::STANDARD, Engine};
        STANDARD.encode(self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosmos_any_encode() {
        let any = CosmosAny::new("/test.Msg", vec![1, 2, 3]);
        let encoded = any.encode();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_cosmos_pubkey_to_any() {
        let pubkey = CosmosPubKey::new(&[0u8; 33]);
        let any = pubkey.to_any();
        assert_eq!(any.type_url, "/cosmos.crypto.secp256k1.PubKey");
    }

    #[test]
    fn test_cosmos_fee_zero() {
        let fee = CosmosFee::zero();
        assert!(fee.amount.is_empty());
        assert_eq!(fee.gas_limit, 0);
    }

    #[test]
    fn test_cosmos_coin_encode() {
        let coin = CosmosCoin::new("uatom", "1000000");
        let encoded = coin.encode();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_cosmos_tx_body_encode() {
        let body = CosmosTxBody::new(vec![], "test memo");
        let encoded = body.encode();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_cosmos_mode_info_direct() {
        let mode = CosmosModeInfo::direct();
        assert_eq!(mode.mode, SignMode::Direct);
    }

    #[test]
    fn test_cosmos_signer_info_encode() {
        let signer = CosmosSignerInfo::new(&[0u8; 33], 0);
        let encoded = signer.encode();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_cosmos_auth_info_encode() {
        let signer = CosmosSignerInfo::new(&[0u8; 33], 0);
        let auth = CosmosAuthInfo::new(vec![signer], CosmosFee::zero());
        let encoded = auth.encode();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_cosmos_sign_doc_encode() {
        let doc = CosmosSignDoc::new(vec![1, 2], vec![3, 4], "test-chain", 1);
        let encoded = doc.encode();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_cosmos_tx_raw_encode() {
        let tx = CosmosTxRaw::new(vec![1, 2], vec![3, 4], vec![vec![5, 6]]);
        let encoded = tx.encode();
        assert!(!encoded.is_empty());
    }
}
