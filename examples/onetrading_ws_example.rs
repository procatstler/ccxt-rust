//! OneTrading WebSocket Example
//!
//! Demonstrates how to use the OneTrading WebSocket client to subscribe to real-time data.
//!
//! Run with: cargo run --example onetrading_ws_example

use ccxt_rust::client::ExchangeConfig;
use ccxt_rust::exchanges::OnetradingWs;
use ccxt_rust::types::{Timeframe, WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("OneTrading WebSocket Example\n");

    // Create OneTrading WebSocket client
    let config = ExchangeConfig::default();
    let client = OnetradingWs::new(config);

    // Example 1: Watch ticker
    println!("Example 1: Watching BTC/EUR ticker...");
    let mut ticker_rx = client.watch_ticker("BTC/EUR").await?;

    // Receive ticker updates for a few seconds
    let ticker_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = ticker_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ Ticker stream connected"),
                WsMessage::Ticker(event) => {
                    println!(
                        "Ticker: {} - Last: {:?}, High: {:?}, Low: {:?}, Change: {:?}%",
                        event.symbol,
                        event.ticker.last,
                        event.ticker.high,
                        event.ticker.low,
                        event.ticker.percentage
                    );
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
    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(15), ticker_task).await;

    // Example 2: Watch order book
    println!("\nExample 2: Watching BTC/EUR order book...");
    let client = OnetradingWs::new(ExchangeConfig::default());
    let mut book_rx = client.watch_order_book("BTC/EUR", None).await?;

    let book_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = book_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ Order book stream connected"),
                WsMessage::OrderBook(event) => {
                    println!(
                        "Order Book: {} - {} bids, {} asks (snapshot: {})",
                        event.symbol,
                        event.order_book.bids.len(),
                        event.order_book.asks.len(),
                        event.is_snapshot
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
                    if count >= 3 {
                        break;
                    }
                },
                WsMessage::Error(err) => eprintln!("Error: {err}"),
                _ => {},
            }
        }
    });

    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(15), book_task).await;

    // Example 3: Watch OHLCV candles
    println!("\nExample 3: Watching BTC/EUR 1-minute candles...");
    let client = OnetradingWs::new(ExchangeConfig::default());
    let mut ohlcv_rx = client.watch_ohlcv("BTC/EUR", Timeframe::Minute1).await?;

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

    // Example 4: Watch multiple tickers
    println!("\nExample 4: Watching multiple tickers...");
    let client = OnetradingWs::new(ExchangeConfig::default());
    let symbols = ["BTC/EUR", "ETH/EUR"];
    let mut tickers_rx = client.watch_tickers(&symbols).await?;

    let tickers_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = tickers_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ Multiple tickers stream connected"),
                WsMessage::Ticker(event) => {
                    println!(
                        "Ticker: {} - Last: {:?}, Volume: {:?}",
                        event.symbol, event.ticker.last, event.ticker.quote_volume
                    );
                    count += 1;
                    if count >= 4 {
                        break;
                    }
                },
                WsMessage::Error(err) => eprintln!("Error: {err}"),
                _ => {},
            }
        }
    });

    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(15), tickers_task).await;

    println!("\nExample completed!");

    Ok(())
}
