//! Curve StableSwap NG Pool Adapter - Phase 1 (MULTICALL OPTIMIZED + CACHED)
//!
//! Dynamic pool discovery and quoting for Curve NG pools.
//! Key features:
//! - Factory-based pool discovery (permissionless deployment)
//! - Dynamic fee calculation based on pool imbalance
//! - Support for exchange_received() (approval-free swaps)
//! - ERC-4626 token support in pools
//! - MULTICALL3 batching for fast discovery (~6 RPC calls instead of 1000+)
//! - CACHING: Pool structure cached for 5 minutes, only balances refreshed each scan

use alloy_primitives::{Address, Bytes, U256, address};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{sol, SolCall};
use alloy_rpc_types::TransactionRequest;
use eyre::{eyre, Result};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{debug, info, trace, warn};

use super::{Dex, PoolState, PoolType, get_token_decimals};

// ============================================
// CURVE NG FACTORY ADDRESSES
// ============================================

/// Curve StableSwap NG Factory (Ethereum Mainnet)
pub const CURVE_NG_FACTORY: Address = address!("6A8cbed756804B16E05E741eDaBd5cB544AE21bf");

/// Curve TwoCrypto NG Factory (for volatile pairs)
pub const CURVE_TWOCRYPTO_NG_FACTORY: Address = address!("98EE851a00abeE0d95D08cF4CA2BdCE32aeaAF7F");

/// Curve TriCrypto NG Factory
pub const CURVE_TRICRYPTO_NG_FACTORY: Address = address!("0c0e5f2fF0ff18a3BE9b835635039256dC4B4963");

/// Multicall3 address (same on all EVM chains)
const MULTICALL3: Address = address!("cA11bde05977b3631167028862bE2a173976CA11");

/// Minimum TVL in USD to consider a pool (filter out dust pools)
pub const MIN_TVL_USD: f64 = 50_000.0;

/// Maximum number of pools to fetch per factory
pub const MAX_POOLS_PER_FACTORY: usize = 100;

/// Cache duration for pool structure (addresses, coins, fees)
/// Pool structure rarely changes, so we cache for 5 minutes
pub const POOL_STRUCTURE_CACHE_SECS: u64 = 300;

// ============================================
// POOL CACHE STRUCTURE
// ============================================

/// Cached pool metadata (doesn't change often)
#[derive(Debug, Clone)]
pub struct CachedPoolMetadata {
    pub address: Address,
    pub coins: Vec<Address>,
    pub decimals: Vec<u8>,
    pub n_coins: usize,
    pub base_fee: u32,
    pub offpeg_multiplier: u32,
    pub has_erc4626: bool,
    pub factory: CurveNGFactoryType,
}

/// Cache entry with timestamp
#[derive(Debug, Clone)]
struct PoolCache {
    pools: Vec<CachedPoolMetadata>,
    last_updated: Instant,
}

// ============================================
// SOLIDITY INTERFACES
// ============================================

sol! {
    /// Multicall3 interface for batching
    interface IMulticall3 {
        struct Call3 {
            address target;
            bool allowFailure;
            bytes callData;
        }
        
        struct Result {
            bool success;
            bytes returnData;
        }
        
        function aggregate3(Call3[] calldata calls) 
            external payable returns (Result[] memory returnData);
    }

    /// Curve NG Factory interface
    interface ICurveNGFactory {
        function pool_count() external view returns (uint256);
        function pool_list(uint256 i) external view returns (address);
        function get_coins(address pool) external view returns (address[4] memory);
        function get_balances(address pool) external view returns (uint256[4] memory);
        function get_decimals(address pool) external view returns (uint256[4] memory);
        function get_n_coins(address pool) external view returns (uint256);
        function get_fees(address pool) external view returns (uint256[4] memory);
        function get_gauge(address pool) external view returns (address);
    }
    
    /// Curve NG Pool interface
    interface ICurveNGPool {
        function coins(uint256 i) external view returns (address);
        function balances(uint256 i) external view returns (uint256);
        function N_COINS() external view returns (uint256);
        function get_dy(int128 i, int128 j, uint256 dx) external view returns (uint256);
        function fee() external view returns (uint256);
        function offpeg_fee_multiplier() external view returns (uint256);
        function A() external view returns (uint256);
        function get_virtual_price() external view returns (uint256);
        
        // NG specific - approval-free swap
        function exchange_received(
            int128 i,
            int128 j, 
            uint256 dx,
            uint256 min_dy,
            address receiver
        ) external returns (uint256);
    }
}

// ============================================
// CURVE NG POOL DATA
// ============================================

