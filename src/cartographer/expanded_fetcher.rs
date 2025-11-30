//! Expanded Pool Fetcher - Phase 4
//!
//! Integrates all pool sources:
//! - Existing Uniswap V2/V3, SushiSwap, PancakeSwap, Balancer
//! - NEW: Curve StableSwap NG (dynamic discovery)
//! - NEW: Sky Ecosystem (sUSDS, USDS)
//! - NEW: USD3/Reserve Protocol
//!
//! OPTIMIZATIONS:
//! - Throttles slow-moving data sources to reduce RPC calls
//! - Curve NG: Only fetch every 5th scan (stablecoin pools change slowly)
//! - Sky/USD3: Only fetch every 2nd scan (vault rates are slow-moving)
//! - Caches last fetched data between throttled scans
//!
//! Safe extension - does NOT modify existing contracts or handlers.

use alloy_primitives::{Address, U256, address};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{sol, SolCall};
use alloy_rpc_types::TransactionRequest;
use eyre::{eyre, Result};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Instant;
use tracing::{debug, info, trace, warn};
use lazy_static::lazy_static;

use super::{Dex, PoolState, PoolType, get_token_decimals};
use super::curve_ng::{CurveNGFetcher, CurveNGPool};
use super::sky_ecosystem::{SkyAdapter, ERC4626State, SUSDS_TOKEN, USDS_TOKEN, SDAI_TOKEN, DAI_TOKEN};
use super::usd3_reserve::{USD3Adapter, USD3State, USD3_TOKEN};
use super::curve_lp::{
    CurveLPAdapter, LPNavCalculator, LPMarketDiscovery,
    CachedLPPool, LPNavArbitrage, LPNavResult as LPNavCalcResult,
    SecondaryMarket, DISCOVERY_THROTTLE_INTERVAL as LP_DISCOVERY_THROTTLE,
};
use crate::tokens::build_symbol_map as build_base_symbol_map;
use std::collections::HashSet;

// ============================================
// THROTTLE CONFIGURATION
// ============================================

/// Curve NG: Fetch every N scans (stablecoin pools are slow-moving)
const CURVE_NG_THROTTLE_INTERVAL: u64 = 5;

/// Sky/USD3: Fetch every N scans (vault rates change slowly)
const SKY_USD3_THROTTLE_INTERVAL: u64 = 2;

/// Curve LP NAV: Discover markets every N scans (LP tokens trade infrequently)
const CURVE_LP_THROTTLE_INTERVAL: u64 = LP_DISCOVERY_THROTTLE;

/// Cached data from throttled sources
struct ThrottledCache {
    scan_counter: u64,
    curve_ng_pools: Vec<CurveNGPool>,
    curve_ng_states: Vec<PoolState>,
    sky_vaults: Vec<ERC4626State>,
    sky_virtual_pools: Vec<PoolState>,
    usd3_state: Option<USD3State>,
    // Curve LP NAV arbitrage cache
    lp_pools: Vec<CachedLPPool>,
    lp_secondary_markets: HashMap<Address, Vec<SecondaryMarket>>,
    lp_market_states: Vec<PoolState>,
    lp_nav_results: Vec<LPNavCalcResult>,
    lp_opportunities: Vec<LPNavArbitrage>,
}

impl Default for ThrottledCache {
    fn default() -> Self {
        Self {
            scan_counter: 0,
            curve_ng_pools: Vec::new(),
            curve_ng_states: Vec::new(),
            sky_vaults: Vec::new(),
            sky_virtual_pools: Vec::new(),
            usd3_state: None,
            // LP NAV cache
            lp_pools: Vec::new(),
            lp_secondary_markets: HashMap::new(),
            lp_market_states: Vec::new(),
            lp_nav_results: Vec::new(),
            lp_opportunities: Vec::new(),
        }
    }
}

lazy_static! {
    /// Global cache for throttled data sources
    static ref THROTTLE_CACHE: RwLock<ThrottledCache> = RwLock::new(ThrottledCache::default());
}
// ============================================
// BLOCKED TOKENS (known scams/fakes)
// ============================================

/// Known scam/fake token addresses to filter out
const BLOCKED_TOKENS: [&str; 1] = [
    "6b175474e89094c44da98b954eedeac495271d0f", // Fake DAI (different from real DAI)
];

/// Check if a token address is blocked
fn is_blocked_token(addr: &Address) -> bool {
    let addr_str = format!("{:?}", addr).to_lowercase();
    BLOCKED_TOKENS.iter().any(|blocked| addr_str.contains(blocked))
}

/// Check if a pool pair is a stablecoin pair (both tokens are USD-pegged)
fn is_stablecoin_pair(info: &NewPoolInfo) -> bool {
    let stables = ["USD", "DAI", "FRAX", "DOLA", "GHO", "LUSD", "TUSD", "GUSD"];

    let sym0 = info.token0_symbol.to_uppercase();
    let sym1 = info.token1_symbol.to_uppercase();

    stables.iter().any(|s| sym0.contains(s)) && stables.iter().any(|s| sym1.contains(s))
}

// ============================================
// PRIORITY TOKENS (Phase 4)
// ============================================

