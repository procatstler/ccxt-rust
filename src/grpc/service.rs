//! gRPC Service Implementation
//!
//! Implements the ExchangeService gRPC service for external integrations.

use tonic::{Request, Response, Status};

use crate::client::ExchangeConfig;
use crate::exchanges::cex::{
    Binance, BinanceUs, Bithumb, Bitget, Bybit, Coinbase, Gate, Htx, Kraken, Kucoin, Mexc, Okx,
    Upbit,
};
use crate::types::{Balances, DepositAddress, Exchange, ExchangeId, Trade, Transaction};

use super::generated::{
    exchange_service_server::ExchangeService,
    Balance as ProtoBalance,
    Credentials,
    DepositAddress as ProtoDepositAddress,
    ExchangeInfo,
    ExchangeType,
    Fee as ProtoFee,
    FetchBalanceRequest,
    FetchBalanceResponse,
    FetchDepositAddressRequest,
    FetchDepositAddressResponse,
    FetchMyTradesRequest,
    FetchMyTradesResponse,
    FetchTransactionsRequest,
    FetchTransactionsResponse,
    GetSupportedExchangesRequest,
    GetSupportedExchangesResponse,
    TakerOrMaker as ProtoTakerOrMaker,
    TestConnectionRequest,
    TestConnectionResponse,
    Trade as ProtoTrade,
    TradeSide as ProtoTradeSide,
    Transaction as ProtoTransaction,
    TransactionStatus as ProtoTransactionStatus,
    TransactionType as ProtoTransactionType,
};

/// ExchangeService implementation
pub struct ExchangeServiceImpl;

impl ExchangeServiceImpl {
    pub fn new() -> Self {
        Self
    }

    /// Convert proto ExchangeType to ccxt-rust ExchangeId
    fn to_exchange_id(exchange_type: i32) -> Result<ExchangeId, Status> {
        match ExchangeType::try_from(exchange_type) {
            Ok(ExchangeType::Upbit) => Ok(ExchangeId::Upbit),
            Ok(ExchangeType::Bithumb) => Ok(ExchangeId::Bithumb),
            Ok(ExchangeType::Binance) => Ok(ExchangeId::Binance),
            Ok(ExchangeType::BinanceUs) => Ok(ExchangeId::BinanceUs),
            Ok(ExchangeType::Coinbase) => Ok(ExchangeId::Coinbase),
            Ok(ExchangeType::Kraken) => Ok(ExchangeId::Kraken),
            Ok(ExchangeType::Okx) => Ok(ExchangeId::Okx),
            Ok(ExchangeType::Bybit) => Ok(ExchangeId::Bybit),
            Ok(ExchangeType::Kucoin) => Ok(ExchangeId::Kucoin),
            Ok(ExchangeType::Gate) => Ok(ExchangeId::Gate),
            Ok(ExchangeType::Bitget) => Ok(ExchangeId::Bitget),
            Ok(ExchangeType::Mexc) => Ok(ExchangeId::Mexc),
            Ok(ExchangeType::Htx) => Ok(ExchangeId::Htx),
            _ => Err(Status::invalid_argument("Unsupported exchange type")),
        }
    }

    /// Create exchange config from credentials
    fn create_config(creds: &Credentials) -> ExchangeConfig {
        let mut config = ExchangeConfig::default()
            .with_api_key(&creds.api_key)
            .with_api_secret(&creds.api_secret);

        if let Some(ref passphrase) = creds.passphrase {
            config = config.with_password(passphrase);
        }

        if creds.sandbox {
            config = config.with_sandbox(true);
        }

        config
    }

