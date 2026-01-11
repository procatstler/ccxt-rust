//! Hyperliquid WebSocket Example
//!
//! Demonstrates how to use the Hyperliquid WebSocket client to subscribe to real-time data.
//!
//! Run with: cargo run --example hyperliquid_ws_example

use ccxt_rust::exchanges::HyperliquidWs;
use ccxt_rust::types::{Timeframe, WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Hyperliquid WebSocket Example\n");

    // Create Hyperliquid WebSocket client
    let client = HyperliquidWs::new();

    // Example 1: Watch ticker (mid price updates)
    println!("Example 1: Watching BTC/USDC:USDC ticker...");
    let mut ticker_rx = client.watch_ticker("BTC/USDC:USDC").await?;

    // Receive ticker updates for 5 seconds
    let ticker_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = ticker_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ Ticker stream connected"),
                WsMessage::Ticker(event) => {
                    println!("Ticker: {} @ {:?}", event.symbol, event.ticker.last);
                    count += 1;
                    if count >= 3 {
                        break;
                    }
                },
                WsMessage::Error(err) => eprintln!("Error: {err}"),
                _ => {},
            }
        }
    });

    // Wait for ticker task
    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(10), ticker_task).await;

    // Example 2: Watch order book (L2)
    println!("\nExample 2: Watching BTC/USDC:USDC order book...");
    let mut book_rx = client.watch_order_book("BTC/USDC:USDC", Some(10)).await?;

    let book_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = book_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ Order book stream connected"),
                WsMessage::OrderBook(event) => {
                    println!(
                        "Order Book: {} - {} bids, {} asks",
                        event.symbol,
                        event.order_book.bids.len(),
                        event.order_book.asks.len()
                    );
                    if !event.order_book.bids.is_empty() && !event.order_book.asks.is_empty() {
                        println!(
                            "  Best bid: {} @ {}, Best ask: {} @ {}",
                            event.order_book.bids[0].amount,
                            event.order_book.bids[0].price,
                            event.order_book.asks[0].amount,
                            event.order_book.asks[0].price
                        );
                    }
                    count += 1;
                    if count >= 2 {
                        break;
                    }
                },
                WsMessage::Error(err) => eprintln!("Error: {err}"),
                _ => {},
            }
        }
    });

    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(10), book_task).await;

    // Example 3: Watch trades
    println!("\nExample 3: Watching BTC/USDC:USDC trades...");
    let mut trade_rx = client.watch_trades("BTC/USDC:USDC").await?;

    let trade_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = trade_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ Trade stream connected"),
                WsMessage::Trade(event) => {
                    for trade in &event.trades {
                        println!(
                            "Trade: {} {} @ {} (size: {})",
                            event.symbol,
                            trade.side.as_ref().unwrap_or(&"unknown".to_string()),
                            trade.price,
                            trade.amount
                        );
                        count += 1;
                        if count >= 3 {
                            break;
                        }
                    }
                },
                WsMessage::Error(err) => eprintln!("Error: {err}"),
                _ => {},
            }
        }
    });

    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(15), trade_task).await;

    // Example 4: Watch OHLCV candles
    println!("\nExample 4: Watching BTC/USDC:USDC 1-minute candles...");
    let mut ohlcv_rx = client
        .watch_ohlcv("BTC/USDC:USDC", Timeframe::Minute1)
        .await?;

    let ohlcv_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = ohlcv_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ OHLCV stream connected"),
                WsMessage::Ohlcv(event) => {
                    println!(
                        "Candle: {} [{:?}] O:{} H:{} L:{} C:{} V:{}",
                        event.symbol,
                        event.timeframe,
                        event.ohlcv.open,
                        event.ohlcv.high,
                        event.ohlcv.low,
                        event.ohlcv.close,
                        event.ohlcv.volume
                    );
                    count += 1;
                    if count >= 2 {
                        break;
                    }
                },
                WsMessage::Error(err) => eprintln!("Error: {err}"),
                _ => {},
            }
        }
    });

    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(70), ohlcv_task).await;

    println!("\nExample completed!");

    Ok(())
}