/// Tokens to ALWAYS include in arbitrage search
pub fn get_priority_tokens() -> Vec<(Address, &'static str, u8)> {
    vec![
        // Base tokens (existing)
        (address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"), "WETH", 18),
        (address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"), "USDC", 6),
        (address!("dAC17F958D2ee523a2206206994597C13D831ec7"), "USDT", 6),
        (address!("6B175474E89094C44Da98b954EedcdeCB5BE3830"), "DAI", 18),
        (address!("2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"), "WBTC", 8),

        // SKY Ecosystem (NEW - Phase 2)
        (USDS_TOKEN, "USDS", 18),
        (SUSDS_TOKEN, "sUSDS", 18),
        (SDAI_TOKEN, "sDAI", 18),

        // USD3 (NEW - Phase 3)
        (USD3_TOKEN, "USD3", 18),

        // Curve crvUSD
        (address!("f939E0A03FB07F59A73314E73794Be0E57ac1b4E"), "crvUSD", 18),

        // FRAX ecosystem
        (address!("853d955aCEf822Db058eb8505911ED77F175b99e"), "FRAX", 18),
        (address!("3432B6A60D23Ca0dFCa7761B7ab56459D9C964D0"), "FXS", 18),

        // DOLA (Inverse Finance)
        (address!("865377367054516e17014CcdED1e7d814EDC9ce4"), "DOLA", 18),

        // GHO (Aave)
        (address!("40D16FC0246aD3160Ccc09B8D0D3A2cD28aE6C2f"), "GHO", 18),

        // pyUSD (PayPal)
        (address!("6c3ea9036406852006290770BEdFcAbA0e23A0e8"), "pyUSD", 6),

        // ============================================
        // HIGH VOLATILITY TOKENS (Q4 2025)
        // ============================================

        // AI/Compute tokens
        (address!("6de037ef9ad2725eb40118bb1702ebb27e4aeb24"), "RNDR", 18),
        (address!("aea46A60368A7bD060eec7DF8CBa43b7EF41Ad85"), "FET", 18),
        (address!("77e06c9eccf2e797fd462a92b6d7642ef85b0a44"), "wTAO", 9),

        // Meme tokens (V2/V3 arb targets)
        (address!("aaee1a9723aadb7afa2810263653a34ba2c21c7a"), "MOG", 18),
        (address!("e0f63a424a4439cbe457d80e4f4b51ad25b2c56c"), "SPX6900", 8),

        // Restaking tokens (NAV discount)
        (address!("ec53bF9167f50cDEB3Ae105f56099aaaB9061F83"), "EIGEN", 18),
        (address!("D9A442856C234a39a81a089C06451EBAa4306a72"), "pufETH", 18),
        (address!("bf5495Efe5DB9ce00f80364C8B423567e58d2110"), "ezETH", 18),

        // RWA tokens
        (address!("fAbA6f8e4a5E8Ab82F62fe7C39859FA577269BE3"), "ONDO", 18),
        (address!("96F6eF951840721AdBF46Ac996b59E0235CB985C"), "USDY", 18),
    ]
}

/// Build token symbol map including new tokens
/// Build token symbol map including new tokens
/// FIXED: Now merges with tokens from tokens.rs
pub fn build_expanded_symbol_map() -> HashMap<Address, &'static str> {
    let mut map = HashMap::new();
    
    // STEP 1: Load ALL tokens from tokens.rs FIRST
    // This includes USDC, WBTC, crvUSD, sDAI, pyUSD, etc.
    let base_map = build_base_symbol_map();
    for (addr, symbol) in base_map {
        map.insert(addr, symbol);
    }
    
    // STEP 2: Add priority tokens (may override some)
    for (addr, symbol, _) in get_priority_tokens() {
        map.insert(addr, symbol);
    }
    
    // STEP 3: Add tokens discovered from Curve NG and high-volatility tokens
    let additional: [(&str, &str); 40] = [
        ("0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0", "wstETH"),
        ("0x514910771AF9Ca656af840dff83E8264EcF986CA", "LINK"),
        ("0x1f9840a85d5aF5bf1D1762F925BDADdC4201F984", "UNI"),
        ("0x6982508145454Ce325dDbE47a25d4ec3d2311933", "PEPE"),
        ("0x95aD61b0a150d79219dCF64E1E6Cc01f0B64C4cE", "SHIB"),
        ("0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DDaE9", "AAVE"),
        ("0x4c9EDD5852cd905f086C759E8383e09bff1E68B3", "USDe"),
        ("0xCd5fE23C85820F7B72D0926FC9b05b43E359b7ee", "weETH"),
        ("0xae78736Cd615f374D3085123A210448E74Fc6393", "rETH"),
        ("0xBe9895146f7AF43049ca1c1AE358B0541Ea49704", "cbETH"),
        ("0x0655977FEb2f289A4aB78af67BAB0d17aAb84367", "scrvUSD"),
        ("0xae7ab96520DE3A18E5e111B5EaAb095312D7fE84", "stETH"),
        ("0x4591DBfF62656E7859Afe5e45f6f47D3669fBB28", "OETH"),
        ("0x73968b9a57c6E53d41345FD57a6E6ae27d6CDb2F", "sUSDe"),
        ("0x5Ca135cB8527d76e932f34B5145575F9d8cBe08E", "PT-sUSDe"),
        // AI/Compute tokens
        ("0x6de037ef9ad2725eb40118bb1702ebb27e4aeb24", "RNDR"),
        ("0xaea46A60368A7bD060eec7DF8CBa43b7EF41Ad85", "FET"),
        ("0x5B7533812759B45C2B44C19e320ba2cD2681b542", "AGIX"),
        ("0x77e06c9eccf2e797fd462a92b6d7642ef85b0a44", "wTAO"),
        ("0xb60acd2057067dc9ed8c083f5aa227a244044fd6", "stTAO"),
        // Gaming tokens
        ("0xf57e7e7c23978c3caec3c3548e3d615c346e79ff", "IMX"),
        ("0xd1d2eb1b1e90b638588728b4130137d262c87cae", "GALA"),
        ("0x3845badAde8e6dFF049820680d1F14bD3903a5d0", "SAND"),
        ("0xbb0e17ef65f82ab018d8edd776e8dd940327b28b", "AXS"),
        // Meme tokens
        ("0xaaee1a9723aadb7afa2810263653a34ba2c21c7a", "MOG"),
        ("0xe0f63a424a4439cbe457d80e4f4b51ad25b2c56c", "SPX6900"),
        ("0xa35923162c49cf95e6bf26623385eb431ad920d3", "TURBO"),
        ("0xcf0c122c6b73ff809c693db761e7baebe62b6a2e", "FLOKI"),
        // Restaking tokens
        ("0xec53bF9167f50cDEB3Ae105f56099aaaB9061F83", "EIGEN"),
        ("0x3B50805453023a91a8bf641e279401a0b23FA6F9", "REZ"),
        ("0x4d1C297d39C5c1277964D0E3f8Aa901493664530", "PUFFER"),
        ("0xD9A442856C234a39a81a089C06451EBAa4306a72", "pufETH"),
        ("0xbf5495Efe5DB9ce00f80364C8B423567e58d2110", "ezETH"),
        ("0x35fA164735182de50811E8e2E824cFb9B6118ac2", "eETH"),
        // RWA tokens
        ("0xfAbA6f8e4a5E8Ab82F62fe7C39859FA577269BE3", "ONDO"),
        ("0xc221b7e65ffc80de234bbb6667abdd46593d34f0", "CFG"),
        ("0x643C4E15d7d62Ad0aBeC4a9BD4b001aA3Ef52d66", "SYRUP"),
        ("0x96F6eF951840721AdBF46Ac996b59E0235CB985C", "USDY"),
        ("0x1B19C19393e2d034D8Ff31ff34c81252FcBbee92", "OUSG"),
        ("0xaf37c1167910ebC994e266949387d2c7C326b879", "rOUSG"),
    ];

    for (addr_str, symbol) in additional {
        if let Ok(addr) = addr_str.parse::<Address>() {
            map.entry(addr).or_insert(symbol);
        }
    }

    map
}
// ============================================
// STATIC POOL DEFINITIONS (Phase 4 Priority)
// ============================================

/// New static pools to add for high-priority pairs
/// FIXED V3: Removed non-standard pools that don't support coins() interface
pub fn get_new_priority_pools() -> Vec<NewPoolInfo> {
    vec![
        // ============================================
        // CRUVSD POOLS (pegkeeper dynamics)
        // ============================================

        // crvUSD/USDT - high volume, standard 2-coin pool
        NewPoolInfo {
            address: "0x390f3595bCa2Df7d23783dFd126427CCeb997BF4",
            token0: address!("f939E0A03FB07F59A73314E73794Be0E57ac1b4E"), // crvUSD
            token1: address!("dAC17F958D2ee523a2206206994597C13D831ec7"), // USDT
            token0_symbol: "crvUSD",
            token1_symbol: "USDT",
            token0_decimals: 18,  // crvUSD = 18 decimals
            token1_decimals: 6,   // USDT = 6 decimals
            fee: 4, // 0.04%
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            note: "Pegkeeper dynamics create spreads",
        },

        // crvUSD/USDC - standard 2-coin pool
        NewPoolInfo {
            address: "0x4DEcE678ceceb27446b35C672dC7d61F30bAD69E",
            token0: address!("f939E0A03FB07F59A73314E73794Be0E57ac1b4E"), // crvUSD
            token1: address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"), // USDC
            token0_symbol: "crvUSD",
            token1_symbol: "USDC",
            token0_decimals: 18,  // crvUSD = 18 decimals
            token1_decimals: 6,   // USDC = 6 decimals
            fee: 4,
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            note: "High volume crvUSD pool",
        },

        // ============================================
        // FRAX ECOSYSTEM
        // ============================================

        // FRAX/USDC - standard 2-coin pool
        NewPoolInfo {
            address: "0xDcEF968d416a41Cdac0ED8702fAC8128A64241A2",
            token0: address!("853d955aCEf822Db058eb8505911ED77F175b99e"), // FRAX
            token1: address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"), // USDC
            token0_symbol: "FRAX",
            token1_symbol: "USDC",
            token0_decimals: 18,  // FRAX = 18 decimals
            token1_decimals: 6,   // USDC = 6 decimals
            fee: 4,
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            note: "FRAX peg maintenance creates opportunities",
        },

        // ============================================
        // REMOVED PROBLEMATIC POOLS:
        // ============================================
        // - DAI_USDS_CONVERTER (0x3225737a...) - Not a standard Curve pool
        // - GHO/USDT (0x...) - Placeholder address
        // - DOLA/USDC (0xAA5A67c2...) - Metapool (DOLA/3CRV), not 2-coin
    ]
}

/// New pool info for static definition - WITH DECIMALS for proper pricing
#[derive(Debug, Clone)]
pub struct NewPoolInfo {
    pub address: &'static str,
    pub token0: Address,
    pub token1: Address,
    pub token0_symbol: &'static str,
    pub token1_symbol: &'static str,
    pub token0_decimals: u8,  // FIXED: Added for proper decimal handling
    pub token1_decimals: u8,  // FIXED: Added for proper decimal handling
    pub fee: u32,
    pub dex: Dex,
    pub pool_type: PoolType,
    pub note: &'static str,
}

// ============================================
// EXPANDED POOL FETCHER
// ============================================

/// Expanded fetcher that combines all pool sources
pub struct ExpandedPoolFetcher {
    rpc_url: String,
    curve_ng_fetcher: CurveNGFetcher,
    sky_adapter: SkyAdapter,
    usd3_adapter: USD3Adapter,
    // Curve LP NAV arbitrage components
    lp_adapter: CurveLPAdapter,
    lp_market_discovery: LPMarketDiscovery,
    lp_nav_calculator: LPNavCalculator,
}
// ============================================
// POOL QUALITY FILTER
// ============================================

/// Known legitimate tokens - these should NEVER be filtered
fn get_whitelisted_tokens() -> HashSet<Address> {
    [
        // Major tokens
        "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", // WETH
        "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48", // USDC
        "0xdAC17F958D2ee523a2206206994597C13D831ec7", // USDT
        "0x6B175474E89094C44Da98b954EesdeACB5BE3830", // DAI
        "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599", // WBTC
        // Stablecoins
        "0x853d955aCEf822Db058eb8505911ED77F175b99e", // FRAX
        "0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E", // crvUSD
        "0x6c3ea9036406852006290770BEdFcAbA0e23A0e8", // pyUSD
        "0x0000206329b97DB379d5E1Bf586BbDB969C63274", // USDA
        "0x865377367054516e17014CcdED1e7d814EDC9ce4", // DOLA
        // Yield tokens
        "0x83F20F44975D03b1b09e64809B757c47f942BEeA", // sDAI
        "0xdC035D45d973E3EC169d2276DDab16f1e407384F", // USDS
        "0xa3931d71877C0E7a3148CB7Eb4463524FEc27fbD", // sUSDS
        "0x4c9EDD5852cd905f086C759E8383e09bff1E68B3", // USDe
        "0x9D39A5DE30e57443BfF2A8307A4256c8797A3497", // sUSDe
        // LSTs
        "0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0", // wstETH
        "0xae78736Cd615f374D3085123A210448E74Fc6393", // rETH
        "0xBe9895146f7AF43049ca1c1AE358B0541Ea49704", // cbETH
        // DeFi tokens
        "0x1f9840a85d5aF5bf1D1762F925BDADdC4201F984", // UNI
        "0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DDaE9", // AAVE
        "0x514910771AF9Ca656af840dff83E8264EcF986CA", // LINK
        "0xD533a949740bb3306d119CC777fa900bA034cd52", // CRV
        "0xba100000625a3754423978a60c9317c58a424e3D", // BAL
        // Meme coins (legitimate high-volume)
        "0x6982508145454Ce325dDbE47a25d4ec3d2311933", // PEPE
        "0x95aD61b0a150d79219dCF64E1E6Cc01f0B64C4cE", // SHIB
    ]
    .iter()
    .filter_map(|s| s.parse().ok())
    .collect()
}

/// Known stablecoins for price validation
fn get_stablecoins() -> HashSet<Address> {
    [
        "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48", // USDC
        "0xdAC17F958D2ee523a2206206994597C13D831ec7", // USDT
        "0x6B175474E89094C44Da98b954EedeACB5BE3830", // DAI
        "0x853d955aCEf822Db058eb8505911ED77F175b99e", // FRAX
        "0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E", // crvUSD
        "0x6c3ea9036406852006290770BEdFcAbA0e23A0e8", // pyUSD
        "0x865377367054516e17014CcdED1e7d814EDC9ce4", // DOLA
        "0xdC035D45d973E3EC169d2276DDab16f1e407384F", // USDS
        "0x4c9EDD5852cd905f086C759E8383e09bff1E68B3", // USDe
    ]
    .iter()
    .filter_map(|s| s.parse().ok())
    .collect()
}

/// Known yield-bearing stablecoins (trade at premium to $1)
fn get_yield_stablecoins() -> HashSet<Address> {
    [
        "0x83F20F44975D03b1b09e64809B757c47f942BEeA", // sDAI (~1.05-1.20)
        "0xa3931d71877C0E7a3148CB7Eb4463524FEc27fbD", // sUSDS (~1.0-1.15)
        "0x9D39A5DE30e57443BfF2A8307A4256c8797A3497", // sUSDe (~1.0-1.15)
    ]
    .iter()
    .filter_map(|s| s.parse().ok())
    .collect()
}


/// Filter suspicious pools with smart validation
fn filter_suspicious_pools(pools: Vec<PoolState>) -> Vec<PoolState> {
    let symbol_map = build_expanded_symbol_map();
    let whitelist = get_whitelisted_tokens();
    let stablecoins = get_stablecoins();
    let yield_stables = get_yield_stablecoins();
    let before_count = pools.len();
    
    let filtered: Vec<PoolState> = pools.into_iter().filter(|pool| {
        let t0_addr = pool.token0;
        let t1_addr = pool.token1;
        let t0 = symbol_map.get(&t0_addr).copied().unwrap_or("???");
        let t1 = symbol_map.get(&t1_addr).copied().unwrap_or("???");
        
        // 1. Never filter pools with whitelisted tokens
        let t0_whitelisted = whitelist.contains(&t0_addr);
        let t1_whitelisted = whitelist.contains(&t1_addr);
        
        // 2. Block known bad tokens
        if is_blocked_token(&t0_addr) || is_blocked_token(&t1_addr) {
            debug!("Filtered blocked token: {} / {}", t0, t1);
            return false;
        }
        
        // 3. Basic price sanity
        let price = pool.normalized_price();
        if price <= 0.0 || !price.is_finite() {
            debug!("Filtered zero/invalid price: {:?} ({}/{})", pool.address, t0, t1);
            return false;
        }
        
        // 4. If BOTH tokens are whitelisted, always allow
        if t0_whitelisted && t1_whitelisted {
            return true;
        }
        
        // 5. If at least one token is whitelisted, apply lenient checks
        if t0_whitelisted || t1_whitelisted {
            // Allow wide price range for mixed pairs
            if price > 1e12 || price < 1e-12 {
                debug!("Filtered extreme price {} for {:?}", price, pool.address);
                return false;
            }
            return true;
        }
        
        // 6. Both tokens unknown - be more careful
        if t0 == "???" && t1 == "???" {
            debug!("Filtered both unknown: {:?}", pool.address);
            return false;
        }
        
        // 7. Stablecoin pair price validation (both are stables)
        let t0_stable = stablecoins.contains(&t0_addr);
        let t1_stable = stablecoins.contains(&t1_addr);
        let t0_yield = yield_stables.contains(&t0_addr);
        let t1_yield = yield_stables.contains(&t1_addr);
        
        if t0_stable && t1_stable {
            // Regular stable/stable should be ~1.0 (0.95 - 1.05)
            if price < 0.90 || price > 1.10 {
                debug!("Filtered bad stable/stable price {}: {:?} ({}/{})", price, pool.address, t0, t1);
                return false;
            }
        } else if (t0_stable && t1_yield) || (t0_yield && t1_stable) {
            // Yield stable vs regular stable (0.85 - 1.25 range)
            if price < 0.80 || price > 1.30 {
                debug!("Filtered bad yield-stable price {}: {:?} ({}/{})", price, pool.address, t0, t1);
                return false;
            }
        }
        
        // 8. Extreme price filter for remaining pools
        if price > 1e15 || price < 1e-15 {
            debug!("Filtered extreme price {} for {:?}", price, pool.address);
            return false;
        }
        
        true
    }).collect();
    
    let removed = before_count - filtered.len();
    if removed > 0 {
        info!("🧹 Filtered {} suspicious pools, {} remaining", removed, filtered.len());
    }
    
    // Debug: Show what made it through
    debug!("Pools after filter:");
    for pool in &filtered {
        let t0 = symbol_map.get(&pool.token0).copied().unwrap_or("???");
        let t1 = symbol_map.get(&pool.token1).copied().unwrap_or("???");
        debug!("  {:?}: {}/{} price={:.6}", pool.address, t0, t1, pool.normalized_price());
    }
    
    filtered
}
impl ExpandedPoolFetcher {
    pub fn new(rpc_url: String) -> Self {
        Self {
            curve_ng_fetcher: CurveNGFetcher::new(rpc_url.clone()),
            sky_adapter: SkyAdapter::new(rpc_url.clone()),
            usd3_adapter: USD3Adapter::new(rpc_url.clone()),
            lp_adapter: CurveLPAdapter::new(rpc_url.clone()),
            lp_market_discovery: LPMarketDiscovery::new(rpc_url.clone()),
            lp_nav_calculator: LPNavCalculator::new(),
            rpc_url,
        }
    }
    
    /// Fetch ALL pools including new sources
    /// Uses throttling to reduce RPC calls for slow-moving data sources
    pub async fn fetch_all_pools(&self) -> Result<ExpandedPoolResult> {
        let start = Instant::now();

        let mut result = ExpandedPoolResult::default();

        // Get and increment scan counter
        let scan_number = {
            let mut cache = THROTTLE_CACHE.write().unwrap();
            cache.scan_counter += 1;
            cache.scan_counter
        };

        let should_fetch_curve_ng = scan_number % CURVE_NG_THROTTLE_INTERVAL == 1;
        let should_fetch_sky_usd3 = scan_number % SKY_USD3_THROTTLE_INTERVAL == 1;

        debug!(
            "Scan #{}: Curve NG={}, Sky/USD3={}",
            scan_number,
            if should_fetch_curve_ng { "FETCH" } else { "CACHE" },
            if should_fetch_sky_usd3 { "FETCH" } else { "CACHE" }
        );

        // 1. Fetch existing pools (from original fetcher) - ALWAYS fetch
        info!("📦 Fetching existing pools...");
        let existing_pools = self.fetch_existing_pools().await?;
        result.existing_pools = existing_pools.len();
        result.pool_states.extend(existing_pools);

        // 1.5. Add static bridging pools (connects ecosystems) - ALWAYS fetch
        info!("🔗 Adding bridging pools...");
        let bridging_count = self.add_bridging_pools(&mut result.pool_states).await;
        info!("   Added {} bridging pool edges", bridging_count);

        // 2. Discover Curve NG pools (THROTTLED - every 5th scan)
        // NOW USING ACCURATE get_dy PRICING instead of balance ratios
        if should_fetch_curve_ng {
            info!("🔍 Discovering Curve NG pools with accurate get_dy pricing (fresh)...");
            match self.curve_ng_fetcher.discover_all_ng_pools().await {
                Ok(ng_pools) => {
                    // Use the new accurate pricing method that fetches real get_dy prices
                    let states = self.curve_ng_fetcher.convert_to_pool_states_accurate(&ng_pools).await;
                    result.curve_ng_pools = ng_pools.len();
                    result.curve_ng_states = states.len();
                    result.pool_states.extend(states.clone());
                    result.ng_pool_details = ng_pools.clone();

                    // Cache for future throttled scans
                    let mut cache = THROTTLE_CACHE.write().unwrap();
                    cache.curve_ng_pools = ng_pools;
                    cache.curve_ng_states = states;
                }
                Err(e) => {
                    warn!("Failed to discover Curve NG pools: {}", e);
                }
            }
        } else {
            // Use cached data
            let cache = THROTTLE_CACHE.read().unwrap();
            if !cache.curve_ng_states.is_empty() {
                debug!("Using cached Curve NG data ({} pools)", cache.curve_ng_states.len());
                result.curve_ng_pools = cache.curve_ng_pools.len();
                result.curve_ng_states = cache.curve_ng_states.len();
                result.pool_states.extend(cache.curve_ng_states.clone());
                result.ng_pool_details = cache.curve_ng_pools.clone();
            }
        }

        // Debug NG pools
        for ng_pool in &result.ng_pool_details {
            trace!("  NG Pool {:?}: {} coins", ng_pool.address, ng_pool.n_coins);
        }

        // 3. Fetch Sky ecosystem state (THROTTLED - every 2nd scan)
        if should_fetch_sky_usd3 {
            info!("🌤️ Fetching Sky ecosystem state (fresh)...");
            match self.sky_adapter.fetch_all_vaults().await {
                Ok(vaults) => {
                    result.erc4626_vaults = vaults.clone();

                    // Create virtual pools for ERC-4626 deposit/redeem
                    let mut virtual_pools_states = Vec::new();
                    for vault in &vaults {
                        let virtual_pools = super::sky_ecosystem::create_erc4626_virtual_pools(vault);
                        for vp in virtual_pools {
                            if let Some(state) = self.virtual_pool_to_state(&vp, vault) {
                                virtual_pools_states.push(state);
                                result.virtual_erc4626_edges += 1;
                            }
                        }
                    }
                    result.pool_states.extend(virtual_pools_states.clone());

                    // Cache for future throttled scans
                    let mut cache = THROTTLE_CACHE.write().unwrap();
                    cache.sky_vaults = vaults;
                    cache.sky_virtual_pools = virtual_pools_states;
                }
                Err(e) => {
                    warn!("Failed to fetch Sky ecosystem: {}", e);
                }
            }
        } else {
            // Use cached data
            let cache = THROTTLE_CACHE.read().unwrap();
            if !cache.sky_virtual_pools.is_empty() {
                debug!("Using cached Sky data ({} virtual pools)", cache.sky_virtual_pools.len());
                result.erc4626_vaults = cache.sky_vaults.clone();
                result.virtual_erc4626_edges = cache.sky_virtual_pools.len();
                result.pool_states.extend(cache.sky_virtual_pools.clone());
            }
        }

        // 4. Fetch USD3 state (THROTTLED - every 2nd scan, same as Sky)
        if should_fetch_sky_usd3 {
            info!("💵 Fetching USD3 NAV (fresh)...");
            match self.usd3_adapter.fetch_usd3_state().await {
                Ok(state) => {
                    result.usd3_state = Some(state.clone());

                    // Cache for future throttled scans
                    let mut cache = THROTTLE_CACHE.write().unwrap();
                    cache.usd3_state = Some(state);
                }
                Err(e) => {
                    warn!("Failed to fetch USD3 state: {}", e);
                }
            }
        } else {
            // Use cached data
            let cache = THROTTLE_CACHE.read().unwrap();
            if cache.usd3_state.is_some() {
                debug!("Using cached USD3 data");
                result.usd3_state = cache.usd3_state.clone();
            }
        }

        // 5. Fetch Curve LP NAV arbitrage opportunities (THROTTLED - every 10th scan)
        let should_fetch_lp = scan_number % CURVE_LP_THROTTLE_INTERVAL == 1;
        if should_fetch_lp {
            info!("🎯 Discovering LP NAV arbitrage opportunities (fresh)...");
            match self.fetch_lp_nav_opportunities().await {
                Ok((lp_states, lp_pools, lp_markets, nav_results, opportunities)) => {
                    result.lp_pools = lp_pools.len();
                    result.lp_secondary_markets = lp_markets.values().map(|v| v.len()).sum();
                    result.lp_nav_opportunities = opportunities.clone();
                    result.pool_states.extend(lp_states.clone());

                    // Always log LP discovery status
                    info!(
                        "   LP Discovery: {} pools, {} secondary markets, {} opportunities",
                        lp_pools.len(),
                        result.lp_secondary_markets,
                        opportunities.len()
                    );

                    // Log opportunities if found
                    for opp in &opportunities {
                        info!(
                            "  💰 LP Arb: {} - {}bps discount, {} route",
                            opp.pool_name, opp.discount_bps, opp.direction
                        );
                    }

                    // Log if no markets found (expected for most LP tokens)
                    if result.lp_secondary_markets == 0 {
                        debug!(
                            "   No UniV3 secondary markets found for LP tokens (this is normal - LP tokens rarely trade on UniV3)"
                        );
                    }

                    // Cache for future scans
                    let mut cache = THROTTLE_CACHE.write().unwrap();
                    cache.lp_pools = lp_pools;
                    cache.lp_secondary_markets = lp_markets;
                    cache.lp_market_states = lp_states;
                    cache.lp_nav_results = nav_results;
                    cache.lp_opportunities = opportunities;
                }
                Err(e) => {
                    warn!("Failed to fetch LP NAV opportunities: {}", e);
                }
            }
        } else {
            // Use cached LP data
            let cache = THROTTLE_CACHE.read().unwrap();
            if !cache.lp_market_states.is_empty() {
                debug!(
                    "Using cached LP NAV data ({} pools, {} markets)",
                    cache.lp_pools.len(),
                    cache.lp_secondary_markets.values().map(|v| v.len()).sum::<usize>()
                );
                result.lp_pools = cache.lp_pools.len();
                result.lp_secondary_markets = cache.lp_secondary_markets.values().map(|v| v.len()).sum();
                result.lp_nav_opportunities = cache.lp_opportunities.clone();
                result.pool_states.extend(cache.lp_market_states.clone());
            }
        }

        result.fetch_duration = start.elapsed();

        info!(
            "✅ Scan #{}: {} pools ({} existing, {} NG{}, {} virtual{}, {} LP markets{}) in {:?}",
            scan_number,
            result.pool_states.len(),
            result.existing_pools,
            result.curve_ng_states,
            if should_fetch_curve_ng { "" } else { " [cached]" },
            result.virtual_erc4626_edges,
            if should_fetch_sky_usd3 { "" } else { " [cached]" },
            result.lp_secondary_markets,
            if should_fetch_lp { "" } else { " [cached]" },
            result.fetch_duration
        );

        // Log LP NAV opportunities if any
        if !result.lp_nav_opportunities.is_empty() {
            info!(
                "🎯 {} LP NAV arbitrage opportunities detected!",
                result.lp_nav_opportunities.len()
            );
        }
        
        // ============================================
        // FILTER: Remove suspicious/scam pools
        // ============================================
        let before_filter = result.pool_states.len();
        result.pool_states = filter_suspicious_pools(result.pool_states);
        let after_filter = result.pool_states.len();
        
        if before_filter != after_filter {
            info!("🧹 Pool filter: {} → {} pools", before_filter, after_filter);
        }
        
        Ok(result)
    }
    
    /// Fetch existing pools (calls the original fetcher logic)
    async fn fetch_existing_pools(&self) -> Result<Vec<PoolState>> {
        // Import and use the original pool fetcher
        let fetcher = super::PoolFetcher::new(self.rpc_url.clone());
        fetcher.fetch_all_pools().await
    }
    
    /// Add bridging pools WITH actual on-chain prices (BATCHED)
    /// Uses Multicall3 to fetch all prices in a single RPC call
    /// FIXED V3: Now queries actual token indices from pool's coins() function
    /// to handle Curve pools where token order differs from our NewPoolInfo definition
    async fn add_bridging_pools(&self, pool_states: &mut Vec<PoolState>) -> usize {
        // Collect valid pools
        let priority_pools = get_new_priority_pools();
        let mut valid_pools: Vec<(Address, &NewPoolInfo)> = Vec::new();

        for pool_info in &priority_pools {
            if pool_info.address.starts_with("0x...") || pool_info.address.len() < 20 {
                continue;
            }
            if let Ok(pool_address) = pool_info.address.parse::<Address>() {
                valid_pools.push((pool_address, pool_info));
            }
        }

        if valid_pools.is_empty() {
            return 0;
        }

        // STEP 1: Query actual coin order from each pool via Multicall
        let pool_coins = self.fetch_pool_coins(&valid_pools).await;

        // STEP 2: Build requests with CORRECT indices based on actual pool coin order
        let base_amount_usd = 10000.0;
        let mut forward_requests: Vec<(Address, i128, i128, U256)> = Vec::new();
        let mut reverse_requests: Vec<(Address, i128, i128, U256)> = Vec::new();
        // Track metadata for each request: (pool_address, pool_info, actual_token0_dec, actual_token1_dec)
        let mut request_metadata: Vec<(Address, &NewPoolInfo, u8, u8)> = Vec::new();

        for (pool_address, pool_info) in &valid_pools {
            // Find actual indices for our tokens in the pool's coins array
            let coins = match pool_coins.get(pool_address) {
                Some(c) => c,
                None => {
                    warn!("Could not get coins for pool {:?}, skipping", pool_address);
                    continue;
                }
            };

            // Find index of token0 and token1 in pool's actual coins array
            let i_idx = coins.iter().position(|c| *c == pool_info.token0);
            let j_idx = coins.iter().position(|c| *c == pool_info.token1);

            match (i_idx, j_idx) {
                (Some(i), Some(j)) => {
                    // Get decimals for tokens at their ACTUAL positions
                    let dec_i = pool_info.token0_decimals; // token0's decimals
                    let dec_j = pool_info.token1_decimals; // token1's decimals

                    // Forward: token0 -> token1 (using actual pool indices)
                    let dx_forward = U256::from((base_amount_usd * 10_f64.powi(dec_i as i32)) as u128);
                    forward_requests.push((*pool_address, i as i128, j as i128, dx_forward));

                    // Reverse: token1 -> token0 (using actual pool indices, swapped)
                    let dx_reverse = U256::from((base_amount_usd * 10_f64.powi(dec_j as i32)) as u128);
                    reverse_requests.push((*pool_address, j as i128, i as i128, dx_reverse));

                    request_metadata.push((*pool_address, *pool_info, dec_i, dec_j));

                    debug!(
                        "Pool {:?}: {} at pool index {}, {} at pool index {}",
                        pool_address, pool_info.token0_symbol, i, pool_info.token1_symbol, j
                    );
                }
                _ => {
                    warn!(
                        "Tokens not found in pool {:?}: {} ({:?}) or {} ({:?}) not in coins {:?}",
                        pool_address, pool_info.token0_symbol, pool_info.token0,
                        pool_info.token1_symbol, pool_info.token1, coins
                    );
                    continue;
                }
            }
        }

        if forward_requests.is_empty() {
            warn!("No valid forward requests after token index lookup");
            return 0;
        }

        // STEP 3: Batch fetch forward prices (token0 -> token1)
        let forward_prices = match self.curve_ng_fetcher.batch_get_dy(&forward_requests).await {
            Ok(results) => results,
            Err(e) => {
                warn!("Batch forward price fetch failed: {}, skipping bridging pools", e);
                return 0;
            }
        };

        // Batch fetch reverse prices (token1 -> token0)
        let reverse_prices = self.curve_ng_fetcher.batch_get_dy(&reverse_requests).await
            .unwrap_or_else(|_| vec![None; reverse_requests.len()]);

        // STEP 4: Create pool states with correct decimal normalization
        let mut count = 0;
        for (idx, (pool_address, pool_info, dec_i, dec_j)) in request_metadata.iter().enumerate() {
            // === FORWARD DIRECTION: token0 -> token1 ===
            if let Some(dy) = forward_prices.get(idx).and_then(|p| p.as_ref()) {
                let (_, _, _, dx) = forward_requests[idx];

                // Normalize using ACTUAL decimals
                let dx_normalized = dx.to::<u128>() as f64 / 10_f64.powi(*dec_i as i32);
                let dy_normalized = dy.to::<u128>() as f64 / 10_f64.powi(*dec_j as i32);

                if dx_normalized > 0.0 {
                    let price = dy_normalized / dx_normalized;

                    debug!(
                        "Pool {:?} ({}/{}): dx_norm={:.2}, dy_norm={:.2}, price={:.6}",
                        pool_address, pool_info.token0_symbol, pool_info.token1_symbol,
                        dx_normalized, dy_normalized, price
                    );

                    // Validate stablecoin prices
                    if is_stablecoin_pair(pool_info) && (price < 0.8 || price > 1.25) {
                        warn!(
                            "Suspicious stablecoin price {:.4} for {} -> {}, skipping",
                            price, pool_info.token0_symbol, pool_info.token1_symbol
                        );
                        continue;
                    }

                    if price > 0.0 && price.is_finite() {
                        let sqrt_price = price.sqrt() * 2_f64.powi(96);

                        let state = PoolState {
                            address: *pool_address,
                            token0: pool_info.token0,
                            token1: pool_info.token1,
                            token0_decimals: pool_info.token0_decimals,
                            token1_decimals: pool_info.token1_decimals,
                            sqrt_price_x96: U256::from(sqrt_price as u128),
                            tick: 0,
                            liquidity: 10u128.pow(24),
                            reserve1: 10u128.pow(24),
                            fee: pool_info.fee,
                            is_v4: false,
                            dex: pool_info.dex,
                            pool_type: pool_info.pool_type,
                            weight0: 5 * 10u128.pow(17),
                        };

                        pool_states.push(state);
                        count += 1;
                    }
                }
            } else {
                warn!("Could not fetch forward price for {:?}, skipping", pool_address);
            }

            // === REVERSE DIRECTION: token1 -> token0 ===
            if let Some(dy) = reverse_prices.get(idx).and_then(|p| p.as_ref()) {
                let (_, _, _, dx) = reverse_requests[idx];

                // Normalize by decimals (swapped for reverse direction)
                let dx_normalized = dx.to::<u128>() as f64 / 10_f64.powi(*dec_j as i32);
                let dy_normalized = dy.to::<u128>() as f64 / 10_f64.powi(*dec_i as i32);

                if dx_normalized > 0.0 {
                    let price = dy_normalized / dx_normalized;

                    debug!(
                        "Pool {:?} ({}/{}): REVERSE dx_norm={:.2}, dy_norm={:.2}, price={:.6}",
                        pool_address, pool_info.token1_symbol, pool_info.token0_symbol,
                        dx_normalized, dy_normalized, price
                    );

                    // Validate stablecoin prices
                    if is_stablecoin_pair(pool_info) && (price < 0.8 || price > 1.25) {
                        warn!(
                            "Suspicious stablecoin reverse price {:.4} for {} -> {}, skipping",
                            price, pool_info.token1_symbol, pool_info.token0_symbol
                        );
                        continue;
                    }

                    if price > 0.0 && price.is_finite() {
                        let sqrt_price = price.sqrt() * 2_f64.powi(96);

                        let reverse_state = PoolState {
                            address: *pool_address,
                            token0: pool_info.token1,
                            token1: pool_info.token0,
                            token0_decimals: pool_info.token1_decimals,
                            token1_decimals: pool_info.token0_decimals,
                            sqrt_price_x96: U256::from(sqrt_price as u128),
                            tick: 0,
                            liquidity: 10u128.pow(24),
                            reserve1: 10u128.pow(24),
                            fee: pool_info.fee,
                            is_v4: false,
                            dex: pool_info.dex,
                            pool_type: pool_info.pool_type,
                            weight0: 5 * 10u128.pow(17),
                        };

                        pool_states.push(reverse_state);
                        count += 1;
                    }
                }
            }
        }

        info!("Added {} bridging pool edges (forward + reverse) with correct token indices", count);
        count
    }

    /// Fetch coins array for each pool via Multicall
    /// Returns HashMap<pool_address, Vec<coin_addresses>>
    async fn fetch_pool_coins(
        &self,
        pools: &[(Address, &NewPoolInfo)],
    ) -> HashMap<Address, Vec<Address>> {
        use alloy_sol_types::sol;
        use alloy_rpc_types::TransactionRequest;

        // Define the coins function interface locally
        sol! {
            interface ICurvePoolCoins {
                function coins(uint256 i) external view returns (address);
            }
        }

        // Build multicall for coins(0), coins(1) for each pool
        let mut calls = Vec::new();

        for (pool_address, _) in pools {
            // Query coins(0) and coins(1)
            let call0 = ICurvePoolCoins::coinsCall { i: U256::from(0) };
            let call1 = ICurvePoolCoins::coinsCall { i: U256::from(1) };

            calls.push(super::curve_ng::IMulticall3::Call3 {
                target: *pool_address,
                allowFailure: true,
                callData: call0.abi_encode().into(),
            });
            calls.push(super::curve_ng::IMulticall3::Call3 {
                target: *pool_address,
                allowFailure: true,
                callData: call1.abi_encode().into(),
            });
        }

        if calls.is_empty() {
            return HashMap::new();
        }

        // Execute multicall
        let provider = match ProviderBuilder::new().on_http(self.rpc_url.parse().unwrap()) {
            p => p,
        };

        let calldata = super::curve_ng::IMulticall3::aggregate3Call { calls }.abi_encode();
        let multicall3 = address!("cA11bde05977b3631167028862bE2a173976CA11");

        let tx = TransactionRequest::default()
            .to(multicall3)
            .input(calldata.into());

        let results = match provider.call(tx).await {
            Ok(result) => {
                match super::curve_ng::IMulticall3::aggregate3Call::abi_decode_returns(&result) {
                    Ok(decoded) => decoded,
                    Err(e) => {
                        warn!("Failed to decode multicall result for pool coins: {}", e);
                        return HashMap::new();
                    }
                }
            }
            Err(e) => {
                warn!("Failed to fetch pool coins via multicall: {}", e);
                return HashMap::new();
            }
        };

        // Parse results - 2 calls per pool (coins(0), coins(1))
        let mut pool_coins: HashMap<Address, Vec<Address>> = HashMap::new();

        for (i, (pool_address, _)) in pools.iter().enumerate() {
            let idx0 = i * 2;
            let idx1 = i * 2 + 1;

            if idx1 >= results.len() {
                continue;
            }

            // Parse coin0
            let coin0 = if results[idx0].success && results[idx0].returnData.len() >= 32 {
                // Address is right-padded in the 32-byte return data
                Address::from_slice(&results[idx0].returnData[12..32])
            } else {
                continue;
            };

            // Parse coin1
            let coin1 = if results[idx1].success && results[idx1].returnData.len() >= 32 {
                Address::from_slice(&results[idx1].returnData[12..32])
            } else {
                continue;
            };

            debug!(
                "Pool {:?} actual coins: [0]={:?}, [1]={:?}",
                pool_address, coin0, coin1
            );

            pool_coins.insert(*pool_address, vec![coin0, coin1]);
        }

        info!("Fetched coin order for {} pools", pool_coins.len());
        pool_coins
    }
    /// Convert virtual ERC-4626 pool to PoolState
    fn virtual_pool_to_state(
        &self,
        vp: &super::sky_ecosystem::VirtualERC4626Pool,
        vault_state: &ERC4626State,
    ) -> Option<PoolState> {
        use super::sky_ecosystem::ERC4626Direction;
        
        let (token0, token1) = match vp.direction {
            ERC4626Direction::Deposit => (vp.underlying, vp.vault),  // underlying -> shares
            ERC4626Direction::Redeem => (vp.vault, vp.underlying),   // shares -> underlying
        };
        
        let d0 = get_token_decimals(&token0);
        let d1 = get_token_decimals(&token1);
        
        // Calculate price from rate
        let rate_f64 = vp.rate.to::<u128>() as f64 / 1e18;
        if rate_f64 <= 0.0 || !rate_f64.is_finite() {
            return None;
        }
        
        let sqrt_price = rate_f64.sqrt() * 2_f64.powi(96);
        
        // Virtual pool for ERC-4626 - very low "fee" since deposit/redeem is feeless
        Some(PoolState {
            address: vp.vault, // Use vault address as pool address
            token0,
            token1,
            token0_decimals: d0,
            token1_decimals: d1,
            sqrt_price_x96: U256::from(sqrt_price as u128),
            tick: 0,
            liquidity: vault_state.total_assets.to::<u128>(),
            reserve1: vault_state.total_supply.to::<u128>(),
            fee: 1, // Minimal fee for deposit/redeem (gas only)
            is_v4: false,
            dex: Dex::BalancerV2, // Reuse BalancerV2 type for ERC-4626 vaults
            pool_type: PoolType::Balancer, // Custom type would be better
            weight0: 5 * 10u128.pow(17), // 0.5
        })
    }
    
    /// Get expanded token symbol map
    pub fn get_symbol_map(&self) -> HashMap<Address, &'static str> {
        build_expanded_symbol_map()
    }

    /// Fetch LP NAV arbitrage opportunities
    /// Returns: (pool_states, lp_pools, secondary_markets, nav_results, opportunities)
    async fn fetch_lp_nav_opportunities(
        &self,
    ) -> Result<(
        Vec<PoolState>,
        Vec<CachedLPPool>,
        HashMap<Address, Vec<SecondaryMarket>>,
        Vec<LPNavCalcResult>,
        Vec<LPNavArbitrage>,
    )> {
        // 1. Get LP pools (uses internal caching)
        let lp_pools = self.lp_adapter.get_lp_pools().await?;
        if lp_pools.is_empty() {
            debug!("No LP pools found");
            return Ok((Vec::new(), Vec::new(), HashMap::new(), Vec::new(), Vec::new()));
        }

        info!("  Found {} Curve LP pools", lp_pools.len());

        // 2. Get LP token addresses
        let lp_tokens: Vec<Address> = lp_pools.iter().map(|p| p.lp_token).collect();

        // 3. Fetch virtual prices (batched)
        let virtual_prices = self.lp_adapter.fetch_virtual_prices(&lp_tokens).await?;
        info!("  Fetched {} virtual prices", virtual_prices.len());

        // 4. Discover secondary markets (UniV3 pools where LP tokens trade)
        let secondary_markets = self.lp_market_discovery.discover_markets(&lp_tokens).await?;
        let total_markets: usize = secondary_markets.values().map(|v| v.len()).sum();
        info!("  Discovered {} secondary markets", total_markets);

        // 5. Convert secondary markets to PoolState for routing graph
        let lp_market_states = self.lp_market_discovery.markets_to_pool_states(&secondary_markets);

        // 6. Fetch UniV3 prices for secondary markets
        let univ3_pools = self.lp_market_discovery.get_univ3_pool_addresses(&secondary_markets);
        let univ3_prices = self.lp_market_discovery.fetch_univ3_prices(&univ3_pools).await?;
        debug!("  Fetched {} UniV3 prices", univ3_prices.len());

        // 7. Calculate NAV for each LP token
        let nav_results = self.lp_nav_calculator.batch_calculate_nav(&lp_pools, &virtual_prices);
        info!("  Calculated NAV for {} LP tokens", nav_results.len());

        // 8. Build market price map (LP token -> (price, market))
        let mut market_prices: HashMap<Address, (U256, SecondaryMarket)> = HashMap::new();

        for (lp_token, markets) in &secondary_markets {
            for market in markets {
                // Get price from UniV3 data
                if let Some((sqrt_price_x96, liquidity)) = univ3_prices.get(&market.pool_address) {
                    if *liquidity == 0 {
                        continue;
                    }

                    // Calculate LP token price in USD
                    // Assuming quote token is a stablecoin or WETH (need to adjust)
                    let quote_decimals = get_quote_decimals(&market.quote_token);
                    let lp_is_token0 = true; // Simplified assumption

                    let price_f64 = super::curve_lp::calculate_lp_price_from_sqrt(
                        *sqrt_price_x96,
                        18, // LP tokens are 18 decimals
                        quote_decimals,
                        lp_is_token0,
                    );

                    if price_f64 > 0.0 && price_f64.is_finite() {
                        // Convert to U256 (18 decimals)
                        let price_1e18 = U256::from((price_f64 * 1e18) as u128);

                        // Estimate liquidity in USD
                        let liq_usd = super::curve_lp::estimate_market_liquidity_usd(
                            *liquidity,
                            *sqrt_price_x96,
                            1.0, // Assume quote token at $1 for stables
                        );

                        let mut market_with_liq = market.clone();
                        market_with_liq.liquidity_usd = liq_usd;

                        // Only use markets with sufficient liquidity
                        if liq_usd >= super::curve_lp::MIN_SECONDARY_LIQUIDITY_USD {
                            market_prices
                                .entry(*lp_token)
                                .or_insert((price_1e18, market_with_liq));
                        }
                    }
                }
            }
        }

        // 9. Scan for arbitrage opportunities
        let opportunities = self
            .lp_nav_calculator
            .scan_for_opportunities(&nav_results, &market_prices);

        if !opportunities.is_empty() {
            info!(
                "  🎯 Found {} LP NAV arbitrage opportunities!",
                opportunities.len()
            );
        }

        Ok((lp_market_states, lp_pools, secondary_markets, nav_results, opportunities))
    }
}

