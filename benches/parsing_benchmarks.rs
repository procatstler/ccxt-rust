//! Parsing Benchmarks
//!
//! Benchmarks for JSON parsing and data structure operations

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::json;

fn bench_decimal_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("decimal_operations");

    let price = dec!(50000.12345678);
    let amount = dec!(0.00123456);

    group.bench_function("decimal_multiply", |b| b.iter(|| black_box(price * amount)));

    group.bench_function("decimal_divide", |b| b.iter(|| black_box(price / amount)));

    group.bench_function("decimal_from_str", |b| {
        b.iter(|| black_box("50000.12345678".parse::<Decimal>()))
    });

    group.bench_function("decimal_to_string", |b| {
        b.iter(|| black_box(price.to_string()))
    });

    group.finish();
}

fn bench_json_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_parsing");

    // Simulated ticker response
    let ticker_json = json!({
        "symbol": "BTCUSDT",
        "lastPrice": "50000.00",
        "bidPrice": "49999.00",
        "askPrice": "50001.00",
        "highPrice": "51000.00",
        "lowPrice": "49000.00",
        "volume": "1000.00",
        "quoteVolume": "50000000.00"
    });

    let ticker_str = ticker_json.to_string();

    group.bench_function("json_parse_ticker", |b| {
        b.iter(|| {
            let _: serde_json::Value = black_box(serde_json::from_str(&ticker_str).unwrap());
        })
    });

    // Simulated order book response
    let orderbook_json = json!({
        "lastUpdateId": 123456789,
        "bids": [
            ["49999.00", "1.000"],
            ["49998.00", "2.000"],
            ["49997.00", "3.000"],
            ["49996.00", "4.000"],
            ["49995.00", "5.000"]
        ],
        "asks": [
            ["50001.00", "1.000"],
            ["50002.00", "2.000"],
            ["50003.00", "3.000"],
            ["50004.00", "4.000"],
            ["50005.00", "5.000"]
        ]
    });

    let orderbook_str = orderbook_json.to_string();

    group.bench_function("json_parse_orderbook", |b| {
        b.iter(|| {
            let _: serde_json::Value = black_box(serde_json::from_str(&orderbook_str).unwrap());
        })
    });

    // Large order book (100 levels)
    let large_bids: Vec<Vec<String>> = (0..100)
        .map(|i| vec![format!("{}.00", 50000 - i), format!("{}.000", i + 1)])
        .collect();
    let large_asks: Vec<Vec<String>> = (0..100)
        .map(|i| vec![format!("{}.00", 50000 + i + 1), format!("{}.000", i + 1)])
        .collect();

    let large_orderbook_json = json!({
        "lastUpdateId": 123456789,
        "bids": large_bids,
        "asks": large_asks
    });

    let large_orderbook_str = large_orderbook_json.to_string();

    group.bench_function("json_parse_large_orderbook", |b| {
        b.iter(|| {
            let _: serde_json::Value =
                black_box(serde_json::from_str(&large_orderbook_str).unwrap());
        })
    });

    group.finish();
}

fn bench_order_book_operations(c: &mut Criterion) {
    use ccxt_rust::types::{OrderBook, OrderBookEntry};

    let mut group = c.benchmark_group("orderbook_operations");

    // Create order book with various sizes
    for size in [10, 50, 100, 500] {
        let bids: Vec<OrderBookEntry> = (0..size)
            .map(|i| OrderBookEntry {
                price: Decimal::from(50000 - i),
                amount: Decimal::from(i + 1),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = (0..size)
            .map(|i| OrderBookEntry {
                price: Decimal::from(50001 + i),
                amount: Decimal::from(i + 1),
            })
            .collect();

        let orderbook = OrderBook {
            symbol: "BTC/USDT".to_string(),
            timestamp: Some(1234567890000),
            datetime: None,
            nonce: None,
            bids: bids.clone(),
            asks: asks.clone(),
            checksum: None,
        };

        group.bench_with_input(
            BenchmarkId::new("orderbook_clone", size),
            &orderbook,
            |b, ob| b.iter(|| black_box(ob.clone())),
        );

        group.bench_with_input(
            BenchmarkId::new("orderbook_best_bid", size),
            &orderbook,
            |b, ob| b.iter(|| black_box(ob.bids.first())),
        );

        group.bench_with_input(
            BenchmarkId::new("orderbook_best_ask", size),
            &orderbook,
            |b, ob| b.iter(|| black_box(ob.asks.first())),
        );
    }

    group.finish();
}

fn bench_signature_generation(c: &mut Criterion) {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    let mut group = c.benchmark_group("signature_generation");

    let secret = b"your_secret_key_here_1234567890";
    let message =
        "symbol=BTCUSDT&side=BUY&type=LIMIT&quantity=0.001&price=50000&timestamp=1234567890000";

    group.bench_function("hmac_sha256", |b| {
        b.iter(|| {
            let mut mac = HmacSha256::new_from_slice(secret).unwrap();
            mac.update(message.as_bytes());
            let result = mac.finalize();
            black_box(hex::encode(result.into_bytes()))
        })
    });

    // Base64 encoding
    group.bench_function("base64_encode", |b| {
        let data = secret;
        b.iter(|| {
            use base64::{engine::general_purpose::STANDARD, Engine};
            black_box(STANDARD.encode(data))
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_decimal_operations,
    bench_json_parsing,
    bench_order_book_operations,
    bench_signature_generation,
);

criterion_main!(benches);
