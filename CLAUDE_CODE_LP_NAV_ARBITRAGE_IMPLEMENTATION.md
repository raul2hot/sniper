# Claude Code Implementation Instructions: Curve LP Token NAV Discount Arbitrage

> **CRITICAL CONSTRAINT**: The executor contract at `EXECUTOR_CONTRACT_ADDRESS` is already deployed and signed. **DO NOT** create any new Solidity contracts, modify any existing contracts, or suggest any on-chain changes. All implementation MUST be in Rust off-chain code only.

## Executive Summary

Implement LP token NAV discount arbitrage by:
1. Discovering LP token secondary markets (Uniswap V3, Balancer)
2. Calculating LP token NAV using `virtual_price` and underlying prices
3. Detecting when secondary market price < NAV
4. Routing arbitrage through existing DEX infrastructure (Curve `exchange`, UniV3 swaps)
5. Using existing flash loan and executor infrastructure

**NO executor contract changes required** - we trade LP tokens as regular ERC20s on secondary markets.

---

## Table of Contents

1. [Critical Constraints](#1-critical-constraints)
2. [Architecture Overview](#2-architecture-overview)
3. [File Structure](#3-file-structure)
4. [Contract Addresses](#4-contract-addresses)
5. [Solidity ABIs](#5-solidity-abis)
6. [Implementation: curve_lp_adapter.rs](#6-implementation-curve_lp_adapterrs)
7. [Implementation: lp_nav_calculator.rs](#7-implementation-lp_nav_calculatorrs)
8. [Implementation: lp_market_discovery.rs](#8-implementation-lp_market_discoveryrs)
9. [Integration with expanded_fetcher.rs](#9-integration-with-expanded_fetcherrs)
10. [Integration with brain module](#10-integration-with-brain-module)
11. [Caching Strategy](#11-caching-strategy)
12. [Multicall Optimization](#12-multicall-optimization)
13. [Safety Checks](#13-safety-checks)
14. [Testing](#14-testing)
15. [Gotchas and Edge Cases](#15-gotchas-and-edge-cases)

---

## 1. Critical Constraints

### 1.1 Executor Contract is IMMUTABLE

```
⚠️  ABSOLUTE RULE: The executor contract is deployed and signed.
    
    - DO NOT create new Solidity files
    - DO NOT suggest deploying new contracts
    - DO NOT modify src/executor/flash_loan.rs contract source
    - ALL changes must be Rust off-chain code only
```

### 1.2 Supported DEX Types (from existing executor)

The executor ONLY supports these DEX types for swaps:

```rust
// From src/executor/flash_loan.rs - DO NOT ADD NEW TYPES
uint8 constant DEX_UNISWAP_V3 = 0;
uint8 constant DEX_UNISWAP_V2 = 1;
uint8 constant DEX_SUSHISWAP_V2 = 2;
uint8 constant DEX_PANCAKE_V3 = 3;
uint8 constant DEX_BALANCER_V2 = 4;
// Note: Curve swaps go through DEX_BALANCER_V2 type or direct pool interaction
```

### 1.3 RPC Throughput Constraints (Alchemy)

```
⚠️  MINIMIZE RPC CALLS - Alchemy rate limits apply

    - Use Multicall3 for ALL batch operations
    - Cache immutable data (LP token addresses, pool coins, decimals)
    - Cache slow-moving data (virtual_price) for 60 seconds
    - Throttle LP discovery to every 10th scan (LP markets change slowly)
    - Target: <5 RPC calls per scan for LP NAV checks
```

---

## 2. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                        LP NAV ARBITRAGE FLOW                         │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  1. DISCOVERY (every 10th scan)                                     │
│     ├─ Enumerate Curve LP tokens from factory                       │
│     ├─ Find secondary markets (UniV3/Balancer pools with LP token)  │
│     └─ Cache LP token → market mappings (5 min TTL)                 │
│                                                                      │
│  2. PRICING (every scan, batched)                                   │
│     ├─ Fetch virtual_price for all LP tokens (1 multicall)          │
│     ├─ Fetch secondary market prices (1 multicall)                  │
│     └─ Calculate NAV discount/premium                               │
│                                                                      │
│  3. OPPORTUNITY DETECTION                                           │
│     ├─ If secondary_price < NAV - threshold:                        │
│     │   └─ Buy LP on secondary, add to routing graph                │
│     └─ LP tokens become tradeable assets in existing graph          │
│                                                                      │
│  4. EXECUTION (uses EXISTING infrastructure)                        │
│     ├─ Flash loan USDC/WETH                                         │
│     ├─ Swap to LP token on UniV3/Balancer (existing DEX type)       │
│     ├─ Swap LP token back via Curve metapool or secondary           │
│     └─ Repay flash loan + profit                                    │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 3. File Structure

Create these NEW files (do not modify existing files except where noted):

```
src/cartographer/
├── mod.rs                    # ADD: pub mod curve_lp;
├── curve_lp/
│   ├── mod.rs               # NEW: Module exports
│   ├── adapter.rs           # NEW: CurveLPAdapter - pool/LP discovery
│   ├── nav_calculator.rs    # NEW: NAV calculation logic
│   ├── market_discovery.rs  # NEW: Secondary market discovery
│   └── types.rs             # NEW: LP-specific types
├── expanded_fetcher.rs      # MODIFY: Add LP integration (minimal changes)
```

---

## 4. Contract Addresses

### 4.1 Curve Infrastructure (Ethereum Mainnet)

```rust
// File: src/cartographer/curve_lp/types.rs

use alloy_primitives::{address, Address};

// ============================================
// CURVE CORE CONTRACTS
// ============================================

/// Curve Address Provider - entry point for all addresses
pub const CURVE_ADDRESS_PROVIDER: Address = address!("0000000022D53366457F9d5E68Ec105046FC4383");

/// Curve StableSwap-NG Factory (use for new pool discovery)
pub const CURVE_NG_FACTORY: Address = address!("6A8cbed756804B16E05E741eDaBd5cB544AE21bf");

/// Curve TwoCrypto-NG Factory
pub const CURVE_TWOCRYPTO_FACTORY: Address = address!("98EE851a00abeE0d95D08cF4CA2BdCE32aeaAF7F");

/// Curve MetaRegistry (for LP token lookups)
pub const CURVE_META_REGISTRY: Address = address!("F98B45FA17DE75FB1aD0e7aFD971b0ca00e379fC");

// ============================================
// HIGH-TVL CURVE POOLS WITH LIQUID LP TOKENS
// ============================================

/// Pool and LP token pairs for NAV arbitrage
pub const LP_POOLS: &[(Address, Address, &str)] = &[
    // (Pool Address, LP Token Address, Name)
    (
        address!("bEbc44782C7dB0a1A60Cb6fe97d0b483032FF1C7"), // 3pool
        address!("6c3F90f043a72FA612cbac8115EE7e52BDe6E490"), // 3CRV
        "3pool"
    ),
    (
        address!("DcEF968d416a41Cdac0ED8702fAC8128A64241A2"), // FRAX/USDC (FRAXBP)
        address!("3175Df0976dFA876431C2E9eE6Bc45b65d3473CC"), // crvFRAX
        "FRAXBP"
    ),
    (
        address!("DC24316b9AE028F1497c275EB9192a3Ea0f67022"), // stETH/ETH
        address!("06325440D014e39736583c165C2963BA99fAf14E"), // steCRV
        "stETH"
    ),
    (
        address!("A5407eAE9Ba41422680e2e00537571bcC53efBfD"), // sUSD pool
        address!("C25a3A3b969415c80451098fa907EC722572917F"), // sCRV
        "sUSD"
    ),
    (
        address!("4DEcE678ceceb27446b35C672dC7d61F30bAD69E"), // crvUSD/USDC
        address!("4DEcE678ceceb27446b35C672dC7d61F30bAD69E"), // LP = pool for NG
        "crvUSD-USDC"
    ),
];

// ============================================
// SECONDARY MARKET INFRASTRUCTURE
// ============================================

/// Uniswap V3 Factory (for LP token pool discovery)
pub const UNISWAP_V3_FACTORY: Address = address!("1F98431c8aD98523631AE4a59f267346ea31F984");

/// Balancer V2 Vault
pub const BALANCER_VAULT: Address = address!("BA12222222228d8Ba445958a75a0704d566BF2C8");

/// Common quote tokens to check for LP token pairs
pub const QUOTE_TOKENS: &[(Address, &str, u8)] = &[
    (address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"), "WETH", 18),
    (address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"), "USDC", 6),
    (address!("dAC17F958D2ee523a2206206994597C13D831ec7"), "USDT", 6),
    (address!("6B175474E89094C44Da98b954EedcdeCB5BE3830"), "DAI", 18),
];

/// Multicall3 (same on all chains)
pub const MULTICALL3: Address = address!("cA11bde05977b3631167028862bE2a173976CA11");
```

---

## 5. Solidity ABIs

### 5.1 Complete ABI Definitions

```rust
// File: src/cartographer/curve_lp/types.rs (continued)

use alloy_sol_types::sol;

// ============================================
// CURVE POOL INTERFACE
// ============================================

sol! {
    /// Curve StableSwap Pool Interface
    /// IMPORTANT: StableSwap uses int128 for indices, CryptoSwap uses uint256
    interface ICurvePool {
        // ============================================
        // VIEW FUNCTIONS (for NAV calculation)
        // ============================================
        
        /// Get virtual price of LP token (18 decimals, only increases)
        /// WARNING: Can be manipulated during remove_liquidity via reentrancy
        /// Use with caution - verify not in callback context
        function get_virtual_price() external view returns (uint256);
        
        /// Get coin address at index
        function coins(uint256 i) external view returns (address);
        
        /// Get pool balance for coin at index
        function balances(uint256 i) external view returns (uint256);
        
        /// Get number of coins (not always available, try-catch)
        function N_COINS() external view returns (uint256);
        
        /// Get LP token address (for factory pools)
        function token() external view returns (address);
        
        /// Get pool fee (1e10 precision: 4000000 = 0.04%)
        function fee() external view returns (uint256);
        
        /// Get amplification coefficient
        function A() external view returns (uint256);
        
        // ============================================
        // QUOTE FUNCTIONS (for output estimation)
        // ============================================
        
        /// Estimate LP tokens from deposit
        /// WARNING: Does NOT include fees - apply 0.5-1% buffer
        /// @param amounts Array of deposit amounts per coin
        /// @param is_deposit true for deposit, false for withdraw
        function calc_token_amount(uint256[] memory amounts, bool is_deposit) external view returns (uint256);
        
        /// Estimate coins from single-coin withdrawal (INCLUDES fees)
        /// More accurate than calc_token_amount for withdrawals
        /// @param lp_amount Amount of LP tokens to burn
        /// @param i Index of coin to receive (int128 for StableSwap!)
        function calc_withdraw_one_coin(uint256 lp_amount, int128 i) external view returns (uint256);
        
        // ============================================
        // EXCHANGE FUNCTIONS (executor can use these)
        // ============================================
        
        /// Standard swap (requires approval)
        function exchange(int128 i, int128 j, uint256 dx, uint256 min_dy) external returns (uint256);
        
        /// Approval-free swap (tokens must be transferred first)
        /// YOUR EXECUTOR SUPPORTS THIS via Dex::Curve
        function exchange_received(int128 i, int128 j, uint256 dx, uint256 min_dy, address receiver) external returns (uint256);
    }
    
    /// Curve Factory Interface (for pool discovery)
    interface ICurveFactory {
        /// Get number of pools deployed by factory
        function pool_count() external view returns (uint256);
        
        /// Get pool address at index
        function pool_list(uint256 i) external view returns (address);
        
        /// Get LP token for a pool
        function get_lp_token(address pool) external view returns (address);
        
        /// Get coins for a pool
        function get_coins(address pool) external view returns (address[4] memory);
        
        /// Get underlying coins (for metapools)
        function get_underlying_coins(address pool) external view returns (address[8] memory);
        
        /// Get pool balances
        function get_balances(address pool) external view returns (uint256[4] memory);
        
        /// Get pool fees
        function get_fees(address pool) external view returns (uint256, uint256);
    }
    
    /// Curve MetaRegistry (unified pool lookup)
    interface ICurveMetaRegistry {
        /// Get LP token for any pool
        function get_lp_token(address pool) external view returns (address);
        
        /// Get pool for LP token
        function get_pool_from_lp_token(address lp_token) external view returns (address);
        
        /// Get virtual price safely
        function get_virtual_price_from_lp_token(address lp_token) external view returns (uint256);
    }
}

// ============================================
// UNISWAP V3 INTERFACE (for secondary markets)
// ============================================

sol! {
    interface IUniswapV3Factory {
        /// Get pool address for token pair and fee
        function getPool(address tokenA, address tokenB, uint24 fee) external view returns (address);
    }
    
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

// ============================================
// MULTICALL3 INTERFACE
// ============================================

sol! {
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
}

// ============================================
// ERC20 INTERFACE (for LP token basics)
// ============================================

sol! {
    interface IERC20 {
        function totalSupply() external view returns (uint256);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string memory);
        function balanceOf(address account) external view returns (uint256);
    }
}
```

---

## 6. Implementation: curve_lp/adapter.rs

```rust
// File: src/cartographer/curve_lp/adapter.rs

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
use alloy_sol_types::SolCall;
use alloy_rpc_types::TransactionRequest;
use eyre::{eyre, Result};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::types::*;

// ============================================
// CACHE CONFIGURATION
// ============================================

/// Cache duration for pool structure (addresses, coins) - rarely changes
const POOL_STRUCTURE_CACHE_SECS: u64 = 300; // 5 minutes

/// Cache duration for virtual_price - slow moving but important for accuracy
const VIRTUAL_PRICE_CACHE_SECS: u64 = 60; // 1 minute

/// Discovery throttle - only discover new LP markets every N scans
const DISCOVERY_THROTTLE_INTERVAL: u64 = 10;

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
            .map_err(|e| eyre!("Failed to decode multicall: {}", e))?;
        
        Ok(decoded)
    }
    
    // ============================================
    // POOL DISCOVERY (THROTTLED)
    // ============================================
    
    /// Check if we should run discovery this scan
    fn should_discover(&self) -> bool {
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
        if self.should_discover() {
            info!("🔍 Discovering Curve LP pools...");
            self.discover_lp_pools().await
        } else {
            // Return stale cache rather than nothing
            let cache = LP_CACHE.read().unwrap();
            Ok(cache.pools.values().cloned().collect())
        }
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
        for (pool_addr, lp_addr, name) in LP_POOLS.iter() {
            // Try to get 4 coins (most pools have 2-4)
            for i in 0u8..4 {
                calls.push(IMulticall3::Call3 {
                    target: *pool_addr,
                    allowFailure: true, // Some pools have fewer coins
                    callData: ICurvePool::coinsCall { i: U256::from(i) }.abi_encode().into(),
                });
            }
            
            // Get virtual price
            calls.push(IMulticall3::Call3 {
                target: *pool_addr,
                allowFailure: true,
                callData: ICurvePool::get_virtual_priceCall {}.abi_encode().into(),
            });
        }
        
        debug!("LP Discovery: fetching {} calls in 1 multicall", calls.len());
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
                    if let Ok(coin) = ICurvePool::coinsCall::abi_decode_returns(&results[result_idx].returnData) {
                        if coin != Address::ZERO {
                            coins.push(coin);
                            // Default decimals - will refine later if needed
                            coin_decimals.push(18);
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
                if let Ok(vp) = ICurvePool::get_virtual_priceCall::abi_decode_returns(&results[vp_idx].returnData) {
                    let mut cache = LP_CACHE.write().unwrap();
                    cache.virtual_prices.insert(*lp_addr, CachedVirtualPrice {
                        price: vp,
                        cached_at: Instant::now(),
                    });
                }
            }
            
            let pool = CachedLPPool {
                pool_address: *pool_addr,
                lp_token: *lp_addr,
                name: name.to_string(),
                n_coins: coins.len(),
                coins,
                coin_decimals,
                is_metapool: false, // TODO: Detect metapools
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
        
        info!("✅ Discovered {} Curve LP pools", pools.len());
        Ok(pools)
    }
    
    // ============================================
    // VIRTUAL PRICE FETCHING (BATCHED)
    // ============================================
    
    /// Fetch virtual prices for all LP tokens in ONE multicall
    /// Returns map: LP token address -> virtual price (U256, 18 decimals)
    pub async fn fetch_virtual_prices(&self, lp_tokens: &[Address]) -> Result<HashMap<Address, U256>> {
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
                if let Ok(vp) = ICurvePool::get_virtual_priceCall::abi_decode_returns(&results[i].returnData) {
                    result_map.insert(*lp_token, vp);
                    cache.virtual_prices.insert(*lp_token, CachedVirtualPrice {
                        price: vp,
                        cached_at: Instant::now(),
                    });
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
}
```

---

## 7. Implementation: lp_nav_calculator.rs

```rust
// File: src/cartographer/curve_lp/nav_calculator.rs

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
use eyre::Result;
use std::collections::HashMap;
use tracing::{debug, info, warn};

use super::adapter::CachedLPPool;

// ============================================
// CONFIGURATION
// ============================================

/// Minimum NAV discount (bps) to consider opportunity
/// 20 bps = 0.20% discount required
pub const MIN_NAV_DISCOUNT_BPS: u64 = 20;

/// Maximum NAV premium (bps) - LP trading above NAV
/// Usually means pool is in demand (gauge rewards, etc.)
pub const MAX_NAV_PREMIUM_BPS: u64 = 100;

/// Gas cost buffer in bps to add to minimum threshold
/// Accounts for gas costs of LP arbitrage route
pub const GAS_BUFFER_BPS: u64 = 15;

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

/// Secondary market info
#[derive(Debug, Clone)]
pub struct SecondaryMarket {
    pub pool_address: Address,
    pub dex_type: SecondaryDex,
    pub fee_bps: u32,
    pub quote_token: Address,
    pub liquidity_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryDex {
    UniswapV3,
    Balancer,
    CurveMetapool,
}

// ============================================
// NAV CALCULATOR
// ============================================

/// Calculator for LP token NAV and arbitrage detection
pub struct LPNavCalculator {
    /// Known stablecoin addresses -> assumed price of $1
    stablecoins: HashMap<Address, bool>,
    
    /// Chainlink price feeds (token -> price in USD * 1e18)
    /// In production, fetch these from Chainlink oracles
    price_feeds: HashMap<Address, U256>,
}

impl LPNavCalculator {
    pub fn new() -> Self {
        let mut stablecoins = HashMap::new();
        
        // Known stablecoins (assume $1 price)
        // USDC
        stablecoins.insert(
            "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".parse().unwrap(),
            true
        );
        // USDT
        stablecoins.insert(
            "0xdAC17F958D2ee523a2206206994597C13D831ec7".parse().unwrap(),
            true
        );
        // DAI
        stablecoins.insert(
            "0x6B175474E89094C44Da98b954EedcdeCB5BE3830".parse().unwrap(),
            true
        );
        // FRAX
        stablecoins.insert(
            "0x853d955aCEf822Db058eb8505911ED77F175b99e".parse().unwrap(),
            true
        );
        // crvUSD
        stablecoins.insert(
            "0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E".parse().unwrap(),
            true
        );
        
        Self {
            stablecoins,
            price_feeds: HashMap::new(),
        }
    }
    
    /// Update price feed for a token
    pub fn update_price(&mut self, token: Address, price_usd_1e18: U256) {
        self.price_feeds.insert(token, price_usd_1e18);
    }
    
    /// Set ETH price (used for stETH pools)
    pub fn set_eth_price(&mut self, price_usd: f64) {
        let price_1e18 = U256::from((price_usd * 1e18) as u128);
        // WETH
        self.price_feeds.insert(
            "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".parse().unwrap(),
            price_1e18
        );
        // stETH (assume 1:1 with ETH for simplicity)
        self.price_feeds.insert(
            "0xae7ab96520DE3A18E5e111B5EaAb095312D7fE84".parse().unwrap(),
            price_1e18
        );
        // wstETH (slightly higher due to rebasing)
        let wsteth_price = U256::from((price_usd * 1.15 * 1e18) as u128);
        self.price_feeds.insert(
            "0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0".parse().unwrap(),
            wsteth_price
        );
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
    pub fn calculate_nav(
        &self,
        pool: &CachedLPPool,
        virtual_price: U256,
    ) -> LPNavResult {
        // Get prices for all underlying tokens
        let underlying_prices: Vec<U256> = pool.coins
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
}
```

---

## 8. Implementation: lp_market_discovery.rs

```rust
// File: src/cartographer/curve_lp/market_discovery.rs

//! LP Token Secondary Market Discovery
//!
//! Discovers Uniswap V3 and Balancer pools that trade Curve LP tokens.
//! These secondary markets enable NAV discount arbitrage without
//! using add_liquidity/remove_liquidity (which executor doesn't support).
//!
//! OPTIMIZATION: Discovery is throttled and heavily cached.

use alloy_primitives::{Address, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::SolCall;
use alloy_rpc_types::TransactionRequest;
use eyre::{eyre, Result};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::types::*;
use super::nav_calculator::{SecondaryMarket, SecondaryDex};
use crate::cartographer::{Dex, PoolState, PoolType};

// ============================================
// CACHE CONFIGURATION
// ============================================

/// Cache duration for secondary market discovery
const MARKET_CACHE_SECS: u64 = 300; // 5 minutes

/// Minimum liquidity in USD to consider a market
const MIN_MARKET_LIQUIDITY_USD: f64 = 50_000.0;

/// Uniswap V3 fee tiers to check
const UNIV3_FEE_TIERS: [u32; 4] = [100, 500, 3000, 10000];

// ============================================
// CACHED STRUCTURES
// ============================================

#[derive(Debug, Clone)]
struct CachedMarket {
    pub lp_token: Address,
    pub markets: Vec<SecondaryMarket>,
    pub cached_at: Instant,
}

struct MarketCache {
    markets: HashMap<Address, CachedMarket>,
}

impl Default for MarketCache {
    fn default() -> Self {
        Self {
            markets: HashMap::new(),
        }
    }
}

lazy_static::lazy_static! {
    static ref MARKET_CACHE: RwLock<MarketCache> = RwLock::new(MarketCache::default());
}

// ============================================
// MARKET DISCOVERY
// ============================================

/// Discovers secondary markets for LP token trading
pub struct LPMarketDiscovery {
    rpc_url: String,
}

impl LPMarketDiscovery {
    pub fn new(rpc_url: String) -> Self {
        Self { rpc_url }
    }
    
    /// Execute multicall
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
            .map_err(|e| eyre!("Failed to decode multicall: {}", e))?;
        
        Ok(decoded)
    }
    
    /// Find secondary markets for LP tokens (batched)
    /// Returns map: LP token -> list of secondary markets
    pub async fn discover_markets(
        &self,
        lp_tokens: &[Address],
    ) -> Result<HashMap<Address, Vec<SecondaryMarket>>> {
        let mut result = HashMap::new();
        let mut tokens_to_fetch = Vec::new();
        
        // Check cache first
        {
            let cache = MARKET_CACHE.read().unwrap();
            for lp_token in lp_tokens {
                if let Some(cached) = cache.markets.get(lp_token) {
                    if cached.cached_at.elapsed() < Duration::from_secs(MARKET_CACHE_SECS) {
                        result.insert(*lp_token, cached.markets.clone());
                    } else {
                        tokens_to_fetch.push(*lp_token);
                    }
                } else {
                    tokens_to_fetch.push(*lp_token);
                }
            }
        }
        
        if tokens_to_fetch.is_empty() {
            debug!("All {} LP market lookups from cache", result.len());
            return Ok(result);
        }
        
        // Build multicall to check UniV3 factory for all LP/quote pairs
        let mut calls = Vec::new();
        let mut call_map: Vec<(Address, Address, u32)> = Vec::new(); // (lp_token, quote, fee)
        
        for lp_token in &tokens_to_fetch {
            for (quote_token, _, _) in QUOTE_TOKENS.iter() {
                for fee_tier in UNIV3_FEE_TIERS.iter() {
                    calls.push(IMulticall3::Call3 {
                        target: UNISWAP_V3_FACTORY,
                        allowFailure: true,
                        callData: IUniswapV3Factory::getPoolCall {
                            tokenA: *lp_token,
                            tokenB: *quote_token,
                            fee: *fee_tier as u32,
                        }.abi_encode().into(),
                    });
                    call_map.push((*lp_token, *quote_token, *fee_tier));
                }
            }
        }
        
        debug!("LP Market Discovery: {} calls in 1 multicall", calls.len());
        let results = self.execute_multicall(calls).await?;
        
        // Parse results
        let mut discovered: HashMap<Address, Vec<SecondaryMarket>> = HashMap::new();
        
        for (i, (lp_token, quote_token, fee_tier)) in call_map.iter().enumerate() {
            if i >= results.len() || !results[i].success {
                continue;
            }
            
            if let Ok(pool_addr) = IUniswapV3Factory::getPoolCall::abi_decode_returns(&results[i].returnData) {
                if pool_addr != Address::ZERO {
                    let market = SecondaryMarket {
                        pool_address: pool_addr,
                        dex_type: SecondaryDex::UniswapV3,
                        fee_bps: fee_tier / 100, // Convert from parts per million to bps
                        quote_token: *quote_token,
                        liquidity_usd: 0.0, // TODO: Fetch liquidity
                    };
                    
                    discovered
                        .entry(*lp_token)
                        .or_insert_with(Vec::new)
                        .push(market);
                }
            }
        }
        
        // Update cache
        {
            let mut cache = MARKET_CACHE.write().unwrap();
            for (lp_token, markets) in &discovered {
                cache.markets.insert(*lp_token, CachedMarket {
                    lp_token: *lp_token,
                    markets: markets.clone(),
                    cached_at: Instant::now(),
                });
            }
        }
        
        // Merge with cached results
        for (lp_token, markets) in discovered {
            result.insert(lp_token, markets);
        }
        
        info!(
            "✅ Discovered secondary markets for {} LP tokens ({} total markets)",
            result.len(),
            result.values().map(|v| v.len()).sum::<usize>()
        );
        
        Ok(result)
    }
    
    /// Convert discovered markets to PoolState for routing graph
    /// These are LP token <-> quote token swaps on Uniswap V3
    pub fn markets_to_pool_states(
        &self,
        markets: &HashMap<Address, Vec<SecondaryMarket>>,
    ) -> Vec<PoolState> {
        let mut states = Vec::new();
        
        for (lp_token, market_list) in markets {
            for market in market_list {
                if market.dex_type != SecondaryDex::UniswapV3 {
                    continue; // Only UniV3 supported by executor currently
                }
                
                // Create pool state for LP token <-> quote token
                // This allows the routing engine to find paths through LP tokens
                let state = PoolState {
                    address: market.pool_address,
                    token0: *lp_token,
                    token1: market.quote_token,
                    token0_decimals: 18, // LP tokens are always 18 decimals
                    token1_decimals: get_quote_decimals(&market.quote_token),
                    sqrt_price_x96: U256::ZERO, // Will be fetched during simulation
                    tick: 0,
                    liquidity: 0, // Will be fetched during simulation
                    reserve1: 0,
                    fee: market.fee_bps * 100, // Convert bps to fee format
                    is_v4: false,
                    dex: Dex::UniswapV3, // IMPORTANT: Uses existing executor DEX type!
                    pool_type: PoolType::V3,
                    weight0: 5 * 10u128.pow(17), // 0.5 for V3
                };
                
                states.push(state);
            }
        }
        
        states
    }
}

/// Get decimals for quote tokens
fn get_quote_decimals(token: &Address) -> u8 {
    for (addr, _, decimals) in QUOTE_TOKENS.iter() {
        if addr == token {
            return *decimals;
        }
    }
    18 // Default
}
```

---

## 9. Integration with expanded_fetcher.rs

Add the following to `src/cartographer/expanded_fetcher.rs`:

```rust
// ============================================
// ADD TO IMPORTS at top of file
// ============================================

use super::curve_lp::{
    CurveLPAdapter,
    LPNavCalculator,
    LPMarketDiscovery,
    LPNavArbitrage,
};

// ============================================
// ADD TO ExpandedPoolFetcher struct
// ============================================

pub struct ExpandedPoolFetcher {
    rpc_url: String,
    curve_ng_fetcher: CurveNGFetcher,
    sky_adapter: SkyAdapter,
    usd3_adapter: USD3Adapter,
    // NEW: LP token components
    lp_adapter: CurveLPAdapter,
    lp_nav_calculator: LPNavCalculator,
    lp_market_discovery: LPMarketDiscovery,
}

impl ExpandedPoolFetcher {
    pub fn new(rpc_url: String) -> Self {
        Self {
            curve_ng_fetcher: CurveNGFetcher::new(rpc_url.clone()),
            sky_adapter: SkyAdapter::new(rpc_url.clone()),
            usd3_adapter: USD3Adapter::new(rpc_url.clone()),
            // NEW: Initialize LP components
            lp_adapter: CurveLPAdapter::new(rpc_url.clone()),
            lp_nav_calculator: LPNavCalculator::new(),
            lp_market_discovery: LPMarketDiscovery::new(rpc_url.clone()),
            rpc_url,
        }
    }
    
    // ============================================
    // ADD NEW METHOD: Fetch LP NAV opportunities
    // ============================================
    
    /// Fetch LP token NAV arbitrage opportunities
    /// Returns pool states for LP secondary markets + detected opportunities
    pub async fn fetch_lp_nav_opportunities(
        &self,
        eth_price_usd: f64,
    ) -> Result<LPNavResult> {
        // Update ETH price for NAV calculations
        self.lp_nav_calculator.set_eth_price(eth_price_usd);
        
        // 1. Get LP pools (cached, 1 multicall if cache miss)
        let lp_pools = self.lp_adapter.get_lp_pools().await?;
        let lp_tokens: Vec<Address> = lp_pools.iter().map(|p| p.lp_token).collect();
        
        // 2. Fetch virtual prices (1 multicall)
        let virtual_prices = self.lp_adapter.fetch_virtual_prices(&lp_tokens).await?;
        
        // 3. Discover secondary markets (1 multicall, heavily cached)
        let markets = self.lp_market_discovery.discover_markets(&lp_tokens).await?;
        
        // 4. Convert markets to pool states for routing
        let market_pool_states = self.lp_market_discovery.markets_to_pool_states(&markets);
        
        // 5. Calculate NAV and detect opportunities
        let mut opportunities = Vec::new();
        
        for pool in &lp_pools {
            if let Some(vp) = virtual_prices.get(&pool.lp_token) {
                let nav_result = self.lp_nav_calculator.calculate_nav(pool, *vp);
                
                // Check each secondary market for this LP token
                if let Some(pool_markets) = markets.get(&pool.lp_token) {
                    for market in pool_markets {
                        // TODO: Fetch actual market price from UniV3 pool
                        // For now, detect opportunities during simulation
                        // when we have accurate spot prices
                    }
                }
                
                debug!(
                    "LP {} NAV: ${:.4} (vp: {})",
                    pool.name,
                    nav_result.nav_usd.to::<u128>() as f64 / 1e18,
                    vp
                );
            }
        }
        
        Ok(LPNavResult {
            pool_states: market_pool_states,
            lp_pools,
            virtual_prices,
            opportunities,
        })
    }
}

/// Result of LP NAV fetch
#[derive(Debug)]
pub struct LPNavResult {
    /// Pool states for secondary markets (add to routing graph)
    pub pool_states: Vec<PoolState>,
    
    /// Discovered LP pools
    pub lp_pools: Vec<CachedLPPool>,
    
    /// Virtual prices (LP token -> price)
    pub virtual_prices: HashMap<Address, U256>,
    
    /// Detected arbitrage opportunities
    pub opportunities: Vec<LPNavArbitrage>,
}

// ============================================
// MODIFY fetch_all_pools to include LP markets
// ============================================

// In the fetch_all_pools method, add after Sky/USD3 section:

// 5. Fetch LP token secondary markets (THROTTLED)
if should_fetch_lp_markets {
    info!("🪙 Fetching LP token markets...");
    match self.fetch_lp_nav_opportunities(eth_price_usd).await {
        Ok(lp_result) => {
            result.lp_market_states = lp_result.pool_states.len();
            result.pool_states.extend(lp_result.pool_states);
            result.lp_opportunities = lp_result.opportunities;
            info!("   Added {} LP secondary market pools", result.lp_market_states);
        }
        Err(e) => warn!("Failed to fetch LP markets: {}", e),
    }
}
```

---

## 10. Integration with brain module

The LP token pool states integrate automatically because they use existing `Dex::UniswapV3` type. No brain changes needed!

```rust
// The routing graph already handles these because:
// 1. LP secondary markets are PoolState with Dex::UniswapV3
// 2. The executor already supports DEX_UNISWAP_V3 = 0
// 3. Flash loans work normally (borrow USDC, swap to LP, swap LP to X, ...)

// Example arbitrage cycle that brain might find:
// USDC -> 3CRV (on UniV3 secondary market) -> USDT (on Curve 3pool) -> USDC
// 
// This works because:
// - Step 1: UniV3 swap (executor supports)
// - Step 2: Curve exchange (executor supports via Dex::Curve)
```

---

## 11. Caching Strategy

```rust
// ============================================
// CACHE HIERARCHY (minimize RPC calls)
// ============================================

// LEVEL 1: Immutable data (cache forever until restart)
// - LP token addresses
// - Pool coin addresses  
// - Pool fees
// - Token decimals

// LEVEL 2: Slow-moving data (5 minute cache)
// - Pool structure (coins, n_coins)
// - Secondary market existence
// - MetaRegistry mappings

// LEVEL 3: Medium data (60 second cache)
// - virtual_price (increases ~4-8 bps daily)
// - Pool balances

// LEVEL 4: Fast data (no cache, fetch each scan)
// - UniV3 slot0 (sqrtPriceX96)
// - UniV3 liquidity
// - Actual execution quotes

// ============================================
// RPC CALL BUDGET PER SCAN
// ============================================

// Target: <5 RPC calls for LP NAV system
//
// Scan 1 (cold start):
//   1. LP pool discovery multicall (~50 calls batched)
//   2. Secondary market discovery multicall (~100 calls batched)
//   Total: 2 RPC calls
//
// Scan 2-9 (warm cache):
//   1. Virtual price refresh multicall (~10 calls batched)
//   Total: 1 RPC call
//
// Scan 10 (rediscovery):
//   Same as Scan 1
//   Total: 2 RPC calls
//
// Average: 1.2 RPC calls per scan for LP system
```

---

## 12. Multicall Optimization

```rust
// ============================================
// MULTICALL BATCHING RULES
// ============================================

// RULE 1: Never make individual calls for batachable data
// BAD:
for pool in pools {
    let vp = pool.get_virtual_price().call().await?; // N calls!
}

// GOOD:
let calls: Vec<_> = pools.iter().map(|p| Call3 {
    target: p.address,
    callData: get_virtual_price().encode(),
    allowFailure: true,
}).collect();
let results = multicall.aggregate3(calls).await?; // 1 call!

// ============================================
// RULE 2: Combine related calls
// ============================================

// Instead of separate multicalls for:
// - virtual_price (1 multicall)
// - pool balances (1 multicall)  
// - coin addresses (1 multicall)

// Do ONE multicall with all calls:
let mut calls = Vec::new();
for pool in pools {
    calls.push(/* virtual_price */);
    calls.push(/* balances[0] */);
    calls.push(/* balances[1] */);
    calls.push(/* coins[0] */);
    calls.push(/* coins[1] */);
}
let results = multicall.aggregate3(calls).await?; // Still 1 call!

// ============================================
// RULE 3: Use allowFailure for optional data
// ============================================

// Some pools don't have all fields (e.g., N_COINS)
Call3 {
    target: pool,
    callData: N_COINS().encode(),
    allowFailure: true, // Don't fail entire batch if this reverts
}
```

---

## 13. Safety Checks

```rust
// ============================================
// CRITICAL SAFETY: virtual_price REENTRANCY
// ============================================

/// The virtual_price can be manipulated during remove_liquidity callbacks
/// via read-only reentrancy. This was exploited in April 2022.
/// 
/// SAFE USAGE:
/// 1. Only use virtual_price for off-chain calculations
/// 2. Never use as on-chain oracle in same tx as liquidity operations
/// 3. Verify value is in expected range
/// 
/// YOUR SYSTEM IS SAFE because:
/// - virtual_price is fetched off-chain via eth_call
/// - Executor doesn't call remove_liquidity
/// - We only trade LP tokens on secondary markets

pub fn validate_virtual_price(vp: U256, pool_name: &str) -> bool {
    let vp_f64 = vp.to::<u128>() as f64 / 1e18;
    
    // Virtual price should be >= 1.0 (starts at 1.0)
    if vp_f64 < 1.0 {
        warn!("Invalid virtual_price {} for {} (< 1.0)", vp_f64, pool_name);
        return false;
    }
    
    // Virtual price should be < 2.0 (grows slowly over years)
    if vp_f64 > 2.0 {
        warn!("Suspicious virtual_price {} for {} (> 2.0)", vp_f64, pool_name);
        return false;
    }
    
    true
}

// ============================================
// SAFETY: Secondary market liquidity check
// ============================================

/// Minimum liquidity in secondary market to consider trading
const MIN_SECONDARY_LIQUIDITY_USD: f64 = 50_000.0;

/// Don't trade more than 10% of secondary market liquidity
const MAX_TRADE_PCT_OF_LIQUIDITY: f64 = 0.10;

pub fn safe_trade_amount(
    desired_amount_usd: f64,
    market_liquidity_usd: f64,
) -> f64 {
    let max_safe = market_liquidity_usd * MAX_TRADE_PCT_OF_LIQUIDITY;
    desired_amount_usd.min(max_safe)
}
```

---

## 14. Testing

```rust
// File: src/cartographer/curve_lp/tests.rs

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_nav_calculation_stablecoin_pool() {
        let mut calc = LPNavCalculator::new();
        
        let pool = CachedLPPool {
            pool_address: Address::ZERO,
            lp_token: Address::ZERO,
            name: "test-3pool".to_string(),
            coins: vec![
                "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".parse().unwrap(), // USDC
                "0xdAC17F958D2ee523a2206206994597C13D831ec7".parse().unwrap(), // USDT
                "0x6B175474E89094C44Da98b954EedcdeCB5BE3830".parse().unwrap(), // DAI
            ],
            coin_decimals: vec![6, 6, 18],
            n_coins: 3,
            is_metapool: false,
            base_pool: None,
        };
        
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
    fn test_virtual_price_validation() {
        // Valid prices
        assert!(validate_virtual_price(U256::from(10u64).pow(U256::from(18)), "test"));
        assert!(validate_virtual_price(U256::from(105u64) * U256::from(10u64).pow(U256::from(16)), "test"));
        
        // Invalid prices
        assert!(!validate_virtual_price(U256::from(5u64) * U256::from(10u64).pow(U256::from(17)), "test")); // 0.5
        assert!(!validate_virtual_price(U256::from(25u64) * U256::from(10u64).pow(U256::from(17)), "test")); // 2.5
    }
}
```

---

## 15. Gotchas and Edge Cases

### 15.1 DO NOT Attempt These (Executor Doesn't Support)

```rust
// ❌ NEVER TRY: add_liquidity (requires approval, not exchange_received pattern)
// ❌ NEVER TRY: remove_liquidity_one_coin (same issue)
// ❌ NEVER TRY: Deploy new contracts
// ❌ NEVER TRY: Modify executor contract source

// ✅ DO: Trade LP tokens as ERC20s on existing secondary markets
// ✅ DO: Use existing Dex::UniswapV3 for LP token swaps
// ✅ DO: Use existing Dex::Curve for exchanges within Curve pools
```

### 15.2 Index Type Warning (int128 vs uint256)

```rust
// ⚠️ Curve StableSwap uses int128 for coin indices
// ⚠️ Curve CryptoSwap uses uint256 for coin indices

// For exchange_received on StableSwap:
function exchange_received(int128 i, int128 j, ...) // int128!

// For calc_withdraw_one_coin on StableSwap:
function calc_withdraw_one_coin(uint256 lp_amount, int128 i) // int128!

// Your ABI must match the pool type or calls will revert
```

### 15.3 LP Token Decimals

```rust
// All Curve LP tokens use 18 decimals
// This is consistent across all pool types
// No need to fetch decimals for LP tokens
```

### 15.4 Metapool Complexity

```rust
// Metapools (like FRAX/3CRV) have TWO ways to swap:
// 1. exchange(i, j) - swap between pool's direct coins (FRAX, 3CRV)
// 2. exchange_underlying(i, j) - swap between ALL underlying (FRAX, DAI, USDC, USDT)

// exchange_underlying is more gas but allows direct stablecoin access
// Your executor may not support exchange_underlying - verify before using
```

### 15.5 Gas Costs

```rust
// Typical gas costs for LP arbitrage routes:

// Simple route (2 hops):
// USDC -> 3CRV (UniV3) -> USDT (Curve 3pool)
// Gas: ~300k

// Complex route (3 hops):  
// USDC -> 3CRV (UniV3) -> FRAX (Curve metapool) -> USDC (UniV3)
// Gas: ~450k

// MINIMUM PROFIT THRESHOLD:
// At 30 gwei: 300k gas = 0.009 ETH = ~$31.50 (at $3500 ETH)
// Need at least $35-40 profit to be worthwhile
// This requires ~40bps edge on $100k position
```

---

## Summary Checklist for Claude Code

Before starting implementation, verify:

- [ ] Executor contract is NOT being modified
- [ ] All new code is in `src/cartographer/curve_lp/` directory
- [ ] Using `Dex::UniswapV3` for LP secondary market trades (executor supports this)
- [ ] Using `Multicall3` for ALL batch operations
- [ ] Cache durations match specification (5min structure, 60s prices)
- [ ] Discovery throttled to every 10th scan
- [ ] virtual_price validation in place
- [ ] No add_liquidity or remove_liquidity calls
- [ ] Tests cover NAV calculation and arbitrage detection

The key insight: **LP tokens are just ERC20s that trade on Uniswap/Balancer**. Your executor already supports those DEXes. The new code only adds off-chain discovery and NAV calculation.