/// Get decimals for quote tokens used in LP secondary markets
fn get_quote_decimals(token: &Address) -> u8 {
    use super::curve_lp::QUOTE_TOKENS;

    for (addr, _, decimals) in QUOTE_TOKENS.iter() {
        if addr == token {
            return *decimals;
        }
    }
    18 // Default
}

/// Result of expanded pool fetch
#[derive(Debug, Default)]
pub struct ExpandedPoolResult {
    /// All pool states for graph construction
    pub pool_states: Vec<PoolState>,

    /// Count of existing (original) pools
    pub existing_pools: usize,

    /// Count of discovered Curve NG pools
    pub curve_ng_pools: usize,

    /// Count of pool states from Curve NG
    pub curve_ng_states: usize,

    /// Count of virtual ERC-4626 edges
    pub virtual_erc4626_edges: usize,

    /// Detailed Curve NG pool info
    pub ng_pool_details: Vec<CurveNGPool>,

    /// ERC-4626 vault states
    pub erc4626_vaults: Vec<ERC4626State>,

    /// USD3 state (optional)
    pub usd3_state: Option<USD3State>,

    /// Time to fetch
    pub fetch_duration: std::time::Duration,

    // ============================================
    // LP NAV ARBITRAGE FIELDS
    // ============================================

