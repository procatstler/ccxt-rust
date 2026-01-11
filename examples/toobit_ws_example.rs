//! Toobit WebSocket Example
//!
//! Demonstrates using the Toobit exchange WebSocket API
//! Run with: cargo run --example toobit_ws_example

use ccxt_rust::exchanges::ToobitWs;
use ccxt_rust::types::{WsExchange, WsMessage};
use ccxt_rust::ExchangeConfig;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Toobit Exchange WebSocket Example");
    println!("==================================\n");

    let config = ExchangeConfig::default();
    let ws = ToobitWs::new(config.clone());
    println!("Exchange: Toobit (toobit)");
    println!();

    // Example 1: Watch ticker
    println!("Subscribing to BTC/USDT ticker...");
    let mut ticker_rx = ws.watch_ticker("BTC/USDT").await?;

    println!("Waiting for ticker updates (5 messages)...\n");
    let mut count = 0;
    let timeout = Duration::from_secs(30);

    loop {
        tokio::select! {
            Some(msg) = ticker_rx.recv() => {
                match msg {
                    WsMessage::Connected => {
                        println!("WebSocket connected!");
                    }
                    WsMessage::Disconnected => {
                        println!("WebSocket disconnected!");
                    }
                    WsMessage::Ticker(event) => {
                        println!("Ticker Update:");
                        println!("  Symbol: {}", event.ticker.symbol);
                        println!("  Last: {:?}", event.ticker.last);
                        println!("  Bid: {:?}", event.ticker.bid);
                        println!("  Ask: {:?}", event.ticker.ask);
                        println!("  Volume: {:?}", event.ticker.base_volume);
                        println!();
                        count += 1;
                        if count >= 5 {
                            break;
                        }
                    }
                    WsMessage::Error(e) => {
                        println!("Error: {e}");
                    }
                    other => {
                        println!("Other message: {other:?}");
                    }
                }
            }
            _ = tokio::time::sleep(timeout) => {
                println!("Timeout waiting for ticker");
                break;
            }
        }
    }

    // Example 2: Watch order book
    println!("\nSubscribing to BTC/USDT order book...");
    let ws2 = ToobitWs::new(config.clone());
    let mut orderbook_rx = ws2.watch_order_book("BTC/USDT", Some(10)).await?;

    println!("Waiting for order book updates (3 messages)...\n");
    count = 0;

    loop {
        tokio::select! {
            Some(msg) = orderbook_rx.recv() => {
                match msg {
                    WsMessage::Connected => {
                        println!("WebSocket connected!");
                    }
                    WsMessage::OrderBook(event) => {
                        println!("Order Book Update:");
                        println!("  Symbol: {}", event.order_book.symbol);
                        println!("  Bids: {} levels", event.order_book.bids.len());
                        println!("  Asks: {} levels", event.order_book.asks.len());
                        if let Some(bid) = event.order_book.bids.first() {
                            println!("  Best Bid: {} @ {}", bid.amount, bid.price);
                        }
                        if let Some(ask) = event.order_book.asks.first() {
                            println!("  Best Ask: {} @ {}", ask.amount, ask.price);
                        }
                        println!();
                        count += 1;
                        if count >= 3 {
                            break;
                        }
                    }
                    WsMessage::Error(e) => {
                        println!("Error: {e}");
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(timeout) => {
                println!("Timeout waiting for order book");
                break;
            }
        }
    }

    // Example 3: Watch trades
    println!("\nSubscribing to BTC/USDT trades...");
    let ws3 = ToobitWs::new(config);
    let mut trades_rx = ws3.watch_trades("BTC/USDT").await?;

    println!("Waiting for trade updates (5 messages)...\n");
    count = 0;

    loop {
        tokio::select! {
            Some(msg) = trades_rx.recv() => {
                match msg {
                    WsMessage::Connected => {
                        println!("WebSocket connected!");
                    }
                    WsMessage::Trade(event) => {
                        for trade in &event.trades {
                            println!("Trade: {} {} {} @ {} = {:?}",
                                trade.datetime.clone().unwrap_or_else(|| "?".to_string()),
                                trade.side.clone().unwrap_or_else(|| "?".to_string()),
                                trade.amount,
                                trade.price,
                                trade.cost
                            );
                        }
                        count += 1;
                        if count >= 5 {
                            break;
                        }
                    }
                    WsMessage::Error(e) => {
                        println!("Error: {e}");
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(timeout) => {
                println!("Timeout waiting for trades");
                break;
            }
        }
    }

    println!("\nDone!");
    Ok(())
}
