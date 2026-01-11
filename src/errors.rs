//! CCXT Error Hierarchy
//!
//! Complete port of CCXT's 40+ error types to Rust

use thiserror::Error;

/// CCXT error hierarchy
///
/// Error classes follow the same hierarchy as CCXT TypeScript:
/// - BaseError
///   - ExchangeError (exchange-specific errors)
///     - AuthenticationError
///       - PermissionDenied
///         - AccountNotEnabled
///       - AccountSuspended
///     - ArgumentsRequired
///     - BadRequest
///       - BadSymbol
///     - OperationRejected
///       - NoChange
///         - MarginModeAlreadySet
///       - MarketClosed
///       - ManualInteractionNeeded
///       - RestrictedLocation
///     - InsufficientFunds
///     - InvalidAddress
///       - AddressPending
///     - InvalidOrder
///       - OrderNotFound
///       - OrderNotCached
///       - OrderImmediatelyFillable
///       - OrderNotFillable
///       - DuplicateOrderId
///       - ContractUnavailable
///     - NotSupported
///     - InvalidProxySettings
///     - ExchangeClosedByUser
///   - OperationFailed (operation failures)
///     - NetworkError
///       - DDoSProtection
///       - RateLimitExceeded
///       - ExchangeNotAvailable
///         - OnMaintenance
///       - InvalidNonce
///         - ChecksumError
///       - RequestTimeout
///     - BadResponse
///       - NullResponse
///     - CancelPending
///   - UnsubscribeError (WebSocket specific)
#[derive(Error, Debug)]
pub enum CcxtError {
    // === ExchangeError family ===
    /// Generic exchange error
    #[error("Exchange error: {message}")]
    ExchangeError { message: String },

    /// Authentication failed (invalid API key, signature, etc.)
    #[error("Authentication error: {message}")]
    AuthenticationError { message: String },

    /// API key lacks permission for the operation
    #[error("Permission denied: {message}")]
    PermissionDenied { message: String },

    /// Account feature not enabled (e.g., futures trading not activated)
    #[error("Account not enabled: {message}")]
    AccountNotEnabled { message: String },

    /// Account is suspended
    #[error("Account suspended: {message}")]
    AccountSuspended { message: String },

    /// Required arguments missing
    #[error("Arguments required: {message}")]
    ArgumentsRequired { message: String },

    /// Invalid request parameters
    #[error("Bad request: {message}")]
    BadRequest { message: String },

    /// Invalid trading symbol
    #[error("Bad symbol: {symbol}")]
    BadSymbol { symbol: String },

    /// Operation rejected by exchange
    #[error("Operation rejected: {message}")]
    OperationRejected { message: String },

    /// No change was made (e.g., setting same value)
    #[error("No change: {message}")]
    NoChange { message: String },

    /// Margin mode already set to requested value
    #[error("Margin mode already set: {message}")]
    MarginModeAlreadySet { message: String },

    /// Market is closed for trading
    #[error("Market closed: {symbol}")]
    MarketClosed { symbol: String },

    /// Manual interaction required on exchange website
    #[error("Manual interaction needed: {message}")]
    ManualInteractionNeeded { message: String },

    /// Operation not available in user's location
    #[error("Restricted location: {message}")]
    RestrictedLocation { message: String },

    /// Not enough balance
    #[error("Insufficient funds: {currency}, required: {required}, available: {available}")]
    InsufficientFunds {
        currency: String,
        required: String,
        available: String,
    },

    /// Invalid deposit/withdrawal address
    #[error("Invalid address: {address}")]
    InvalidAddress { address: String },

    /// Address is being generated
    #[error("Address pending: {address}")]
    AddressPending { address: String },

    /// Generic invalid order error
    #[error("Invalid order: {message}")]
    InvalidOrder { message: String },

    /// Order not found on exchange
    #[error("Order not found: {order_id}")]
    OrderNotFound { order_id: String },

    /// Order not in local cache
    #[error("Order not cached: {order_id}")]
    OrderNotCached { order_id: String },

