//! Token definitions for The Sniper
//!
//! Contains the "Base" tokens (high liquidity) that we use as starting points
//! for arbitrage detection.

use alloy::primitives::Address;
use std::str::FromStr;

/// Represents a token we're tracking
#[derive(Debug, Clone)]
pub struct Token {
    /// Token symbol (e.g., "WETH")
    pub symbol: &'static str,
    /// Contract address on Ethereum mainnet
    pub address: Address,
    /// Decimal places (most are 18, USDC/USDT are 6)
    pub decimals: u8,
    /// Is this a "base" token (high liquidity)?
    pub is_base: bool,
}

/// Base tokens - High liquidity, stable, used as starting points for arbitrage search
pub fn base_tokens() -> Vec<Token> {
    vec![
        Token {
            symbol: "WETH",
            address: Address::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap(),
            decimals: 18,
            is_base: true,
        },
        Token {
            symbol: "USDC",
            address: Address::from_str("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap(),
            decimals: 6,
            is_base: true,
        },
        Token {
            symbol: "USDT",
            address: Address::from_str("0xdAC17F958D2ee523a2206206994597C13D831ec7").unwrap(),
            decimals: 6,
            is_base: true,
        },
        Token {
            symbol: "DAI",
            address: Address::from_str("0x6B175474E89094C44Da98b954EescdeCB5BE3830").unwrap(),
            decimals: 18,
            is_base: true,
        },
        Token {
            symbol: "WBTC",
            address: Address::from_str("0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599").unwrap(),
            decimals: 8,
            is_base: true,
        },
    ]
}

/// Get all base token addresses
pub fn base_token_addresses() -> Vec<Address> {
    base_tokens().into_iter().map(|t| t.address).collect()
}

/// Get a token by address
pub fn get_token_by_address(address: &Address) -> Option<Token> {
    base_tokens().into_iter().find(|t| &t.address == address)
}

/// Get token symbol by address
pub fn get_symbol(address: &Address) -> Option<&'static str> {
    get_token_by_address(address).map(|t| t.symbol)
}