//! UniswapV3/V2 Quoter - Provider-based Simulation
//!
//! Uses the official Uniswap QuoterV2 contract via eth_call for V3 quotes.
//! Uses constant product formula for V2 quotes.
//!
//! OPTIMIZATIONS:
//! - Caches immutable pool data (token0, fees) to reduce RPC calls
//! - Token0/fee lookups are immutable per pool - cache forever
//! - Caches reserves for scan duration (15s) to avoid redundant fetches
//! - Batch fetches reserves using Multicall3 for multiple pools

use alloy_primitives::{Address, Bytes, U256, address};
use alloy_sol_types::{sol, SolCall};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types::TransactionRequest;
use eyre::{eyre, Result};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tracing::debug;
use lazy_static::lazy_static;

/// Cache duration for reserves (should match or be slightly less than scan interval)
const RESERVES_CACHE_DURATION_SECS: u64 = 15;

/// Cached reserves with timestamp
#[derive(Clone, Debug)]
struct CachedReserves {
    reserve0: u128,
    reserve1: u128,
    cached_at: Instant,
}

impl CachedReserves {
    fn is_valid(&self) -> bool {
        self.cached_at.elapsed() < Duration::from_secs(RESERVES_CACHE_DURATION_SECS)
    }
}

lazy_static! {
    /// Global cache for pool token0 addresses (immutable per pool)
    static ref TOKEN0_CACHE: RwLock<HashMap<Address, Address>> = RwLock::new(HashMap::new());

    /// Global cache for V3 pool fees (immutable per pool)
    static ref FEE_CACHE: RwLock<HashMap<Address, u32>> = RwLock::new(HashMap::new());

    /// Global cache for V2 reserves (short TTL - scan duration)
    static ref RESERVES_CACHE: RwLock<HashMap<Address, CachedReserves>> = RwLock::new(HashMap::new());
}

// ============================================
// SOLIDITY INTERFACES
// ============================================

sol! {
    /// Uniswap V3 QuoterV2 interface
    #[derive(Debug)]
    interface IQuoterV2 {
        struct QuoteExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint256 amountIn;
            uint24 fee;
            uint160 sqrtPriceLimitX96;
        }
        
        function quoteExactInputSingle(QuoteExactInputSingleParams memory params)
            external
            returns (
                uint256 amountOut,
                uint160 sqrtPriceX96After,
                uint32 initializedTicksCrossed,
                uint256 gasEstimate
            );
    }
    
    /// Uniswap V3 Pool interface (for fee lookup)
    #[derive(Debug)]
    interface IUniswapV3Pool {
        function fee() external view returns (uint24);
        function token0() external view returns (address);
        function token1() external view returns (address);
    }
    
    /// Uniswap V2 Pair interface
    #[derive(Debug)]
    interface IUniswapV2Pair {
        function getReserves() external view returns (
            uint112 reserve0,
            uint112 reserve1,
            uint32 blockTimestampLast
        );
        function token0() external view returns (address);
        function token1() external view returns (address);
    }

    /// Multicall3 interface for batching calls
    #[derive(Debug)]
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

        function aggregate3(Call3[] calldata calls) external payable returns (Result[] memory returnData);
    }
}

/// Multicall3 contract address (same on all chains)
const MULTICALL3: Address = address!("cA11bde05977b3631167028862bE2a173976CA11");

/// Quote result from simulation
#[derive(Debug, Clone)]
pub struct QuoteResult {
    pub amount_in: U256,
    pub amount_out: U256,
    pub pool: Address,
    pub zero_for_one: bool,
    pub gas_estimate: u64,
}

/// Official Uniswap V3 QuoterV2 address on mainnet
const QUOTER_V2: Address = address!("61fFE014bA17989E743c5F6cB21bF9697530B21e");

/// UniV3 Quoter using Provider's eth_call
///
/// OPTIMIZATIONS:
/// - Caches token0 lookups (immutable per pool) - saves ~1 RPC call per swap
/// - Caches fee lookups (immutable per pool) - saves ~1 RPC call per V3 pool
/// - Caches reserves for scan duration - avoids refetching same pool reserves
/// - Batch prefetch reserves via Multicall3 - 1 RPC call for N pools
/// - After warmup with prefetch, V2 quotes need 0 RPC calls!
pub struct UniV3Quoter {
    rpc_url: String,
}

impl UniV3Quoter {
    pub fn new(rpc_url: String) -> Self {
        Self { rpc_url }
    }

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

    /// Execute a Multicall3 batch - single RPC call for multiple contract calls
    async fn execute_multicall(&self, calls: Vec<IMulticall3::Call3>) -> Result<Vec<IMulticall3::Result>> {
        if calls.is_empty() {
            return Ok(Vec::new());
        }

        let calldata = IMulticall3::aggregate3Call { calls }.abi_encode();
        let result = self.call_contract(MULTICALL3, calldata).await?;

        let decoded = IMulticall3::aggregate3Call::abi_decode_returns(&result)
            .map_err(|e| eyre!("Failed to decode multicall result: {}", e))?;

        Ok(decoded)
    }