    /// Post-only order would execute immediately
    #[error("Order immediately fillable")]
    OrderImmediatelyFillable,

    /// Order cannot be filled (e.g., price too far from market)
    #[error("Order not fillable")]
    OrderNotFillable,

    /// Client order ID already used
    #[error("Duplicate order ID: {order_id}")]
    DuplicateOrderId { order_id: String },

    /// Contract not available for trading
    #[error("Contract unavailable: {symbol}")]
    ContractUnavailable { symbol: String },

    /// Feature not supported by this exchange
    #[error("Not supported: {feature}")]
    NotSupported { feature: String },

    /// Invalid proxy configuration
    #[error("Invalid proxy settings: {message}")]
    InvalidProxySettings { message: String },

    /// Exchange connection closed by user
    #[error("Exchange closed by user")]
    ExchangeClosedByUser,

    // === OperationFailed / NetworkError family ===
    /// Generic operation failed error (parent of NetworkError family)
    #[error("Operation failed: {message}")]
    OperationFailed { message: String },

    /// Generic network error
    #[error("Network error: {url} - {message}")]
    NetworkError { url: String, message: String },

    /// CloudFlare or similar DDoS protection triggered
    #[error("DDoS protection triggered: {message}")]
    DDoSProtection { message: String },

    /// Rate limit exceeded
    #[error("Rate limit exceeded: {message}")]
    RateLimitExceeded {
        message: String,
        /// Suggested retry after in milliseconds (if provided by exchange)
        retry_after_ms: Option<u64>,
    },

    /// Exchange is temporarily unavailable
    #[error("Exchange not available: {message}")]
    ExchangeNotAvailable { message: String },

    /// Request timed out
    #[error("Request timeout: {url}")]
    RequestTimeout { url: String },

    /// Exchange is under maintenance
    #[error("On maintenance: {message}")]
    OnMaintenance { message: String },

    /// Invalid nonce (request timestamp/counter issue)
    #[error("Invalid nonce: {message}")]
    InvalidNonce { message: String },

    /// Checksum validation failed (usually WebSocket)
    #[error("Checksum error: {message}")]
    ChecksumError { message: String },

    /// Invalid response from exchange
    #[error("Bad response: {message}")]
    BadResponse { message: String },

    /// Empty/null response from exchange
    #[error("Null response from: {url}")]
    NullResponse { url: String },

    /// Cancel request pending
    #[error("Cancel pending: {order_id}")]
    CancelPending { order_id: String },

    // === WebSocket specific ===
    /// WebSocket unsubscribe error
    #[error("Unsubscribe error: {message}")]
    UnsubscribeError { message: String },

    // === Parsing errors ===
    /// Failed to parse response data
    #[error("Parse error: {data_type} - {message}")]
    ParseError { data_type: String, message: String },

    /// JSON parsing error
    #[error("JSON error: {message}")]
    JsonError { message: String },

    // === Cryptographic errors ===
    /// Invalid cryptographic signature
    #[error("Invalid signature: {message}")]
    InvalidSignature { message: String },

    /// Invalid private key
    #[error("Invalid private key: {message}")]
    InvalidPrivateKey { message: String },
}