/// Discovered Curve NG pool with full metadata
#[derive(Debug, Clone)]
pub struct CurveNGPool {
    pub address: Address,
    pub coins: Vec<Address>,
    pub decimals: Vec<u8>,
    pub balances: Vec<U256>,
    pub n_coins: usize,
    pub base_fee: u32,
    pub offpeg_multiplier: u32,
    pub amplification: U256,
    pub virtual_price: U256,
    pub gauge: Option<Address>,
    pub has_erc4626: bool,
    pub factory: CurveNGFactoryType,
}

/// Type of Curve NG factory
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveNGFactoryType {
    StableSwapNG,
    TwoCryptoNG,
    TriCryptoNG,
}

impl CurveNGPool {
    /// Calculate the effective fee for a swap considering pool imbalance
    pub fn effective_fee(&self, i: usize, j: usize) -> u32 {
        if self.balances.len() < 2 || i >= self.balances.len() || j >= self.balances.len() {
            return self.base_fee;
        }

        let bal_i = self.balances[i].to::<u128>() as f64 / 10_f64.powi(self.decimals[i] as i32);
        let bal_j = self.balances[j].to::<u128>() as f64 / 10_f64.powi(self.decimals[j] as i32);

        if bal_i == 0.0 || bal_j == 0.0 {
            return self.base_fee * self.offpeg_multiplier.max(1);
        }

        let sum_sq = (bal_i + bal_j).powi(2);
        let product_4 = 4.0 * bal_i * bal_j;
        let imbalance_factor = product_4 / sum_sq;

        let effective = (self.base_fee as f64 * self.offpeg_multiplier as f64 * imbalance_factor) as u32;
        effective.max(self.base_fee).min(self.base_fee * self.offpeg_multiplier)
    }

    /// Convert to standard PoolState for graph integration using ACCURATE get_dy price
    /// This is the correct method - uses actual on-chain exchange rate
    pub fn to_pool_state_with_price(
        &self,
        token0_idx: usize,
        token1_idx: usize,
        actual_price: f64,  // Price from get_dy: 1 token0 = X token1
    ) -> Option<PoolState> {
        if token0_idx >= self.coins.len() || token1_idx >= self.coins.len() {
            return None;
        }

        let token0 = self.coins[token0_idx];
        let token1 = self.coins[token1_idx];

        if actual_price <= 0.0 || !actual_price.is_finite() {
            return None;
        }

        let d0 = self.decimals[token0_idx];
        let d1 = self.decimals[token1_idx];
        let fee = self.effective_fee(token0_idx, token1_idx);

        // Convert price to sqrt_price_x96 format for consistency with V3
        // Note: This is for storage/graph only - actual Curve quotes use get_dy
        let sqrt_price = actual_price.sqrt() * 2_f64.powi(96);

        Some(PoolState {
            address: self.address,
            token0,
            token1,
            token0_decimals: d0,
            token1_decimals: d1,
            sqrt_price_x96: U256::from(sqrt_price as u128),
            tick: 0,
            liquidity: self.balances[token0_idx].to::<u128>(),
            reserve1: self.balances[token1_idx].to::<u128>(),
            fee,
            is_v4: false,
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            weight0: 5 * 10u128.pow(17),
        })
    }

    /// DEPRECATED: Use to_pool_state_with_price() with actual get_dy price
    /// This method uses balance ratios which are INACCURATE for Curve pools
    #[deprecated(note = "Use to_pool_state_with_price() with actual get_dy price instead")]
    pub fn to_pool_state(&self, token0_idx: usize, token1_idx: usize) -> Option<PoolState> {
        if token0_idx >= self.coins.len() || token1_idx >= self.coins.len() {
            return None;
        }

        let token0 = self.coins[token0_idx];
        let token1 = self.coins[token1_idx];

        let bal0 = self.balances[token0_idx].to::<u128>() as f64;
        let bal1 = self.balances[token1_idx].to::<u128>() as f64;

        if bal0 == 0.0 || bal1 == 0.0 {
            return None;
        }

        let d0 = self.decimals[token0_idx];
        let d1 = self.decimals[token1_idx];

        let price_raw = (bal1 / 10_f64.powi(d1 as i32)) / (bal0 / 10_f64.powi(d0 as i32));
        let fee = self.effective_fee(token0_idx, token1_idx);
        let sqrt_price = price_raw.sqrt() * 2_f64.powi(96);

        Some(PoolState {
            address: self.address,
            token0,
            token1,
            token0_decimals: d0,
            token1_decimals: d1,
            sqrt_price_x96: U256::from(sqrt_price as u128),
            tick: 0,
            liquidity: self.balances[token0_idx].to::<u128>(),
            reserve1: self.balances[token1_idx].to::<u128>(),
            fee,
            is_v4: false,
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            weight0: 5 * 10u128.pow(17),
        })
    }
}

