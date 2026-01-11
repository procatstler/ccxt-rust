//! Account API Trait
//!
//! Account and wallet management operations.

use async_trait::async_trait;
use rust_decimal::Decimal;
use std::collections::HashMap;

use crate::errors::CcxtResult;
use crate::not_supported;
use crate::types::{
    Account, DepositAddress, LedgerEntry, Transaction, TransferEntry,
};

/// Account and wallet API
///
/// Operations for managing accounts, deposits, withdrawals, and transfers:
/// - Account information
/// - Deposit addresses
/// - Withdrawals
/// - Internal transfers
/// - Transaction history
/// - Ledger entries
#[async_trait]
pub trait AccountApi: Send + Sync {
    // ========================================================================
    // Account Information
    // ========================================================================

    /// Fetch account information
    async fn fetch_accounts(&self) -> CcxtResult<Vec<Account>> {
        not_supported!("fetchAccounts")
    }

    // ========================================================================
    // Deposits
    // ========================================================================

    /// Fetch deposit history
    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let _ = (code, since, limit);
        not_supported!("fetchDeposits")
    }

    /// Fetch deposit address for a currency
    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<DepositAddress> {
        let _ = (code, network);
        not_supported!("fetchDepositAddress")
    }

    /// Fetch deposit addresses for multiple currencies
    async fn fetch_deposit_addresses(
        &self,
        codes: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, DepositAddress>> {
        let _ = codes;
        not_supported!("fetchDepositAddresses")
    }

    /// Fetch deposit addresses by network for a currency
    async fn fetch_deposit_addresses_by_network(
        &self,
        code: &str,
    ) -> CcxtResult<HashMap<String, DepositAddress>> {
        let _ = code;
        not_supported!("fetchDepositAddressesByNetwork")
    }

    /// Create a new deposit address
    async fn create_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<DepositAddress> {
        let _ = (code, network);
        not_supported!("createDepositAddress")
    }

    // ========================================================================
    // Withdrawals
    // ========================================================================

    /// Fetch withdrawal history
    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let _ = (code, since, limit);
        not_supported!("fetchWithdrawals")
    }

    /// Withdraw funds to an external address
    async fn withdraw(
        &self,
        code: &str,
        amount: Decimal,
        address: &str,
        tag: Option<&str>,
        network: Option<&str>,
    ) -> CcxtResult<Transaction> {
        let _ = (code, amount, address, tag, network);
        not_supported!("withdraw")
    }

    // ========================================================================
    // Transfers
    // ========================================================================

    /// Transfer funds between accounts (e.g., spot to futures)
    async fn transfer(
        &self,
        code: &str,
        amount: Decimal,
        from_account: &str,
        to_account: &str,
    ) -> CcxtResult<TransferEntry> {
        let _ = (code, amount, from_account, to_account);
        not_supported!("transfer")
    }

    /// Fetch transfer history
    async fn fetch_transfers(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<TransferEntry>> {
        let _ = (code, since, limit);
        not_supported!("fetchTransfers")
    }

    // ========================================================================
    // Ledger
    // ========================================================================

    /// Fetch account ledger (all transactions)
    async fn fetch_ledger(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<LedgerEntry>> {
        let _ = (code, since, limit);
        not_supported!("fetchLedger")
    }

    // ========================================================================
    // Fees
    // ========================================================================

    /// Fetch deposit and withdrawal fees
    async fn fetch_deposit_withdraw_fees(
        &self,
        codes: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, crate::types::DepositWithdrawFee>> {
        let _ = codes;
        not_supported!("fetchDepositWithdrawFees")
    }
}