    /// Prefetch reserves for multiple V2 pools in a single RPC call
    /// Call this before simulating cycles to warm the cache
    pub async fn prefetch_v2_reserves(&self, pools: &[Address]) -> Result<usize> {
        // Filter out pools that already have valid cached reserves
        let pools_to_fetch: Vec<Address> = {
            let cache = RESERVES_CACHE.read().unwrap();
            pools.iter()
                .filter(|p| {
                    cache.get(*p).map(|c| !c.is_valid()).unwrap_or(true)
                })
                .copied()
                .collect()
        };

        if pools_to_fetch.is_empty() {
            debug!("All {} pools have valid cached reserves", pools.len());
            return Ok(0);
        }

        // Build multicall for getReserves on all pools
        let calls: Vec<IMulticall3::Call3> = pools_to_fetch.iter()
            .map(|pool| IMulticall3::Call3 {
                target: *pool,
                allowFailure: true,
                callData: IUniswapV2Pair::getReservesCall {}.abi_encode().into(),
            })
            .collect();

        debug!("Batch fetching reserves for {} V2 pools", calls.len());
        let results = self.execute_multicall(calls).await?;

        // Parse results and cache
        let mut cache = RESERVES_CACHE.write().unwrap();
        let now = Instant::now();
        let mut fetched = 0;

        for (pool, result) in pools_to_fetch.iter().zip(results.iter()) {
            if result.success {
                if let Ok(reserves) = IUniswapV2Pair::getReservesCall::abi_decode_returns(&result.returnData) {
                    cache.insert(*pool, CachedReserves {
                        reserve0: reserves.reserve0.to(),
                        reserve1: reserves.reserve1.to(),
                        cached_at: now,
                    });
                    fetched += 1;
                }
            }
        }

        debug!("Cached reserves for {}/{} pools", fetched, pools_to_fetch.len());
        Ok(fetched)
    }

    /// Invalidate the reserves cache (call at start of new scan)
    pub fn invalidate_reserves_cache() {
        let mut cache = RESERVES_CACHE.write().unwrap();
        cache.clear();
        debug!("Reserves cache invalidated");
    }
    
    /// Quote a V3 swap using the official QuoterV2 contract
    pub async fn quote_v3(
        &self,
        pool: Address,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
        fee: u32,
    ) -> Result<QuoteResult> {
        debug!(
            "Quoting V3 swap: {} -> {} via {:?}, amount: {}",
            token_in, token_out, pool, amount_in
        );
        
        // Get token0 to determine direction
        let token0 = self.get_pool_token0(pool).await?;
        let zero_for_one = token_in == token0;
        
        // Build the quote call with U160::ZERO for sqrtPriceLimitX96
        let params = IQuoterV2::QuoteExactInputSingleParams {
            tokenIn: token_in,
            tokenOut: token_out,
            amountIn: amount_in,
            fee: fee.try_into().unwrap_or(3000u32).try_into().unwrap(),
            sqrtPriceLimitX96: alloy_primitives::Uint::<160, 3>::ZERO,
        };
        
        let calldata = IQuoterV2::quoteExactInputSingleCall { params }.abi_encode();
        
        match self.call_contract(QUOTER_V2, calldata).await {
            Ok(output) => {
                // Decode the output
                let decoded = IQuoterV2::quoteExactInputSingleCall::abi_decode_returns(&output)
                    .map_err(|e| eyre!("Failed to decode quoter output: {}", e))?;
                
                let gas: u64 = decoded.gasEstimate.to();
                
                Ok(QuoteResult {
                    amount_in,
                    amount_out: decoded.amountOut,
                    pool,
                    zero_for_one,
                    gas_estimate: gas,
                })
            }
            Err(e) => {
                // The quoter might revert if the swap would fail
                Err(eyre!("Quote failed: {}", e))
            }
        }
    }
    
    /// Quote a V2 swap using constant product formula
    /// Uses cached reserves if available, otherwise fetches and caches
    pub async fn quote_v2(
        &self,
        pool: Address,
        token_in: Address,
        amount_in: U256,
    ) -> Result<QuoteResult> {
        debug!(
            "Quoting V2 swap: {} via {:?}, amount: {}",
            token_in, pool, amount_in
        );

        // Try to get reserves from cache first
        let (r0, r1) = self.get_v2_reserves(pool).await?;

        // Get token0 to determine direction (also cached)
        let token0 = self.get_v2_token0(pool).await?;
        let zero_for_one = token_in == token0;

        let (reserve_in, reserve_out) = if zero_for_one {
            (U256::from(r0), U256::from(r1))
        } else {
            (U256::from(r1), U256::from(r0))
        };

        // Constant product formula with 0.3% fee
        // amountOut = (amountIn * 997 * reserveOut) / (reserveIn * 1000 + amountIn * 997)
        let amount_in_with_fee = amount_in * U256::from(997);
        let numerator = amount_in_with_fee * reserve_out;
        let denominator = reserve_in * U256::from(1000) + amount_in_with_fee;

        if denominator == U256::ZERO {
            return Err(eyre!("Division by zero in V2 quote"));
        }

        let amount_out = numerator / denominator;

        Ok(QuoteResult {
            amount_in,
            amount_out,
            pool,
            zero_for_one,
            gas_estimate: 100_000, // V2 swaps are cheaper
        })
    }

