//! Token definitions for The Sniper - EXPANDED EDITION v2
//!
//! Now includes:
//! - Original base tokens (WETH, USDC, USDT, DAI, WBTC)
//! - Sky Ecosystem (USDS, sUSDS)
//! - USD3 + basket components
//! - Curve crvUSD
//! - GHO (Aave)
//! - DOLA (Inverse)
//! - FRAX ecosystem
//! - pyUSD (PayPal)

use alloy_primitives::Address;
use std::str::FromStr;
use std::collections::HashMap;

/// Represents a token we're tracking
#[derive(Debug, Clone)]
pub struct Token {
    pub symbol: &'static str,
    pub address: Address,
    pub decimals: u8,
    pub is_base: bool,
    pub category: TokenCategory,
}

/// Token categories for filtering and analysis
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenCategory {
    /// Primary base tokens (WETH, USDC, USDT, DAI)
    BaseStable,
    BaseVolatile,
    
    /// Yield-bearing tokens (ERC-4626 and similar)
    YieldBearing,
    
    /// Algorithmic stablecoins (crvUSD, FRAX, GHO, DOLA)
    AlgoStable,
    
    /// Basket-backed tokens (USD3)
    BasketBacked,
    
    /// Liquid staking derivatives
    LiquidStaking,
    
    /// Governance tokens
    Governance,
    
    /// Meme/volatile
    Meme,
    
    /// DeFi blue chips
    DeFi,
}

// ============================================
// BASE TOKENS (High Liquidity Starting Points)
// ============================================

