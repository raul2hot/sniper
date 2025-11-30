//! LP Token Secondary Market Discovery
//!
//! Discovers Uniswap V3 and Balancer pools that trade Curve LP tokens.
//! These secondary markets enable NAV discount arbitrage without
//! using add_liquidity/remove_liquidity (which executor doesn't support).
//!
//! OPTIMIZATION: Discovery is throttled and heavily cached.

use alloy_primitives::{Address, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types::TransactionRequest;
use alloy_sol_types::SolCall;
use eyre::{eyre, Result};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::nav_calculator::{SecondaryDex, SecondaryMarket};
use super::types::*;
use crate::cartographer::{Dex, PoolState, PoolType};

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
                    // fee is uint24 in the contract, use the raw value as it is
                    let fee = alloy_primitives::Uint::<24, 1>::from(*fee_tier);
                    calls.push(IMulticall3::Call3 {
                        target: UNISWAP_V3_FACTORY,
                        allowFailure: true,
                        callData: IUniswapV3Factory::getPoolCall {
                            tokenA: *lp_token,
                            tokenB: *quote_token,
                            fee,
                        }
                        .abi_encode()
                        .into(),
                    });
                    call_map.push((*lp_token, *quote_token, *fee_tier));
                }
            }
        }

        debug!(
            "LP Market Discovery: {} calls in 1 multicall",
            calls.len()
        );
        let results = self.execute_multicall(calls).await?;

        // Parse results
        let mut discovered: HashMap<Address, Vec<SecondaryMarket>> = HashMap::new();

        for (i, (lp_token, quote_token, fee_tier)) in call_map.iter().enumerate() {
            if i >= results.len() || !results[i].success {
                continue;
            }

            if let Ok(pool_addr) =
                IUniswapV3Factory::getPoolCall::abi_decode_returns(&results[i].returnData)
            {
                if pool_addr != Address::ZERO {
                    let market = SecondaryMarket {
                        pool_address: pool_addr,
                        dex_type: SecondaryDex::UniswapV3,
                        fee_bps: fee_tier / 100, // Convert from parts per million to bps
                        quote_token: *quote_token,
                        liquidity_usd: 0.0, // Will be fetched when needed
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
                cache.markets.insert(
                    *lp_token,
                    CachedMarket {
                        lp_token: *lp_token,
                        markets: markets.clone(),
                        cached_at: Instant::now(),
                    },
                );
            }
            // Also cache empty results
            for lp_token in &tokens_to_fetch {
                if !discovered.contains_key(lp_token) {
                    cache.markets.insert(
                        *lp_token,
                        CachedMarket {
                            lp_token: *lp_token,
                            markets: Vec::new(),
                            cached_at: Instant::now(),
                        },
                    );
                }
            }
        }

        // Merge with cached results
        for (lp_token, markets) in discovered {
            result.insert(lp_token, markets);
        }

        let total_markets: usize = result.values().map(|v| v.len()).sum();
        info!(
            "Discovered secondary markets for {} LP tokens ({} total markets)",
            result.len(),
            total_markets
        );

        Ok(result)
    }

    /// Fetch slot0 data for UniV3 pools to get current prices
    /// Returns map: pool_address -> (sqrtPriceX96, liquidity)
    pub async fn fetch_univ3_prices(
        &self,
        pool_addresses: &[Address],
    ) -> Result<HashMap<Address, (U256, u128)>> {
        if pool_addresses.is_empty() {
            return Ok(HashMap::new());
        }

        let mut calls = Vec::new();

        // Fetch slot0 and liquidity for each pool
        for pool in pool_addresses {
            calls.push(IMulticall3::Call3 {
                target: *pool,
                allowFailure: true,
                callData: IUniswapV3Pool::slot0Call {}.abi_encode().into(),
            });
            calls.push(IMulticall3::Call3 {
                target: *pool,
                allowFailure: true,
                callData: IUniswapV3Pool::liquidityCall {}.abi_encode().into(),
            });
        }

        debug!(
            "Fetching UniV3 prices for {} pools in 1 multicall",
            pool_addresses.len()
        );
        let results = self.execute_multicall(calls).await?;

        let mut prices = HashMap::new();

        for (i, pool) in pool_addresses.iter().enumerate() {
            let slot0_idx = i * 2;
            let liq_idx = i * 2 + 1;

            if slot0_idx >= results.len() || liq_idx >= results.len() {
                continue;
            }

            let sqrt_price = if results[slot0_idx].success {
                IUniswapV3Pool::slot0Call::abi_decode_returns(&results[slot0_idx].returnData)
                    .ok()
                    .map(|s| U256::from(s.sqrtPriceX96.to::<u128>()))
            } else {
                None
            };

            let liquidity = if results[liq_idx].success {
                IUniswapV3Pool::liquidityCall::abi_decode_returns(&results[liq_idx].returnData).ok()
            } else {
                None
            };

            if let (Some(sp), Some(liq)) = (sqrt_price, liquidity) {
                prices.insert(*pool, (sp, liq));
            }
        }

        Ok(prices)
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

    /// Get all unique UniV3 pool addresses from markets
    pub fn get_univ3_pool_addresses(
        &self,
        markets: &HashMap<Address, Vec<SecondaryMarket>>,
    ) -> Vec<Address> {
        let mut addresses = Vec::new();

        for market_list in markets.values() {
            for market in market_list {
                if market.dex_type == SecondaryDex::UniswapV3 {
                    addresses.push(market.pool_address);
                }
            }
        }

        // Deduplicate
        addresses.sort();
        addresses.dedup();

        addresses
    }

    /// Clear cache (for testing)
    #[cfg(test)]
    pub fn clear_cache(&self) {
        let mut cache = MARKET_CACHE.write().unwrap();
        cache.markets.clear();
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

/// Calculate LP token price from UniV3 sqrtPriceX96
/// Returns price in terms of quote token (e.g., USDC per LP token)
pub fn calculate_lp_price_from_sqrt(
    sqrt_price_x96: U256,
    lp_decimals: u8,
    quote_decimals: u8,
    lp_is_token0: bool,
) -> f64 {
    if sqrt_price_x96 == U256::ZERO {
        return 0.0;
    }

    let sp = sqrt_price_x96.to::<u128>() as f64;
    let price_raw = (sp / 2_f64.powi(96)).powi(2);

    // Adjust for decimals
    let decimal_adjustment = 10_f64.powi(lp_decimals as i32 - quote_decimals as i32);

    if lp_is_token0 {
        price_raw * decimal_adjustment
    } else {
        1.0 / (price_raw * decimal_adjustment)
    }
}

/// Calculate liquidity in USD for a UniV3 LP token market
/// Rough estimate using sqrt(liquidity) * price
pub fn estimate_market_liquidity_usd(
    liquidity: u128,
    sqrt_price_x96: U256,
    quote_price_usd: f64,
) -> f64 {
    if liquidity == 0 || sqrt_price_x96 == U256::ZERO {
        return 0.0;
    }

    // This is a simplified estimate
    // Real liquidity depends on tick range and current tick
    let liq_sqrt = (liquidity as f64).sqrt();
    let sp = sqrt_price_x96.to::<u128>() as f64 / 2_f64.powi(96);

    // Approximate TVL ~ 2 * sqrt(L * price) * quote_price
    2.0 * liq_sqrt * sp * quote_price_usd
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    #[test]
    fn test_get_quote_decimals() {
        let usdc = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
        let weth = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
        let random = address!("1111111111111111111111111111111111111111");

        assert_eq!(get_quote_decimals(&usdc), 6);
        assert_eq!(get_quote_decimals(&weth), 18);
        assert_eq!(get_quote_decimals(&random), 18); // Default
    }

    #[test]
    fn test_calculate_lp_price_from_sqrt() {
        // sqrtPriceX96 for price = 1.0 is 2^96
        let sqrt_price_x96 = U256::from(79228162514264337593543950336u128); // 2^96

        let price = calculate_lp_price_from_sqrt(sqrt_price_x96, 18, 18, true);
        assert!((price - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_markets_to_pool_states() {
        let discovery = LPMarketDiscovery::new("http://localhost:8545".to_string());

        let lp_token = address!("6c3F90f043a72FA612cbac8115EE7e52BDe6E490");
        let usdc = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");

        let market = SecondaryMarket {
            pool_address: address!("1234567890123456789012345678901234567890"),
            dex_type: SecondaryDex::UniswapV3,
            fee_bps: 30,
            quote_token: usdc,
            liquidity_usd: 100_000.0,
        };

        let mut markets = HashMap::new();
        markets.insert(lp_token, vec![market]);

        let states = discovery.markets_to_pool_states(&markets);

        assert_eq!(states.len(), 1);
        assert_eq!(states[0].token0, lp_token);
        assert_eq!(states[0].token1, usdc);
        assert_eq!(states[0].dex, Dex::UniswapV3);
        assert_eq!(states[0].pool_type, PoolType::V3);
    }
}