    /// Fetch balance for a specific exchange
    async fn do_fetch_balance(creds: &Credentials) -> Result<Balances, Status> {
        let config = Self::create_config(creds);
        let exchange_id = Self::to_exchange_id(creds.exchange)?;

        match exchange_id {
            ExchangeId::Upbit => {
                let exchange = Upbit::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            ExchangeId::Bithumb => {
                let exchange = Bithumb::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            ExchangeId::Binance => {
                let exchange = Binance::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            ExchangeId::BinanceUs => {
                let exchange = BinanceUs::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            ExchangeId::Coinbase => {
                let exchange = Coinbase::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            ExchangeId::Kraken => {
                let exchange = Kraken::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            ExchangeId::Okx => {
                let exchange = Okx::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            ExchangeId::Bybit => {
                let exchange = Bybit::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            ExchangeId::Kucoin => {
                let exchange = Kucoin::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            ExchangeId::Gate => {
                let exchange = Gate::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            ExchangeId::Bitget => {
                let exchange = Bitget::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            ExchangeId::Mexc => {
                let exchange = Mexc::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            ExchangeId::Htx => {
                let exchange = Htx::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_balance()
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch balance: {}", e)))
            }
            _ => Err(Status::unimplemented(format!(
                "Exchange {:?} not yet implemented in gRPC service",
                exchange_id
            ))),
        }
    }

    /// Fetch my trades for a specific exchange
    async fn do_fetch_my_trades(
        creds: &Credentials,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> Result<Vec<Trade>, Status> {
        let config = Self::create_config(creds);
        let exchange_id = Self::to_exchange_id(creds.exchange)?;

        match exchange_id {
            ExchangeId::Upbit => {
                let exchange = Upbit::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            ExchangeId::Bithumb => {
                let exchange = Bithumb::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            ExchangeId::Binance => {
                let exchange = Binance::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            ExchangeId::BinanceUs => {
                let exchange = BinanceUs::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            ExchangeId::Coinbase => {
                let exchange = Coinbase::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            ExchangeId::Kraken => {
                let exchange = Kraken::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            ExchangeId::Okx => {
                let exchange = Okx::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            ExchangeId::Bybit => {
                let exchange = Bybit::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            ExchangeId::Kucoin => {
                let exchange = Kucoin::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            ExchangeId::Gate => {
                let exchange = Gate::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            ExchangeId::Bitget => {
                let exchange = Bitget::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            ExchangeId::Mexc => {
                let exchange = Mexc::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            ExchangeId::Htx => {
                let exchange = Htx::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_my_trades(symbol, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch trades: {}", e)))
            }
            _ => Err(Status::unimplemented(format!(
                "Exchange {:?} not yet implemented in gRPC service",
                exchange_id
            ))),
        }
    }

    /// Fetch deposits for a specific exchange
    async fn do_fetch_deposits(
        creds: &Credentials,
        currency: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> Result<Vec<Transaction>, Status> {
        let config = Self::create_config(creds);
        let exchange_id = Self::to_exchange_id(creds.exchange)?;

        match exchange_id {
            ExchangeId::Upbit => {
                let exchange = Upbit::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            ExchangeId::Bithumb => {
                let exchange = Bithumb::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            ExchangeId::Binance => {
                let exchange = Binance::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            ExchangeId::BinanceUs => {
                let exchange = BinanceUs::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            ExchangeId::Coinbase => {
                let exchange = Coinbase::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            ExchangeId::Kraken => {
                let exchange = Kraken::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            ExchangeId::Okx => {
                let exchange = Okx::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            ExchangeId::Bybit => {
                let exchange = Bybit::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            ExchangeId::Kucoin => {
                let exchange = Kucoin::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            ExchangeId::Gate => {
                let exchange = Gate::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            ExchangeId::Bitget => {
                let exchange = Bitget::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            ExchangeId::Mexc => {
                let exchange = Mexc::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            ExchangeId::Htx => {
                let exchange = Htx::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposits(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch deposits: {}", e)))
            }
            _ => Err(Status::unimplemented(format!(
                "Exchange {:?} not yet implemented in gRPC service",
                exchange_id
            ))),
        }
    }

    /// Fetch withdrawals for a specific exchange
    async fn do_fetch_withdrawals(
        creds: &Credentials,
        currency: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> Result<Vec<Transaction>, Status> {
        let config = Self::create_config(creds);
        let exchange_id = Self::to_exchange_id(creds.exchange)?;

        match exchange_id {
            ExchangeId::Upbit => {
                let exchange = Upbit::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            ExchangeId::Bithumb => {
                let exchange = Bithumb::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            ExchangeId::Binance => {
                let exchange = Binance::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            ExchangeId::BinanceUs => {
                let exchange = BinanceUs::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            ExchangeId::Coinbase => {
                let exchange = Coinbase::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            ExchangeId::Kraken => {
                let exchange = Kraken::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            ExchangeId::Okx => {
                let exchange = Okx::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            ExchangeId::Bybit => {
                let exchange = Bybit::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            ExchangeId::Kucoin => {
                let exchange = Kucoin::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            ExchangeId::Gate => {
                let exchange = Gate::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            ExchangeId::Bitget => {
                let exchange = Bitget::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            ExchangeId::Mexc => {
                let exchange = Mexc::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            ExchangeId::Htx => {
                let exchange = Htx::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_withdrawals(currency, since, limit)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to fetch withdrawals: {}", e)))
            }
            _ => Err(Status::unimplemented(format!(
                "Exchange {:?} not yet implemented in gRPC service",
                exchange_id
            ))),
        }
    }

    /// Fetch deposit address for a specific exchange
    async fn do_fetch_deposit_address(
        creds: &Credentials,
        currency: &str,
        network: Option<&str>,
    ) -> Result<DepositAddress, Status> {
        let config = Self::create_config(creds);
        let exchange_id = Self::to_exchange_id(creds.exchange)?;

        match exchange_id {
            ExchangeId::Upbit => {
                let exchange = Upbit::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            ExchangeId::Bithumb => {
                let exchange = Bithumb::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            ExchangeId::Binance => {
                let exchange = Binance::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            ExchangeId::BinanceUs => {
                let exchange = BinanceUs::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            ExchangeId::Coinbase => {
                let exchange = Coinbase::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            ExchangeId::Kraken => {
                let exchange = Kraken::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            ExchangeId::Okx => {
                let exchange = Okx::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            ExchangeId::Bybit => {
                let exchange = Bybit::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            ExchangeId::Kucoin => {
                let exchange = Kucoin::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            ExchangeId::Gate => {
                let exchange = Gate::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            ExchangeId::Bitget => {
                let exchange = Bitget::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            ExchangeId::Mexc => {
                let exchange = Mexc::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            ExchangeId::Htx => {
                let exchange = Htx::new(config)
                    .map_err(|e| Status::internal(format!("Failed to create exchange: {}", e)))?;
                exchange
                    .fetch_deposit_address(currency, network)
                    .await
                    .map_err(|e| {
                        Status::internal(format!("Failed to fetch deposit address: {}", e))
                    })
            }
            _ => Err(Status::unimplemented(format!(
                "Exchange {:?} not yet implemented in gRPC service",
                exchange_id
            ))),
        }
    }
}

impl Default for ExchangeServiceImpl {
    fn default() -> Self {
        Self::new()
    }
}

#[tonic::async_trait]
impl ExchangeService for ExchangeServiceImpl {
    async fn test_connection(
        &self,
        request: Request<TestConnectionRequest>,
    ) -> Result<Response<TestConnectionResponse>, Status> {
        let req = request.into_inner();
        let creds = req
            .credentials
            .ok_or_else(|| Status::invalid_argument("Missing credentials"))?;

        match Self::do_fetch_balance(&creds).await {
            Ok(_) => Ok(Response::new(TestConnectionResponse {
                success: true,
                error_message: None,
                server_time: Some(chrono::Utc::now().timestamp_millis()),
            })),
            Err(e) => Ok(Response::new(TestConnectionResponse {
                success: false,
                error_message: Some(e.message().to_string()),
                server_time: None,
            })),
        }
    }

    async fn fetch_balance(
        &self,
        request: Request<FetchBalanceRequest>,
    ) -> Result<Response<FetchBalanceResponse>, Status> {
        let req = request.into_inner();
        let creds = req
            .credentials
            .ok_or_else(|| Status::invalid_argument("Missing credentials"))?;

        match Self::do_fetch_balance(&creds).await {
            Ok(balances) => {
                let proto_balances: Vec<ProtoBalance> = balances
                    .currencies
                    .into_iter()
                    .map(|(currency, balance)| ProtoBalance {
                        currency,
                        free: balance.free.map(|d| d.to_string()).unwrap_or_default(),
                        used: balance.used.map(|d| d.to_string()).unwrap_or_default(),
                        total: balance.total.map(|d| d.to_string()).unwrap_or_default(),
                        debt: balance.debt.map(|d| d.to_string()),
                    })
                    .collect();

                Ok(Response::new(FetchBalanceResponse {
                    balances: proto_balances,
                    timestamp: balances.timestamp,
                    error_message: None,
                }))
            }
            Err(e) => Ok(Response::new(FetchBalanceResponse {
                balances: vec![],
                timestamp: None,
                error_message: Some(e.message().to_string()),
            })),
        }
    }

    async fn fetch_my_trades(
        &self,
        request: Request<FetchMyTradesRequest>,
    ) -> Result<Response<FetchMyTradesResponse>, Status> {
        let req = request.into_inner();
        let creds = req
            .credentials
            .ok_or_else(|| Status::invalid_argument("Missing credentials"))?;

        let symbol = req.symbol.as_deref();
        let since = req.since;
        let limit = req.limit;

        match Self::do_fetch_my_trades(&creds, symbol, since, limit).await {
            Ok(trades) => {
                let proto_trades: Vec<ProtoTrade> = trades
                    .into_iter()
                    .map(|trade| {
                        let side = match trade.side.as_deref() {
                            Some("buy") => ProtoTradeSide::Buy.into(),
                            Some("sell") => ProtoTradeSide::Sell.into(),
                            _ => ProtoTradeSide::Unspecified.into(),
                        };

                        let taker_or_maker = match trade.taker_or_maker {
                            Some(crate::types::TakerOrMaker::Taker) => {
                                ProtoTakerOrMaker::Taker.into()
                            }
                            Some(crate::types::TakerOrMaker::Maker) => {
                                ProtoTakerOrMaker::Maker.into()
                            }
                            None => ProtoTakerOrMaker::Unspecified.into(),
                        };

                        ProtoTrade {
                            id: trade.id,
                            order_id: trade.order,
                            symbol: trade.symbol,
                            timestamp: trade.timestamp,
                            datetime: trade.datetime,
                            side,
                            taker_or_maker,
                            price: trade.price.to_string(),
                            amount: trade.amount.to_string(),
                            cost: trade.cost.map(|d| d.to_string()).unwrap_or_default(),
                            fee: trade.fee.map(|f| ProtoFee {
                                currency: f.currency.unwrap_or_default(),
                                cost: f.cost.map(|c| c.to_string()).unwrap_or_default(),
                                rate: f.rate.map(|r| r.to_string()),
                            }),
                            fees: trade
                                .fees
                                .into_iter()
                                .map(|f| ProtoFee {
                                    currency: f.currency.unwrap_or_default(),
                                    cost: f.cost.map(|c| c.to_string()).unwrap_or_default(),
                                    rate: f.rate.map(|r| r.to_string()),
                                })
                                .collect(),
                            raw_info: Some(trade.info.to_string()),
                        }
                    })
                    .collect();

                Ok(Response::new(FetchMyTradesResponse {
                    trades: proto_trades,
                    error_message: None,
                }))
            }
            Err(e) => Ok(Response::new(FetchMyTradesResponse {
                trades: vec![],
                error_message: Some(e.message().to_string()),
            })),
        }
    }

    async fn fetch_deposits(
        &self,
        request: Request<FetchTransactionsRequest>,
    ) -> Result<Response<FetchTransactionsResponse>, Status> {
        let req = request.into_inner();
        let creds = req
            .credentials
            .ok_or_else(|| Status::invalid_argument("Missing credentials"))?;

        let currency = req.currency.as_deref();
        let since = req.since;
        let limit = req.limit;

        match Self::do_fetch_deposits(&creds, currency, since, limit).await {
            Ok(transactions) => {
                let proto_txs = convert_transactions(transactions);
                Ok(Response::new(FetchTransactionsResponse {
                    transactions: proto_txs,
                    error_message: None,
                }))
            }
            Err(e) => Ok(Response::new(FetchTransactionsResponse {
                transactions: vec![],
                error_message: Some(e.message().to_string()),
            })),
        }
    }

    async fn fetch_withdrawals(
        &self,
        request: Request<FetchTransactionsRequest>,
    ) -> Result<Response<FetchTransactionsResponse>, Status> {
        let req = request.into_inner();
        let creds = req
            .credentials
            .ok_or_else(|| Status::invalid_argument("Missing credentials"))?;

        let currency = req.currency.as_deref();
        let since = req.since;
        let limit = req.limit;

        match Self::do_fetch_withdrawals(&creds, currency, since, limit).await {
            Ok(transactions) => {
                let proto_txs = convert_transactions(transactions);
                Ok(Response::new(FetchTransactionsResponse {
                    transactions: proto_txs,
                    error_message: None,
                }))
            }
            Err(e) => Ok(Response::new(FetchTransactionsResponse {
                transactions: vec![],
                error_message: Some(e.message().to_string()),
            })),
        }
    }

    async fn fetch_deposit_address(
        &self,
        request: Request<FetchDepositAddressRequest>,
    ) -> Result<Response<FetchDepositAddressResponse>, Status> {
        let req = request.into_inner();
        let creds = req
            .credentials
            .ok_or_else(|| Status::invalid_argument("Missing credentials"))?;

        let currency = &req.currency;
        let network = req.network.as_deref();

        match Self::do_fetch_deposit_address(&creds, currency, network).await {
            Ok(addr) => Ok(Response::new(FetchDepositAddressResponse {
                deposit_address: Some(ProtoDepositAddress {
                    currency: addr.currency,
                    address: addr.address,
                    tag: addr.tag,
                    network: addr.network,
                    raw_info: Some(addr.info.to_string()),
                }),
                error_message: None,
            })),
            Err(e) => Ok(Response::new(FetchDepositAddressResponse {
                deposit_address: None,
                error_message: Some(e.message().to_string()),
            })),
        }
    }

    async fn get_supported_exchanges(
        &self,
        _request: Request<GetSupportedExchangesRequest>,
    ) -> Result<Response<GetSupportedExchangesResponse>, Status> {
        let exchanges = vec![
            ExchangeInfo {
                exchange_type: ExchangeType::Upbit.into(),
                name: "Upbit".to_string(),
                countries: vec!["KR".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
            ExchangeInfo {
                exchange_type: ExchangeType::Bithumb.into(),
                name: "Bithumb".to_string(),
                countries: vec!["KR".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
            ExchangeInfo {
                exchange_type: ExchangeType::Binance.into(),
                name: "Binance".to_string(),
                countries: vec!["GLOBAL".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
            ExchangeInfo {
                exchange_type: ExchangeType::BinanceUs.into(),
                name: "Binance US".to_string(),
                countries: vec!["US".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
            ExchangeInfo {
                exchange_type: ExchangeType::Coinbase.into(),
                name: "Coinbase".to_string(),
                countries: vec!["US".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
            ExchangeInfo {
                exchange_type: ExchangeType::Kraken.into(),
                name: "Kraken".to_string(),
                countries: vec!["US".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
            ExchangeInfo {
                exchange_type: ExchangeType::Okx.into(),
                name: "OKX".to_string(),
                countries: vec!["GLOBAL".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
            ExchangeInfo {
                exchange_type: ExchangeType::Bybit.into(),
                name: "Bybit".to_string(),
                countries: vec!["GLOBAL".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
            ExchangeInfo {
                exchange_type: ExchangeType::Kucoin.into(),
                name: "KuCoin".to_string(),
                countries: vec!["GLOBAL".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
            ExchangeInfo {
                exchange_type: ExchangeType::Gate.into(),
                name: "Gate.io".to_string(),
                countries: vec!["GLOBAL".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
            ExchangeInfo {
                exchange_type: ExchangeType::Bitget.into(),
                name: "Bitget".to_string(),
                countries: vec!["GLOBAL".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
            ExchangeInfo {
                exchange_type: ExchangeType::Mexc.into(),
                name: "MEXC".to_string(),
                countries: vec!["GLOBAL".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
            ExchangeInfo {
                exchange_type: ExchangeType::Htx.into(),
                name: "HTX".to_string(),
                countries: vec!["GLOBAL".to_string()],
                has_fetch_balance: true,
                has_fetch_my_trades: true,
                has_fetch_deposits: true,
                has_fetch_withdrawals: true,
                has_fetch_deposit_address: true,
            },
        ];

        Ok(Response::new(GetSupportedExchangesResponse { exchanges }))
    }
}

/// Convert ccxt-rust Transaction to proto Transaction
fn convert_transactions(transactions: Vec<Transaction>) -> Vec<ProtoTransaction> {
    transactions
        .into_iter()
        .map(|tx| {
            let tx_type = match tx.tx_type {
                crate::types::TransactionType::Deposit => ProtoTransactionType::Deposit.into(),
                crate::types::TransactionType::Withdrawal => {
                    ProtoTransactionType::Withdrawal.into()
                }
            };

            let status = match tx.status {
                crate::types::TransactionStatus::Pending => ProtoTransactionStatus::Pending.into(),
                crate::types::TransactionStatus::Ok => ProtoTransactionStatus::Ok.into(),
                crate::types::TransactionStatus::Canceled => {
                    ProtoTransactionStatus::Canceled.into()
                }
                crate::types::TransactionStatus::Failed => ProtoTransactionStatus::Failed.into(),
            };

            ProtoTransaction {
                id: tx.id,
                tx_type,
                currency: tx.currency,
                amount: tx.amount.to_string(),
                status,
                timestamp: tx.timestamp,
                datetime: tx.datetime,
                network: tx.network,
                address: tx.address,
                tag: tx.tag,
                txid: tx.txid,
                fee: tx.fee.map(|f| ProtoFee {
                    currency: f.currency.unwrap_or_default(),
                    cost: f.cost.map(|c| c.to_string()).unwrap_or_default(),
                    rate: f.rate.map(|r| r.to_string()),
                }),
                internal: tx.internal,
                confirmations: tx.confirmations,
                raw_info: Some(tx.info.to_string()),
            }
        })
        .collect()
}
