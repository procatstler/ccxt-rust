//! Exchange Implementations
//!
//! CEX (Centralized Exchange) 및 DEX (Decentralized Exchange) 구현체
//!
//! # Feature Flags
//!
//! - `cex` (default): Centralized exchange support (Binance, OKX, etc.)
//! - `dex`: Decentralized exchange support (Hyperliquid, dYdX, Paradex, etc.)
//! - `native`: Required for exchange implementations (uses tokio runtime)
//!
//! Note: Exchange implementations require the `native` feature and are not
//! available in WASM builds. For WASM, use the crypto utilities and types directly.

#[cfg(all(feature = "cex", feature = "native"))]
pub mod cex;
#[cfg(all(feature = "dex", feature = "native"))]
pub mod dex;

// CEX re-exports (requires "cex" and "native" features)
#[cfg(all(feature = "cex", feature = "native"))]
pub use cex::{
    Alpaca, AlpacaWs, Ascendex, AscendexWs, Backpack, BackpackWs, Bequant, BequantWs, Bigone,
    BigoneWs, Binance, BinanceCoinM, BinanceCoinmWs, BinanceFutures, BinanceFuturesWs, BinanceUs,
    BinanceUsWs, BinanceUsdm, BinanceUsdmWs, BinanceWs, Bingx, BingxWs, Bit2c, Bit2cWs, Bitbank,
    BitbankWs, Bitbns, BitbnsWs, Bitfinex, BitfinexWs, Bitflyer, BitflyerWs, Bitget, BitgetWs,
    Bithumb, BithumbWs, Bitmart, BitmartWs, Bitmex, BitmexWs, Bitopro, BitoproWs, Bitrue, BitrueWs,
    Bitso, BitsoWs, Bitstamp, BitstampWs, Bittrade, BittradeWs, Bitvavo, BitvavoWs, BlockchainCom,
    BlockchainComWs, Blofin, BlofinWs, Btcalpha, BtcalphaWs, Btcbox, BtcboxWs, Btcmarkets,
    BtcmarketsWs, Btcturk, BtcturkWs, Bullish, BullishWs, Bybit, BybitWs, Cex, CexWs, CoinCatch,
    CoinCatchWs, Coinbase, CoinbaseAdvanced, CoinbaseAdvancedWs, CoinbaseExchange,
    CoinbaseExchangeWs, CoinbaseInternational, CoinbaseInternationalWs, CoinbaseWs, Coincheck,
    CoincheckWs, Coinex, CoinexWs, Coinmate, CoinmateWs, Coinmetro, CoinmetroWs, Coinone,
    CoinoneWs, Coinsph, CoinsphWs, Coinspot, CoinspotWs, CryptoCom, CryptoComWs, Cryptomus,
    CryptomusWs, Deepcoin, DeepcoinWs, Delta, DeltaWs, Deribit, DeribitWs, Digifinex, DigifinexWs,
    Exmo, ExmoWs, Fmfwio, Foxbit, FoxbitWs, Gate, GateWs, Gateio, GateioWs, Gemini, GeminiWs,
    Hashkey, HashkeyWs, Hitbtc, HitbtcWs, Hollaex, HollaexWs, Htx, HtxWs, Huobi, HuobiWs,
    IndependentReserveWs, Independentreserve, Indodax, IndodaxWs, Korbit, KorbitWs, Kraken,
    KrakenFutures, KrakenFuturesWs, KrakenWs, Kucoin, KucoinFutures, KucoinFuturesWs, KucoinWs,
    Kucoinfutures, KucoinfuturesWs, Latoken, LatokenWs, Lbank, LbankWs, Luno, LunoWs, Mercado,
    MercadoWs, Mexc, MexcWs, MyOkx, MyOkxWs, Ndax, NdaxWs, Novadax, NovadaxWs, Oceanex, OceanexWs,
    Okx, OkxUs, OkxUsWs, OkxWs, Onetrading, OnetradingWs, Oxfun, OxfunWs, P2b, P2bWs, Paymium,
    PaymiumWs, Phemex, PhemexWs, Poloniex, PoloniexWs, ProBit, ProbitWs, Timex, TimexWs,
    Tokocrypto, TokocryptoWs, Toobit, ToobitWs, Upbit, UpbitWs, Whitebit, WhitebitWs, Woo, WooWs,
    Xt, XtWs, Yobit, YobitWs, Zaif, ZaifWs, Zebpay, ZebpayWs, Zonda, ZondaWs,
};

// DEX re-exports (requires "dex" and "native" features)
#[cfg(all(feature = "dex", feature = "native"))]
pub use dex::{
    Apex, ApexWs, Defx, DefxWs, Derive, DeriveWs, Dydx, DydxV4, DydxV4Ws, DydxWs, Hyperliquid,
    HyperliquidWs, Paradex, ParadexWs, Wavesexchange, WavesexchangeWs,
};
