//! Pool Data Fetcher - Concurrent Edition (no multicall)
//!
//! Uses concurrent futures for parallel fetching - reliable and fast

use alloy_primitives::{Address, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{sol, SolCall};
use alloy_rpc_types::TransactionRequest;
use eyre::{eyre, Result};
use futures::future::join_all;
use std::str::FromStr;
use std::time::Instant;
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{info, debug};

sol! {
    interface IUniswapV3Pool {
        function slot0() external view returns (
            uint160 sqrtPriceX96, int24 tick, uint16 observationIndex,
            uint16 observationCardinality, uint16 observationCardinalityNext,
            uint8 feeProtocol, bool unlocked
        );
        function liquidity() external view returns (uint128);
        function token0() external view returns (address);
        function token1() external view returns (address);
        function fee() external view returns (uint24);
    }
    
    interface IUniswapV2Pair {
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
        function token0() external view returns (address);
        function token1() external view returns (address);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dex { UniswapV3, UniswapV2, SushiswapV3, SushiswapV2, PancakeSwapV3, BalancerV2, Curve }

impl std::fmt::Display for Dex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Dex::UniswapV3 => write!(f, "UniV3"), Dex::UniswapV2 => write!(f, "UniV2"),
            Dex::SushiswapV3 => write!(f, "SushiV3"), Dex::SushiswapV2 => write!(f, "SushiV2"),
            Dex::PancakeSwapV3 => write!(f, "PancakeV3"), Dex::BalancerV2 => write!(f, "BalV2"),
            Dex::Curve => write!(f, "Curve"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolType { V2, V3, Balancer, Curve }

#[derive(Debug, Clone)]
pub struct PoolState {
    pub address: Address, pub token0: Address, pub token1: Address,
    pub token0_decimals: u8, pub token1_decimals: u8,
    pub sqrt_price_x96: U256, pub tick: i32, pub liquidity: u128, pub reserve1: u128,
    pub fee: u32, pub is_v4: bool, pub dex: Dex, pub pool_type: PoolType, pub weight0: u128,
}

impl PoolState {
    pub fn price(&self, _: u8, _: u8) -> f64 { self.normalized_price() }
    
    pub fn normalized_price(&self) -> f64 {
        match self.pool_type {
            PoolType::V3 => {
                let sp = self.sqrt_price_x96.to::<u128>() as f64;
                if sp == 0.0 { return 0.0; }
                let price_raw = (sp / 2_f64.powi(96)).powi(2);
                price_raw * 10_f64.powi(self.token0_decimals as i32 - self.token1_decimals as i32)
            }
            _ => {
                if self.liquidity == 0 || self.reserve1 == 0 { return 0.0; }
                let price = (self.reserve1 as f64 / self.liquidity as f64)
                    * 10_f64.powi(self.token0_decimals as i32 - self.token1_decimals as i32);
                if self.pool_type == PoolType::Balancer && self.weight0 != 0 {
                    let w0 = self.weight0 as f64 / 1e18;
                    return price * (w0 / (1.0 - w0));
                }
                price
            }
        }
    }
    pub fn raw_price(&self) -> f64 { self.normalized_price() }
}

#[derive(Clone)]
pub struct PoolInfo {
    pub address: &'static str, pub token0_symbol: &'static str, pub token1_symbol: &'static str,
    pub fee: u32, pub dex: Dex, pub pool_type: PoolType, pub weight0: Option<f64>,
}

#[derive(Debug, Clone)]
struct CachedPoolData { token0: Address, token1: Address, token0_decimals: u8, token1_decimals: u8, fee: u32 }

lazy_static::lazy_static! {
    static ref POOL_CACHE: RwLock<HashMap<Address, CachedPoolData>> = RwLock::new(HashMap::new());
}

pub fn get_token_decimals(address: &Address) -> u8 {
    let a = format!("{:?}", address).to_lowercase();
    if a.contains("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48") || a.contains("dac17f958d2ee523a2206206994597c13d831ec7") { 6 }
    else if a.contains("2260fac5e5542a773aa44fbcfedf7c193bc2c599") { 8 }
    else { 18 }
}

pub fn get_all_known_pools() -> Vec<PoolInfo> {
    vec![
        // UniV3 Core
        PoolInfo { address: "0x88e6A0c2dDD26FEEb64F039a2c41296FcB3f5640", token0_symbol: "USDC", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x8ad599c3A0ff1De082011EFDDc58f1908eb6e6D8", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x11b815efB8f581194ae79006d24E0d814B7697F6", token0_symbol: "WETH", token1_symbol: "USDT", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x4e68Ccd3E89f51C3074ca5072bbAC773960dFa36", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x60594a405d53811d3BC4766596EFD80fd545A270", token0_symbol: "DAI", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xC2e9F25Be6257c210d7Adf0D4Cd6E3E881ba25f8", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x4585FE77225b41b697C938B018E2Ac67Ac5a20c0", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xCBCdF9626bC03E24f779434178A73a0B4bad62eD", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x3416cF6C708Da44DB2624D63ea0AAef7113527C6", token0_symbol: "USDC", token1_symbol: "USDT", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x5777d92f208679DB4b9778590Fa3CAB3aC9e2168", token0_symbol: "DAI", token1_symbol: "USDC", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x109830a1AAaD605BbF02a9dFA7B0B92EC2FB7dAa", token0_symbol: "wstETH", token1_symbol: "WETH", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        // UniV3 Long tail
        PoolInfo { address: "0x11950d141EcB863F01007AdD7D1A342041227b58", token0_symbol: "PEPE", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x2F62f2B4c5fcd7570a709DeC05D68EA19c82A9ec", token0_symbol: "SHIB", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xa6Cc3C2531FdaA6Ae1A3CA84c2855806728693e8", token0_symbol: "LINK", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x1d42064Fc4Beb5F8aAF85F4617AE8b3b5B8Bd801", token0_symbol: "UNI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x5aB53EE1d50eeF2C1DD3d5402789cd27bB52c1bB", token0_symbol: "AAVE", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        // UniV2
        PoolInfo { address: "0xB4e16d0168e52d35CaCD2c6185b44281Ec28C9Dc", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x0d4a11d5EEaaC28EC3F61d100daF4d40471f1852", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xA478c2975Ab1Ea89e8196811F51A7B7Ade33eB11", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xBb2b8038a1640196FbE3e38816F3e67Cba72D940", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xd3d2E2692501A5c9Ca623199D38826e513033a17", token0_symbol: "LINK", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xA43fe16908251ee70EF74718545e4FE6C5cCec9f", token0_symbol: "PEPE", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        // SushiV2
        PoolInfo { address: "0x397FF1542f962076d0BFE58eA045FfA2d347ACa0", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x06da0fd433C1A5d7a4faa01111c044910A184553", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xC3D03e4F041Fd4cD388c549Ee2A29a9E5075882f", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xCEfF51756c56CeFFCA006cD410B03FFC46dd3a58", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xC40D16476380e4037e6b1A2594cAF6a6cc8Da967", token0_symbol: "LINK", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        // PancakeV3
        PoolInfo { address: "0x1ac1A8FEaAEa1900C4166dEeed0C11cC10669D36", token0_symbol: "USDC", token1_symbol: "WETH", fee: 500, dex: Dex::PancakeSwapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x6CA298D2983aB03Aa1dA7679389D955A4eFEE15C", token0_symbol: "WETH", token1_symbol: "USDT", fee: 500, dex: Dex::PancakeSwapV3, pool_type: PoolType::V3, weight0: None },
        // Balancer
        PoolInfo { address: "0x32296969Ef14EB0c6d29669C550D4a0449130230", token0_symbol: "wstETH", token1_symbol: "WETH", fee: 4, dex: Dex::BalancerV2, pool_type: PoolType::Balancer, weight0: Some(0.5) },
    ]
}

pub struct PoolFetcher { rpc_url: String }

impl PoolFetcher {
    pub fn new(rpc_url: String) -> Self { Self { rpc_url } }

    async fn call(&self, to: Address, data: Vec<u8>) -> Result<Vec<u8>> {
        let provider = ProviderBuilder::new().on_http(self.rpc_url.parse()?);
        let tx = TransactionRequest::default().to(to).input(data.into());
        Ok(provider.call(tx).await?.to_vec())
    }

    pub async fn cache_stats(&self) -> (usize, usize) {
        (POOL_CACHE.read().await.len(), get_all_known_pools().len())
    }

    async fn fetch_v3(&self, addr: Address, dex: Dex) -> Result<PoolState> {
        let cached = POOL_CACHE.read().await.get(&addr).cloned();
        let (t0, t1, d0, d1, fee) = if let Some(c) = cached {
            (c.token0, c.token1, c.token0_decimals, c.token1_decimals, c.fee)
        } else {
            let t0 = IUniswapV3Pool::token0Call::abi_decode_returns(&self.call(addr, IUniswapV3Pool::token0Call{}.abi_encode()).await?)?;
            let t1 = IUniswapV3Pool::token1Call::abi_decode_returns(&self.call(addr, IUniswapV3Pool::token1Call{}.abi_encode()).await?)?;
            let f = IUniswapV3Pool::feeCall::abi_decode_returns(&self.call(addr, IUniswapV3Pool::feeCall{}.abi_encode()).await?)?;
            let (d0, d1, fee) = (get_token_decimals(&t0), get_token_decimals(&t1), f.to());
            POOL_CACHE.write().await.insert(addr, CachedPoolData{token0:t0,token1:t1,token0_decimals:d0,token1_decimals:d1,fee});
            (t0, t1, d0, d1, fee)
        };
        let s = IUniswapV3Pool::slot0Call::abi_decode_returns(&self.call(addr, IUniswapV3Pool::slot0Call{}.abi_encode()).await?)?;
        let l = IUniswapV3Pool::liquidityCall::abi_decode_returns(&self.call(addr, IUniswapV3Pool::liquidityCall{}.abi_encode()).await?)?;
        Ok(PoolState{address:addr,token0:t0,token1:t1,token0_decimals:d0,token1_decimals:d1,sqrt_price_x96:U256::from(s.sqrtPriceX96.to::<u128>()),tick:s.tick.as_i32(),liquidity:l,reserve1:0,fee,is_v4:false,dex,pool_type:PoolType::V3,weight0:0})
    }

    async fn fetch_v2(&self, addr: Address, dex: Dex, fee: u32, w: Option<f64>) -> Result<PoolState> {
        let cached = POOL_CACHE.read().await.get(&addr).cloned();
        let (t0, t1, d0, d1) = if let Some(c) = cached {
            (c.token0, c.token1, c.token0_decimals, c.token1_decimals)
        } else {
            let t0 = IUniswapV2Pair::token0Call::abi_decode_returns(&self.call(addr, IUniswapV2Pair::token0Call{}.abi_encode()).await?)?;
            let t1 = IUniswapV2Pair::token1Call::abi_decode_returns(&self.call(addr, IUniswapV2Pair::token1Call{}.abi_encode()).await?)?;
            let (d0, d1) = (get_token_decimals(&t0), get_token_decimals(&t1));
            POOL_CACHE.write().await.insert(addr, CachedPoolData{token0:t0,token1:t1,token0_decimals:d0,token1_decimals:d1,fee});
            (t0, t1, d0, d1)
        };
        let r = IUniswapV2Pair::getReservesCall::abi_decode_returns(&self.call(addr, IUniswapV2Pair::getReservesCall{}.abi_encode()).await?)?;
        let pt = if dex == Dex::BalancerV2 { PoolType::Balancer } else { PoolType::V2 };
        Ok(PoolState{address:addr,token0:t0,token1:t1,token0_decimals:d0,token1_decimals:d1,sqrt_price_x96:U256::ZERO,tick:0,liquidity:r.reserve0.to(),reserve1:r.reserve1.to(),fee,is_v4:false,dex,pool_type:pt,weight0:(w.unwrap_or(0.5)*1e18) as u128})
    }

    async fn fetch_pool(&self, i: &PoolInfo) -> Result<PoolState> {
        let a = Address::from_str(i.address)?;
        match i.pool_type {
            PoolType::V3 => self.fetch_v3(a, i.dex).await,
            _ => self.fetch_v2(a, i.dex, i.fee, i.weight0).await,
        }
    }

    pub async fn fetch_all_pools(&self) -> Result<Vec<PoolState>> {
        let start = Instant::now();
        let infos = get_all_known_pools();
        let (cached, total) = self.cache_stats().await;
        info!("{} Fetching {} pools (cache: {}/{})", if cached==0 {"🚀"} else {"🔄"}, total, cached, total);
        
        let futs: Vec<_> = infos.iter().map(|i| self.fetch_pool(i)).collect();
        let results = join_all(futs).await;
        
        let mut pools = Vec::new();
        let mut fail = 0;
        for (r, i) in results.into_iter().zip(infos.iter()) {
            match r {
                Ok(p) if p.normalized_price() > 0.0 && p.normalized_price() < 1e12 => pools.push(p),
                Ok(_) => { fail += 1; debug!("Invalid price: {}", i.address); }
                Err(e) => { fail += 1; debug!("Failed {}: {}", i.address, e); }
            }
        }
        info!("✅ Fetched {} pools in {:?} ({} failed)", pools.len(), start.elapsed(), fail);
        if pools.is_empty() { return Err(eyre!("No pools!")); }
        Ok(pools)
    }
}