impl CcxtError {
    /// Returns the error code as a string constant
    pub fn code(&self) -> &'static str {
        match self {
            // ExchangeError family
            CcxtError::ExchangeError { .. } => "EXCHANGE_ERROR",
            CcxtError::AuthenticationError { .. } => "AUTHENTICATION_ERROR",
            CcxtError::PermissionDenied { .. } => "PERMISSION_DENIED",
            CcxtError::AccountNotEnabled { .. } => "ACCOUNT_NOT_ENABLED",
            CcxtError::AccountSuspended { .. } => "ACCOUNT_SUSPENDED",
            CcxtError::ArgumentsRequired { .. } => "ARGUMENTS_REQUIRED",
            CcxtError::BadRequest { .. } => "BAD_REQUEST",
            CcxtError::BadSymbol { .. } => "BAD_SYMBOL",
            CcxtError::OperationRejected { .. } => "OPERATION_REJECTED",
            CcxtError::NoChange { .. } => "NO_CHANGE",
            CcxtError::MarginModeAlreadySet { .. } => "MARGIN_MODE_ALREADY_SET",
            CcxtError::MarketClosed { .. } => "MARKET_CLOSED",
            CcxtError::ManualInteractionNeeded { .. } => "MANUAL_INTERACTION_NEEDED",
            CcxtError::RestrictedLocation { .. } => "RESTRICTED_LOCATION",
            CcxtError::InsufficientFunds { .. } => "INSUFFICIENT_FUNDS",
            CcxtError::InvalidAddress { .. } => "INVALID_ADDRESS",
            CcxtError::AddressPending { .. } => "ADDRESS_PENDING",
            CcxtError::InvalidOrder { .. } => "INVALID_ORDER",
            CcxtError::OrderNotFound { .. } => "ORDER_NOT_FOUND",
            CcxtError::OrderNotCached { .. } => "ORDER_NOT_CACHED",
            CcxtError::OrderImmediatelyFillable => "ORDER_IMMEDIATELY_FILLABLE",
            CcxtError::OrderNotFillable => "ORDER_NOT_FILLABLE",
            CcxtError::DuplicateOrderId { .. } => "DUPLICATE_ORDER_ID",
            CcxtError::ContractUnavailable { .. } => "CONTRACT_UNAVAILABLE",
            CcxtError::NotSupported { .. } => "NOT_SUPPORTED",
            CcxtError::InvalidProxySettings { .. } => "INVALID_PROXY_SETTINGS",
            CcxtError::ExchangeClosedByUser => "EXCHANGE_CLOSED_BY_USER",
            // OperationFailed / NetworkError family
            CcxtError::OperationFailed { .. } => "OPERATION_FAILED",
            CcxtError::NetworkError { .. } => "NETWORK_ERROR",
            CcxtError::DDoSProtection { .. } => "DDOS_PROTECTION",
            CcxtError::RateLimitExceeded { .. } => "RATE_LIMIT_EXCEEDED",
            CcxtError::ExchangeNotAvailable { .. } => "EXCHANGE_NOT_AVAILABLE",
            CcxtError::RequestTimeout { .. } => "REQUEST_TIMEOUT",
            CcxtError::OnMaintenance { .. } => "ON_MAINTENANCE",
            CcxtError::InvalidNonce { .. } => "INVALID_NONCE",
            CcxtError::ChecksumError { .. } => "CHECKSUM_ERROR",
            CcxtError::BadResponse { .. } => "BAD_RESPONSE",
            CcxtError::NullResponse { .. } => "NULL_RESPONSE",
            CcxtError::CancelPending { .. } => "CANCEL_PENDING",
            // WebSocket specific
            CcxtError::UnsubscribeError { .. } => "UNSUBSCRIBE_ERROR",
            // Parsing
            CcxtError::ParseError { .. } => "PARSE_ERROR",
            CcxtError::JsonError { .. } => "JSON_ERROR",
            // Cryptographic
            CcxtError::InvalidSignature { .. } => "INVALID_SIGNATURE",
            CcxtError::InvalidPrivateKey { .. } => "INVALID_PRIVATE_KEY",
        }
    }

    /// Returns true if this error is temporary and the operation can be retried
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            CcxtError::OperationFailed { .. }
                | CcxtError::NetworkError { .. }
                | CcxtError::RequestTimeout { .. }
                | CcxtError::RateLimitExceeded { .. }
                | CcxtError::ExchangeNotAvailable { .. }
                | CcxtError::OnMaintenance { .. }
                | CcxtError::InvalidNonce { .. }
        )
    }

    /// Returns true if this is an authentication-related error
    pub fn is_auth_error(&self) -> bool {
        matches!(
            self,
            CcxtError::AuthenticationError { .. }
                | CcxtError::PermissionDenied { .. }
                | CcxtError::AccountNotEnabled { .. }
                | CcxtError::AccountSuspended { .. }
        )
    }

    /// Returns true if this is an order-related error
    pub fn is_order_error(&self) -> bool {
        matches!(
            self,
            CcxtError::InvalidOrder { .. }
                | CcxtError::OrderNotFound { .. }
                | CcxtError::OrderNotCached { .. }
                | CcxtError::OrderImmediatelyFillable
                | CcxtError::OrderNotFillable
                | CcxtError::DuplicateOrderId { .. }
                | CcxtError::ContractUnavailable { .. }
        )
    }

    /// Returns true if this is a network-related error
    pub fn is_network_error(&self) -> bool {
        matches!(
            self,
            CcxtError::OperationFailed { .. }
                | CcxtError::NetworkError { .. }
                | CcxtError::DDoSProtection { .. }
                | CcxtError::RateLimitExceeded { .. }
                | CcxtError::ExchangeNotAvailable { .. }
                | CcxtError::RequestTimeout { .. }
                | CcxtError::OnMaintenance { .. }
                | CcxtError::InvalidNonce { .. }
                | CcxtError::ChecksumError { .. }
        )
    }

    /// Returns the suggested retry delay in milliseconds for retryable errors
    /// Returns None for non-retryable errors or when no delay is suggested
    pub fn suggested_retry_after(&self) -> Option<u64> {
        match self {
            CcxtError::RateLimitExceeded { retry_after_ms, .. } => {
                retry_after_ms.or(Some(1000)) // Default 1 second for rate limits
            },
            CcxtError::RequestTimeout { .. } => Some(5000), // 5 seconds for timeouts
            CcxtError::ExchangeNotAvailable { .. } => Some(30000), // 30 seconds
            CcxtError::OnMaintenance { .. } => Some(60000), // 1 minute for maintenance
            CcxtError::NetworkError { .. } => Some(1000),   // 1 second for network errors
            CcxtError::OperationFailed { .. } => Some(1000), // 1 second for operation failures
            CcxtError::InvalidNonce { .. } => Some(100),    // 100ms for nonce issues
            CcxtError::DDoSProtection { .. } => Some(60000), // 1 minute for DDoS protection
            _ => None,
        }
    }

    /// Creates a RateLimitExceeded error with a retry delay
    pub fn rate_limit_with_retry(message: impl Into<String>, retry_after_ms: u64) -> Self {
        CcxtError::RateLimitExceeded {
            message: message.into(),
            retry_after_ms: Some(retry_after_ms),
        }
    }

    /// Returns true if this error represents a temporary condition
    pub fn is_temporary(&self) -> bool {
        self.is_retryable()
    }

    /// Returns true if this error represents a permanent condition
    pub fn is_permanent(&self) -> bool {
        !self.is_retryable()
    }
}