    /// Get V2 reserves (CACHED for scan duration)
    async fn get_v2_reserves(&self, pool: Address) -> Result<(u128, u128)> {
        // Check cache first
        {
            let cache = RESERVES_CACHE.read().unwrap();
            if let Some(cached) = cache.get(&pool) {
                if cached.is_valid() {
                    return Ok((cached.reserve0, cached.reserve1));
                }
            }
        }

        // Fetch from chain
        let calldata = IUniswapV2Pair::getReservesCall {}.abi_encode();
        let output = self.call_contract(pool, calldata).await?;

        let reserves = IUniswapV2Pair::getReservesCall::abi_decode_returns(&output)
            .map_err(|e| eyre!("Failed to decode reserves: {}", e))?;

        let r0: u128 = reserves.reserve0.to();
        let r1: u128 = reserves.reserve1.to();

        // Cache it
        {
            let mut cache = RESERVES_CACHE.write().unwrap();
            cache.insert(pool, CachedReserves {
                reserve0: r0,
                reserve1: r1,
                cached_at: Instant::now(),
            });
        }
        debug!("Cached reserves for V2 pool {:?}", pool);

        Ok((r0, r1))
    }
    
    /// Get token0 for a V3 pool (CACHED - immutable per pool)
    async fn get_pool_token0(&self, pool: Address) -> Result<Address> {
        // Check cache first
        if let Some(token0) = TOKEN0_CACHE.read().unwrap().get(&pool) {
            return Ok(*token0);
        }

        // Fetch from chain
        let calldata = IUniswapV3Pool::token0Call {}.abi_encode();
        let output = self.call_contract(pool, calldata).await?;

        let decoded = IUniswapV3Pool::token0Call::abi_decode_returns(&output)
            .map_err(|e| eyre!("Failed to decode token0: {}", e))?;

        // Cache it (token0 is immutable)
        TOKEN0_CACHE.write().unwrap().insert(pool, decoded);
        debug!("Cached token0 for pool {:?}", pool);

        Ok(decoded)
    }

    /// Get token0 for a V2 pair (CACHED - immutable per pool)
    async fn get_v2_token0(&self, pool: Address) -> Result<Address> {
        // Check cache first (shared with V3 - token0 is token0)
        if let Some(token0) = TOKEN0_CACHE.read().unwrap().get(&pool) {
            return Ok(*token0);
        }

        // Fetch from chain
        let calldata = IUniswapV2Pair::token0Call {}.abi_encode();
        let output = self.call_contract(pool, calldata).await?;

        let decoded = IUniswapV2Pair::token0Call::abi_decode_returns(&output)
            .map_err(|e| eyre!("Failed to decode token0: {}", e))?;

        // Cache it (token0 is immutable)
        TOKEN0_CACHE.write().unwrap().insert(pool, decoded);
        debug!("Cached V2 token0 for pair {:?}", pool);

        Ok(decoded)
    }

    /// Get fee tier for a V3 pool (CACHED - immutable per pool)
    pub async fn get_pool_fee(&self, pool: Address) -> Result<u32> {
        // Check cache first
        if let Some(fee) = FEE_CACHE.read().unwrap().get(&pool) {
            return Ok(*fee);
        }

        // Fetch from chain
        let calldata = IUniswapV3Pool::feeCall {}.abi_encode();
        let output = self.call_contract(pool, calldata).await?;

        let decoded = IUniswapV3Pool::feeCall::abi_decode_returns(&output)
            .map_err(|e| eyre!("Failed to decode fee: {}", e))?;

        let fee: u32 = decoded.to();

        // Cache it (fee is immutable)
        FEE_CACHE.write().unwrap().insert(pool, fee);
        debug!("Cached fee {} for pool {:?}", fee, pool);

        Ok(fee)
    }

    /// Get cache statistics for monitoring
    /// Returns (token0_count, fee_count, reserves_count, valid_reserves_count)
    pub fn cache_stats() -> (usize, usize, usize, usize) {
        let token0_count = TOKEN0_CACHE.read().unwrap().len();
        let fee_count = FEE_CACHE.read().unwrap().len();
        let reserves_cache = RESERVES_CACHE.read().unwrap();
        let reserves_count = reserves_cache.len();
        let valid_reserves = reserves_cache.values().filter(|c| c.is_valid()).count();
        (token0_count, fee_count, reserves_count, valid_reserves)
    }
}
