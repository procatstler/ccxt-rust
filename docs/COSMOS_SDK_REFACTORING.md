# Cosmos SDK Common Code Extraction Plan

## Overview

This document describes the plan to extract reusable Cosmos SDK components from `dydxv4_order.rs` to `src/crypto/cosmos/` for future Cosmos-based DEX support (Osmosis, Injective, Sei, etc.).

## Current State

### File: `src/exchanges/dex/dydxv4_order.rs` (1,314 lines)

| Section | Lines | Content | Reusability |
|---------|-------|---------|-------------|
| dYdX Order Types | 1-399 | SubaccountId, OrderId, DydxOrder, DydxOrderSide, etc. | dYdX specific |
| **Cosmos SDK Types** | 400-681 | CosmosAny, CosmosTxBody, CosmosFee, etc. | **Reusable** |
| **Protobuf Encoding** | 683-788 | encode_varint, encode_string, etc. | **Reusable** |
| dYdX Market Info | 789-844 | DydxMarketInfo | dYdX specific |
| dYdX Tx Builder | 845-914 | DydxTransactionBuilder | dYdX specific |
| dYdX Node Client | 915-1193 | DydxNodeClient | dYdX specific |
| Tests | 1194-1314 | Unit tests | Mixed |

## Target Structure

### Before
```
src/crypto/cosmos/
├── mod.rs           # Module exports
├── wallet.rs        # CosmosWallet implementation
├── signer.rs        # Transaction signing
├── keys.rs          # Key management
└── address.rs       # Address utilities

src/exchanges/dex/
├── dydxv4.rs        # Main exchange implementation
├── dydxv4_ws.rs     # WebSocket implementation
└── dydxv4_order.rs  # Orders + Cosmos types + Protobuf (mixed)
```

### After
```
src/crypto/cosmos/
├── mod.rs           # Module exports (updated)
├── wallet.rs        # CosmosWallet implementation
├── signer.rs        # Transaction signing
├── keys.rs          # Key management
├── address.rs       # Address utilities
├── protobuf.rs      # NEW: Protobuf encoding utilities
└── transaction.rs   # NEW: Cosmos SDK transaction types

src/exchanges/dex/
├── dydxv4.rs        # Main exchange implementation
├── dydxv4_ws.rs     # WebSocket implementation
└── dydxv4_order.rs  # dYdX-specific orders only (reduced)
```

## Components to Extract

### 1. `protobuf.rs` - Protobuf Encoding Utilities

Low-level protobuf encoding functions used by Cosmos SDK:

```rust
// Functions to extract:
pub fn encode_varint(buf: &mut Vec<u8>, value: u64)
pub fn encode_tag(buf: &mut Vec<u8>, field_number: u32, wire_type: u8)
pub fn encode_string(buf: &mut Vec<u8>, field_number: u32, value: &str)
pub fn encode_bytes(buf: &mut Vec<u8>, field_number: u32, value: &[u8])
pub fn encode_uint32(buf: &mut Vec<u8>, field_number: u32, value: u32)
pub fn encode_uint64(buf: &mut Vec<u8>, field_number: u32, value: u64)
pub fn encode_fixed32(buf: &mut Vec<u8>, field_number: u32, value: u32)
pub fn encode_length_delimited(buf: &mut Vec<u8>, field_number: u32, value: &[u8])
```

### 2. `transaction.rs` - Cosmos SDK Transaction Types

Standard Cosmos SDK types for transaction building:

```rust
// Types to extract:
pub struct CosmosAny { type_url: String, value: Vec<u8> }
pub struct CosmosPubKey { key: Vec<u8> }
pub struct CosmosFee { amount: Vec<CosmosCoin>, gas_limit: u64, ... }
pub struct CosmosCoin { denom: String, amount: String }
pub struct CosmosTxBody { messages: Vec<CosmosAny>, memo: String, ... }
pub struct CosmosModeInfo { /* DIRECT signing mode */ }
pub struct CosmosSignerInfo { public_key: CosmosAny, mode_info: CosmosModeInfo, sequence: u64 }
pub struct CosmosAuthInfo { signer_infos: Vec<CosmosSignerInfo>, fee: CosmosFee }
pub struct CosmosSignDoc { body_bytes: Vec<u8>, auth_info_bytes: Vec<u8>, chain_id: String, account_number: u64 }
pub struct CosmosTxRaw { body_bytes: Vec<u8>, auth_info_bytes: Vec<u8>, signatures: Vec<Vec<u8>> }
```

## Benefits

### 1. Reusability for Future Cosmos DEXes
- **Osmosis**: Cosmos SDK-based DEX with AMM
- **Injective**: Cosmos SDK-based derivatives DEX
- **Sei**: High-performance Cosmos L1 for trading
- **Neutron**: Cosmos smart contract platform

### 2. Code Organization
- Exchange files contain only exchange-specific logic
- Common infrastructure in `src/crypto/`
- Clear separation of concerns

### 3. Testing
- Cosmos SDK components can be tested independently
- Easier to verify protocol compliance
- Better test coverage

### 4. Maintenance
- Protocol updates affect single location
- Consistent implementation across exchanges
- Reduced code duplication

## Implementation Steps

1. **Create `src/crypto/cosmos/protobuf.rs`**
   - Extract all `encode_*` functions
   - Add documentation and examples
   - Add unit tests

2. **Create `src/crypto/cosmos/transaction.rs`**
   - Extract Cosmos SDK types (CosmosAny, CosmosTxBody, etc.)
   - Import from protobuf.rs
   - Add documentation

3. **Update `src/crypto/cosmos/mod.rs`**
   - Export new modules
   - Update public API

4. **Update `src/exchanges/dex/dydxv4_order.rs`**
   - Remove extracted code
   - Add imports from `crate::crypto::cosmos::{protobuf, transaction}`
   - Verify all references updated

5. **Verify**
   - Run `cargo build --all-features`
   - Run `cargo test --all-features`
   - Run `cargo clippy --all-features`

## Future Considerations

### Potential Additional Extractions
- `CosmosNodeClient` - Generic Cosmos node RPC client
- `CosmosChainConfig` - Chain configuration (chain_id, endpoints)
- `CosmosAccountInfo` - Account number and sequence management

### Versioning
- Cosmos SDK types follow protobuf definitions
- May need versioning if SDK versions diverge significantly

## References

- [Cosmos SDK Documentation](https://docs.cosmos.network/)
- [Cosmos Protobuf Definitions](https://github.com/cosmos/cosmos-sdk/tree/main/proto)
- [dYdX v4 Protocol](https://github.com/dydxprotocol/v4-chain)
- [Osmosis](https://github.com/osmosis-labs/osmosis)
- [Injective](https://github.com/InjectiveLabs/injective-core)
