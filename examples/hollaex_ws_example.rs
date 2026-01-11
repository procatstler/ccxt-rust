//! HollaEx WebSocket Example
//!
//! Demonstrates how to use the HollaEx WebSocket implementation
//! to receive real-time market data.

use ccxt_rust::exchanges::HollaexWs;
use ccxt_rust::types::{WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("HollaEx WebSocket Example");
    println!("=========================\n");

    // Example 1: Watch Order Book
    println!("Example 1: Watching BTC/USDT order book...");
    let mut ws1 = HollaexWs::new();
    ws1.ws_connect().await?;
    let mut orderbook_rx = ws1.watch_order_book("BTC/USDT", None).await?;

    tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = orderbook_rx.recv().await {
            match msg {
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
                        println!("[OrderBook] Received 3 updates, stopping...");
                        break;
                    }
                }
                WsMessage::Error(err) => {
                    eprintln!("[OrderBook] Error: {err}");
                }
                WsMessage::Disconnected => {
                    println!("[OrderBook] Disconnected");
                    break;
                }
                _ => {}
            }
        }
    });

    // Example 2: Watch Trades
    println!("\nExample 2: Watching BTC/USDT trades...");
    let mut ws2 = HollaexWs::new();
    ws2.ws_connect().await?;
    let mut trades_rx = ws2.watch_trades("BTC/USDT").await?;

    tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = trades_rx.recv().await {
            match msg {
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
                        println!("[Trades] Received 5 trade events, stopping...");
                        break;
                    }
                }
                WsMessage::Error(err) => {
                    eprintln!("[Trades] Error: {err}");
                }
                WsMessage::Disconnected => {
                    println!("[Trades] Disconnected");
                    break;
                }
                _ => {}
            }
        }
    });

    // Example 3: Watch Multiple Symbols
    println!("\nExample 3: Watching multiple symbols (BTC/USDT, ETH/USDT)...");
    let mut ws3 = HollaexWs::new();
    ws3.ws_connect().await?;
    let mut multi_trades_rx = ws3.watch_trades_for_symbols(&["BTC/USDT", "ETH/USDT"]).await?;

    tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = multi_trades_rx.recv().await {
            match msg {
                WsMessage::Trade(event) => {
                    count += 1;
                    for trade in &event.trades {
                        println!(
                            "[Multi] {} - Side: {:?}, Price: {}, Amount: {}",
                            event.symbol,
                            trade.side,
                            trade.price,
                            trade.amount
                        );
                    }
                    if count >= 10 {
                        println!("[Multi] Received 10 trade events, stopping...");
                        break;
                    }
                }
                WsMessage::Error(err) => {
                    eprintln!("[Multi] Error: {err}");
                }
                WsMessage::Disconnected => {
                    println!("[Multi] Disconnected");
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
