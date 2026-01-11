//! Live API Integration Tests
//!
//! These tests make actual API calls to cryptocurrency exchanges.
//! They are ignored by default to avoid rate limiting and network issues in CI.
//!
//! Run these tests manually with:
//! ```bash
//! cargo test --features full live_api -- --ignored --test-threads=1
//! ```
//!
//! **Note**: These tests require network access and may fail due to:
//! - Rate limiting
//! - Network issues
//! - Exchange maintenance
//! - API changes

#![cfg(feature = "cex")]

use ccxt_rust::{Exchange, ExchangeConfig, Timeframe};
use std::time::Duration;
use tokio::time::sleep;

/// Helper to add delay between API calls to avoid rate limiting
async fn rate_limit_delay() {
    sleep(Duration::from_millis(500)).await;
}

// =============================================================================
// Binance Live API Tests
// =============================================================================

mod binance_live {
    use super::*;
    use ccxt_rust::exchanges::Binance;

    fn create_exchange() -> Binance {
        let config = ExchangeConfig::new();
        Binance::new(config).expect("Failed to create Binance exchange")
    }

    #[tokio::test]
    #[ignore]
    async fn test_binance_fetch_markets() {
        let exchange = create_exchange();

        let markets = exchange.fetch_markets().await;
        assert!(markets.is_ok(), "fetch_markets failed: {:?}", markets.err());

        let markets = markets.unwrap();
        assert!(!markets.is_empty(), "No markets returned");

        // Verify BTC/USDT exists
        let btc_usdt = markets.iter().find(|m| m.symbol == "BTC/USDT");
        assert!(btc_usdt.is_some(), "BTC/USDT market not found");

        let market = btc_usdt.unwrap();
        assert_eq!(market.base, "BTC");
        assert_eq!(market.quote, "USDT");

        println!("✅ Binance: Found {} markets", markets.len());
    }

    #[tokio::test]
    #[ignore]
    async fn test_binance_fetch_ticker() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let ticker = exchange.fetch_ticker("BTC/USDT").await;
        assert!(ticker.is_ok(), "fetch_ticker failed: {:?}", ticker.err());

        let ticker = ticker.unwrap();
        assert_eq!(ticker.symbol, "BTC/USDT");
        assert!(ticker.last.is_some(), "Ticker missing last price");
        assert!(ticker.bid.is_some(), "Ticker missing bid");
        assert!(ticker.ask.is_some(), "Ticker missing ask");

        let last = ticker.last.unwrap();
        assert!(
            last > rust_decimal::Decimal::ZERO,
            "Last price should be positive"
        );

        println!(
            "✅ Binance BTC/USDT: Last={}, Bid={:?}, Ask={:?}",
            last, ticker.bid, ticker.ask
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_binance_fetch_order_book() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let orderbook = exchange.fetch_order_book("BTC/USDT", Some(10)).await;
        assert!(
            orderbook.is_ok(),
            "fetch_order_book failed: {:?}",
            orderbook.err()
        );

        let orderbook = orderbook.unwrap();
        assert_eq!(orderbook.symbol, "BTC/USDT");
        assert!(!orderbook.bids.is_empty(), "No bids returned");
        assert!(!orderbook.asks.is_empty(), "No asks returned");

        // Verify bid < ask (no crossed book)
        let best_bid = &orderbook.bids[0];
        let best_ask = &orderbook.asks[0];
        assert!(
            best_bid.price < best_ask.price,
            "Crossed orderbook: bid {} >= ask {}",
            best_bid.price,
            best_ask.price
        );

        println!(
            "✅ Binance BTC/USDT Orderbook: Bid={}, Ask={}",
            best_bid.price, best_ask.price
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_binance_fetch_ohlcv() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let ohlcv = exchange
            .fetch_ohlcv("BTC/USDT", Timeframe::Hour1, None, Some(10))
            .await;
        assert!(ohlcv.is_ok(), "fetch_ohlcv failed: {:?}", ohlcv.err());

        let candles = ohlcv.unwrap();
        assert!(!candles.is_empty(), "No candles returned");

        let candle = &candles[0];
        assert!(candle.open > rust_decimal::Decimal::ZERO);
        assert!(candle.high >= candle.low);
        assert!(candle.volume >= rust_decimal::Decimal::ZERO);

        println!(
            "✅ Binance BTC/USDT OHLCV: {} candles, latest O={} H={} L={} C={}",
            candles.len(),
            candle.open,
            candle.high,
            candle.low,
            candle.close
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_binance_fetch_trades() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let trades = exchange.fetch_trades("BTC/USDT", None, Some(10)).await;
        assert!(trades.is_ok(), "fetch_trades failed: {:?}", trades.err());

        let trades = trades.unwrap();
        assert!(!trades.is_empty(), "No trades returned");

        let trade = &trades[0];
        assert_eq!(trade.symbol, "BTC/USDT");
        assert!(trade.price > rust_decimal::Decimal::ZERO);
        assert!(trade.amount > rust_decimal::Decimal::ZERO);

        println!(
            "✅ Binance BTC/USDT Trades: {} trades, latest price={}",
            trades.len(),
            trade.price
        );
    }
}

// =============================================================================
// OKX Live API Tests
// =============================================================================

mod okx_live {
    use super::*;
    use ccxt_rust::exchanges::Okx;

    fn create_exchange() -> Okx {
        let config = ExchangeConfig::new();
        Okx::new(config).expect("Failed to create OKX exchange")
    }

    #[tokio::test]
    #[ignore]
    async fn test_okx_fetch_markets() {
        let exchange = create_exchange();

        let markets = exchange.fetch_markets().await;
        assert!(markets.is_ok(), "fetch_markets failed: {:?}", markets.err());

        let markets = markets.unwrap();
        assert!(!markets.is_empty(), "No markets returned");

        println!("✅ OKX: Found {} markets", markets.len());
    }

    #[tokio::test]
    #[ignore]
    async fn test_okx_fetch_ticker() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let ticker = exchange.fetch_ticker("BTC/USDT").await;
        assert!(ticker.is_ok(), "fetch_ticker failed: {:?}", ticker.err());

        let ticker = ticker.unwrap();
        assert!(ticker.last.is_some(), "Ticker missing last price");

        println!("✅ OKX BTC/USDT: Last={:?}", ticker.last);
    }

    #[tokio::test]
    #[ignore]
    async fn test_okx_fetch_order_book() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let orderbook = exchange.fetch_order_book("BTC/USDT", Some(10)).await;
        assert!(
            orderbook.is_ok(),
            "fetch_order_book failed: {:?}",
            orderbook.err()
        );

        let orderbook = orderbook.unwrap();
        assert!(!orderbook.bids.is_empty(), "No bids returned");
        assert!(!orderbook.asks.is_empty(), "No asks returned");

        println!(
            "✅ OKX BTC/USDT Orderbook: {} bids, {} asks",
            orderbook.bids.len(),
            orderbook.asks.len()
        );
    }
}