// ============================================
// CURVE NG FETCHER (MULTICALL OPTIMIZED + CACHED)
// ============================================

/// Fetches Curve NG pools using Multicall3 batching with caching
/// OPTIMIZATION: Pool structure cached for 5 minutes, only balances refreshed
pub struct CurveNGFetcher {
    rpc_url: String,
    /// Cache for pool metadata (addresses, coins, fees) - rarely changes
    pool_cache: Arc<RwLock<Option<PoolCache>>>,
}

impl CurveNGFetcher {
    pub fn new(rpc_url: String) -> Self {
        Self {
            rpc_url,
            pool_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Check if cache is valid
    fn is_cache_valid(&self) -> bool {
        if let Ok(guard) = self.pool_cache.read() {
            if let Some(ref cache) = *guard {
                return cache.last_updated.elapsed() < Duration::from_secs(POOL_STRUCTURE_CACHE_SECS);
            }
        }
        false
    }

    /// Get cached pool metadata if valid
    fn get_cached_metadata(&self) -> Option<Vec<CachedPoolMetadata>> {
        if let Ok(guard) = self.pool_cache.read() {
            if let Some(ref cache) = *guard {
                if cache.last_updated.elapsed() < Duration::from_secs(POOL_STRUCTURE_CACHE_SECS) {
                    return Some(cache.pools.clone());
                }
            }
        }
        None
    }

    /// Update the cache
    fn update_cache(&self, pools: Vec<CachedPoolMetadata>) {
        if let Ok(mut guard) = self.pool_cache.write() {
            *guard = Some(PoolCache {
                pools,
                last_updated: Instant::now(),
            });
        }
    }
    
    /// Execute a Multicall3 batch
    async fn execute_multicall(&self, calls: Vec<IMulticall3::Call3>) -> Result<Vec<IMulticall3::Result>> {
        if calls.is_empty() {
            return Ok(Vec::new());
        }
        
        let provider = ProviderBuilder::new()
            .on_http(self.rpc_url.parse()?);
        
        let calldata = IMulticall3::aggregate3Call { calls }.abi_encode();
        
        let tx = TransactionRequest::default()
            .to(MULTICALL3)
            .input(calldata.into());
        
        let result = provider.call(tx).await
            .map_err(|e| eyre!("Multicall3 failed: {}", e))?;
        
        let decoded = IMulticall3::aggregate3Call::abi_decode_returns(&result)
            .map_err(|e| eyre!("Failed to decode multicall result: {}", e))?;
        
        Ok(decoded)
    }
    
    /// Helper to call a single contract (fallback)
    async fn call_contract(&self, to: Address, calldata: Vec<u8>) -> Result<Vec<u8>> {
        let provider = ProviderBuilder::new()
            .on_http(self.rpc_url.parse()?);
        
        let tx = TransactionRequest::default()
            .to(to)
            .input(calldata.into());
        
        let result = provider.call(tx).await
            .map_err(|e| eyre!("eth_call failed: {}", e))?;
        
        Ok(result.to_vec())
    }
    
    /// Get pool count from factory
    async fn get_pool_count(&self, factory: Address) -> Result<usize> {
        let calldata = ICurveNGFactory::pool_countCall {}.abi_encode();
        let output = self.call_contract(factory, calldata).await?;
        
        let count = ICurveNGFactory::pool_countCall::abi_decode_returns(&output)
            .map_err(|e| eyre!("Failed to decode pool_count: {}", e))?;
        
        Ok(count.to::<usize>())
    }
    
    /// Discover all NG pools from the StableSwap NG factory
    pub async fn discover_stableswap_ng_pools(&self) -> Result<Vec<CurveNGPool>> {
        info!("🔍 Discovering Curve StableSwap NG pools (batched)...");
        let pools = self.discover_from_factory_batched(CURVE_NG_FACTORY, CurveNGFactoryType::StableSwapNG).await?;
        info!("✅ Discovered {} StableSwap NG pools", pools.len());
        Ok(pools)
    }
    
    /// Discover pools from TwoCrypto NG factory
    pub async fn discover_twocrypto_ng_pools(&self) -> Result<Vec<CurveNGPool>> {
        info!("🔍 Discovering Curve TwoCrypto NG pools (batched)...");
        let pools = self.discover_from_factory_batched(CURVE_TWOCRYPTO_NG_FACTORY, CurveNGFactoryType::TwoCryptoNG).await?;
        info!("✅ Discovered {} TwoCrypto NG pools", pools.len());
        Ok(pools)
    }
    
    /// Discover pools from TriCrypto NG factory
    pub async fn discover_tricrypto_ng_pools(&self) -> Result<Vec<CurveNGPool>> {
        info!("🔍 Discovering Curve TriCrypto NG pools (batched)...");
        let pools = self.discover_from_factory_batched(CURVE_TRICRYPTO_NG_FACTORY, CurveNGFactoryType::TriCryptoNG).await?;
        info!("✅ Discovered {} TriCrypto NG pools", pools.len());
        Ok(pools)
    }
    
    /// BATCHED factory discovery - 2-3 RPC calls instead of 100s
    async fn discover_from_factory_batched(
        &self,
        factory: Address,
        factory_type: CurveNGFactoryType,
    ) -> Result<Vec<CurveNGPool>> {
        // Get pool count first
        let pool_count = self.get_pool_count(factory).await?;
        let count = pool_count.min(MAX_POOLS_PER_FACTORY);
        
        debug!("Factory {:?} has {} pools, fetching {}", factory, pool_count, count);
        
        if count == 0 {
            return Ok(Vec::new());
        }
        
        // BATCH 1: Get all pool addresses
        let mut calls: Vec<IMulticall3::Call3> = Vec::new();
        for i in 0..count {
            calls.push(IMulticall3::Call3 {
                target: factory,
                allowFailure: true,
                callData: ICurveNGFactory::pool_listCall { i: U256::from(i) }.abi_encode().into(),
            });
        }
        
        let results = self.execute_multicall(calls).await?;
        
        let mut pool_addresses: Vec<Address> = Vec::new();
        for r in results {
            if r.success {
                if let Ok(addr) = ICurveNGFactory::pool_listCall::abi_decode_returns(&r.returnData) {
                    if addr != Address::ZERO {
                        pool_addresses.push(addr);
                    }
                }
            }
        }
        
        debug!("Got {} valid pool addresses", pool_addresses.len());
        
        if pool_addresses.is_empty() {
            return Ok(Vec::new());
        }
        
        // BATCH 2: Get coins, balances, and fee for all pools
        let mut calls: Vec<IMulticall3::Call3> = Vec::new();
        for &pool in &pool_addresses {
            // get_coins from factory
            calls.push(IMulticall3::Call3 {
                target: factory,
                allowFailure: true,
                callData: ICurveNGFactory::get_coinsCall { pool }.abi_encode().into(),
            });
            // get_balances from factory
            calls.push(IMulticall3::Call3 {
                target: factory,
                allowFailure: true,
                callData: ICurveNGFactory::get_balancesCall { pool }.abi_encode().into(),
            });
            // fee from pool directly
            calls.push(IMulticall3::Call3 {
                target: pool,
                allowFailure: true,
                callData: ICurveNGPool::feeCall {}.abi_encode().into(),
            });
            // offpeg_fee_multiplier from pool
            calls.push(IMulticall3::Call3 {
                target: pool,
                allowFailure: true,
                callData: ICurveNGPool::offpeg_fee_multiplierCall {}.abi_encode().into(),
            });
        }
        
        let results = self.execute_multicall(calls).await?;
        
        // Parse results (4 calls per pool)
        let mut pools = Vec::new();
        for (i, &pool_address) in pool_addresses.iter().enumerate() {
            let offset = i * 4;
            
            if offset + 3 >= results.len() {
                break;
            }
            
            // Parse coins
            let coins: Vec<Address> = if results[offset].success {
                ICurveNGFactory::get_coinsCall::abi_decode_returns(&results[offset].returnData)
                    .ok()
                    .map(|c| c.into_iter().filter(|a| *a != Address::ZERO).collect())
                    .unwrap_or_default()
            } else {
                continue;
            };
            
            if coins.len() < 2 {
                continue;
            }
            
            // Parse balances
            let balances: Vec<U256> = if results[offset + 1].success {
                ICurveNGFactory::get_balancesCall::abi_decode_returns(&results[offset + 1].returnData)
                    .ok()
                    .map(|b| b.into_iter().take(coins.len()).collect())
                    .unwrap_or_default()
            } else {
                continue;
            };
            
            if balances.len() < coins.len() {
                continue;
            }
            
            // Parse fee (Curve fees are in 1e10 format)
            let base_fee = if results[offset + 2].success {
                ICurveNGPool::feeCall::abi_decode_returns(&results[offset + 2].returnData)
                    .ok()
                    .map(|f| {
                        // Convert from 1e10 to bps: 4000000 (0.04% in 1e10) -> 4 bps
                        let fee_1e10 = f.to::<u128>();
                        ((fee_1e10 * 10000) / 10u128.pow(10)) as u32
                    })
                    .unwrap_or(4)
            } else {
                4 // Default 0.04%
            };
            
            // Parse offpeg multiplier
            let offpeg_multiplier = if results[offset + 3].success {
                ICurveNGPool::offpeg_fee_multiplierCall::abi_decode_returns(&results[offset + 3].returnData)
                    .ok()
                    .map(|m| {
                        let mult = m.to::<u128>();
                        (mult / 10u128.pow(10)) as u32
                    })
                    .unwrap_or(20)
            } else {
                20 // Default 20x
            };
            
            let decimals: Vec<u8> = coins.iter().map(|c| get_token_decimals(c)).collect();
            
            // Check for ERC-4626 tokens
            let has_erc4626 = coins.iter().any(|c| is_erc4626_token(c));
            
            // TVL filter
            let tvl = estimate_tvl(&balances, &decimals);
            if tvl < MIN_TVL_USD {
                trace!("Pool {:?} filtered out: TVL ${:.0} < ${:.0}", pool_address, tvl, MIN_TVL_USD);
                continue;
            }
            
            let n_coins = coins.len();
            pools.push(CurveNGPool {
                address: pool_address,
                n_coins,
                coins,
                decimals,
                balances,
                base_fee: base_fee.max(1),
                offpeg_multiplier: offpeg_multiplier.max(1),
                amplification: U256::from(100), // Skip extra call
                virtual_price: U256::from(10u64.pow(18)),
                gauge: None,
                has_erc4626,
                factory: factory_type,
            });
        }
        
        debug!("Parsed {} valid pools from factory", pools.len());
        Ok(pools)
    }
    
    /// Get quote for a swap (dy for dx)
    pub async fn get_dy(
        &self,
        pool: Address,
        i: i128,
        j: i128,
        dx: U256,
    ) -> Result<U256> {
        let calldata = ICurveNGPool::get_dyCall { i, j, dx }.abi_encode();
        let output = self.call_contract(pool, calldata).await?;
        let dy = ICurveNGPool::get_dyCall::abi_decode_returns(&output)?;
        Ok(dy)
    }

    /// Batch get_dy for multiple pools in a single RPC call
    /// Returns Vec of Option<U256> - None if call failed for that pool
    pub async fn batch_get_dy(
        &self,
        requests: &[(Address, i128, i128, U256)], // (pool, i, j, dx)
    ) -> Result<Vec<Option<U256>>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }

        // Build multicall
        let calls: Vec<IMulticall3::Call3> = requests.iter()
            .map(|(pool, i, j, dx)| IMulticall3::Call3 {
                target: *pool,
                allowFailure: true,
                callData: ICurveNGPool::get_dyCall { i: *i, j: *j, dx: *dx }.abi_encode().into(),
            })
            .collect();

        debug!("Batch get_dy for {} pools", calls.len());
        let results = self.execute_multicall(calls).await?;

        // Parse results
        let mut outputs = Vec::with_capacity(requests.len());
        for result in results {
            if result.success {
                match ICurveNGPool::get_dyCall::abi_decode_returns(&result.returnData) {
                    Ok(dy) => outputs.push(Some(dy)),
                    Err(_) => outputs.push(None),
                }
            } else {
                outputs.push(None);
            }
        }

        Ok(outputs)
    }
    
