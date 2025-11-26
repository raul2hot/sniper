//! Pool Data Fetcher
//!
//! Step 1.1: The Scout
//!
//! Connects to RPC and fetches pool state (liquidity, sqrtPriceX96) for V3 pools.
//!
//! Success Criteria:
//! - Console logs: "Fetched 124 pools. WETH/USDC Price: 3105.40"

use alloy::{
    primitives::{Address, U256},
    providers::ProviderBuilder,
    sol,
};
use eyre::{eyre, Result};
use std::str::FromStr;
use tracing::{info, warn};

// Define the Uniswap V3 Pool interface using alloy's sol! macro
sol! {
    #[sol(rpc)]
    interface IUniswapV3Pool {
        function slot0() external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint16 observationIndex,
            uint16 observationCardinality,
            uint16 observationCardinalityNext,
            uint8 feeProtocol,
            bool unlocked
        );
        
        function liquidity() external view returns (uint128);
        function token0() external view returns (address);
        function token1() external view returns (address);
        function fee() external view returns (uint24);
    }
}

/// Represents a Uniswap pool's current state
#[derive(Debug, Clone)]
pub struct PoolState {
    /// Pool contract address
    pub address: Address,
    /// Token 0 address
    pub token0: Address,
    /// Token 1 address
    pub token1: Address,
    /// Current sqrt price (Q64.96 format)
    pub sqrt_price_x96: U256,
    /// Current tick
    pub tick: i32,
    /// Available liquidity
    pub liquidity: u128,
    /// Fee tier (500 = 0.05%, 3000 = 0.3%, 10000 = 1%)
    pub fee: u32,
    /// Is this a V4 pool?
    pub is_v4: bool,
}

impl PoolState {
    /// Calculate the price of token0 in terms of token1
    /// Adjusts for decimal differences between tokens
    pub fn price(&self, token0_decimals: u8, token1_decimals: u8) -> f64 {
        // sqrtPriceX96 = sqrt(price) * 2^96
        // price = (sqrtPriceX96 / 2^96)^2
        let sqrt_price_x96 = self.sqrt_price_x96.to::<u128>() as f64;
        let q96 = 2_f64.powi(96);
        let price = (sqrt_price_x96 / q96).powi(2);
        
        // Adjust for decimals: price * 10^(token0_decimals - token1_decimals)
        let decimal_adjustment = 10_f64.powi(token0_decimals as i32 - token1_decimals as i32);
        price * decimal_adjustment
    }

    /// Calculate the price of token1 in terms of token0
    pub fn inverse_price(&self, token0_decimals: u8, token1_decimals: u8) -> f64 {
        1.0 / self.price(token0_decimals, token1_decimals)
    }
    
    /// Get raw price without decimal adjustment (for graph weights)
    pub fn raw_price(&self) -> f64 {
        let sqrt_price_x96 = self.sqrt_price_x96.to::<u128>() as f64;
        let q96 = 2_f64.powi(96);
        (sqrt_price_x96 / q96).powi(2)
    }
}