// =============================================================================
// Bybit Live API Tests
// =============================================================================

mod bybit_live {
    use super::*;
    use ccxt_rust::exchanges::Bybit;

    fn create_exchange() -> Bybit {
        let config = ExchangeConfig::new();
        Bybit::new(config).expect("Failed to create Bybit exchange")
    }

    #[tokio::test]
    #[ignore]
    async fn test_bybit_fetch_markets() {
        let exchange = create_exchange();

        let markets = exchange.fetch_markets().await;
        assert!(markets.is_ok(), "fetch_markets failed: {:?}", markets.err());

        let markets = markets.unwrap();
        assert!(!markets.is_empty(), "No markets returned");

        println!("✅ Bybit: Found {} markets", markets.len());
    }

    #[tokio::test]
    #[ignore]
    async fn test_bybit_fetch_ticker() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let ticker = exchange.fetch_ticker("BTC/USDT").await;
        assert!(ticker.is_ok(), "fetch_ticker failed: {:?}", ticker.err());

        let ticker = ticker.unwrap();
        assert!(ticker.last.is_some(), "Ticker missing last price");

        println!("✅ Bybit BTC/USDT: Last={:?}", ticker.last);
    }

    #[tokio::test]
    #[ignore]
    async fn test_bybit_fetch_order_book() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let orderbook = exchange.fetch_order_book("BTC/USDT", Some(10)).await;
        assert!(
            orderbook.is_ok(),
            "fetch_order_book failed: {:?}",
            orderbook.err()
        );

        let orderbook = orderbook.unwrap();
        assert!(!orderbook.bids.is_empty(), "No bids returned");
        assert!(!orderbook.asks.is_empty(), "No asks returned");

        println!(
            "✅ Bybit BTC/USDT Orderbook: {} bids, {} asks",
            orderbook.bids.len(),
            orderbook.asks.len()
        );
    }
}

// =============================================================================
// Kraken Live API Tests
// =============================================================================

mod kraken_live {
    use super::*;
    use ccxt_rust::exchanges::Kraken;

    fn create_exchange() -> Kraken {
        let config = ExchangeConfig::new();
        Kraken::new(config).expect("Failed to create Kraken exchange")
    }

    #[tokio::test]
    #[ignore]
    async fn test_kraken_fetch_markets() {
        let exchange = create_exchange();

        let markets = exchange.fetch_markets().await;
        assert!(markets.is_ok(), "fetch_markets failed: {:?}", markets.err());

        let markets = markets.unwrap();
        assert!(!markets.is_empty(), "No markets returned");

        println!("✅ Kraken: Found {} markets", markets.len());
    }

