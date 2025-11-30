//! Curve LP Token Adapter
//!
//! Discovers Curve LP tokens and their associated pools.
//! Uses aggressive caching to minimize RPC calls.
//!
//! RPC OPTIMIZATION:
//! - Pool structure cached for 5 minutes (rarely changes)
//! - Virtual prices cached for 60 seconds (slow-moving)
//! - Discovery throttled to every 10th scan

use alloy_primitives::{Address, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types::TransactionRequest;
use alloy_sol_types::SolCall;
use eyre::{eyre, Result};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::types::*;

// ============================================
// CACHED DATA STRUCTURES
// ============================================

/// Cached LP pool metadata (rarely changes)
#[derive(Debug, Clone)]
pub struct CachedLPPool {
    pub pool_address: Address,
    pub lp_token: Address,
    pub name: String,
    pub coins: Vec<Address>,
    pub coin_decimals: Vec<u8>,
    pub n_coins: usize,
    pub is_metapool: bool,
    pub base_pool: Option<Address>, // For metapools
}

/// Cached virtual price with timestamp
#[derive(Debug, Clone)]
struct CachedVirtualPrice {
    pub price: U256,
    pub cached_at: Instant,
}

/// LP Adapter cache
struct LPCache {
    /// Pool structure cache
    pools: HashMap<Address, CachedLPPool>,
    pools_last_updated: Option<Instant>,

    /// Virtual price cache (LP token -> price)
    virtual_prices: HashMap<Address, CachedVirtualPrice>,

    /// Scan counter for throttling
    scan_counter: u64,
}

impl Default for LPCache {
    fn default() -> Self {
        Self {
            pools: HashMap::new(),
            pools_last_updated: None,
            virtual_prices: HashMap::new(),
            scan_counter: 0,
        }
    }
}

lazy_static::lazy_static! {
    static ref LP_CACHE: RwLock<LPCache> = RwLock::new(LPCache::default());
}

// ============================================
// CURVE LP ADAPTER
// ============================================

/// Adapter for Curve LP token discovery and pricing
pub struct CurveLPAdapter {
    rpc_url: String,
}

impl CurveLPAdapter {
    pub fn new(rpc_url: String) -> Self {
        Self { rpc_url }
    }

    // ============================================
    // MULTICALL HELPERS
    // ============================================

    /// Execute Multicall3 batch - SINGLE RPC call
    async fn execute_multicall(
        &self,
        calls: Vec<IMulticall3::Call3>,
    ) -> Result<Vec<IMulticall3::Result>> {
        if calls.is_empty() {
            return Ok(Vec::new());
        }

        let provider = ProviderBuilder::new().on_http(self.rpc_url.parse()?);

        let calldata = IMulticall3::aggregate3Call { calls }.abi_encode();

        let tx = TransactionRequest::default()
            .to(MULTICALL3)
            .input(calldata.into());

        let result = provider
            .call(tx)
            .await
            .map_err(|e| eyre!("Multicall3 failed: {}", e))?;

        let decoded = IMulticall3::aggregate3Call::abi_decode_returns(&result)
            .map_err(|e| eyre!("Failed to decode multicall: {}", e))?;

        Ok(decoded)
    }

    // ============================================
    // POOL DISCOVERY (THROTTLED)
    // ============================================

    /// Check if we should run discovery this scan
    pub fn should_discover(&self) -> bool {
        let mut cache = LP_CACHE.write().unwrap();
        cache.scan_counter += 1;

        // Run discovery on first scan or every Nth scan
        if cache.scan_counter == 1 {
            return true;
        }

        if cache.scan_counter % DISCOVERY_THROTTLE_INTERVAL == 0 {
            return true;
        }

        // Also run if cache is stale
        if let Some(last_updated) = cache.pools_last_updated {
            if last_updated.elapsed() > Duration::from_secs(POOL_STRUCTURE_CACHE_SECS) {
                return true;
            }
        } else {
            return true;
        }

        false
    }

    /// Get cached pools or discover if needed
    pub async fn get_lp_pools(&self) -> Result<Vec<CachedLPPool>> {
        // Check if we have valid cache
        {
            let cache = LP_CACHE.read().unwrap();
            if let Some(last_updated) = cache.pools_last_updated {
                if last_updated.elapsed() < Duration::from_secs(POOL_STRUCTURE_CACHE_SECS) {
                    debug!("Using cached LP pool data ({} pools)", cache.pools.len());
                    return Ok(cache.pools.values().cloned().collect());
                }
            }
        }

        // Need to fetch
        info!("Discovering Curve LP pools...");
        self.discover_lp_pools().await
    }

    /// Discover LP pools from static list + factory
    /// Uses 1-2 multicalls total
    async fn discover_lp_pools(&self) -> Result<Vec<CachedLPPool>> {
        let mut pools = Vec::new();

        // ============================================
        // BATCH 1: Fetch data for known high-TVL pools
        // ============================================

        let mut calls = Vec::new();

        // For each known pool, fetch: coins[0], coins[1], coins[2], coins[3]
        for (pool_addr, _lp_addr, _name) in LP_POOLS.iter() {
            // Try to get 4 coins (most pools have 2-4)
            for i in 0u8..4 {
                calls.push(IMulticall3::Call3 {
                    target: *pool_addr,
                    allowFailure: true, // Some pools have fewer coins
                    callData: ICurvePool::coinsCall { i: U256::from(i) }
                        .abi_encode()
                        .into(),
                });
            }

            // Get virtual price
            calls.push(IMulticall3::Call3 {
                target: *pool_addr,
                allowFailure: true,
                callData: ICurvePool::get_virtual_priceCall {}.abi_encode().into(),
            });
        }

        debug!(
            "LP Discovery: fetching {} calls in 1 multicall",
            calls.len()
        );
        let results = self.execute_multicall(calls).await?;

        // Parse results
        let calls_per_pool = 5; // 4 coins + 1 virtual_price

        for (idx, (pool_addr, lp_addr, name)) in LP_POOLS.iter().enumerate() {
            let base_idx = idx * calls_per_pool;

            // Parse coins
            let mut coins = Vec::new();
            let mut coin_decimals = Vec::new();

            for i in 0..4 {
                let result_idx = base_idx + i;
                if result_idx < results.len() && results[result_idx].success {
                    if let Ok(coin) =
                        ICurvePool::coinsCall::abi_decode_returns(&results[result_idx].returnData)
                    {
                        if coin != Address::ZERO {
                            coins.push(coin);
                            // Get decimals for known tokens
                            let decimals = get_token_decimals(&coin);
                            coin_decimals.push(decimals);
                        }
                    }
                }
            }

            if coins.is_empty() {
                warn!("No coins found for pool {}", name);
                continue;
            }

            // Parse virtual price (for cache)
            let vp_idx = base_idx + 4;
            if vp_idx < results.len() && results[vp_idx].success {
                if let Ok(vp) =
                    ICurvePool::get_virtual_priceCall::abi_decode_returns(&results[vp_idx].returnData)
                {
                    let mut cache = LP_CACHE.write().unwrap();
                    cache.virtual_prices.insert(
                        *lp_addr,
                        CachedVirtualPrice {
                            price: vp,
                            cached_at: Instant::now(),
                        },
                    );
                }
            }

            let pool = CachedLPPool {
                pool_address: *pool_addr,
                lp_token: *lp_addr,
                name: name.to_string(),
                n_coins: coins.len(),
                coins,
                coin_decimals,
                is_metapool: false, // Detect metapools based on coin count
                base_pool: None,
            };

            pools.push(pool);
        }

        // Update cache
        {
            let mut cache = LP_CACHE.write().unwrap();
            cache.pools.clear();
            for pool in &pools {
                cache.pools.insert(pool.lp_token, pool.clone());
            }
            cache.pools_last_updated = Some(Instant::now());
        }

        info!("Discovered {} Curve LP pools", pools.len());
        Ok(pools)
    }

    // ============================================
    // VIRTUAL PRICE FETCHING (BATCHED)
    // ============================================

    /// Fetch virtual prices for all LP tokens in ONE multicall
    /// Returns map: LP token address -> virtual price (U256, 18 decimals)
    pub async fn fetch_virtual_prices(
        &self,
        lp_tokens: &[Address],
    ) -> Result<HashMap<Address, U256>> {
        let mut result_map = HashMap::new();
        let mut tokens_to_fetch = Vec::new();

        // Check cache first
        {
            let cache = LP_CACHE.read().unwrap();
            for lp_token in lp_tokens {
                if let Some(cached) = cache.virtual_prices.get(lp_token) {
                    if cached.cached_at.elapsed() < Duration::from_secs(VIRTUAL_PRICE_CACHE_SECS) {
                        result_map.insert(*lp_token, cached.price);
                    } else {
                        tokens_to_fetch.push(*lp_token);
                    }
                } else {
                    tokens_to_fetch.push(*lp_token);
                }
            }
        }

        if tokens_to_fetch.is_empty() {
            debug!("All {} virtual prices from cache", result_map.len());
            return Ok(result_map);
        }

        // Build multicall for missing prices
        let mut calls = Vec::new();
        let mut token_to_pool: HashMap<Address, Address> = HashMap::new();

        {
            let cache = LP_CACHE.read().unwrap();
            for lp_token in &tokens_to_fetch {
                if let Some(pool_info) = cache.pools.get(lp_token) {
                    calls.push(IMulticall3::Call3 {
                        target: pool_info.pool_address,
                        allowFailure: true,
                        callData: ICurvePool::get_virtual_priceCall {}.abi_encode().into(),
                    });
                    token_to_pool.insert(*lp_token, pool_info.pool_address);
                }
            }
        }

        if calls.is_empty() {
            return Ok(result_map);
        }

        debug!("Fetching {} virtual prices in 1 multicall", calls.len());
        let results = self.execute_multicall(calls).await?;

        // Parse and cache results
        let mut cache = LP_CACHE.write().unwrap();
        for (i, lp_token) in tokens_to_fetch.iter().enumerate() {
            if i < results.len() && results[i].success {
                if let Ok(vp) =
                    ICurvePool::get_virtual_priceCall::abi_decode_returns(&results[i].returnData)
                {
                    result_map.insert(*lp_token, vp);
                    cache.virtual_prices.insert(
                        *lp_token,
                        CachedVirtualPrice {
                            price: vp,
                            cached_at: Instant::now(),
                        },
                    );
                }
            }
        }

        Ok(result_map)
    }

    // ============================================
    // UTILITY FUNCTIONS
    // ============================================

    /// Get pool info for an LP token
    pub fn get_pool_for_lp(&self, lp_token: &Address) -> Option<CachedLPPool> {
        let cache = LP_CACHE.read().unwrap();
        cache.pools.get(lp_token).cloned()
    }

    /// Get all tracked LP tokens
    pub fn get_all_lp_tokens(&self) -> Vec<Address> {
        let cache = LP_CACHE.read().unwrap();
        cache.pools.keys().cloned().collect()
    }

    /// Clear all caches (for testing)
    #[cfg(test)]
    pub fn clear_cache(&self) {
        let mut cache = LP_CACHE.write().unwrap();
        cache.pools.clear();
        cache.virtual_prices.clear();
        cache.pools_last_updated = None;
    }

    /// Get current scan counter (for testing/debugging)
    pub fn get_scan_counter(&self) -> u64 {
        let cache = LP_CACHE.read().unwrap();
        cache.scan_counter
    }
}

// ============================================
// HELPER FUNCTIONS
// ============================================

/// Get token decimals for common tokens
pub fn get_token_decimals(address: &Address) -> u8 {
    let a = format!("{:?}", address).to_lowercase();

    // USDC and USDT have 6 decimals
    if a.contains("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")
        || a.contains("dac17f958d2ee523a2206206994597c13d831ec7")
    {
        return 6;
    }

    // WBTC has 8 decimals
    if a.contains("2260fac5e5542a773aa44fbcfedf7c193bc2c599") {
        return 8;
    }

    // Default to 18 decimals
    18
}

/// Validate virtual price is in expected range
pub fn validate_virtual_price(vp: U256, pool_name: &str) -> bool {
    let vp_f64 = vp.to::<u128>() as f64 / 1e18;

    // Virtual price should be >= 1.0 (starts at 1.0)
    if vp_f64 < 1.0 {
        warn!(
            "Invalid virtual_price {} for {} (< 1.0)",
            vp_f64, pool_name
        );
        return false;
    }

    // Virtual price should be < 2.0 (grows slowly over years)
    if vp_f64 > 2.0 {
        warn!(
            "Suspicious virtual_price {} for {} (> 2.0)",
            vp_f64, pool_name
        );
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    #[test]
    fn test_get_token_decimals() {
        let usdc = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
        let usdt = address!("dAC17F958D2ee523a2206206994597C13D831ec7");
        let wbtc = address!("2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599");
        let dai = address!("6B175474E89094C44Da98b954EedcdeCB5BE3830");

        assert_eq!(get_token_decimals(&usdc), 6);
        assert_eq!(get_token_decimals(&usdt), 6);
        assert_eq!(get_token_decimals(&wbtc), 8);
        assert_eq!(get_token_decimals(&dai), 18);
    }

    #[test]
    fn test_validate_virtual_price() {
        // Valid prices
        assert!(validate_virtual_price(
            U256::from(10u64).pow(U256::from(18)),
            "test"
        )); // 1.0
        assert!(validate_virtual_price(
            U256::from(105u64) * U256::from(10u64).pow(U256::from(16)),
            "test"
        )); // 1.05

        // Invalid prices - less than 1.0
        assert!(!validate_virtual_price(
            U256::from(5u64) * U256::from(10u64).pow(U256::from(17)),
            "test"
        )); // 0.5

        // Invalid prices - greater than 2.0
        assert!(!validate_virtual_price(
            U256::from(25u64) * U256::from(10u64).pow(U256::from(17)),
            "test"
        )); // 2.5
    }
}