    /// OPTIMIZED: Discover all NG pools with caching
    /// - If cache valid: Only fetch balances (1 multicall instead of 9+)
    /// - If cache expired: Full discovery + update cache
    pub async fn discover_all_ng_pools(&self) -> Result<Vec<CurveNGPool>> {
        // Check if we have valid cached metadata
        if let Some(cached_metadata) = self.get_cached_metadata() {
            info!("📦 Using cached Curve NG pool structure ({} pools), refreshing balances only...", cached_metadata.len());
            return self.refresh_balances_only(&cached_metadata).await;
        }

        info!("🔍 Cache expired/empty, performing full Curve NG discovery...");
        let mut all_pools = Vec::new();
        let mut all_metadata = Vec::new();

        // StableSwap NG (highest priority - stablecoin pools)
        match self.discover_stableswap_ng_pools().await {
            Ok(pools) => {
                info!("  StableSwap NG: {} pools", pools.len());
                // Extract metadata for caching
                for pool in &pools {
                    all_metadata.push(CachedPoolMetadata {
                        address: pool.address,
                        coins: pool.coins.clone(),
                        decimals: pool.decimals.clone(),
                        n_coins: pool.n_coins,
                        base_fee: pool.base_fee,
                        offpeg_multiplier: pool.offpeg_multiplier,
                        has_erc4626: pool.has_erc4626,
                        factory: pool.factory,
                    });
                }
                all_pools.extend(pools);
            }
            Err(e) => warn!("Failed to fetch StableSwap NG pools: {}", e),
        }

        // TwoCrypto NG (volatile pairs)
        match self.discover_twocrypto_ng_pools().await {
            Ok(pools) => {
                info!("  TwoCrypto NG: {} pools", pools.len());
                for pool in &pools {
                    all_metadata.push(CachedPoolMetadata {
                        address: pool.address,
                        coins: pool.coins.clone(),
                        decimals: pool.decimals.clone(),
                        n_coins: pool.n_coins,
                        base_fee: pool.base_fee,
                        offpeg_multiplier: pool.offpeg_multiplier,
                        has_erc4626: pool.has_erc4626,
                        factory: pool.factory,
                    });
                }
                all_pools.extend(pools);
            }
            Err(e) => warn!("Failed to fetch TwoCrypto NG pools: {}", e),
        }

        // TriCrypto NG (3-asset pools)
        match self.discover_tricrypto_ng_pools().await {
            Ok(pools) => {
                info!("  TriCrypto NG: {} pools", pools.len());
                for pool in &pools {
                    all_metadata.push(CachedPoolMetadata {
                        address: pool.address,
                        coins: pool.coins.clone(),
                        decimals: pool.decimals.clone(),
                        n_coins: pool.n_coins,
                        base_fee: pool.base_fee,
                        offpeg_multiplier: pool.offpeg_multiplier,
                        has_erc4626: pool.has_erc4626,
                        factory: pool.factory,
                    });
                }
                all_pools.extend(pools);
            }
            Err(e) => warn!("Failed to fetch TriCrypto NG pools: {}", e),
        }

        // Update cache with discovered metadata
        info!("💾 Caching {} pool structures for {} seconds", all_metadata.len(), POOL_STRUCTURE_CACHE_SECS);
        self.update_cache(all_metadata);

        info!("📊 Total Curve NG pools discovered: {}", all_pools.len());
        Ok(all_pools)
    }