// === From implementations for common error types ===

impl From<serde_json::Error> for CcxtError {
    fn from(err: serde_json::Error) -> Self {
        CcxtError::JsonError {
            message: err.to_string(),
        }
    }
}

impl From<reqwest::Error> for CcxtError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            CcxtError::RequestTimeout {
                url: err.url().map(|u| u.to_string()).unwrap_or_default(),
            }
        } else if err.is_connect() {
            CcxtError::NetworkError {
                url: err.url().map(|u| u.to_string()).unwrap_or_default(),
                message: "Connection failed".into(),
            }
        } else {
            CcxtError::NetworkError {
                url: err.url().map(|u| u.to_string()).unwrap_or_default(),
                message: err.to_string(),
            }
        }
    }
}

impl From<std::io::Error> for CcxtError {
    fn from(err: std::io::Error) -> Self {
        CcxtError::NetworkError {
            url: String::new(),
            message: err.to_string(),
        }
    }
}

/// Result 타입 alias
pub type CcxtResult<T> = Result<T, CcxtError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes() {
        let err = CcxtError::AuthenticationError {
            message: "Invalid API key".into(),
        };
        assert_eq!(err.code(), "AUTHENTICATION_ERROR");

        let err = CcxtError::MarginModeAlreadySet {
            message: "Already set to isolated".into(),
        };
        assert_eq!(err.code(), "MARGIN_MODE_ALREADY_SET");

        let err = CcxtError::InvalidNonce {
            message: "Nonce too small".into(),
        };
        assert_eq!(err.code(), "INVALID_NONCE");
    }

    #[test]
    fn test_retryable_errors() {
        let network_err = CcxtError::NetworkError {
            url: "https://api.example.com".into(),
            message: "Connection refused".into(),
        };
        assert!(network_err.is_retryable());

        let timeout_err = CcxtError::RequestTimeout {
            url: "https://api.example.com".into(),
        };
        assert!(timeout_err.is_retryable());

        let maintenance_err = CcxtError::OnMaintenance {
            message: "System upgrade".into(),
        };
        assert!(maintenance_err.is_retryable());

        let auth_err = CcxtError::AuthenticationError {
            message: "Invalid key".into(),
        };
        assert!(!auth_err.is_retryable());
    }

    #[test]
    fn test_insufficient_funds() {
        let err = CcxtError::InsufficientFunds {
            currency: "KRW".into(),
            required: "1000000".into(),
            available: "500000".into(),
        };
        assert_eq!(err.code(), "INSUFFICIENT_FUNDS");
        assert!(err.to_string().contains("KRW"));
    }

    #[test]
    fn test_is_auth_error() {
        let auth_err = CcxtError::AuthenticationError {
            message: "Invalid key".into(),
        };
        assert!(auth_err.is_auth_error());

        let perm_err = CcxtError::PermissionDenied {
            message: "No permission".into(),
        };
        assert!(perm_err.is_auth_error());

        let network_err = CcxtError::NetworkError {
            url: "https://api.example.com".into(),
            message: "Connection refused".into(),
        };
        assert!(!network_err.is_auth_error());
    }

    #[test]
    fn test_is_order_error() {
        let order_err = CcxtError::OrderNotFound {
            order_id: "12345".into(),
        };
        assert!(order_err.is_order_error());

        let fillable_err = CcxtError::OrderImmediatelyFillable;
        assert!(fillable_err.is_order_error());

        let auth_err = CcxtError::AuthenticationError {
            message: "Invalid key".into(),
        };
        assert!(!auth_err.is_order_error());
    }

    #[test]
    fn test_is_network_error() {
        let network_err = CcxtError::NetworkError {
            url: "https://api.example.com".into(),
            message: "Connection refused".into(),
        };
        assert!(network_err.is_network_error());

        let ddos_err = CcxtError::DDoSProtection {
            message: "CloudFlare protection".into(),
        };
        assert!(ddos_err.is_network_error());

        let checksum_err = CcxtError::ChecksumError {
            message: "Checksum mismatch".into(),
        };
        assert!(checksum_err.is_network_error());

        let order_err = CcxtError::OrderNotFound {
            order_id: "12345".into(),
        };
        assert!(!order_err.is_network_error());
    }

    #[test]
    fn test_new_error_types() {
        // Test new error types added from CCXT reference
        let err = CcxtError::ManualInteractionNeeded {
            message: "Please verify on website".into(),
        };
        assert_eq!(err.code(), "MANUAL_INTERACTION_NEEDED");

        let err = CcxtError::RestrictedLocation {
            message: "Service not available in your region".into(),
        };
        assert_eq!(err.code(), "RESTRICTED_LOCATION");

        let err = CcxtError::ExchangeClosedByUser;
        assert_eq!(err.code(), "EXCHANGE_CLOSED_BY_USER");

        let err = CcxtError::CancelPending {
            order_id: "order123".into(),
        };
        assert_eq!(err.code(), "CANCEL_PENDING");

        let err = CcxtError::UnsubscribeError {
            message: "Failed to unsubscribe".into(),
        };
        assert_eq!(err.code(), "UNSUBSCRIBE_ERROR");
    }

    #[test]
    fn test_operation_failed() {
        let err = CcxtError::OperationFailed {
            message: "Operation could not complete".into(),
        };
        assert_eq!(err.code(), "OPERATION_FAILED");
        assert!(err.is_retryable());
        assert!(err.is_network_error());
        assert_eq!(err.suggested_retry_after(), Some(1000));
    }
}