    /// Count of discovered Curve LP pools
    pub lp_pools: usize,

    /// Count of secondary markets for LP tokens
    pub lp_secondary_markets: usize,

    /// Detected LP NAV arbitrage opportunities
    pub lp_nav_opportunities: Vec<LPNavArbitrage>,
}

impl ExpandedPoolResult {
    /// Total number of pools/edges
    pub fn total_pools(&self) -> usize {
        self.pool_states.len()
    }
    
    /// Summary string
    pub fn summary(&self) -> String {
        format!(
            "{} pools: {} existing + {} NG + {} virtual + {} LP markets ({:?})",
            self.total_pools(),
            self.existing_pools,
            self.curve_ng_states,
            self.virtual_erc4626_edges,
            self.lp_secondary_markets,
            self.fetch_duration
        )
    }

    /// Check if LP NAV opportunities are present
    pub fn has_lp_opportunities(&self) -> bool {
        !self.lp_nav_opportunities.is_empty()
    }

    /// Get best LP NAV opportunity (highest discount)
    pub fn best_lp_opportunity(&self) -> Option<&LPNavArbitrage> {
        self.lp_nav_opportunities.first()
    }
}

// ============================================
// HELPER: Detect opportunities across new pools
// ============================================

/// Check for special arbitrage opportunities unique to new pools
pub async fn check_special_opportunities(
    result: &ExpandedPoolResult,
    min_profit_bps: f64,
) -> Vec<SpecialOpportunity> {
    let mut opportunities = Vec::new();
    
    // 1. Check ERC-4626 yield drift
    for vault in &result.erc4626_vaults {
        if let Some(arb) = vault.check_arb_opportunity(min_profit_bps) {
            opportunities.push(SpecialOpportunity::YieldDrift {
                vault: vault.vault_address,
                symbol: vault.symbol.clone(),
                spread_pct: arb.spread_pct,
            });
        }
    }
    
    // 2. Check USD3 NAV arbitrage
    if let Some(ref usd3) = result.usd3_state {
        if let Some(arb) = usd3.check_nav_arb(min_profit_bps) {
            opportunities.push(SpecialOpportunity::NAVArb {
                token: USD3_TOKEN,
                symbol: "USD3".to_string(),
                spread_pct: arb.spread_pct,
            });
        }
    }
    
    // 3. Check Curve NG pools with high imbalance (higher fees = opportunity)
    for ng_pool in &result.ng_pool_details {
        if ng_pool.has_erc4626 {
            // Pools with ERC-4626 tokens may have yield drift
            let effective_fee = ng_pool.effective_fee(0, 1);
            if effective_fee > ng_pool.base_fee * 2 {
                // Pool is significantly imbalanced
                opportunities.push(SpecialOpportunity::ImbalancedPool {
                    pool: ng_pool.address,
                    base_fee: ng_pool.base_fee,
                    effective_fee,
                });
            }
        }
    }

    // 4. Check LP NAV arbitrage opportunities
    for lp_opp in &result.lp_nav_opportunities {
        opportunities.push(SpecialOpportunity::LPNavArbitrage {
            lp_token: lp_opp.lp_token,
            pool_name: lp_opp.pool_name.clone(),
            discount_bps: lp_opp.discount_bps,
            estimated_profit_usd: lp_opp.estimated_profit_usd,
        });
    }

    opportunities
}