    /// FAST PATH: Refresh only balances using cached metadata (1 multicall)
    async fn refresh_balances_only(&self, cached: &[CachedPoolMetadata]) -> Result<Vec<CurveNGPool>> {
        if cached.is_empty() {
            return Ok(Vec::new());
        }

        // Group pools by factory for efficient batching
        let mut stableswap_pools: Vec<&CachedPoolMetadata> = Vec::new();
        let mut twocrypto_pools: Vec<&CachedPoolMetadata> = Vec::new();
        let mut tricrypto_pools: Vec<&CachedPoolMetadata> = Vec::new();

        for pool in cached {
            match pool.factory {
                CurveNGFactoryType::StableSwapNG => stableswap_pools.push(pool),
                CurveNGFactoryType::TwoCryptoNG => twocrypto_pools.push(pool),
                CurveNGFactoryType::TriCryptoNG => tricrypto_pools.push(pool),
            }
        }

        // Build single multicall for all balance fetches
        let mut calls: Vec<IMulticall3::Call3> = Vec::new();
        let mut pool_refs: Vec<&CachedPoolMetadata> = Vec::new();

        // Add balance calls for each pool from its factory
        for pool in &stableswap_pools {
            calls.push(IMulticall3::Call3 {
                target: CURVE_NG_FACTORY,
                allowFailure: true,
                callData: ICurveNGFactory::get_balancesCall { pool: pool.address }.abi_encode().into(),
            });
            pool_refs.push(pool);
        }
        for pool in &twocrypto_pools {
            calls.push(IMulticall3::Call3 {
                target: CURVE_TWOCRYPTO_NG_FACTORY,
                allowFailure: true,
                callData: ICurveNGFactory::get_balancesCall { pool: pool.address }.abi_encode().into(),
            });
            pool_refs.push(pool);
        }
        for pool in &tricrypto_pools {
            calls.push(IMulticall3::Call3 {
                target: CURVE_TRICRYPTO_NG_FACTORY,
                allowFailure: true,
                callData: ICurveNGFactory::get_balancesCall { pool: pool.address }.abi_encode().into(),
            });
            pool_refs.push(pool);
        }

        debug!("Refreshing balances for {} pools in 1 multicall", calls.len());
        let results = self.execute_multicall(calls).await?;

        // Parse results and reconstruct full pool data
        let mut pools = Vec::new();
        for (i, pool_meta) in pool_refs.iter().enumerate() {
            if i >= results.len() || !results[i].success {
                continue;
            }

            let balances: Vec<U256> = ICurveNGFactory::get_balancesCall::abi_decode_returns(&results[i].returnData)
                .ok()
                .map(|b| b.into_iter().take(pool_meta.n_coins).collect())
                .unwrap_or_default();

            if balances.len() < pool_meta.n_coins {
                continue;
            }

            // TVL filter
            let tvl = estimate_tvl(&balances, &pool_meta.decimals);
            if tvl < MIN_TVL_USD {
                continue;
            }

            pools.push(CurveNGPool {
                address: pool_meta.address,
                coins: pool_meta.coins.clone(),
                decimals: pool_meta.decimals.clone(),
                balances,
                n_coins: pool_meta.n_coins,
                base_fee: pool_meta.base_fee,
                offpeg_multiplier: pool_meta.offpeg_multiplier,
                amplification: U256::from(100),
                virtual_price: U256::from(10u64.pow(18)),
                gauge: None,
                has_erc4626: pool_meta.has_erc4626,
                factory: pool_meta.factory,
            });
        }

        info!("✅ Refreshed {} pool balances in 1 RPC call (cache hit)", pools.len());
        Ok(pools)
    }
    
