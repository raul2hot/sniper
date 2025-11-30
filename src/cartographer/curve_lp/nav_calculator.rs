//! LP Token NAV Calculator
//!
//! Calculates Net Asset Value for Curve LP tokens and detects
//! arbitrage opportunities when secondary market price diverges.
//!
//! NAV CALCULATION:
//! NAV = virtual_price * min(underlying_prices)
//!
//! This is conservative (Chainlink methodology) - uses minimum
//! underlying price to avoid overvaluation.

use alloy_primitives::{Address, U256};
use std::collections::HashMap;
use tracing::{debug, warn};

use super::adapter::CachedLPPool;
use super::types::*;

// ============================================
// TYPES
// ============================================

/// NAV calculation result for an LP token
#[derive(Debug, Clone)]
pub struct LPNavResult {
    pub lp_token: Address,
    pub pool_address: Address,
    pub pool_name: String,

    /// Virtual price from Curve (18 decimals, always >= 1e18)
    pub virtual_price: U256,

    /// Calculated NAV in USD (18 decimals)
    pub nav_usd: U256,

    /// Prices of underlying tokens used (USD, 18 decimals)
    pub underlying_prices: Vec<U256>,

    /// Minimum underlying price (used for conservative NAV)
    pub min_underlying_price: U256,
}

/// Detected NAV arbitrage opportunity
#[derive(Debug, Clone)]
pub struct LPNavArbitrage {
    pub lp_token: Address,
    pub pool_address: Address,
    pub pool_name: String,

    /// NAV in USD (18 decimals)
    pub nav_usd: U256,

    /// Secondary market price in USD (18 decimals)
    pub market_price_usd: U256,

    /// Discount (positive) or premium (negative) in bps
    /// discount_bps = (nav - market_price) / nav * 10000
    pub discount_bps: i64,

    /// Recommended direction
    pub direction: LPArbDirection,

    /// Secondary market where LP token trades
    pub secondary_market: SecondaryMarket,

    /// Estimated profit in USD (before gas)
    pub estimated_profit_usd: f64,
}

/// Direction for LP NAV arbitrage
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LPArbDirection {
    /// Market price < NAV: Buy LP on secondary, redeem value via Curve
    BuySecondaryRedeemCurve,
    /// Market price > NAV: Mint LP on Curve, sell on secondary
    /// NOTE: This requires add_liquidity which executor doesn't support!
    /// We can only detect this, not execute it.
    MintCurveSellSecondary,
}

impl std::fmt::Display for LPArbDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LPArbDirection::BuySecondaryRedeemCurve => write!(f, "BuySecondary->RedeemCurve"),
            LPArbDirection::MintCurveSellSecondary => write!(f, "MintCurve->SellSecondary"),
        }
    }
}

/// Secondary market info
#[derive(Debug, Clone)]
pub struct SecondaryMarket {
    pub pool_address: Address,
    pub dex_type: SecondaryDex,
    pub fee_bps: u32,
    pub quote_token: Address,
    pub liquidity_usd: f64,
}

/// Type of secondary market DEX
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryDex {
    UniswapV3,
    Balancer,
    CurveMetapool,
}

impl std::fmt::Display for SecondaryDex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecondaryDex::UniswapV3 => write!(f, "UniV3"),
            SecondaryDex::Balancer => write!(f, "Balancer"),
            SecondaryDex::CurveMetapool => write!(f, "CurveMetapool"),
        }
    }
}

// ============================================
// NAV CALCULATOR
// ============================================

/// Calculator for LP token NAV and arbitrage detection
pub struct LPNavCalculator {
    /// Known stablecoin addresses -> assumed price of $1
    stablecoins: HashMap<Address, bool>,

    /// Price feeds (token -> price in USD * 1e18)
    /// In production, fetch these from Chainlink oracles or DEX pools
    price_feeds: HashMap<Address, U256>,
}

