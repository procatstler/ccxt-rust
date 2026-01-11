//! Exchange Operation Benchmarks
//!
//! Benchmarks for common exchange operations including:
//! - Exchange instantiation
//! - Symbol conversion
//! - Market data parsing

use ccxt_rust::client::ExchangeConfig;
use ccxt_rust::exchanges::cex::{Binance, Bybit, Okx};
use ccxt_rust::types::Exchange;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_exchange_creation(c: &mut Criterion) {
    let config = ExchangeConfig::new();

    let mut group = c.benchmark_group("exchange_creation");

    group.bench_function("binance_new", |b| {
        b.iter(|| {
            let _ = black_box(Binance::new(config.clone()));
        })
    });

    group.bench_function("bybit_new", |b| {
        b.iter(|| {
            let _ = black_box(Bybit::new(config.clone()));
        })
    });

    group.bench_function("okx_new", |b| {
        b.iter(|| {
            let _ = black_box(Okx::new(config.clone()));
        })
    });

    group.finish();
}

fn bench_exchange_id(c: &mut Criterion) {
    let config = ExchangeConfig::new();
    let binance = Binance::new(config.clone()).unwrap();
    let bybit = Bybit::new(config.clone()).unwrap();
    let okx = Okx::new(config).unwrap();

    let mut group = c.benchmark_group("exchange_id");

    group.bench_function("binance_id", |b| b.iter(|| black_box(binance.id())));

    group.bench_function("bybit_id", |b| b.iter(|| black_box(bybit.id())));

    group.bench_function("okx_id", |b| b.iter(|| black_box(okx.id())));

    group.finish();
}

fn bench_exchange_features(c: &mut Criterion) {
    let config = ExchangeConfig::new();
    let binance = Binance::new(config.clone()).unwrap();
    let bybit = Bybit::new(config.clone()).unwrap();
    let okx = Okx::new(config).unwrap();

    let mut group = c.benchmark_group("exchange_features");

    group.bench_function("binance_has", |b| b.iter(|| black_box(binance.has())));

    group.bench_function("bybit_has", |b| b.iter(|| black_box(bybit.has())));

    group.bench_function("okx_has", |b| b.iter(|| black_box(okx.has())));

    group.finish();
}

criterion_group!(
    benches,
    bench_exchange_creation,
    bench_exchange_id,
    bench_exchange_features,
);

criterion_main!(benches);
