//! WebSocket Streaming Example
//!
//! Demonstrates real-time data streaming using WebSocket connections:
//! - Ticker updates
//! - Order book updates
//! - Trade stream

use ccxt_rust::exchanges::cex::BinanceWs;
use ccxt_rust::types::{WsExchange, WsMessage};
use ccxt_rust::CcxtResult;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::main]
async fn main() -> CcxtResult<()> {
    println!("=== WebSocket Streaming Example ===\n");

    let symbol = "BTC/USDT";

    // Example 1: Ticker Stream
    println!("--- Ticker Stream ---");
    ticker_stream(symbol).await?;

    // Example 2: Trade Stream
    println!("\n--- Trade Stream ---");
    trade_stream(symbol).await?;

    // Example 3: Order Book Stream
    println!("\n--- Order Book Stream ---");
    orderbook_stream(symbol).await?;

    println!("\n=== Example Complete ===");
    Ok(())
}

async fn ticker_stream(symbol: &str) -> CcxtResult<()> {
    let ws = BinanceWs::new();
    let mut rx = ws.watch_ticker(symbol).await?;

    println!("Watching {} ticker (5 updates)...", symbol);

    let mut count = 0;
    while count < 5 {
        match timeout(Duration::from_secs(10), rx.recv()).await {
            Ok(Some(WsMessage::Ticker(event))) => {
                println!(
                    "  [{}] Last: {:?}, Bid: {:?}, Ask: {:?}",
                    event.symbol, event.ticker.last, event.ticker.bid, event.ticker.ask
                );
                count += 1;
            },
            Ok(Some(WsMessage::Connected)) => {
                println!("  Connected to WebSocket");
            },
            Ok(Some(WsMessage::Error(e))) => {
                println!("  Error: {}", e);
                break;
            },
            Ok(None) => {
                println!("  Stream ended");
                break;
            },
            Err(_) => {
                println!("  Timeout waiting for data");
                break;
            },
            _ => {},
        }
    }

    Ok(())
}

async fn trade_stream(symbol: &str) -> CcxtResult<()> {
    let ws = BinanceWs::new();
    let mut rx = ws.watch_trades(symbol).await?;

    println!("Watching {} trades (5 updates)...", symbol);

    let mut count = 0;
    while count < 5 {
        match timeout(Duration::from_secs(10), rx.recv()).await {
            Ok(Some(WsMessage::Trade(event))) => {
                for trade in &event.trades {
                    println!(
                        "  Trade: Price={}, Amount={}, Side={:?}",
                        trade.price, trade.amount, trade.side
                    );
                }
                count += 1;
            },
            Ok(Some(WsMessage::Connected)) => {
                println!("  Connected to WebSocket");
            },
            Ok(None) => {
                println!("  Stream ended");
                break;
            },
            Err(_) => {
                println!("  Timeout waiting for data");
                break;
            },
            _ => {},
        }
    }

    Ok(())
}

async fn orderbook_stream(symbol: &str) -> CcxtResult<()> {
    let ws = BinanceWs::new();
    let mut rx = ws.watch_order_book(symbol, Some(5)).await?;

    println!("Watching {} order book (3 updates)...", symbol);

    let mut count = 0;
    while count < 3 {
        match timeout(Duration::from_secs(10), rx.recv()).await {
            Ok(Some(WsMessage::OrderBook(event))) => {
                let ob = &event.order_book;
                println!("  Order Book Update:");
                println!(
                    "    Top Bid: {:?}",
                    ob.bids.first().map(|b| (&b.price, &b.amount))
                );
                println!(
                    "    Top Ask: {:?}",
                    ob.asks.first().map(|a| (&a.price, &a.amount))
                );
                count += 1;
            },
            Ok(Some(WsMessage::Connected)) => {
                println!("  Connected to WebSocket");
            },
            Ok(None) => {
                println!("  Stream ended");
                break;
            },
            Err(_) => {
                println!("  Timeout waiting for data");
                break;
            },
            _ => {},
        }
    }

    Ok(())
}