impl LPNavCalculator {
    pub fn new() -> Self {
        let mut stablecoins = HashMap::new();

        // Initialize stablecoins from types.rs
        for addr in STABLECOINS.iter() {
            stablecoins.insert(*addr, true);
        }

        Self {
            stablecoins,
            price_feeds: HashMap::new(),
        }
    }

    /// Update price feed for a token
    pub fn update_price(&mut self, token: Address, price_usd_1e18: U256) {
        self.price_feeds.insert(token, price_usd_1e18);
    }

    /// Set ETH price (used for stETH pools and ETH-related assets)
    pub fn set_eth_price(&mut self, price_usd: f64) {
        let price_1e18 = U256::from((price_usd * 1e18) as u128);

        // WETH
        self.price_feeds.insert(WETH, price_1e18);

        // stETH (assume 1:1 with ETH for simplicity)
        self.price_feeds.insert(STETH, price_1e18);

        // wstETH (slightly higher due to rebasing - approximately 1.15x)
        let wsteth_price = U256::from((price_usd * 1.15 * 1e18) as u128);
        self.price_feeds.insert(WSTETH, wsteth_price);
    }

    /// Get price for a token (USD * 1e18)
    fn get_token_price(&self, token: &Address) -> U256 {
        // Check if stablecoin
        if self.stablecoins.contains_key(token) {
            return U256::from(10u64).pow(U256::from(18)); // $1
        }

        // Check price feeds
        if let Some(price) = self.price_feeds.get(token) {
            return *price;
        }

        // Default: assume $1 (conservative for stablecoin pools)
        warn!("No price feed for {:?}, assuming $1", token);
        U256::from(10u64).pow(U256::from(18))
    }

    /// Calculate NAV for an LP token
    ///
    /// NAV = virtual_price * min(underlying_prices) / 1e18
    ///
    /// Using minimum price is conservative and standard practice (Chainlink).
    pub fn calculate_nav(&self, pool: &CachedLPPool, virtual_price: U256) -> LPNavResult {
        // Get prices for all underlying tokens
        let underlying_prices: Vec<U256> = pool
            .coins
            .iter()
            .map(|coin| self.get_token_price(coin))
            .collect();

        // Find minimum price (conservative NAV)
        let min_price = underlying_prices
            .iter()
            .min()
            .copied()
            .unwrap_or(U256::from(10u64).pow(U256::from(18)));

        // NAV = virtual_price * min_price / 1e18
        let nav = virtual_price * min_price / U256::from(10u64).pow(U256::from(18));

        LPNavResult {
            lp_token: pool.lp_token,
            pool_address: pool.pool_address,
            pool_name: pool.name.clone(),
            virtual_price,
            nav_usd: nav,
            underlying_prices,
            min_underlying_price: min_price,
        }
    }

    /// Detect arbitrage opportunity
    pub fn detect_arbitrage(
        &self,
        nav_result: &LPNavResult,
        market_price_usd: U256,
        secondary_market: SecondaryMarket,
    ) -> Option<LPNavArbitrage> {
        let nav = nav_result.nav_usd;

        if nav == U256::ZERO || market_price_usd == U256::ZERO {
            return None;
        }

        // Calculate discount/premium in bps
        // discount_bps = (nav - market_price) / nav * 10000
        let discount_bps = if nav > market_price_usd {
            // Trading at discount (opportunity to buy)
            let diff = nav - market_price_usd;
            (diff * U256::from(10000) / nav).to::<i64>()
        } else {
            // Trading at premium (opportunity to sell, but we can't mint)
            let diff = market_price_usd - nav;
            -((diff * U256::from(10000) / nav).to::<i64>())
        };

        // Check thresholds
        let threshold = (MIN_NAV_DISCOUNT_BPS + GAS_BUFFER_BPS) as i64;

        let direction = if discount_bps >= threshold {
            // Profitable to buy on secondary and redeem
            LPArbDirection::BuySecondaryRedeemCurve
        } else if discount_bps <= -(MAX_NAV_PREMIUM_BPS as i64) {
            // Could mint and sell, but executor doesn't support add_liquidity
            // Log but don't create opportunity
            debug!(
                "LP {} trading at {}bps premium - mint/sell not supported",
                nav_result.pool_name, -discount_bps
            );
            return None;
        } else {
            // Not enough edge
            return None;
        };

        // Calculate estimated profit
        let nav_f64 = nav.to::<u128>() as f64 / 1e18;
        let discount_pct = discount_bps as f64 / 10000.0;
        let estimated_profit = nav_f64 * discount_pct;

        Some(LPNavArbitrage {
            lp_token: nav_result.lp_token,
            pool_address: nav_result.pool_address,
            pool_name: nav_result.pool_name.clone(),
            nav_usd: nav,
            market_price_usd,
            discount_bps,
            direction,
            secondary_market,
            estimated_profit_usd: estimated_profit,
        })
    }