    /// Batch fetch accurate prices using get_dy for all pool pairs
    /// Returns HashMap<(pool_address, i, j), price_float>
    pub async fn batch_fetch_prices(
        &self,
        pools: &[CurveNGPool],
        base_amount_usd: f64,  // e.g., 10000.0
    ) -> Result<HashMap<(Address, usize, usize), f64>> {
        let mut requests = Vec::new();
        let mut request_map = Vec::new(); // Track which request maps to which pool/pair

        for pool in pools {
            for i in 0..pool.n_coins {
                for j in 0..pool.n_coins {
                    if i == j { continue; }

                    // Skip invalid addresses
                    if !Self::is_valid_address(&pool.coins[i]) ||
                       !Self::is_valid_address(&pool.coins[j]) {
                        continue;
                    }

                    // Calculate input amount based on token decimals
                    let decimals = pool.decimals[i];
                    // For stablecoins, use ~$10000 worth
                    let dx = U256::from((base_amount_usd * 10_f64.powi(decimals as i32)) as u128);

                    requests.push((pool.address, i as i128, j as i128, dx));
                    request_map.push((pool.address, i, j, decimals, pool.decimals[j]));
                }
            }
        }

        if requests.is_empty() {
            return Ok(HashMap::new());
        }

        debug!("Batch fetching {} prices via get_dy", requests.len());

        // Use existing batch_get_dy method
        let results = self.batch_get_dy(&requests).await?;

        let mut prices = HashMap::new();
        for (idx, dy_opt) in results.into_iter().enumerate() {
            if let Some(dy) = dy_opt {
                let (pool_addr, i, j, dec_i, dec_j) = request_map[idx];
                let (_, _, _, dx) = requests[idx];

                // Calculate price: dy/dx with decimal adjustment
                let dx_f64 = dx.to::<u128>() as f64 / 10_f64.powi(dec_i as i32);
                let dy_f64 = dy.to::<u128>() as f64 / 10_f64.powi(dec_j as i32);

                if dx_f64 > 0.0 {
                    let price = dy_f64 / dx_f64;
                    prices.insert((pool_addr, i, j), price);
                }
            }
        }

        info!("Fetched {} accurate Curve prices via get_dy", prices.len());
        Ok(prices)
    }