/// Special opportunity types unique to new pools
#[derive(Debug, Clone)]
pub enum SpecialOpportunity {
    /// ERC-4626 vault trading away from redemption value
    YieldDrift {
        vault: Address,
        symbol: String,
        spread_pct: f64,
    },

    /// Basket-backed token trading away from NAV
    NAVArb {
        token: Address,
        symbol: String,
        spread_pct: f64,
    },

    /// Curve NG pool with high imbalance (elevated fees)
    ImbalancedPool {
        pool: Address,
        base_fee: u32,
        effective_fee: u32,
    },

    /// LP token trading below NAV on secondary market
    LPNavArbitrage {
        lp_token: Address,
        pool_name: String,
        discount_bps: i64,
        estimated_profit_usd: f64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_priority_tokens() {
        let tokens = get_priority_tokens();
        assert!(tokens.len() >= 10);
        
        // Check USDS is included
        assert!(tokens.iter().any(|(addr, _, _)| *addr == USDS_TOKEN));
        
        // Check sUSDS is included
        assert!(tokens.iter().any(|(addr, _, _)| *addr == SUSDS_TOKEN));
        
        // Check USD3 is included
        assert!(tokens.iter().any(|(addr, _, _)| *addr == USD3_TOKEN));
    }
    
    #[test]
    fn test_symbol_map() {
        let map = build_expanded_symbol_map();
        
        assert_eq!(map.get(&USDS_TOKEN), Some(&"USDS"));
        assert_eq!(map.get(&SUSDS_TOKEN), Some(&"sUSDS"));
        assert_eq!(map.get(&USD3_TOKEN), Some(&"USD3"));
    }
}