pub fn base_tokens() -> Vec<Token> {
    vec![
        Token {
            symbol: "WETH",
            address: Address::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap(),
            decimals: 18,
            is_base: true,
            category: TokenCategory::BaseVolatile,
        },
        Token {
            symbol: "USDC",
            address: Address::from_str("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap(),
            decimals: 6,
            is_base: true,
            category: TokenCategory::BaseStable,
        },
        Token {
            symbol: "USDT",
            address: Address::from_str("0xdAC17F958D2ee523a2206206994597C13D831ec7").unwrap(),
            decimals: 6,
            is_base: true,
            category: TokenCategory::BaseStable,
        },
        Token {
            symbol: "DAI",
            address: Address::from_str("0x6B175474E89094C44Da98b954EedcdeCB5BE3830").unwrap(),
            decimals: 18,
            is_base: true,
            category: TokenCategory::BaseStable,
        },
        Token {
            symbol: "WBTC",
            address: Address::from_str("0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599").unwrap(),
            decimals: 8,
            is_base: true,
            category: TokenCategory::BaseVolatile,
        },
    ]
}

// ============================================
// SKY ECOSYSTEM TOKENS (Phase 2 - NEW!)
// ============================================

pub fn sky_ecosystem_tokens() -> Vec<Token> {
    vec![
        Token {
            symbol: "USDS",
            address: Address::from_str("0xdC035D45d973E3EC169d2276DDab16f1e407384F").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::BaseStable,
        },
        Token {
            symbol: "sUSDS",
            address: Address::from_str("0xa3931d71877C0E7a3148CB7Eb4463524FEc27fbD").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        Token {
            symbol: "sDAI",
            address: Address::from_str("0x83F20F44975D03b1b09e64809B757c47f942BEeA").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        Token {
            symbol: "SKY",
            address: Address::from_str("0x56072C95FAA701256059aa122697B133aDEd9279").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Governance,
        },
    ]
}

// ============================================
// USD3 / RESERVE PROTOCOL TOKENS (Phase 3 - NEW!)
// ============================================

pub fn usd3_ecosystem_tokens() -> Vec<Token> {
    vec![
        Token {
            symbol: "USD3",
            address: Address::from_str("0x0d86883faf4ffd7aeb116390af37746f45b6f378").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::BasketBacked,
        },
        Token {
            symbol: "pyUSD",
            address: Address::from_str("0x6c3ea9036406852006290770BEdFcAbA0e23A0e8").unwrap(),
            decimals: 6,
            is_base: false,
            category: TokenCategory::BaseStable,
        },
        // cUSDC (Compound V3) - part of USD3 basket
        Token {
            symbol: "cUSDC",
            address: Address::from_str("0xc3d688B66703497DAA19211EEdff47f25384cdc3").unwrap(),
            decimals: 6,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
    ]
}

// ============================================
// ALGORITHMIC STABLECOINS (HIGH PRIORITY)
// ============================================

pub fn algo_stable_tokens() -> Vec<Token> {
    vec![
        // Curve's stablecoin - pegkeeper dynamics create spreads
        Token {
            symbol: "crvUSD",
            address: Address::from_str("0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::AlgoStable,
        },
        // Savings crvUSD (yield-bearing)
        Token {
            symbol: "scrvUSD",
            address: Address::from_str("0x0655977FEb2f289A4aB78af67BAB0d17aAb84367").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        // FRAX - algorithmic stablecoin
        Token {
            symbol: "FRAX",
            address: Address::from_str("0x853d955aCEf822Db058eb8505911ED77F175b99e").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::AlgoStable,
        },
        // sFRAX - staked FRAX
        Token {
            symbol: "sFRAX",
            address: Address::from_str("0xA663B02CF0a4b149d2aD41910CB81e23e1c41c32").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        // FXS - FRAX governance
        Token {
            symbol: "FXS",
            address: Address::from_str("0x3432B6A60D23Ca0dFCa7761B7ab56459D9C964D0").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Governance,
        },
        // GHO - Aave stablecoin
        Token {
            symbol: "GHO",
            address: Address::from_str("0x40D16FC0246aD3160Ccc09B8D0D3A2cD28aE6C2f").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::AlgoStable,
        },
        // DOLA - Inverse Finance
        Token {
            symbol: "DOLA",
            address: Address::from_str("0x865377367054516e17014CcdED1e7d814EDC9ce4").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::AlgoStable,
        },
    ]
}

// ============================================
// LIQUID STAKING DERIVATIVES
// ============================================

pub fn lsd_tokens() -> Vec<Token> {
    vec![
        Token {
            symbol: "wstETH",
            address: Address::from_str("0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::LiquidStaking,
        },
        Token {
            symbol: "stETH",
            address: Address::from_str("0xae7ab96520DE3A18E5e111B5EaAb095312D7fE84").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::LiquidStaking,
        },
        Token {
            symbol: "rETH",
            address: Address::from_str("0xae78736Cd615f374D3085123A210448E74Fc6393").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::LiquidStaking,
        },
        Token {
            symbol: "cbETH",
            address: Address::from_str("0xBe9895146f7AF43049ca1c1AE358B0541Ea49704").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::LiquidStaking,
        },
    ]
}

// ============================================
// DEFI BLUE CHIPS
// ============================================

pub fn defi_tokens() -> Vec<Token> {
    vec![
        Token {
            symbol: "LINK",
            address: Address::from_str("0x514910771AF9Ca656af840dff83E8264EcF986CA").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::DeFi,
        },
        Token {
            symbol: "UNI",
            address: Address::from_str("0x1f9840a85d5aF5bf1D1762F925BDADdC4201F984").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Governance,
        },
        Token {
            symbol: "AAVE",
            address: Address::from_str("0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DDaE9").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Governance,
        },
        Token {
            symbol: "MKR",
            address: Address::from_str("0x9f8F72aA9304c8B593d555F12eF6589cC3A579A2").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Governance,
        },
        Token {
            symbol: "LDO",
            address: Address::from_str("0x5A98FcBEA516Cf06857215779Fd812CA3beF1B32").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Governance,
        },
        Token {
            symbol: "CRV",
            address: Address::from_str("0xD533a949740bb3306d119CC777fa900bA034cd52").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Governance,
        },
        Token {
            symbol: "CVX",
            address: Address::from_str("0x4e3FBD56CD56c3e72c1403e103b45Db9da5B9D2B").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Governance,
        },
    ]
}

// ============================================
// MEME COINS (High Volatility)
// ============================================

pub fn meme_tokens() -> Vec<Token> {
    vec![
        Token {
            symbol: "PEPE",
            address: Address::from_str("0x6982508145454Ce325dDbE47a25d4ec3d2311933").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Meme,
        },
        Token {
            symbol: "SHIB",
            address: Address::from_str("0x95aD61b0a150d79219dCF64E1E6Cc01f0B64C4cE").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Meme,
        },
    ]
}

// ============================================
// AGGREGATION FUNCTIONS
// ============================================

/// Get all tokens (base + all categories)
pub fn all_tokens() -> Vec<Token> {
    let mut tokens = base_tokens();
    tokens.extend(sky_ecosystem_tokens());
    tokens.extend(usd3_ecosystem_tokens());
    tokens.extend(algo_stable_tokens());
    tokens.extend(lsd_tokens());
    tokens.extend(defi_tokens());
    tokens.extend(meme_tokens());
    tokens
}

/// Get tokens by category
pub fn tokens_by_category(category: TokenCategory) -> Vec<Token> {
    all_tokens().into_iter()
        .filter(|t| t.category == category)
        .collect()
}

/// Get all stablecoin tokens (for stablecoin arbitrage focus)
pub fn all_stablecoins() -> Vec<Token> {
    all_tokens().into_iter()
        .filter(|t| matches!(t.category, 
            TokenCategory::BaseStable | 
            TokenCategory::AlgoStable | 
            TokenCategory::BasketBacked
        ))
        .collect()
}

/// Get all yield-bearing tokens (for yield drift arbitrage)
pub fn all_yield_bearing_tokens() -> Vec<Token> {
    all_tokens().into_iter()
        .filter(|t| t.category == TokenCategory::YieldBearing)
        .collect()
}

/// Get all base token addresses (for cycle start points)
pub fn base_token_addresses() -> Vec<Address> {
    base_tokens().into_iter().map(|t| t.address).collect()
}

/// Get EXPANDED base token addresses (includes stablecoins for more cycles)
pub fn expanded_base_addresses() -> Vec<Address> {
    let mut addresses = base_token_addresses();
    
    // Add major stablecoins as potential starting points
    addresses.push(Address::from_str("0xdC035D45d973E3EC169d2276DDab16f1e407384F").unwrap()); // USDS
    addresses.push(Address::from_str("0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E").unwrap()); // crvUSD
    addresses.push(Address::from_str("0x853d955aCEf822Db058eb8505911ED77F175b99e").unwrap()); // FRAX
    
    addresses
}

/// Get all token addresses
pub fn all_token_addresses() -> Vec<Address> {
    all_tokens().into_iter().map(|t| t.address).collect()
}

/// Build a symbol lookup map
pub fn build_symbol_map() -> HashMap<Address, &'static str> {
    let mut map = HashMap::new();
    
    for token in all_tokens() {
        map.insert(token.address, token.symbol);
    }
    
    map
}

/// Get token by address
pub fn get_token(address: &Address) -> Option<Token> {
    all_tokens().into_iter().find(|t| t.address == *address)
}

/// Get token symbol by address
pub fn get_symbol(address: &Address) -> Option<&'static str> {
    build_symbol_map().get(address).copied()
}

/// Check if token is yield-bearing (for special handling)
pub fn is_yield_bearing(address: &Address) -> bool {
    get_token(address)
        .map(|t| t.category == TokenCategory::YieldBearing)
        .unwrap_or(false)
}

// ============================================
// STATISTICS
// ============================================

/// Print token statistics
pub fn print_token_stats() {
    let all = all_tokens();
    let base_count = all.iter().filter(|t| t.is_base).count();
    let stable_count = all.iter().filter(|t| matches!(t.category, 
        TokenCategory::BaseStable | TokenCategory::AlgoStable | TokenCategory::BasketBacked
    )).count();
    let yield_count = all.iter().filter(|t| t.category == TokenCategory::YieldBearing).count();
    
    println!("📊 Token Statistics:");
    println!("   Total tokens: {}", all.len());
    println!("   Base tokens: {}", base_count);
    println!("   Stablecoins: {}", stable_count);
    println!("   Yield-bearing: {}", yield_count);
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_all_tokens_populated() {
        let tokens = all_tokens();
        assert!(tokens.len() >= 25, "Should have at least 25 tokens");
    }
    
    #[test]
    fn test_sky_tokens_included() {
        let symbols = build_symbol_map();
        
        // USDS
        let usds = Address::from_str("0xdC035D45d973E3EC169d2276DDab16f1e407384F").unwrap();
        assert_eq!(symbols.get(&usds), Some(&"USDS"));
        
        // sUSDS
        let susds = Address::from_str("0xa3931d71877C0E7a3148CB7Eb4463524FEc27fbD").unwrap();
        assert_eq!(symbols.get(&susds), Some(&"sUSDS"));
    }
    
    #[test]
    fn test_usd3_included() {
        let symbols = build_symbol_map();
        let usd3 = Address::from_str("0x0d86883faf4ffd7aeb116390af37746f45b6f378").unwrap();
        assert_eq!(symbols.get(&usd3), Some(&"USD3"));
    }
    
    #[test]
    fn test_yield_bearing_tokens() {
        let yield_tokens = all_yield_bearing_tokens();
        assert!(yield_tokens.len() >= 4, "Should have multiple yield-bearing tokens");
        
        // sUSDS should be yield-bearing
        let susds = Address::from_str("0xa3931d71877C0E7a3148CB7Eb4463524FEc27fbD").unwrap();
        assert!(is_yield_bearing(&susds));
    }
    
    #[test]
    fn test_expanded_base_addresses() {
        let expanded = expanded_base_addresses();
        assert!(expanded.len() > 5, "Expanded base should include stablecoins");
    }
}