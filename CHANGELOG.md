# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Phase 30-31: Final Polish and Release Preparation**
  - SECURITY.md with vulnerability reporting policy and security best practices
  - CODE_OF_CONDUCT.md (Contributor Covenant v2.1)
  - codecov.yml for code coverage configuration
  - cliff.toml for git-cliff automated changelog generation
  - .editorconfig for consistent coding style across editors
- **Feature Flags**: Optional `cex` and `dex` features for reduced compile time and binary size
  - `cex` (default): Centralized exchange support
  - `dex`: Decentralized exchange support with crypto primitives
  - `full`: All features enabled
- **Live API Integration Tests**: 28 tests for real exchange API validation
  - CEX tests (17): Binance, OKX, Bybit, Kraken, Upbit, Bithumb
  - DEX tests (11): Hyperliquid, dYdX v4, Paradex
  - Cross-exchange price comparison test
  - DEX latency measurement test
  - All tests marked `#[ignore]` for manual execution
- **Testing Documentation**: Comprehensive TESTING.md guide
- **Examples Documentation**: Comprehensive README.md for examples directory
- **Error Handling Example**: Demonstrates error types, classification, retry patterns
- **Advanced Configuration Example**: Shows timeouts, retry config, credentials management
- **Open Source Readiness**:
  - CONTRIBUTING.md with development guide and contribution workflow
  - GitHub issue templates (bug report, feature request, exchange request)
  - Pull request template with checklist
- **CI/CD Enhancements**:
  - Code coverage workflow with cargo-llvm-cov and Codecov integration
  - Enhanced release workflow with crates.io publishing
  - Automated GitHub Release creation with changelog extraction
  - Documentation deployment to GitHub Pages
  - Version validation against Cargo.toml
- Convert API support (Binance, OKX, Bybit)
- Example files: basic_usage, multi_exchange, websocket_streaming, trading_operations, dex_trading
- Benchmark tests: exchange_benchmarks, parsing_benchmarks
- Prelude module for convenient imports

### Changed
- DEX crypto dependencies are now optional (sha3, k256, starknet-crypto, bip39, etc.)
- Enhanced CI workflow with security audit, MSRV check, feature matrix testing, manual live tests
- Improved lib.rs documentation with comprehensive examples
- Fixed Clippy warnings for better code quality
- Test and example files now properly gated by features
- Updated documentation with accurate test counts (1,747+ tests)
- Updated PORTING_STATUS.md with Phase 24-26 changelog
- Reorganized root directory: moved implementation docs to docs/
- README updated with more badges (Coverage, Crates.io, Documentation)

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
