//! Account Types
//!
//! Types for account management and transfers

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::fee::Fee;

/// Account information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// Account ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Account type (e.g., "spot", "margin", "futures")
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_type: Option<String>,

    /// Currency code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,

    /// Raw exchange response
    #[serde(default)]
    pub info: Value,
}

impl Account {
    /// Create a new Account
    pub fn new() -> Self {
        Account {
            id: None,
            account_type: None,
            code: None,
            info: Value::Null,
        }
    }

    /// Set account ID
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set account type
    pub fn with_type(mut self, account_type: impl Into<String>) -> Self {
        self.account_type = Some(account_type.into());
        self
    }
}

impl Default for Account {
    fn default() -> Self {
        Self::new()
    }
}

/// Deposit address information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositAddress {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Currency code
    pub currency: String,

    /// Network (e.g., "ERC20", "TRC20")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,

    /// Deposit address
    pub address: String,

    /// Address tag/memo (for currencies like XRP, XLM)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl DepositAddress {
    /// Create a new DepositAddress
    pub fn new(currency: impl Into<String>, address: impl Into<String>) -> Self {
        DepositAddress {
            info: Value::Null,
            currency: currency.into(),
            network: None,
            address: address.into(),
            tag: None,
        }
    }

    /// Set network
    pub fn with_network(mut self, network: impl Into<String>) -> Self {
        self.network = Some(network.into());
        self
    }

    /// Set tag
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }
}

/// Withdrawal response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawalResponse {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Withdrawal ID
    pub id: String,
}

impl WithdrawalResponse {
    /// Create a new WithdrawalResponse
    pub fn new(id: impl Into<String>) -> Self {
        WithdrawalResponse {
            info: Value::Null,
            id: id.into(),
        }
    }
}

/// Transfer entry (internal transfer between accounts)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferEntry {
    /// Raw exchange response
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<Value>,

    /// Transfer ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Currency code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,

    /// Transfer amount
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<Decimal>,

    /// Source account type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_account: Option<String>,

    /// Destination account type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_account: Option<String>,

    /// Transfer status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl TransferEntry {
    /// Create a new TransferEntry
    pub fn new() -> Self {
        TransferEntry {
            info: None,
            id: None,
            timestamp: None,
            datetime: None,
            currency: None,
            amount: None,
            from_account: None,
            to_account: None,
            status: None,
        }
    }

    /// Set transfer ID
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set currency
    pub fn with_currency(mut self, currency: impl Into<String>) -> Self {
        self.currency = Some(currency.into());
        self
    }

    /// Set amount
    pub fn with_amount(mut self, amount: Decimal) -> Self {
        self.amount = Some(amount);
        self
    }

    /// Set accounts
    pub fn with_accounts(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.from_account = Some(from.into());
        self.to_account = Some(to.into());
        self
    }

    /// Set from account
    pub fn with_from_account(mut self, from: impl Into<String>) -> Self {
        self.from_account = Some(from.into());
        self
    }

    /// Set to account
    pub fn with_to_account(mut self, to: impl Into<String>) -> Self {
        self.to_account = Some(to.into());
        self
    }

    /// Set timestamp
    pub fn with_timestamp(mut self, timestamp: i64) -> Self {
        self.timestamp = Some(timestamp);
        self.datetime =
            chrono::DateTime::from_timestamp_millis(timestamp).map(|dt| dt.to_rfc3339());
        self
    }

    /// Set status
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }
}

impl Default for TransferEntry {
    fn default() -> Self {
        Self::new()
    }
}

/// Ledger entry (account history record)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Entry ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Direction ("in" or "out")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,

    /// Account ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,

    /// Reference ID (e.g., order ID, trade ID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_id: Option<String>,

    /// Reference account
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_account: Option<String>,

    /// Entry type (e.g., "trade", "fee", "deposit", "withdrawal")
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_type: Option<String>,

    /// Currency code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,

    /// Amount
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<Decimal>,

    /// Balance before this entry
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Decimal>,

    /// Balance after this entry
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Decimal>,

    /// Status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,

    /// Fee information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee: Option<Fee>,
}

impl LedgerEntry {
    /// Create a new LedgerEntry
    pub fn new() -> Self {
        LedgerEntry {
            info: Value::Null,
            id: None,
            timestamp: None,
            datetime: None,
            direction: None,
            account: None,
            reference_id: None,
            reference_account: None,
            entry_type: None,
            currency: None,
            amount: None,
            before: None,
            after: None,
            status: None,
            fee: None,
        }
    }

    /// Set entry ID
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set entry type
    pub fn with_type(mut self, entry_type: impl Into<String>) -> Self {
        self.entry_type = Some(entry_type.into());
        self
    }

    /// Set currency
    pub fn with_currency(mut self, currency: impl Into<String>) -> Self {
        self.currency = Some(currency.into());
        self
    }

    /// Set amount
    pub fn with_amount(mut self, amount: Decimal) -> Self {
        self.amount = Some(amount);
        self
    }
}

impl Default for LedgerEntry {
    fn default() -> Self {
        Self::new()
    }
}

// Note: DepositWithdrawFee is defined in fee.rs with a more complete implementation

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_account() {
        let account = Account::new().with_id("12345").with_type("spot");

        assert_eq!(account.id, Some("12345".to_string()));
        assert_eq!(account.account_type, Some("spot".to_string()));
    }

    #[test]
    fn test_deposit_address() {
        let address = DepositAddress::new("ETH", "0x123...").with_network("ERC20");

        assert_eq!(address.currency, "ETH");
        assert_eq!(address.address, "0x123...");
        assert_eq!(address.network, Some("ERC20".to_string()));
    }

    #[test]
    fn test_transfer_entry() {
        let transfer = TransferEntry::new()
            .with_id("transfer123")
            .with_currency("USDT")
            .with_amount(dec!(1000))
            .with_accounts("spot", "futures");

        assert_eq!(transfer.id, Some("transfer123".to_string()));
        assert_eq!(transfer.currency, Some("USDT".to_string()));
        assert_eq!(transfer.amount, Some(dec!(1000)));
        assert_eq!(transfer.from_account, Some("spot".to_string()));
        assert_eq!(transfer.to_account, Some("futures".to_string()));
    }

    #[test]
    fn test_ledger_entry() {
        let entry = LedgerEntry::new()
            .with_id("ledger123")
            .with_type("trade")
            .with_currency("BTC")
            .with_amount(dec!(0.5));

        assert_eq!(entry.id, Some("ledger123".to_string()));
        assert_eq!(entry.entry_type, Some("trade".to_string()));
        assert_eq!(entry.currency, Some("BTC".to_string()));
        assert_eq!(entry.amount, Some(dec!(0.5)));
    }
}