    #[tokio::test]
    #[ignore]
    async fn test_kraken_fetch_ticker() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let ticker = exchange.fetch_ticker("BTC/USD").await;
        assert!(ticker.is_ok(), "fetch_ticker failed: {:?}", ticker.err());

        let ticker = ticker.unwrap();
        assert!(ticker.last.is_some(), "Ticker missing last price");

        println!("✅ Kraken BTC/USD: Last={:?}", ticker.last);
    }
}

// =============================================================================
// Korean Exchange Live API Tests
// =============================================================================

mod korean_live {
    use super::*;
    use ccxt_rust::exchanges::{Bithumb, Upbit};

    #[tokio::test]
    #[ignore]
    async fn test_upbit_fetch_markets() {
        let config = ExchangeConfig::new();
        let exchange = Upbit::new(config).expect("Failed to create Upbit exchange");

        let markets = exchange.fetch_markets().await;
        assert!(markets.is_ok(), "fetch_markets failed: {:?}", markets.err());

        let markets = markets.unwrap();
        assert!(!markets.is_empty(), "No markets returned");

        // Verify BTC/KRW exists
        let btc_krw = markets.iter().find(|m| m.symbol == "BTC/KRW");
        assert!(btc_krw.is_some(), "BTC/KRW market not found");

        println!("✅ Upbit: Found {} markets", markets.len());
    }

    #[tokio::test]
    #[ignore]
    async fn test_upbit_fetch_ticker() {
        let config = ExchangeConfig::new();
        let exchange = Upbit::new(config).expect("Failed to create Upbit exchange");
        rate_limit_delay().await;

        let ticker = exchange.fetch_ticker("BTC/KRW").await;
        assert!(ticker.is_ok(), "fetch_ticker failed: {:?}", ticker.err());

        let ticker = ticker.unwrap();
        assert!(ticker.last.is_some(), "Ticker missing last price");

        println!("✅ Upbit BTC/KRW: Last={:?}", ticker.last);
    }

    #[tokio::test]
    #[ignore]
    async fn test_bithumb_fetch_markets() {
        let config = ExchangeConfig::new();
        let exchange = Bithumb::new(config).expect("Failed to create Bithumb exchange");

        let markets = exchange.fetch_markets().await;
        assert!(markets.is_ok(), "fetch_markets failed: {:?}", markets.err());

        let markets = markets.unwrap();
        assert!(!markets.is_empty(), "No markets returned");

        println!("✅ Bithumb: Found {} markets", markets.len());
    }
}

// =============================================================================
// Multi-Exchange Comparison Tests
// =============================================================================

mod comparison_tests {
    use super::*;
    use ccxt_rust::exchanges::{Binance, Bybit, Okx};

    /// Compare BTC/USDT prices across multiple exchanges
    #[tokio::test]
    #[ignore]
    async fn test_cross_exchange_price_comparison() {
        let binance = Binance::new(ExchangeConfig::new()).unwrap();
        let okx = Okx::new(ExchangeConfig::new()).unwrap();
        let bybit = Bybit::new(ExchangeConfig::new()).unwrap();

        // Fetch tickers from all exchanges
        let binance_ticker = binance.fetch_ticker("BTC/USDT").await;
        rate_limit_delay().await;
        let okx_ticker = okx.fetch_ticker("BTC/USDT").await;
        rate_limit_delay().await;
        let bybit_ticker = bybit.fetch_ticker("BTC/USDT").await;

        // All should succeed
        assert!(binance_ticker.is_ok(), "Binance ticker failed");
        assert!(okx_ticker.is_ok(), "OKX ticker failed");
        assert!(bybit_ticker.is_ok(), "Bybit ticker failed");

        let binance_price = binance_ticker.unwrap().last.unwrap();
        let okx_price = okx_ticker.unwrap().last.unwrap();
        let bybit_price = bybit_ticker.unwrap().last.unwrap();

        println!("📊 BTC/USDT Price Comparison:");
        println!("   Binance: {}", binance_price);
        println!("   OKX:     {}", okx_price);
        println!("   Bybit:   {}", bybit_price);

        // Prices should be within 1% of each other (reasonable for liquid markets)
        let avg = (binance_price + okx_price + bybit_price) / rust_decimal::Decimal::from(3);
        let tolerance = avg * rust_decimal::Decimal::new(1, 2); // 1%

        assert!(
            (binance_price - avg).abs() < tolerance,
            "Binance price {} deviates too much from average {}",
            binance_price,
            avg
        );
        assert!(
            (okx_price - avg).abs() < tolerance,
            "OKX price {} deviates too much from average {}",
            okx_price,
            avg
        );
        assert!(
            (bybit_price - avg).abs() < tolerance,
            "Bybit price {} deviates too much from average {}",
            bybit_price,
            avg
        );

        println!("✅ All prices within 1% of average: {}", avg);
    }
}