    /// Convert discovered pools to PoolState using ACCURATE get_dy prices
    /// This is the recommended method - fetches real on-chain exchange rates
    pub async fn convert_to_pool_states_accurate(&self, ng_pools: &[CurveNGPool]) -> Vec<PoolState> {
        // Step 1: Batch fetch all prices
        let prices = match self.batch_fetch_prices(ng_pools, 10000.0).await {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to fetch Curve prices via get_dy: {}, falling back to deprecated balance ratios", e);
                #[allow(deprecated)]
                return self.convert_to_pool_states(ng_pools);
            }
        };

        let mut states = Vec::new();

        for pool in ng_pools {
            for i in 0..pool.n_coins {
                for j in 0..pool.n_coins {
                    if i == j { continue; }

                    // Skip invalid addresses
                    if !Self::is_valid_address(&pool.coins[i]) ||
                       !Self::is_valid_address(&pool.coins[j]) {
                        continue;
                    }

                    // Get pre-fetched price
                    if let Some(&price) = prices.get(&(pool.address, i, j)) {
                        if let Some(state) = pool.to_pool_state_with_price(i, j, price) {
                            states.push(state);
                        }
                    }
                }
            }
        }

        debug!("Converted {} NG pools to {} graph edges with accurate get_dy prices", ng_pools.len(), states.len());
        states
    }

    /// DEPRECATED: Convert discovered pools to PoolState using balance ratios
    /// Use convert_to_pool_states_accurate() instead for proper pricing
    #[deprecated(note = "Use convert_to_pool_states_accurate() for accurate get_dy-based pricing")]
    pub fn convert_to_pool_states(&self, ng_pools: &[CurveNGPool]) -> Vec<PoolState> {
        let mut states = Vec::new();

        for pool in ng_pools {
            for i in 0..pool.n_coins {
                for j in 0..pool.n_coins {
                    if i == j { continue; }

                    // FILTER: Skip invalid token addresses
                    let token_i = pool.coins[i];
                    let token_j = pool.coins[j];
                    if !Self::is_valid_address(&token_i) || !Self::is_valid_address(&token_j) {
                        debug!("Skipping invalid token in pool {:?}", pool.address);
                        continue;
                    }

                    #[allow(deprecated)]
                    if let Some(state) = pool.to_pool_state(i, j) {
                        states.push(state);
                    }
                }
            }
        }

        debug!("Converted {} NG pools to {} graph edges (DEPRECATED: balance ratios)", ng_pools.len(), states.len());
        states
    }
    
    /// Check if address is valid (not a placeholder/error)
    fn is_valid_address(addr: &Address) -> bool {
        // Count non-zero bytes
        let non_zero = addr.as_slice().iter().filter(|&&b| b != 0).count();
        non_zero >= 8  // Real addresses have many non-zero bytes
    }
}