/// Known Uniswap V3 pool addresses on Ethereum Mainnet
/// Format: (pool_address, token0_symbol, token1_symbol, fee_tier)
pub fn get_known_pools() -> Vec<(&'static str, &'static str, &'static str, u32)> {
    vec![
        // ============================================
        // WETH <-> Stablecoin pairs (multiple fee tiers = arb potential)
        // ============================================
        ("0x88e6A0c2dDD26FEEb64F039a2c41296FcB3f5640", "USDC", "WETH", 500),    // USDC/WETH 0.05%
        ("0x8ad599c3A0ff1De082011EFDDc58f1908eb6e6D8", "USDC", "WETH", 3000),   // USDC/WETH 0.3%
        ("0xE0554a476A092703abdB3Ef35c80e0D76d32939F", "USDC", "WETH", 10000),  // USDC/WETH 1%
        
        ("0x11b815efB8f581194ae79006d24E0d814B7697F6", "WETH", "USDT", 500),    // WETH/USDT 0.05%
        ("0x4e68Ccd3E89f51C3074ca5072bbAC773960dFa36", "WETH", "USDT", 3000),   // WETH/USDT 0.3%
        
        ("0x60594a405d53811d3BC4766596EFD80fd545A270", "DAI", "WETH", 500),     // DAI/WETH 0.05%
        ("0xC2e9F25Be6257c210d7Adf0D4Cd6E3E881ba25f8", "DAI", "WETH", 3000),    // DAI/WETH 0.3%
        
        // ============================================
        // Stablecoin triangles (USDC <-> USDT <-> DAI)
        // ============================================
        ("0x3416cF6C708Da44DB2624D63ea0AAef7113527C6", "USDC", "USDT", 100),    // USDC/USDT 0.01%
        ("0x7858E59e0C01EA06Df3aF3D20aC7B0003275D4Bf", "USDC", "USDT", 500),    // USDC/USDT 0.05%
        
        ("0x5777d92f208679DB4b9778590Fa3CAB3aC9e2168", "DAI", "USDC", 100),     // DAI/USDC 0.01%
        ("0x6c6Bc977E13Df9b0de53b251522280BB72383700", "DAI", "USDC", 500),     // DAI/USDC 0.05%
        
        ("0x6f48ECa74B38d2936B02ab603FF4e36A6C0E3A77", "DAI", "USDT", 500),     // DAI/USDT 0.05%
        
        // ============================================
        // WBTC pairs
        // ============================================
        ("0xCBCdF9626bC03E24f779434178A73a0B4bad62eD", "WBTC", "WETH", 3000),   // WBTC/WETH 0.3%
        ("0x4585FE77225b41b697C938B018E2Ac67Ac5a20c0", "WBTC", "WETH", 500),    // WBTC/WETH 0.05%
        ("0x99ac8cA7087fA4A2A1FB6357269965A2014ABc35", "WBTC", "USDC", 3000),   // WBTC/USDC 0.3%
        
        // ============================================
        // DeFi blue chips
        // ============================================
        ("0xa6Cc3C2531FdaA6Ae1A3CA84c2855806728693e8", "LINK", "WETH", 3000),   // LINK/WETH 0.3%
        ("0x1d42064Fc4Beb5F8aAF85F4617AE8b3b5B8Bd801", "UNI", "WETH", 3000),    // UNI/WETH 0.3%
        ("0xa3f558aebAecAf0e11cA4b2199cC5Ed341edfd74", "LDO", "WETH", 3000),    // LDO/WETH 0.3%
        ("0xe8c6c9227491C0a8156A0106A0204d881BB7E531", "MKR", "WETH", 3000),    // MKR/WETH 0.3%
        ("0x290A6a7460B308ee3F19023D2D00dE604bcf5B42", "MATIC", "WETH", 3000),  // MATIC/WETH 0.3%
        
        // ============================================
        // High volatility memecoins
        // ============================================
        ("0x11950d141EcB863F01007AdD7D1A342041227b58", "PEPE", "WETH", 3000),   // PEPE/WETH 0.3%
        ("0x2F62f2B4c5fcd7570a709DeC05D68EA19c82A9ec", "SHIB", "WETH", 3000),   // SHIB/WETH 0.3%
    ]
}

/// Pool data fetcher using Alloy
pub struct PoolFetcher {
    /// RPC URL
    rpc_url: String,
}

impl PoolFetcher {
    /// Create a new fetcher with the given RPC URL
    pub fn new(rpc_url: String) -> Self {
        Self { rpc_url }
    }