    /// Batch calculate NAV for multiple LP tokens
    pub fn batch_calculate_nav(
        &self,
        pools: &[CachedLPPool],
        virtual_prices: &HashMap<Address, U256>,
    ) -> Vec<LPNavResult> {
        let mut results = Vec::new();

        for pool in pools {
            if let Some(vp) = virtual_prices.get(&pool.lp_token) {
                let nav_result = self.calculate_nav(pool, *vp);
                results.push(nav_result);
            }
        }

        results
    }

    /// Check all pools for arbitrage opportunities
    pub fn scan_for_opportunities(
        &self,
        nav_results: &[LPNavResult],
        market_prices: &HashMap<Address, (U256, SecondaryMarket)>,
    ) -> Vec<LPNavArbitrage> {
        let mut opportunities = Vec::new();

        for nav_result in nav_results {
            if let Some((market_price, market)) = market_prices.get(&nav_result.lp_token) {
                if let Some(arb) = self.detect_arbitrage(nav_result, *market_price, market.clone())
                {
                    opportunities.push(arb);
                }
            }
        }

        // Sort by discount (highest first)
        opportunities.sort_by(|a, b| b.discount_bps.cmp(&a.discount_bps));

        opportunities
    }
}

impl Default for LPNavCalculator {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================
// SAFETY CHECKS
// ============================================

/// Minimum liquidity in secondary market to consider trading
pub const MIN_SECONDARY_LIQUIDITY_USD: f64 = 50_000.0;

/// Don't trade more than 10% of secondary market liquidity
pub const MAX_TRADE_PCT_OF_LIQUIDITY: f64 = 0.10;

/// Calculate safe trade amount based on market liquidity
pub fn safe_trade_amount(desired_amount_usd: f64, market_liquidity_usd: f64) -> f64 {
    let max_safe = market_liquidity_usd * MAX_TRADE_PCT_OF_LIQUIDITY;
    desired_amount_usd.min(max_safe)
}

/// Validate market has sufficient liquidity
pub fn validate_market_liquidity(market: &SecondaryMarket) -> bool {
    market.liquidity_usd >= MIN_SECONDARY_LIQUIDITY_USD
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    fn create_test_pool() -> CachedLPPool {
        CachedLPPool {
            pool_address: Address::ZERO,
            lp_token: Address::ZERO,
            name: "test-3pool".to_string(),
            coins: vec![
                address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"), // USDC
                address!("dAC17F958D2ee523a2206206994597C13D831ec7"), // USDT
                address!("6B175474E89094C44Da98b954EedcdeCB5BE3830"), // DAI
            ],
            coin_decimals: vec![6, 6, 18],
            n_coins: 3,
            is_metapool: false,
            base_pool: None,
        }
    }

    #[test]
    fn test_nav_calculation_stablecoin_pool() {
        let calc = LPNavCalculator::new();

        let pool = create_test_pool();

        // Virtual price = 1.02 (2% appreciation from fees)
        let virtual_price = U256::from(102u64) * U256::from(10u64).pow(U256::from(16));

        let result = calc.calculate_nav(&pool, virtual_price);

        // NAV should be ~$1.02 (all stablecoins at $1)
        let nav_usd = result.nav_usd.to::<u128>() as f64 / 1e18;
        assert!((nav_usd - 1.02).abs() < 0.001);
    }

    #[test]
    fn test_arbitrage_detection_discount() {
        let calc = LPNavCalculator::new();

        let nav_result = LPNavResult {
            lp_token: Address::ZERO,
            pool_address: Address::ZERO,
            pool_name: "test".to_string(),
            virtual_price: U256::from(10u64).pow(U256::from(18)),
            nav_usd: U256::from(10u64).pow(U256::from(18)), // $1.00
            underlying_prices: vec![U256::from(10u64).pow(U256::from(18))],
            min_underlying_price: U256::from(10u64).pow(U256::from(18)),
        };

        // Market price = $0.995 (50 bps discount)
        let market_price = U256::from(995u64) * U256::from(10u64).pow(U256::from(15));

        let market = SecondaryMarket {
            pool_address: Address::ZERO,
            dex_type: SecondaryDex::UniswapV3,
            fee_bps: 30,
            quote_token: Address::ZERO,
            liquidity_usd: 100_000.0,
        };

        let arb = calc.detect_arbitrage(&nav_result, market_price, market);

        assert!(arb.is_some());
        let arb = arb.unwrap();
        assert_eq!(arb.direction, LPArbDirection::BuySecondaryRedeemCurve);
        assert!(arb.discount_bps > 0);
    }

    #[test]
    fn test_no_arbitrage_small_discount() {
        let calc = LPNavCalculator::new();

        let nav_result = LPNavResult {
            lp_token: Address::ZERO,
            pool_address: Address::ZERO,
            pool_name: "test".to_string(),
            virtual_price: U256::from(10u64).pow(U256::from(18)),
            nav_usd: U256::from(10u64).pow(U256::from(18)), // $1.00
            underlying_prices: vec![U256::from(10u64).pow(U256::from(18))],
            min_underlying_price: U256::from(10u64).pow(U256::from(18)),
        };

        // Market price = $0.999 (10 bps discount - below threshold)
        let market_price = U256::from(999u64) * U256::from(10u64).pow(U256::from(15));

        let market = SecondaryMarket {
            pool_address: Address::ZERO,
            dex_type: SecondaryDex::UniswapV3,
            fee_bps: 30,
            quote_token: Address::ZERO,
            liquidity_usd: 100_000.0,
        };

        let arb = calc.detect_arbitrage(&nav_result, market_price, market);

        // Should return None because discount is below threshold
        assert!(arb.is_none());
    }

    #[test]
    fn test_safe_trade_amount() {
        // Desired $50k trade, $100k liquidity -> max $10k safe
        assert_eq!(safe_trade_amount(50_000.0, 100_000.0), 10_000.0);

        // Desired $5k trade, $100k liquidity -> $5k is fine
        assert_eq!(safe_trade_amount(5_000.0, 100_000.0), 5_000.0);
    }

    #[test]
    fn test_validate_market_liquidity() {
        let good_market = SecondaryMarket {
            pool_address: Address::ZERO,
            dex_type: SecondaryDex::UniswapV3,
            fee_bps: 30,
            quote_token: Address::ZERO,
            liquidity_usd: 100_000.0,
        };

        let bad_market = SecondaryMarket {
            pool_address: Address::ZERO,
            dex_type: SecondaryDex::UniswapV3,
            fee_bps: 30,
            quote_token: Address::ZERO,
            liquidity_usd: 10_000.0, // Below minimum
        };

        assert!(validate_market_liquidity(&good_market));
        assert!(!validate_market_liquidity(&bad_market));
    }
}
