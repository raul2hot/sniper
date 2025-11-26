//! Pool Data Fetcher - DECIMAL-AWARE Edition
//!
//! Step 1.1: The Scout
//!
//! Uses manual eth_call via Provider for contract interactions.

use alloy_primitives::{Address, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{sol, SolCall};
use alloy_rpc_types::TransactionRequest;
use eyre::{eyre, Result};
use futures::future::join_all;
use std::str::FromStr;
use std::time::Instant;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, debug, warn};

// ============================================
// CONTRACT INTERFACES
// ============================================
sol! {
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
    
    interface IUniswapV2Pair {
        function getReserves() external view returns (
            uint112 reserve0,
            uint112 reserve1,
            uint32 blockTimestampLast
        );
        
        function token0() external view returns (address);
        function token1() external view returns (address);
    }
}

/// Which DEX this pool belongs to
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dex {
    UniswapV3,
    UniswapV2,
    SushiswapV3,
    SushiswapV2,
    PancakeSwapV3,
    BalancerV2,
    Curve,
}

impl std::fmt::Display for Dex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Dex::UniswapV3 => write!(f, "UniV3"),
            Dex::UniswapV2 => write!(f, "UniV2"),
            Dex::SushiswapV3 => write!(f, "SushiV3"),
            Dex::SushiswapV2 => write!(f, "SushiV2"),
            Dex::PancakeSwapV3 => write!(f, "PancakeV3"),
            Dex::BalancerV2 => write!(f, "BalV2"),
            Dex::Curve => write!(f, "Curve"),
        }
    }
}

/// Pool type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolType {
    V2,
    V3,
    Balancer,
    Curve,
}

/// Represents a pool's current state with DECIMAL-NORMALIZED prices
#[derive(Debug, Clone)]
pub struct PoolState {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub token0_decimals: u8,
    pub token1_decimals: u8,
    pub sqrt_price_x96: U256,
    pub tick: i32,
    pub liquidity: u128,
    pub reserve1: u128,
    pub fee: u32,
    pub is_v4: bool,
    pub dex: Dex,
    pub pool_type: PoolType,
    pub weight0: u128,
}

impl PoolState {
    pub fn price(&self, _t0_decimals_override: u8, _t1_decimals_override: u8) -> f64 {
        self.normalized_price()
    }
    
    pub fn normalized_price(&self) -> f64 {
        match self.pool_type {
            PoolType::V3 => {
                let sqrt_price_x96 = self.sqrt_price_x96.to::<u128>() as f64;
                if sqrt_price_x96 == 0.0 {
                    return 0.0;
                }
                
                let q96 = 2_f64.powi(96);
                let price_raw = (sqrt_price_x96 / q96).powi(2);
                
                let decimal_diff = self.token0_decimals as i32 - self.token1_decimals as i32;
                let decimal_adjustment = 10_f64.powi(decimal_diff);
                
                price_raw * decimal_adjustment
            }
            PoolType::V2 | PoolType::Balancer | PoolType::Curve => {
                if self.liquidity == 0 || self.reserve1 == 0 {
                    return 0.0;
                }
                
                let reserve0 = self.liquidity as f64;
                let reserve1 = self.reserve1 as f64;
                let price_raw = reserve1 / reserve0;
                
                let decimal_diff = self.token0_decimals as i32 - self.token1_decimals as i32;
                let decimal_adjustment = 10_f64.powi(decimal_diff);
                
                let price = price_raw * decimal_adjustment;
                
                if self.pool_type == PoolType::Balancer && self.weight0 != 0 {
                    let w0 = self.weight0 as f64 / 1e18;
                    let w1 = 1.0 - w0;
                    return price * (w0 / w1);
                }
                
                price
            }
        }
    }

    pub fn raw_price(&self) -> f64 {
        self.normalized_price()
    }
}

/// Pool info for fetching
#[derive(Clone)]
pub struct PoolInfo {
    pub address: &'static str,
    pub token0_symbol: &'static str,
    pub token1_symbol: &'static str,
    pub fee: u32,
    pub dex: Dex,
    pub pool_type: PoolType,
    pub weight0: Option<f64>,
}