    /// Fetch pool state for a V3 pool
    #[allow(deprecated)] // on_http is deprecated but works fine for now
    pub async fn fetch_v3_pool(&self, pool_address: Address) -> Result<PoolState> {
        // Create provider
        let provider = ProviderBuilder::new()
            .on_http(self.rpc_url.parse()?);

        // Create contract instance
        let pool = IUniswapV3Pool::new(pool_address, &provider);

        // Fetch slot0 (price and tick)
        let slot0 = pool.slot0().call().await.map_err(|e| eyre!("Failed to fetch slot0: {}", e))?;
        
        // Fetch liquidity
        let liquidity_result = pool.liquidity().call().await.map_err(|e| eyre!("Failed to fetch liquidity: {}", e))?;
        
        // Fetch token addresses
        let token0_result = pool.token0().call().await.map_err(|e| eyre!("Failed to fetch token0: {}", e))?;
        let token1_result = pool.token1().call().await.map_err(|e| eyre!("Failed to fetch token1: {}", e))?;
        
        // Fetch fee
        let fee_result = pool.fee().call().await.map_err(|e| eyre!("Failed to fetch fee: {}", e))?;

        // Convert tick from Signed<24, 1> to i32
        let tick_i32: i32 = slot0.tick.as_i32();

        Ok(PoolState {
            address: pool_address,
            token0: token0_result,
            token1: token1_result,
            sqrt_price_x96: U256::from(slot0.sqrtPriceX96),
            tick: tick_i32,
            liquidity: liquidity_result,
            fee: fee_result.to::<u32>(),
            is_v4: false,
        })
    }

    /// Fetch all known pools
    pub async fn fetch_all_pools(&self) -> Result<Vec<PoolState>> {
        info!("Fetching pool data from RPC...");
        
        let known_pools = get_known_pools();
        let mut pools = Vec::new();
        let mut success_count = 0;
        let mut fail_count = 0;

        for (pool_addr, token0_sym, token1_sym, fee) in &known_pools {
            let address = Address::from_str(pool_addr)?;
            
            match self.fetch_v3_pool(address).await {
                Ok(pool) => {
                    // Calculate human-readable price for logging
                    let (t0_dec, t1_dec) = get_decimals(token0_sym, token1_sym);
                    let price = pool.price(t0_dec, t1_dec);
                    
                    info!(
                        "✓ {}/{} ({}bps): price={:.6}, liq={}", 
                        token0_sym, token1_sym, fee / 100,
                        price, pool.liquidity
                    );
                    
                    pools.push(pool);
                    success_count += 1;
                }
                Err(e) => {
                    warn!("✗ Failed to fetch {}/{}: {}", token0_sym, token1_sym, e);
                    fail_count += 1;
                }
            }
        }

        info!(
            "Fetched {} pools successfully ({} failed)",
            success_count, fail_count
        );

        if pools.is_empty() {
            return Err(eyre!("No pools fetched! Check your RPC URL."));
        }

        Ok(pools)
    }
}

/// Get decimal places for known tokens
fn get_decimals(token0: &str, token1: &str) -> (u8, u8) {
    let dec = |symbol: &str| -> u8 {
        match symbol {
            "USDC" | "USDT" => 6,
            "WBTC" => 8,
            _ => 18, // WETH, DAI, LINK, UNI, PEPE, SHIB, etc.
        }
    };
    (dec(token0), dec(token1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_price_calculation() {
        let pool = PoolState {
            address: Address::ZERO,
            token0: Address::ZERO,
            token1: Address::ZERO,
            sqrt_price_x96: U256::from(1_500_000_000_000_000_000_000_000_000_u128),
            tick: 0,
            liquidity: 1000000,
            fee: 3000,
            is_v4: false,
        };

        let raw_price = pool.raw_price();
        assert!(raw_price > 0.0, "Price should be positive");
    }

    #[test]
    fn test_known_pools_exist() {
        let pools = get_known_pools();
        assert!(!pools.is_empty(), "Should have known pools");
        assert!(pools.len() >= 10, "Should have at least 10 pools");
    }
}