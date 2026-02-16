//! WASM Exchange Configuration

use wasm_bindgen::prelude::*;

/// Exchange configuration for WASM
#[wasm_bindgen]
pub struct WasmExchangeConfig {
    pub(crate) api_key: Option<String>,
    pub(crate) api_secret: Option<String>,
    pub(crate) password: Option<String>,
    pub(crate) uid: Option<String>,
    pub(crate) sandbox: bool,
    pub(crate) timeout_ms: u64,
}

#[wasm_bindgen]
impl WasmExchangeConfig {
    /// Create new configuration
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            api_key: None,
            api_secret: None,
            password: None,
            uid: None,
            sandbox: false,
            timeout_ms: 30000,
        }
    }

    /// Set API key
    #[wasm_bindgen(js_name = setApiKey)]
    pub fn set_api_key(&mut self, key: String) {
        self.api_key = Some(key);
    }

    /// Set API secret
    #[wasm_bindgen(js_name = setApiSecret)]
    pub fn set_api_secret(&mut self, secret: String) {
        self.api_secret = Some(secret);
    }

    /// Set password (for exchanges that require it)
    #[wasm_bindgen(js_name = setPassword)]
    pub fn set_password(&mut self, password: String) {
        self.password = Some(password);
    }

    /// Set UID (for exchanges that require it)
    #[wasm_bindgen(js_name = setUid)]
    pub fn set_uid(&mut self, uid: String) {
        self.uid = Some(uid);
    }

    /// Enable sandbox mode
    #[wasm_bindgen(js_name = setSandbox)]
    pub fn set_sandbox(&mut self, sandbox: bool) {
        self.sandbox = sandbox;
    }

    /// Set request timeout in milliseconds
    #[wasm_bindgen(js_name = setTimeout)]
    pub fn set_timeout(&mut self, timeout_ms: u64) {
        self.timeout_ms = timeout_ms;
    }
}

// Internal methods (not exposed to JS)
impl WasmExchangeConfig {
    /// Convert to internal ExchangeConfig
    pub fn to_internal(&self) -> crate::client::ExchangeConfig {
        let mut config = crate::client::ExchangeConfig::new()
            .with_timeout(self.timeout_ms);

        if let Some(ref key) = self.api_key {
            config = config.with_api_key(key);
        }
        if let Some(ref secret) = self.api_secret {
            config = config.with_api_secret(secret);
        }
        if let Some(ref password) = self.password {
            config = config.with_password(password);
        }
        if let Some(ref uid) = self.uid {
            config = config.with_uid(uid);
        }
        if self.sandbox {
            config = config.with_sandbox(true);
        }

        config
    }
}

impl Default for WasmExchangeConfig {
    fn default() -> Self {
        Self::new()
    }
}
