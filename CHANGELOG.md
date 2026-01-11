# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-01-11

### Added
- Initial release of ccxt-rust
- **109 REST API implementations** (101 CEX + 8 DEX)
- **108 WebSocket implementations** (100 CEX + 8 DEX)
- Unified `Exchange` trait for REST API operations
- Unified `WsExchange` trait for WebSocket streaming
- Private channel support for 34+ exchanges
- Comprehensive crypto utilities (EVM, StarkNet, Cosmos)
- Full documentation with rustdoc

### Supported Exchanges

#### CEX (Centralized Exchanges) - 101 exchanges
- Major: Binance, Bybit, OKX, Kraken, KuCoin, Coinbase, Gate.io
- Korean: Upbit, Bithumb, Coinone, Korbit
- Regional: HTX, MEXC, Bitget, Bitfinex, and 90+ more

#### DEX (Decentralized Exchanges) - 8 exchanges
- Hyperliquid, dYdX, dYdX v4, Paradex
- Apex, Defx, Derive, Wavesexchange

### Project Structure
- `src/exchanges/cex/` - Centralized exchange implementations
- `src/exchanges/dex/` - Decentralized exchange implementations
- `src/crypto/` - Cryptographic utilities
- `src/types/` - Common types and traits
- `src/client/` - HTTP client and rate limiting

### Development
- CI/CD with GitHub Actions
- Dependabot for dependency updates
- 1,558+ tests passing
- Comprehensive documentation

[Unreleased]: https://github.com/onlyhyde/trading/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/onlyhyde/trading/releases/tag/v0.1.0
