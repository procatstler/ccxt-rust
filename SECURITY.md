# Security Policy

## Supported Versions

Currently supported versions with security updates:

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

We take security vulnerabilities seriously. If you discover a security issue, please report it responsibly.

### How to Report

1. **DO NOT** open a public GitHub issue for security vulnerabilities
2. Email your findings to the maintainers (see repository contact information)
3. Or use GitHub's private vulnerability reporting feature

### What to Include

Please provide:
- Description of the vulnerability
- Steps to reproduce
- Potential impact assessment
- Any suggested fixes (if applicable)

### Response Timeline

- **Initial Response**: Within 48 hours
- **Status Update**: Within 7 days
- **Resolution Target**: Within 30 days (depending on severity)

### Severity Levels

| Level | Description | Example |
|-------|-------------|---------|
| Critical | Immediate threat to user funds or data | Private key exposure, authentication bypass |
| High | Significant security impact | API credential leakage, injection vulnerabilities |
| Medium | Moderate security impact | Information disclosure, rate limit bypass |
| Low | Minor security impact | Non-sensitive information exposure |

## Security Best Practices

When using ccxt-rust:

### API Credentials

```rust
// DO: Use environment variables
let api_key = std::env::var("API_KEY").expect("API_KEY required");
let secret = std::env::var("API_SECRET").expect("API_SECRET required");

let config = ExchangeConfig::default()
    .with_api_key(&api_key)
    .with_secret(&secret);

// DON'T: Hardcode credentials
// let config = ExchangeConfig::default()
//     .with_api_key("hardcoded-key")  // NEVER do this
//     .with_secret("hardcoded-secret");
```

### Sandbox/Testnet

```rust
// Use sandbox mode for development
let config = ExchangeConfig::default()
    .with_sandbox(true);  // Use testnet APIs
```

### Rate Limiting

- Respect exchange rate limits
- Implement exponential backoff for retries
- Monitor for 429 (Too Many Requests) responses

### Network Security

- All connections use HTTPS/WSS
- TLS certificates are verified by default
- Consider using VPN for additional security

## Known Security Considerations

### Exchange API Risks

- API keys may have withdrawal permissions - use read-only keys when possible
- IP whitelisting is recommended for trading keys
- Enable 2FA on exchange accounts

### WebSocket Connections

- WebSocket connections are encrypted (WSS)
- Authentication tokens expire - handle reconnection appropriately
- Private channels require proper authentication

### DEX-Specific Risks

- Private key management is critical for DEX operations
- Never expose mnemonic phrases or private keys
- Use hardware wallets when possible for signing

## Audit Status

This project has not undergone a formal security audit. Users should:
- Review the code before use in production
- Start with small amounts for testing
- Monitor transactions and positions actively

## Responsible Disclosure

We follow responsible disclosure practices:
- Reporters will be credited (unless anonymity is requested)
- We aim to fix vulnerabilities before public disclosure
- Coordinated disclosure timeline will be agreed upon

## Contact

For security concerns, please use:
- GitHub Security Advisories (preferred)
- Repository issue tracker (for non-sensitive security improvements)

Thank you for helping keep ccxt-rust secure!
