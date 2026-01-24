# Supported Exchanges

## CEX (Centralized Exchanges)

| Exchange | REST | WebSocket | Notes |
|----------|:----:|:---------:|-------|
| Binance | ✅ | ✅ | Spot, Futures, CoinM |
| Bybit | ✅ | ✅ | Spot & Derivatives |
| OKX | ✅ | ✅ | |
| Kraken | ✅ | ✅ | Spot & Futures |
| KuCoin | ✅ | ✅ | Spot & Futures |
| Coinbase | ✅ | ✅ | |
| Gate.io | ✅ | ✅ | |
| Bitget | ✅ | ✅ | |
| MEXC | ✅ | ✅ | |
| HTX (Huobi) | ✅ | ✅ | |
| Upbit | ✅ | ✅ | Korean |
| Bithumb | ✅ | ✅ | Korean |
| Alpaca | ✅ | ✅ | |
| AscendEX | ✅ | ✅ | |
| Backpack | ✅ | ✅ | |
| Bequant | ✅ | ✅ | |
| BigONE | ✅ | ✅ | |
| Bit2C | ✅ | ✅ | |
| Bitbank | ✅ | ✅ | |
| Bitbns | ✅ | ✅ | |
| Bitfinex | ✅ | ✅ | |
| Bitflyer | ✅ | ✅ | |
| Bitmart | ✅ | ✅ | |
| BitMEX | ✅ | ✅ | |
| Bitopro | ✅ | ✅ | |
| Bitrue | ✅ | ✅ | |
| Bitso | ✅ | ✅ | |
| Bitstamp | ✅ | ✅ | |
| Bittrade | ✅ | ✅ | |
| Bitvavo | ✅ | ✅ | |
| Blockchain.com | ✅ | ✅ | |
| Blofin | ✅ | ✅ | |
| BTC-Alpha | ✅ | ✅ | |
| BTCBox | ✅ | ✅ | |
| BTCMarkets | ✅ | ✅ | |
| BTCTurk | ✅ | ✅ | |
| Bullish | ✅ | ✅ | |
| CEX.IO | ✅ | ✅ | |
| CoinCatch | ✅ | ✅ | |
| Coincheck | ✅ | ✅ | |
| CoinEx | ✅ | ✅ | |
| Coinmate | ✅ | ✅ | |
| CoinMetro | ✅ | ✅ | |
| CoinOne | ✅ | ✅ | |
| Coins.ph | ✅ | ✅ | |
| CoinSpot | ✅ | ✅ | |
| Crypto.com | ✅ | ✅ | |
| Cryptomus | ✅ | ✅ | |
| Deepcoin | ✅ | ✅ | |
| Delta | ✅ | ✅ | |
| Deribit | ✅ | ✅ | |
| DigiFinex | ✅ | ✅ | |
| EXMO | ✅ | ✅ | |
| FMFW.io | ✅ | - | |
| Foxbit | ✅ | ✅ | |
| Gemini | ✅ | ✅ | |
| HashKey | ✅ | ✅ | |
| HitBTC | ✅ | ✅ | |
| HollaEx | ✅ | ✅ | |
| Independent Reserve | ✅ | ✅ | |
| Indodax | ✅ | ✅ | |
| Korbit | ✅ | ✅ | |
| Kraken Futures | ✅ | ✅ | |
| Latoken | ✅ | ✅ | |
| LBank | ✅ | ✅ | |
| Luno | ✅ | ✅ | |
| Mercado | ✅ | ✅ | |
| MyOKX | ✅ | ✅ | |
| NDAX | ✅ | ✅ | |
| NovaDax | ✅ | ✅ | |
| OceanEx | ✅ | ✅ | |
| OKX US | ✅ | ✅ | |
| OneTrading | ✅ | ✅ | |
| OXFun | ✅ | ✅ | |
| P2B | ✅ | ✅ | |
| Paymium | ✅ | ✅ | |
| Phemex | ✅ | ✅ | |
| Poloniex | ✅ | ✅ | |
| Probit | ✅ | ✅ | |
| TimeX | ✅ | ✅ | |
| Tokocrypto | ✅ | ✅ | |
| Toobit | ✅ | ✅ | |
| WhiteBit | ✅ | ✅ | |
| WOO | ✅ | ✅ | |
| XT | ✅ | ✅ | |
| YoBit | ✅ | ✅ | |
| Zaif | ✅ | ✅ | |
| Zebpay | ✅ | ✅ | |
| Zonda | ✅ | ✅ | |

## DEX (Decentralized Exchanges)

| Exchange | REST | WebSocket | Blockchain | Notes |
|----------|:----:|:---------:|------------|-------|
| Hyperliquid | ✅ | ✅ | EVM (L1) | Perpetuals DEX |
| dYdX v4 | ✅ | ✅ | Cosmos | On-chain orderbook |
| dYdX v3 | ✅ | ✅ | StarkEx | Legacy |
| Paradex | ✅ | ✅ | StarkNet | Perpetuals |
| Apex | ✅ | ✅ | - | Perpetuals & Spot |
| Defx | ✅ | ✅ | - | Derivatives |
| Derive | ✅ | ✅ | - | Derivatives |
| WavesExchange | ✅ | ✅ | Waves | Spot |

## Crypto Modules

DEX support includes blockchain-specific cryptographic modules:

| Module | Blockchain | Features |
|--------|------------|----------|
| `crypto::evm` | Ethereum, L2s | Wallet, EIP-712 signing, Keccak |
| `crypto::starknet` | StarkNet | Wallet, Typed Data, Poseidon hash |
| `crypto::cosmos` | Cosmos SDK | Wallet, Transaction signing, Protobuf |
