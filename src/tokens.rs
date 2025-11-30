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

    /// AI/Compute tokens (RNDR, FET, AGIX, TAO)
    AICompute,

    /// Gaming/Metaverse tokens (IMX, GALA, SAND, AXS)
    Gaming,

    /// Restaking tokens (EIGEN, pufETH, ezETH, weETH)
    Restaking,

    /// Real World Asset tokens (ONDO, USDY, OUSG)
    RWA,
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
        Token {
            symbol: "MOG",
            address: Address::from_str("0xaaee1a9723aadb7afa2810263653a34ba2c21c7a").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Meme,
        },
        Token {
            symbol: "SPX6900",
            address: Address::from_str("0xe0f63a424a4439cbe457d80e4f4b51ad25b2c56c").unwrap(),
            decimals: 8,
            is_base: false,
            category: TokenCategory::Meme,
        },
        Token {
            symbol: "TURBO",
            address: Address::from_str("0xa35923162c49cf95e6bf26623385eb431ad920d3").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Meme,
        },
        Token {
            symbol: "FLOKI",
            address: Address::from_str("0xcf0c122c6b73ff809c693db761e7baebe62b6a2e").unwrap(),
            decimals: 9,
            is_base: false,
            category: TokenCategory::Meme,
        },
    ]
}

// ============================================
// AI/COMPUTE TOKENS (High Volatility - Catalyst Driven)
// ============================================

