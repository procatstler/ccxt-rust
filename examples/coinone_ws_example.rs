//! Coinone WebSocket Example
//!
//! Demonstrates how to use the Coinone WebSocket implementation
//! to receive real-time market data.

use ccxt_rust::exchanges::foreign::CoinoneWs;
use ccxt_rust::types::{WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Coinone WebSocket Example");
    println!("=========================\n");

    let ws = CoinoneWs::new();

    // Example 1: Watch Ticker
    println!("Example 1: Watching BTC/KRW ticker...");
    let mut ticker_rx = ws.watch_ticker("BTC/KRW").await?;

    tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = ticker_rx.recv().await {
            match msg {
                WsMessage::Connected => {
                    println!("[Ticker] Connected to Coinone WebSocket");
                }
                WsMessage::Ticker(event) => {
                    count += 1;
                    println!(
                        "[Ticker] {} - Last: {:?}, Bid: {:?}, Ask: {:?}",
                        event.symbol, event.ticker.last, event.ticker.bid, event.ticker.ask
                    );
                    if count >= 5 {
                        println!("[Ticker] Received 5 updates, stopping...");
                        break;
                    }
                }
                WsMessage::Error(err) => {
                    eprintln!("[Ticker] Error: {}", err);
                }
                WsMessage::Disconnected => {
                    println!("[Ticker] Disconnected");
                    break;
                }
                _ => {}
            }
        }
    });

    // Example 2: Watch Order Book
    println!("\nExample 2: Watching BTC/KRW order book...");
    let ws2 = CoinoneWs::new();
    let mut orderbook_rx = ws2.watch_order_book("BTC/KRW", None).await?;

    tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = orderbook_rx.recv().await {
            match msg {
                WsMessage::Connected => {
                    println!("[OrderBook] Connected to Coinone WebSocket");
                }
                WsMessage::OrderBook(event) => {
                    count += 1;
                    println!(
                        "[OrderBook] {} - Bids: {}, Asks: {}, Snapshot: {}",
                        event.symbol,
                        event.order_book.bids.len(),
                        event.order_book.asks.len(),
                        event.is_snapshot
                    );
                    if !event.order_book.bids.is_empty() {
                        println!(
                            "  Best Bid: {} @ {}",
                            event.order_book.bids[0].amount, event.order_book.bids[0].price
                        );
                    }
                    if !event.order_book.asks.is_empty() {
                        println!(
                            "  Best Ask: {} @ {}",
                            event.order_book.asks[0].amount, event.order_book.asks[0].price
                        );
                    }
                    if count >= 3 {
                        println!("[OrderBook] Received 3 snapshots, stopping...");
                        break;
                    }
                }
                WsMessage::Error(err) => {
                    eprintln!("[OrderBook] Error: {}", err);
                }
                WsMessage::Disconnected => {
                    println!("[OrderBook] Disconnected");
                    break;
                }
                _ => {}
            }
        }
    });

    // Example 3: Watch Trades
    println!("\nExample 3: Watching BTC/KRW trades...");
    let ws3 = CoinoneWs::new();
    let mut trades_rx = ws3.watch_trades("BTC/KRW").await?;

    tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = trades_rx.recv().await {
            match msg {
                WsMessage::Connected => {
                    println!("[Trades] Connected to Coinone WebSocket");
                }
                WsMessage::Trade(event) => {
                    count += 1;
                    for trade in &event.trades {
                        println!(
                            "[Trades] {} - ID: {}, Side: {:?}, Price: {}, Amount: {}",
                            event.symbol,
                            trade.id,
                            trade.side,
                            trade.price,
                            trade.amount
                        );
                    }
                    if count >= 5 {
                        println!("[Trades] Received 5 trades, stopping...");
                        break;
                    }
                }
                WsMessage::Error(err) => {
                    eprintln!("[Trades] Error: {}", err);
                }
                WsMessage::Disconnected => {
                    println!("[Trades] Disconnected");
                    break;
                }
                _ => {}
            }
        }
    });

    // Keep the main thread alive for 30 seconds
    println!("\nListening for 30 seconds...\n");
    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

    println!("\nExample completed!");
    Ok(())
}