/// Get token decimals from address - EXPORTED for use by simulator
pub fn get_token_decimals(address: &Address) -> u8 {
    let addr_lower = format!("{:?}", address).to_lowercase();
    
    match addr_lower.as_str() {
        a if a.contains("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48") => 6,  // USDC
        a if a.contains("dac17f958d2ee523a2206206994597c13d831ec7") => 6,  // USDT
        a if a.contains("2260fac5e5542a773aa44fbcfedf7c193bc2c599") => 8,  // WBTC
        a if a.contains("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2") => 18, // WETH
        a if a.contains("6b175474e89094c44da98b954eedcdecb5be3830") => 18, // DAI
        a if a.contains("7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0") => 18, // wstETH
        a if a.contains("ae7ab96520de3a18e5e111b5eaab095312d7fe84") => 18, // stETH
        _ => 18,
    }
}

// ============================================
// POOL DEFINITIONS
// ============================================
pub fn get_uniswap_v3_pools() -> Vec<PoolInfo> {
    vec![
        PoolInfo { address: "0x3416cF6C708Da44DB2624D63ea0AAef7113527C6", token0_symbol: "USDC", token1_symbol: "USDT", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x5777d92f208679DB4b9778590Fa3CAB3aC9e2168", token0_symbol: "DAI", token1_symbol: "USDC", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x88e6A0c2dDD26FEEb64F039a2c41296FcB3f5640", token0_symbol: "USDC", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x11b815efB8f581194ae79006d24E0d814B7697F6", token0_symbol: "WETH", token1_symbol: "USDT", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x60594a405d53811d3BC4766596EFD80fd545A270", token0_symbol: "DAI", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x4585FE77225b41b697C938B018E2Ac67Ac5a20c0", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x8ad599c3A0ff1De082011EFDDc58f1908eb6e6D8", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xC2e9F25Be6257c210d7Adf0D4Cd6E3E881ba25f8", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xCBCdF9626bC03E24f779434178A73a0B4bad62eD", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
    ]
}

pub fn get_uniswap_v2_pools() -> Vec<PoolInfo> {
    vec![
        PoolInfo { address: "0xB4e16d0168e52d35CaCD2c6185b44281Ec28C9Dc", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x0d4a11d5EEaaC28EC3F61d100daF4d40471f1852", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xA478c2975Ab1Ea89e8196811F51A7B7Ade33eB11", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xBb2b8038a1640196FbE3e38816F3e67Cba72D940", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xAE461cA67B15dc8dc81CE7615e0320dA1A9aB8D5", token0_symbol: "DAI", token1_symbol: "USDC", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
    ]
}

pub fn get_sushiswap_v2_pools() -> Vec<PoolInfo> {
    vec![
        PoolInfo { address: "0x397FF1542f962076d0BFE58eA045FfA2d347ACa0", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x06da0fd433C1A5d7a4faa01111c044910A184553", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xCEfF51756c56CeFFCA006cD410B03FFC46dd3a58", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
    ]
}

pub fn get_pancakeswap_v3_pools() -> Vec<PoolInfo> {
    vec![
        PoolInfo { address: "0x1ac1A8FEaAEa1900C4166dEeed0C11cC10669D36", token0_symbol: "USDC", token1_symbol: "WETH", fee: 500, dex: Dex::PancakeSwapV3, pool_type: PoolType::V3, weight0: None },
    ]
}

pub fn get_balancer_v2_pools() -> Vec<PoolInfo> {
    vec![
        PoolInfo { 
            address: "0x32296969Ef14EB0c6d29669C550D4a0449130230", 
            token0_symbol: "wstETH", 
            token1_symbol: "WETH", 
            fee: 4,
            dex: Dex::BalancerV2, 
            pool_type: PoolType::Balancer, 
            weight0: Some(0.5),
        },
    ]
}

pub fn get_all_known_pools() -> Vec<PoolInfo> {
    let mut pools = Vec::new();
    pools.extend(get_uniswap_v3_pools());
    pools.extend(get_uniswap_v2_pools());
    pools.extend(get_sushiswap_v2_pools());
    pools.extend(get_pancakeswap_v3_pools());
    pools.extend(get_balancer_v2_pools());
    pools
}

/// Pool data fetcher using manual eth_call
pub struct PoolFetcher {
    rpc_url: String,
}

impl PoolFetcher {
    pub fn new(rpc_url: String) -> Self {
        Self { rpc_url }
    }

    fn get_decimals(&self, token: Address) -> u8 {
        get_token_decimals(&token)
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

    async fn fetch_v3_pool_inner(&self, pool_address: Address, dex: Dex) -> Result<PoolState> {
        // Fetch slot0
        let slot0_calldata = IUniswapV3Pool::slot0Call {}.abi_encode();
        let slot0_result = self.call_contract(pool_address, slot0_calldata).await?;
        let slot0 = IUniswapV3Pool::slot0Call::abi_decode_returns(&slot0_result)
            .map_err(|e| eyre!("slot0 decode: {}", e))?;
        
        // Fetch liquidity
        let liq_calldata = IUniswapV3Pool::liquidityCall {}.abi_encode();
        let liq_result = self.call_contract(pool_address, liq_calldata).await?;
        let liquidity = IUniswapV3Pool::liquidityCall::abi_decode_returns(&liq_result)
            .map_err(|e| eyre!("liquidity decode: {}", e))?;
        
        // Fetch token0
        let t0_calldata = IUniswapV3Pool::token0Call {}.abi_encode();
        let t0_result = self.call_contract(pool_address, t0_calldata).await?;
        let token0 = IUniswapV3Pool::token0Call::abi_decode_returns(&t0_result)
            .map_err(|e| eyre!("token0 decode: {}", e))?;
        
        // Fetch token1
        let t1_calldata = IUniswapV3Pool::token1Call {}.abi_encode();
        let t1_result = self.call_contract(pool_address, t1_calldata).await?;
        let token1 = IUniswapV3Pool::token1Call::abi_decode_returns(&t1_result)
            .map_err(|e| eyre!("token1 decode: {}", e))?;
        
        // Fetch fee
        let fee_calldata = IUniswapV3Pool::feeCall {}.abi_encode();
        let fee_result = self.call_contract(pool_address, fee_calldata).await?;
        let fee = IUniswapV3Pool::feeCall::abi_decode_returns(&fee_result)
            .map_err(|e| eyre!("fee decode: {}", e))?;

        let token0_decimals = self.get_decimals(token0);
        let token1_decimals = self.get_decimals(token1);

        // Convert U160 to u128 for sqrtPriceX96
        let sqrt_price: u128 = slot0.sqrtPriceX96.to();
        
        // Convert u24 fee to u32
        let fee_u32: u32 = fee.to();

        Ok(PoolState {
            address: pool_address,
            token0,
            token1,
            token0_decimals,
            token1_decimals,
            sqrt_price_x96: U256::from(sqrt_price),
            tick: slot0.tick.as_i32(),
            liquidity,
            reserve1: 0,
            fee: fee_u32,
            is_v4: false,
            dex,
            pool_type: PoolType::V3,
            weight0: 0,
        })
    }

    async fn fetch_v2_pool_inner(&self, pool_address: Address, dex: Dex, fee: u32) -> Result<PoolState> {
        // Fetch reserves
        let res_calldata = IUniswapV2Pair::getReservesCall {}.abi_encode();
        let res_result = self.call_contract(pool_address, res_calldata).await?;
        let reserves = IUniswapV2Pair::getReservesCall::abi_decode_returns(&res_result)
            .map_err(|e| eyre!("reserves decode: {}", e))?;
        
        // Fetch token0
        let t0_calldata = IUniswapV2Pair::token0Call {}.abi_encode();
        let t0_result = self.call_contract(pool_address, t0_calldata).await?;
        let token0 = IUniswapV2Pair::token0Call::abi_decode_returns(&t0_result)
            .map_err(|e| eyre!("token0 decode: {}", e))?;
        
        // Fetch token1
        let t1_calldata = IUniswapV2Pair::token1Call {}.abi_encode();
        let t1_result = self.call_contract(pool_address, t1_calldata).await?;
        let token1 = IUniswapV2Pair::token1Call::abi_decode_returns(&t1_result)
            .map_err(|e| eyre!("token1 decode: {}", e))?;

        let token0_decimals = self.get_decimals(token0);
        let token1_decimals = self.get_decimals(token1);

        // Convert u112 to u128
        let reserve0: u128 = reserves.reserve0.to();
        let reserve1: u128 = reserves.reserve1.to();

        Ok(PoolState {
            address: pool_address,
            token0,
            token1,
            token0_decimals,
            token1_decimals,
            sqrt_price_x96: U256::ZERO,
            tick: 0,
            liquidity: reserve0,
            reserve1,
            fee,
            is_v4: false,
            dex,
            pool_type: PoolType::V2,
            weight0: 0,
        })
    }

    async fn fetch_pool(&self, pool_info: &PoolInfo) -> Result<PoolState> {
        let address = Address::from_str(pool_info.address)
            .map_err(|_| eyre!("Invalid address: {}", pool_info.address))?;

        match pool_info.pool_type {
            PoolType::V3 => self.fetch_v3_pool_inner(address, pool_info.dex).await,
            PoolType::V2 => self.fetch_v2_pool_inner(address, pool_info.dex, pool_info.fee).await,
            PoolType::Balancer => {
                let mut state = self.fetch_v2_pool_inner(address, pool_info.dex, pool_info.fee).await;
                if let Ok(ref mut s) = state {
                    s.pool_type = PoolType::Balancer;
                    s.weight0 = (pool_info.weight0.unwrap_or(0.5) * 1e18) as u128;
                }
                state
            }
            PoolType::Curve => Err(eyre!("Curve not supported")),
        }
    }

    pub async fn fetch_all_pools(&self) -> Result<Vec<PoolState>> {
        let start = Instant::now();
        info!("🚀 Fetching pools from 5 DEXes...");
        
        let all_pool_infos = get_all_known_pools();
        let total_pools = all_pool_infos.len();
        
        info!("   Queuing {} pool fetch requests...", total_pools);

        let futures: Vec<_> = all_pool_infos
            .iter()
            .map(|info| self.fetch_pool(info))
            .collect();

        let results = join_all(futures).await;

        let mut pools = Vec::new();
        let mut success_count = 0;
        let mut fail_count = 0;

        for (result, info) in results.into_iter().zip(all_pool_infos.iter()) {
            match result {
                Ok(pool) => {
                    let price = pool.normalized_price();
                    
                    if price > 0.0 && price < 1e12 {
                        debug!(
                            "✓ [{}] {}/{}: price={:.6}",
                            pool.dex, info.token0_symbol, info.token1_symbol, price
                        );
                        pools.push(pool);
                        success_count += 1;
                    } else {
                        warn!(
                            "⚠ [{}] {}/{}: INVALID price={:.2e} - skipping",
                            info.dex, info.token0_symbol, info.token1_symbol, price
                        );
                        fail_count += 1;
                    }
                }
                Err(e) => {
                    debug!(
                        "✗ [{}] {}/{}: {}",
                        info.dex, info.token0_symbol, info.token1_symbol, e
                    );
                    fail_count += 1;
                }
            }
        }

        let elapsed = start.elapsed();

        let counts: HashMap<Dex, usize> = pools.iter()
            .fold(HashMap::new(), |mut acc, p| {
                *acc.entry(p.dex).or_insert(0) += 1;
                acc
            });
        
        let low_fee_count = pools.iter()
            .filter(|p| p.pool_type == PoolType::V3 && p.fee <= 500)
            .count();

        info!("✅ Fetched {} pools in {:?} ({} failed/invalid)", success_count, elapsed, fail_count);
        info!("   By DEX:");
        for dex in [Dex::UniswapV3, Dex::UniswapV2, Dex::SushiswapV2, Dex::PancakeSwapV3, Dex::BalancerV2] {
            if let Some(&count) = counts.get(&dex) {
                info!("     {}: {} pools", dex, count);
            }
        }
        info!("   Low-fee pools (≤5bps): {}", low_fee_count);

        if pools.is_empty() {
            return Err(eyre!("No pools fetched! Check your RPC URL."));
        }

        Ok(pools)
    }
}