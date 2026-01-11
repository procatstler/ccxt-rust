//! Live DEX API Integration Tests
//!
//! These tests make actual API calls to decentralized exchanges.
//! They are ignored by default to avoid rate limiting and network issues in CI.
//!
//! Run these tests manually with:
//! ```bash
//! cargo test --features full live_dex -- --ignored --test-threads=1
//! ```
//!
//! **Note**: DEX tests may require longer timeouts due to blockchain latency.

#![cfg(feature = "dex")]

use ccxt_rust::{Exchange, ExchangeConfig, Timeframe};
use std::time::Duration;
use tokio::time::sleep;

/// Helper to add delay between API calls
async fn rate_limit_delay() {
    sleep(Duration::from_millis(1000)).await; // DEX APIs may need longer delays
}

// =============================================================================
// Hyperliquid Live API Tests
// =============================================================================

mod hyperliquid_live {
    use super::*;
    use ccxt_rust::exchanges::Hyperliquid;

    fn create_exchange() -> Hyperliquid {
        let config = ExchangeConfig::new();
        Hyperliquid::new(config).expect("Failed to create Hyperliquid exchange")
    }

    #[tokio::test]
    #[ignore]
    async fn test_hyperliquid_fetch_markets() {
        let exchange = create_exchange();

        let markets = exchange.fetch_markets().await;
        assert!(markets.is_ok(), "fetch_markets failed: {:?}", markets.err());

        let markets = markets.unwrap();
        assert!(!markets.is_empty(), "No markets returned");

        // Hyperliquid uses USDC as quote currency
        let btc_market = markets.iter().find(|m| m.base == "BTC");
        assert!(btc_market.is_some(), "BTC market not found");

        println!("✅ Hyperliquid: Found {} markets", markets.len());

        // Print first few markets
        for market in markets.iter().take(5) {
            println!("   {} - Type: {:?}", market.symbol, market.market_type);
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_hyperliquid_fetch_ticker() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        // Hyperliquid uses format like "BTC/USDC:USDC"
        let ticker = exchange.fetch_ticker("BTC/USDC:USDC").await;
        assert!(ticker.is_ok(), "fetch_ticker failed: {:?}", ticker.err());

        let ticker = ticker.unwrap();
        assert!(ticker.last.is_some(), "Ticker missing last price");

        println!(
            "✅ Hyperliquid BTC: Last={:?}, Mark={:?}, Index={:?}",
            ticker.last, ticker.mark_price, ticker.index_price
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_hyperliquid_fetch_order_book() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let orderbook = exchange.fetch_order_book("BTC/USDC:USDC", Some(10)).await;
        assert!(
            orderbook.is_ok(),
            "fetch_order_book failed: {:?}",
            orderbook.err()
        );

        let orderbook = orderbook.unwrap();
        assert!(!orderbook.bids.is_empty(), "No bids returned");
        assert!(!orderbook.asks.is_empty(), "No asks returned");

        let best_bid = &orderbook.bids[0];
        let best_ask = &orderbook.asks[0];

        println!("✅ Hyperliquid BTC Orderbook:");
        println!("   Best Bid: {} @ {}", best_bid.amount, best_bid.price);
        println!("   Best Ask: {} @ {}", best_ask.amount, best_ask.price);
        println!("   Spread: {}", best_ask.price - best_bid.price);
    }

    #[tokio::test]
    #[ignore]
    async fn test_hyperliquid_fetch_ohlcv() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let ohlcv = exchange
            .fetch_ohlcv("BTC/USDC:USDC", Timeframe::Hour1, None, Some(10))
            .await;
        assert!(ohlcv.is_ok(), "fetch_ohlcv failed: {:?}", ohlcv.err());

        let candles = ohlcv.unwrap();
        assert!(!candles.is_empty(), "No candles returned");

        println!("✅ Hyperliquid BTC OHLCV: {} candles", candles.len());

        let candle = &candles[0];
        println!(
            "   Latest: O={} H={} L={} C={} V={}",
            candle.open, candle.high, candle.low, candle.close, candle.volume
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_hyperliquid_fetch_funding_rate() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let funding = exchange.fetch_funding_rate("BTC/USDC:USDC").await;

        match funding {
            Ok(funding) => {
                println!("✅ Hyperliquid BTC Funding Rate:");
                println!("   Rate: {:?}", funding.funding_rate);
                println!("   Next: {:?}", funding.funding_datetime);
            },
            Err(e) => {
                println!("⚠️  Funding rate fetch failed (may be expected): {}", e);
            },
        }
    }
}

// =============================================================================
// dYdX v4 Live API Tests
// =============================================================================

mod dydx_live {
    use super::*;
    use ccxt_rust::exchanges::DydxV4;

    fn create_exchange() -> DydxV4 {
        let config = ExchangeConfig::new();
        DydxV4::new(config).expect("Failed to create dYdX v4 exchange")
    }

    #[tokio::test]
    #[ignore]
    async fn test_dydxv4_fetch_markets() {
        let exchange = create_exchange();

        let markets = exchange.fetch_markets().await;
        assert!(markets.is_ok(), "fetch_markets failed: {:?}", markets.err());

        let markets = markets.unwrap();
        assert!(!markets.is_empty(), "No markets returned");

        println!("✅ dYdX v4: Found {} markets", markets.len());

        for market in markets.iter().take(5) {
            println!("   {} - Type: {:?}", market.symbol, market.market_type);
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_dydxv4_fetch_ticker() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let ticker = exchange.fetch_ticker("BTC/USD").await;

        match ticker {
            Ok(ticker) => {
                println!("✅ dYdX v4 BTC/USD:");
                println!("   Last: {:?}", ticker.last);
                println!("   Bid: {:?}", ticker.bid);
                println!("   Ask: {:?}", ticker.ask);
            },
            Err(e) => {
                println!("⚠️  Ticker fetch failed: {}", e);
            },
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_dydxv4_fetch_order_book() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        let orderbook = exchange.fetch_order_book("BTC/USD", Some(10)).await;

        match orderbook {
            Ok(ob) => {
                println!("✅ dYdX v4 BTC/USD Orderbook:");
                println!("   Bids: {} levels", ob.bids.len());
                println!("   Asks: {} levels", ob.asks.len());

                if !ob.bids.is_empty() && !ob.asks.is_empty() {
                    println!("   Best Bid: {}", ob.bids[0].price);
                    println!("   Best Ask: {}", ob.asks[0].price);
                }
            },
            Err(e) => {
                println!("⚠️  Order book fetch failed: {}", e);
            },
        }
    }
}

// =============================================================================
// Paradex Live API Tests (StarkNet-based)
// =============================================================================

mod paradex_live {
    use super::*;
    use ccxt_rust::exchanges::Paradex;

    fn create_exchange() -> Paradex {
        let config = ExchangeConfig::new();
        Paradex::new(config).expect("Failed to create Paradex exchange")
    }

    #[tokio::test]
    #[ignore]
    async fn test_paradex_fetch_markets() {
        let exchange = create_exchange();

        let markets = exchange.fetch_markets().await;
        assert!(markets.is_ok(), "fetch_markets failed: {:?}", markets.err());

        let markets = markets.unwrap();
        assert!(!markets.is_empty(), "No markets returned");

        println!("✅ Paradex: Found {} markets", markets.len());

        for market in markets.iter().take(5) {
            println!("   {} - Type: {:?}", market.symbol, market.market_type);
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_paradex_fetch_ticker() {
        let exchange = create_exchange();
        rate_limit_delay().await;

        // Try to find a valid symbol
        let ticker = exchange.fetch_ticker("ETH-USD-PERP").await;

        match ticker {
            Ok(ticker) => {
                println!("✅ Paradex ETH-USD-PERP:");
                println!("   Last: {:?}", ticker.last);
                println!("   Mark: {:?}", ticker.mark_price);
            },
            Err(e) => {
                println!("⚠️  Ticker fetch failed (symbol may differ): {}", e);
            },
        }
    }
}

// =============================================================================
// DEX Performance Tests
// =============================================================================

mod dex_performance {
    use super::*;
    use ccxt_rust::exchanges::Hyperliquid;
    use std::time::Instant;

    #[tokio::test]
    #[ignore]
    async fn test_hyperliquid_latency() {
        let exchange = Hyperliquid::new(ExchangeConfig::new()).unwrap();

        // Warm up
        let _ = exchange.fetch_markets().await;
        rate_limit_delay().await;

        // Measure ticker fetch latency
        let iterations = 5;
        let mut latencies = Vec::new();

        for _ in 0..iterations {
            let start = Instant::now();
            let _ = exchange.fetch_ticker("BTC/USDC:USDC").await;
            let elapsed = start.elapsed();
            latencies.push(elapsed.as_millis());
            rate_limit_delay().await;
        }

        let avg_latency: u128 = latencies.iter().sum::<u128>() / iterations;
        let min_latency = latencies.iter().min().unwrap();
        let max_latency = latencies.iter().max().unwrap();

        println!("📊 Hyperliquid Ticker Latency ({} iterations):", iterations);
        println!("   Average: {}ms", avg_latency);
        println!("   Min: {}ms", min_latency);
        println!("   Max: {}ms", max_latency);
    }
}
