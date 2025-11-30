//! Curve LP Token NAV Arbitrage Module
//!
//! Discovers Curve LP tokens, calculates their NAV (Net Asset Value),
//! and finds arbitrage opportunities when secondary market prices diverge.
//!
//! ## Key Components
//!
//! - **CurveLPAdapter**: Discovers Curve LP tokens and fetches virtual prices
//! - **LPNavCalculator**: Calculates NAV and detects arbitrage opportunities
//! - **LPMarketDiscovery**: Finds secondary markets (UniV3) for LP tokens
//!
//! ## Arbitrage Strategy
//!
//! When LP token price on secondary market < NAV:
//! 1. Buy LP token on UniV3 (cheaper)
//! 2. Redeem value via Curve pool exchange
//! 3. Profit from discount
//!
//! ## Important Constraints
//!
//! - Executor contract is IMMUTABLE - no Solidity changes
//! - Only UniV3 secondary markets are tradeable (executor supports)
//! - No add_liquidity/remove_liquidity - only exchange operations
//! - Aggressive caching to minimize RPC calls

mod adapter;
mod market_discovery;
mod nav_calculator;
mod types;

// Re-export main types and structs
pub use adapter::{
    get_token_decimals, validate_virtual_price, CachedLPPool, CurveLPAdapter,
};

pub use nav_calculator::{
    safe_trade_amount, validate_market_liquidity, LPArbDirection, LPNavArbitrage, LPNavCalculator,
    LPNavResult, SecondaryDex, SecondaryMarket, MAX_TRADE_PCT_OF_LIQUIDITY,
    MIN_SECONDARY_LIQUIDITY_USD,
};

pub use market_discovery::{
    calculate_lp_price_from_sqrt, estimate_market_liquidity_usd, LPMarketDiscovery,
};

pub use types::{
    is_stablecoin, ICurveFactory, ICurveMetaRegistry, ICurvePool, IERC20, IMulticall3,
    IUniswapV3Factory, IUniswapV3Pool, BALANCER_VAULT, CURVE_ADDRESS_PROVIDER, CURVE_META_REGISTRY,
    CURVE_NG_FACTORY, CURVE_TWOCRYPTO_FACTORY, DISCOVERY_THROTTLE_INTERVAL, GAS_BUFFER_BPS,
    LP_POOLS, MARKET_CACHE_SECS, MAX_NAV_PREMIUM_BPS, MIN_MARKET_LIQUIDITY_USD,
    MIN_NAV_DISCOUNT_BPS, MULTICALL3, POOL_STRUCTURE_CACHE_SECS, QUOTE_TOKENS, STABLECOINS, STETH,
    UNISWAP_V3_FACTORY, UNIV3_FEE_TIERS, VIRTUAL_PRICE_CACHE_SECS, WETH, WSTETH,
};

/// LP NAV Fetch Result - aggregates all LP data for a scan
#[derive(Debug, Default)]
pub struct LPNavFetchResult {
    /// Pool states for secondary markets (add to routing graph)
    pub pool_states: Vec<crate::cartographer::PoolState>,

    /// Discovered LP pools
    pub lp_pools: Vec<CachedLPPool>,

    /// Virtual prices (LP token -> price)
    pub virtual_prices: std::collections::HashMap<alloy_primitives::Address, alloy_primitives::U256>,

    /// NAV calculations for each pool
    pub nav_results: Vec<LPNavResult>,

    /// Detected arbitrage opportunities
    pub opportunities: Vec<LPNavArbitrage>,

    /// Number of secondary markets discovered
    pub secondary_markets_count: usize,
}

impl LPNavFetchResult {
    /// Get summary of fetch result
    pub fn summary(&self) -> String {
        format!(
            "{} LP pools, {} virtual prices, {} secondary markets, {} opportunities",
            self.lp_pools.len(),
            self.virtual_prices.len(),
            self.secondary_markets_count,
            self.opportunities.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // Verify key types are exported
        let _ = LPNavCalculator::new();
        let _ = CurveLPAdapter::new("http://localhost:8545".to_string());
        let _ = LPMarketDiscovery::new("http://localhost:8545".to_string());
    }

    #[test]
    fn test_constants_exported() {
        assert!(MIN_NAV_DISCOUNT_BPS > 0);
        assert!(POOL_STRUCTURE_CACHE_SECS > 0);
        assert!(!LP_POOLS.is_empty());
        assert!(!STABLECOINS.is_empty());
    }
}
