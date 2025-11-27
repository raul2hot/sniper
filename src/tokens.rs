//! Token definitions for The Sniper - EXPANDED Edition
//!
//! Includes 30+ tokens for comprehensive arbitrage detection

use alloy_primitives::Address;
use std::str::FromStr;

/// Represents a token we're tracking
#[derive(Debug, Clone)]
pub struct Token {
    pub symbol: &'static str,
    pub address: Address,
    pub decimals: u8,
    pub is_base: bool,
}

/// Base tokens - High liquidity, used as starting points for arbitrage search
/// These should be the most liquid tokens to start/end arbitrage cycles
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
            address: Address::from_str("0x6B175474E89094C44Da98b954EedcdeCB5BE3830").unwrap(),
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

/// Long-tail tokens - Higher volatility, more arbitrage opportunities
/// These are intermediate tokens in the arbitrage path
pub fn longtail_tokens() -> Vec<Token> {
    vec![
        // === Liquid Staking ===
        Token {
            symbol: "wstETH",
            address: Address::from_str("0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "stETH",
            address: Address::from_str("0xae7ab96520DE3A18E5e111B5EaAb095312D7fE84").unwrap(),
            decimals: 18,
            is_base: false,
        },
        
        // === Meme Coins (HIGH VOLATILITY = OPPORTUNITY) ===
        Token {
            symbol: "PEPE",
            address: Address::from_str("0x6982508145454Ce325dDbE47a25d4ec3d2311933").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "SHIB",
            address: Address::from_str("0x95aD61b0a150d79219dCF64E1E6Cc01f0B64C4cE").unwrap(),
            decimals: 18,
            is_base: false,
        },
        
        // === DeFi Blue Chips ===
        Token {
            symbol: "LINK",
            address: Address::from_str("0x514910771AF9Ca656af840dff83E8264EcF986CA").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "UNI",
            address: Address::from_str("0x1f9840a85d5aF5bf1D1762F925BDADdC4201F984").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "AAVE",
            address: Address::from_str("0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DDaE9").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "MKR",
            address: Address::from_str("0x9f8F72aA9304c8B593d555F12eF6589cC3A579A2").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "LDO",
            address: Address::from_str("0x5A98FcBEA516Cf06857215779Fd812CA3beF1B32").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "CRV",
            address: Address::from_str("0xD533a949740bb3306d119CC777fa900bA034cd52").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "SNX",
            address: Address::from_str("0xC011a73ee8576Fb46F5E1c5751cA3B9Fe0af2a6F").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "COMP",
            address: Address::from_str("0xc00e94Cb662C3520282E6f5717214004A7f26888").unwrap(),
            decimals: 18,
            is_base: false,
        },
        
        // === Governance/DAO Tokens ===
        Token {
            symbol: "ENS",
            address: Address::from_str("0xC18360217D8F7Ab5e7c516566761Ea12Ce7F9D72").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "RPL",
            address: Address::from_str("0xD33526068D116cE69F19A9ee46F0bd304F21A51f").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "APE",
            address: Address::from_str("0x4d224452801ACEd8B2F0aebE155379bb5D594381").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "BLUR",
            address: Address::from_str("0x5283D291DBCF85356A21bA090E6db59121208b44").unwrap(),
            decimals: 18,
            is_base: false,
        },
        
        // === DEX Tokens ===
        Token {
            symbol: "SUSHI",
            address: Address::from_str("0x6B3595068778DD592e39A122f4f5a5cF09C90fE2").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "1INCH",
            address: Address::from_str("0x111111111117dC0aa78b770fA6A738034120C302").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "BAL",
            address: Address::from_str("0xba100000625a3754423978a60c9317c58a424e3D").unwrap(),
            decimals: 18,
            is_base: false,
        },
        
        // === Yield/Convex Ecosystem ===
        Token {
            symbol: "CVX",
            address: Address::from_str("0x4e3FBD56CD56c3e72c1403e103b45Db9da5B9D2B").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "YFI",
            address: Address::from_str("0x0bc529c00C6401aEF6D220BE8C6Ea1667F6Ad93e").unwrap(),
            decimals: 18,
            is_base: false,
        },
        
        // === L2/Scaling ===
        Token {
            symbol: "MATIC",
            address: Address::from_str("0x7D1AfA7B718fb893dB30A3aBc0Cfc608AaCfeBB0").unwrap(),
            decimals: 18,
            is_base: false,
        },
        
        // === Stablecoins (Alternative) ===
        Token {
            symbol: "FRAX",
            address: Address::from_str("0x853d955aCEf822Db058eb8505911ED77F175b99e").unwrap(),
            decimals: 18,
            is_base: false,
        },
        Token {
            symbol: "FXS",
            address: Address::from_str("0x3432B6A60D23Ca0dFCa7761B7ab56459D9C964D0").unwrap(),
            decimals: 18,
            is_base: false,
        },
        
        // === Real World Assets ===
        Token {
            symbol: "PAXG",
            address: Address::from_str("0x45804880De22913dAFE09f4980848ECE6EcbAf78").unwrap(),
            decimals: 18,
            is_base: false,
        },
        
        // === Metaverse/Gaming ===
        Token {
            symbol: "MANA",
            address: Address::from_str("0x0F5D2fB29fb7d3CFeE444a200298f468908cC942").unwrap(),
            decimals: 18,
            is_base: false,
        },
    ]
}

/// Get all tokens (base + longtail)
pub fn all_tokens() -> Vec<Token> {
    let mut tokens = base_tokens();
    tokens.extend(longtail_tokens());
    tokens
}

/// Get all base token addresses
pub fn base_token_addresses() -> Vec<Address> {
    base_tokens().into_iter().map(|t| t.address).collect()
}

/// Get all token addresses
pub fn all_token_addresses() -> Vec<Address> {
    all_tokens().into_iter().map(|t| t.address).collect()
}

/// Build a symbol lookup map
pub fn build_symbol_map() -> std::collections::HashMap<Address, &'static str> {
    let mut map = std::collections::HashMap::new();
    
    for token in all_tokens() {
        map.insert(token.address, token.symbol);
    }
    
    map
}