pub fn ai_compute_tokens() -> Vec<Token> {
    vec![
        Token {
            symbol: "RNDR",
            address: Address::from_str("0x6de037ef9ad2725eb40118bb1702ebb27e4aeb24").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::AICompute,
        },
        Token {
            symbol: "FET",
            address: Address::from_str("0xaea46A60368A7bD060eec7DF8CBa43b7EF41Ad85").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::AICompute,
        },
        Token {
            symbol: "AGIX",
            address: Address::from_str("0x5B7533812759B45C2B44C19e320ba2cD2681b542").unwrap(),
            decimals: 8,
            is_base: false,
            category: TokenCategory::AICompute,
        },
        Token {
            symbol: "wTAO",
            address: Address::from_str("0x77e06c9eccf2e797fd462a92b6d7642ef85b0a44").unwrap(),
            decimals: 9,
            is_base: false,
            category: TokenCategory::AICompute,
        },
        // Staked TAO - yield-drift arbitrage opportunity vs wTAO
        Token {
            symbol: "stTAO",
            address: Address::from_str("0xb60acd2057067dc9ed8c083f5aa227a244044fd6").unwrap(),
            decimals: 9,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
    ]
}

// ============================================
// GAMING/METAVERSE TOKENS
// ============================================

pub fn gaming_tokens() -> Vec<Token> {
    vec![
        Token {
            symbol: "IMX",
            address: Address::from_str("0xf57e7e7c23978c3caec3c3548e3d615c346e79ff").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Gaming,
        },
        Token {
            symbol: "GALA",
            address: Address::from_str("0xd1d2eb1b1e90b638588728b4130137d262c87cae").unwrap(),
            decimals: 8,
            is_base: false,
            category: TokenCategory::Gaming,
        },
        Token {
            symbol: "SAND",
            address: Address::from_str("0x3845badAde8e6dFF049820680d1F14bD3903a5d0").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Gaming,
        },
        Token {
            symbol: "AXS",
            address: Address::from_str("0xbb0e17ef65f82ab018d8edd776e8dd940327b28b").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Gaming,
        },
    ]
}

// ============================================
// RESTAKING TOKENS (NAV Discount Arbitrage)
// ============================================

pub fn restaking_tokens() -> Vec<Token> {
    vec![
        // Governance tokens
        Token {
            symbol: "EIGEN",
            address: Address::from_str("0xec53bF9167f50cDEB3Ae105f56099aaaB9061F83").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Restaking,
        },
        Token {
            symbol: "REZ",
            address: Address::from_str("0x3B50805453023a91a8bf641e279401a0b23FA6F9").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Restaking,
        },
        Token {
            symbol: "PUFFER",
            address: Address::from_str("0x4d1C297d39C5c1277964D0E3f8Aa901493664530").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::Restaking,
        },
        // Liquid Restaking Tokens (LRTs) - yield-bearing
        Token {
            symbol: "pufETH",
            address: Address::from_str("0xD9A442856C234a39a81a089C06451EBAa4306a72").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        Token {
            symbol: "ezETH",
            address: Address::from_str("0xbf5495Efe5DB9ce00f80364C8B423567e58d2110").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        Token {
            symbol: "weETH",
            address: Address::from_str("0xCd5fE23C85820F7B72D0926FC9b05b43E359b7ee").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        Token {
            symbol: "eETH",
            address: Address::from_str("0x35fA164735182de50811E8e2E824cFb9B6118ac2").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::LiquidStaking,
        },
    ]
}

// ============================================
// RWA (Real World Asset) TOKENS
// ============================================

pub fn rwa_tokens() -> Vec<Token> {
    vec![
        // Governance
        Token {
            symbol: "ONDO",
            address: Address::from_str("0xfAbA6f8e4a5E8Ab82F62fe7C39859FA577269BE3").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::RWA,
        },
        Token {
            symbol: "CFG",
            address: Address::from_str("0xc221b7e65ffc80de234bbb6667abdd46593d34f0").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::RWA,
        },
        Token {
            symbol: "SYRUP",
            address: Address::from_str("0x643C4E15d7d62Ad0aBeC4a9BD4b001aA3Ef52d66").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::RWA,
        },
        // Yield-bearing RWA tokens
        Token {
            symbol: "USDY",
            address: Address::from_str("0x96F6eF951840721AdBF46Ac996b59E0235CB985C").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        Token {
            symbol: "OUSG",
            address: Address::from_str("0x1B19C19393e2d034D8Ff31ff34c81252FcBbee92").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
        },
        Token {
            symbol: "rOUSG",
            address: Address::from_str("0xaf37c1167910ebC994e266949387d2c7C326b879").unwrap(),
            decimals: 18,
            is_base: false,
            category: TokenCategory::YieldBearing,
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
    // HIGH VOLATILITY ADDITIONS
    tokens.extend(ai_compute_tokens());
    tokens.extend(gaming_tokens());
    tokens.extend(restaking_tokens());
    tokens.extend(rwa_tokens());
    tokens
}

/// Get all AI/Compute tokens
pub fn all_ai_tokens() -> Vec<Token> {
    all_tokens().into_iter()
        .filter(|t| t.category == TokenCategory::AICompute)
        .collect()
}

/// Get all gaming tokens
pub fn all_gaming_tokens() -> Vec<Token> {
    all_tokens().into_iter()
        .filter(|t| t.category == TokenCategory::Gaming)
        .collect()
}

/// Get all restaking tokens (including LRTs)
pub fn all_restaking_tokens() -> Vec<Token> {
    all_tokens().into_iter()
        .filter(|t| t.category == TokenCategory::Restaking)
        .collect()
}

/// Get all RWA tokens
pub fn all_rwa_tokens() -> Vec<Token> {
    all_tokens().into_iter()
        .filter(|t| t.category == TokenCategory::RWA)
        .collect()
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
        assert!(tokens.len() >= 50, "Should have at least 50 tokens after expansion");
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

    #[test]
    fn test_ai_compute_tokens() {
        let tokens = ai_compute_tokens();
        assert!(tokens.len() >= 5, "Should have RNDR, FET, AGIX, wTAO, stTAO");

        // Verify decimals
        let wtao = tokens.iter().find(|t| t.symbol == "wTAO").unwrap();
        assert_eq!(wtao.decimals, 9);

        // Verify RNDR is AICompute category
        let rndr = tokens.iter().find(|t| t.symbol == "RNDR").unwrap();
        assert_eq!(rndr.category, TokenCategory::AICompute);
    }

    #[test]
    fn test_gaming_tokens() {
        let tokens = gaming_tokens();
        assert!(tokens.len() >= 4, "Should have IMX, GALA, SAND, AXS");

        // Verify GALA decimals
        let gala = tokens.iter().find(|t| t.symbol == "GALA").unwrap();
        assert_eq!(gala.decimals, 8);
    }

    #[test]
    fn test_restaking_tokens() {
        let tokens = restaking_tokens();
        assert!(tokens.len() >= 7, "Should have EIGEN, REZ, PUFFER, pufETH, ezETH, weETH, eETH");

        // pufETH should be yield-bearing
        let pufeth = tokens.iter().find(|t| t.symbol == "pufETH").unwrap();
        assert_eq!(pufeth.category, TokenCategory::YieldBearing);

        // EIGEN should be Restaking category
        let eigen = tokens.iter().find(|t| t.symbol == "EIGEN").unwrap();
        assert_eq!(eigen.category, TokenCategory::Restaking);
    }

    #[test]
    fn test_rwa_tokens() {
        let tokens = rwa_tokens();
        assert!(tokens.len() >= 6, "Should have ONDO, CFG, SYRUP, USDY, OUSG, rOUSG");

        // USDY should be yield-bearing
        let usdy = tokens.iter().find(|t| t.symbol == "USDY").unwrap();
        assert_eq!(usdy.category, TokenCategory::YieldBearing);

        // ONDO should be RWA category
        let ondo = tokens.iter().find(|t| t.symbol == "ONDO").unwrap();
        assert_eq!(ondo.category, TokenCategory::RWA);
    }

    #[test]
    fn test_meme_tokens_expanded() {
        let tokens = meme_tokens();
        assert!(tokens.len() >= 6, "Should have PEPE, SHIB, MOG, SPX6900, TURBO, FLOKI");

        // SPX6900 should have 8 decimals
        let spx = tokens.iter().find(|t| t.symbol == "SPX6900").unwrap();
        assert_eq!(spx.decimals, 8);

        // FLOKI should have 9 decimals
        let floki = tokens.iter().find(|t| t.symbol == "FLOKI").unwrap();
        assert_eq!(floki.decimals, 9);
    }

    #[test]
    fn test_new_tokens_in_symbol_map() {
        let symbols = build_symbol_map();

        // AI tokens
        let rndr = Address::from_str("0x6de037ef9ad2725eb40118bb1702ebb27e4aeb24").unwrap();
        assert_eq!(symbols.get(&rndr), Some(&"RNDR"));

        // Meme tokens
        let mog = Address::from_str("0xaaee1a9723aadb7afa2810263653a34ba2c21c7a").unwrap();
        assert_eq!(symbols.get(&mog), Some(&"MOG"));

        // Restaking
        let eigen = Address::from_str("0xec53bF9167f50cDEB3Ae105f56099aaaB9061F83").unwrap();
        assert_eq!(symbols.get(&eigen), Some(&"EIGEN"));

        // RWA
        let ondo = Address::from_str("0xfAbA6f8e4a5E8Ab82F62fe7C39859FA577269BE3").unwrap();
        assert_eq!(symbols.get(&ondo), Some(&"ONDO"));
    }

    #[test]
    fn test_filter_functions() {
        // Test all_ai_tokens
        let ai = all_ai_tokens();
        assert!(!ai.is_empty(), "Should have AI tokens");
        for token in &ai {
            assert_eq!(token.category, TokenCategory::AICompute);
        }

        // Test all_gaming_tokens
        let gaming = all_gaming_tokens();
        assert!(!gaming.is_empty(), "Should have gaming tokens");
        for token in &gaming {
            assert_eq!(token.category, TokenCategory::Gaming);
        }

        // Test all_restaking_tokens
        let restaking = all_restaking_tokens();
        assert!(!restaking.is_empty(), "Should have restaking tokens");
        for token in &restaking {
            assert_eq!(token.category, TokenCategory::Restaking);
        }

        // Test all_rwa_tokens
        let rwa = all_rwa_tokens();
        assert!(!rwa.is_empty(), "Should have RWA tokens");
        for token in &rwa {
            assert_eq!(token.category, TokenCategory::RWA);
        }
    }
}