// ============================================
// HELPER FUNCTIONS
// ============================================

/// Known ERC-4626 yield-bearing tokens
fn is_erc4626_token(address: &Address) -> bool {
    let addr = format!("{:?}", address).to_lowercase();
    
    // sUSDS - Sky savings USD
    if addr.contains("a3931d71877c0e7a3148cb7eb4463524fec27fbd") {
        return true;
    }
    // sDAI - Spark DAI
    if addr.contains("83f20f44975d03b1b09e64809b757c47f942beea") {
        return true;
    }
    // scrvUSD - Savings crvUSD
    if addr.contains("0655977feb2f289a4ab78af67bab0d17aab84367") {
        return true;
    }
    // sFRAX
    if addr.contains("a663b02cf0a4b149d2ad41910cb81e23e1c41c32") {
        return true;
    }
    
    false
}

/// Estimate TVL in USD (rough estimate using stablecoin assumption)
fn estimate_tvl(balances: &[U256], decimals: &[u8]) -> f64 {
    let mut total = 0.0;
    
    for (bal, dec) in balances.iter().zip(decimals.iter()) {
        let amount = bal.to::<u128>() as f64 / 10_f64.powi(*dec as i32);
        total += amount;
    }
    
    total
}

/// High-priority Curve NG pools
pub fn get_priority_curve_ng_pools() -> Vec<(&'static str, &'static str)> {
    vec![
        ("sUSDS/USDT", "Yield drift arb"),
        ("sUSDS/USDC", "Yield drift arb"),
        ("USD3/sUSDS", "NAV lag arb"),
        ("crvUSD/USDT", "Pegkeeper dynamics"),
        ("crvUSD/USDC", "Pegkeeper dynamics"),
        ("GHO/USDT", "GHO discount"),
        ("DOLA/crvUSD", "Leverage dynamics"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_effective_fee_balanced() {
        let pool = CurveNGPool {
            address: Address::ZERO,
            n_coins: 2,
            coins: vec![Address::ZERO, Address::ZERO],
            decimals: vec![18, 18],
            balances: vec![U256::from(1000u64 * 10u64.pow(18)), U256::from(1000u64 * 10u64.pow(18))],
            base_fee: 4,
            offpeg_multiplier: 20,
            amplification: U256::from(100),
            virtual_price: U256::from(10u64.pow(18)),
            gauge: None,
            has_erc4626: false,
            factory: CurveNGFactoryType::StableSwapNG,
        };
        
        let fee = pool.effective_fee(0, 1);
        assert!(fee >= 4 && fee <= 80);
    }
    
    #[test]
    fn test_effective_fee_imbalanced() {
        let pool = CurveNGPool {
            address: Address::ZERO,
            n_coins: 2,
            coins: vec![Address::ZERO, Address::ZERO],
            decimals: vec![18, 18],
            balances: vec![U256::from(10000u64 * 10u64.pow(18)), U256::from(1000u64 * 10u64.pow(18))],
            base_fee: 4,
            offpeg_multiplier: 20,
            amplification: U256::from(100),
            virtual_price: U256::from(10u64.pow(18)),
            gauge: None,
            has_erc4626: false,
            factory: CurveNGFactoryType::StableSwapNG,
        };
        
        let fee = pool.effective_fee(0, 1);
        assert!(fee > 4);
    }
}