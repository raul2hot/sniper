//! Pool Data Fetcher
//!
//! Step 1.1: The Scout
//!
//! Connects to RPC and fetches pool state for V3 and V2 pools across multiple DEXes.
//! Now supports: Uniswap V3, Sushiswap V3, Uniswap V2, Sushiswap V2
//!
//! Cross-DEX arbitrage is where real opportunities exist!
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

// Define the Uniswap V2 Pool interface (also used by Sushiswap V2)
sol! {
    #[sol(rpc)]
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
}

impl std::fmt::Display for Dex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Dex::UniswapV3 => write!(f, "UniV3"),
            Dex::UniswapV2 => write!(f, "UniV2"),
            Dex::SushiswapV3 => write!(f, "SushiV3"),
            Dex::SushiswapV2 => write!(f, "SushiV2"),
        }
    }
}

/// Pool type (V2 constant product vs V3 concentrated liquidity)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolType {
    V2,  // Constant product: x * y = k
    V3,  // Concentrated liquidity with sqrtPriceX96
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
    /// Current sqrt price (Q64.96 format) - only for V3
    pub sqrt_price_x96: U256,
    /// Current tick - only for V3
    pub tick: i32,
    /// Available liquidity (V3) or reserve0 (V2)
    pub liquidity: u128,
    /// Reserve1 for V2 pools (0 for V3)
    pub reserve1: u128,
    /// Fee tier (500 = 0.05%, 3000 = 0.3%, 10000 = 1%)
    pub fee: u32,
    /// Is this a V4 pool?
    pub is_v4: bool,
    /// Which DEX this pool belongs to
    pub dex: Dex,
    /// Pool type (V2 or V3)
    pub pool_type: PoolType,
}

impl PoolState {
    /// Calculate the price of token0 in terms of token1
    /// Adjusts for decimal differences between tokens
    pub fn price(&self, token0_decimals: u8, token1_decimals: u8) -> f64 {
        match self.pool_type {
            PoolType::V3 => {
                // sqrtPriceX96 = sqrt(price) * 2^96
                // price = (sqrtPriceX96 / 2^96)^2
                let sqrt_price_x96 = self.sqrt_price_x96.to::<u128>() as f64;
                let q96 = 2_f64.powi(96);
                let price = (sqrt_price_x96 / q96).powi(2);
                
                // Adjust for decimals: price * 10^(token0_decimals - token1_decimals)
                let decimal_adjustment = 10_f64.powi(token0_decimals as i32 - token1_decimals as i32);
                price * decimal_adjustment
            }
            PoolType::V2 => {
                // V2: price = reserve1 / reserve0
                if self.liquidity == 0 {
                    return 0.0;
                }
                let reserve0 = self.liquidity as f64;
                let reserve1 = self.reserve1 as f64;
                let price = reserve1 / reserve0;
                
                // Adjust for decimals
                let decimal_adjustment = 10_f64.powi(token0_decimals as i32 - token1_decimals as i32);
                price * decimal_adjustment
            }
        }
    }

    /// Calculate the price of token1 in terms of token0
    pub fn inverse_price(&self, token0_decimals: u8, token1_decimals: u8) -> f64 {
        1.0 / self.price(token0_decimals, token1_decimals)
    }
    
    /// Get raw price without decimal adjustment (for graph weights)
    pub fn raw_price(&self) -> f64 {
        match self.pool_type {
            PoolType::V3 => {
                let sqrt_price_x96 = self.sqrt_price_x96.to::<u128>() as f64;
                let q96 = 2_f64.powi(96);
                (sqrt_price_x96 / q96).powi(2)
            }
            PoolType::V2 => {
                if self.liquidity == 0 {
                    return 0.0;
                }
                self.reserve1 as f64 / self.liquidity as f64
            }
        }
    }
}

/// Pool info for fetching - includes DEX and pool type
pub struct PoolInfo {
    pub address: &'static str,
    pub token0_symbol: &'static str,
    pub token1_symbol: &'static str,
    pub fee: u32,
    pub dex: Dex,
    pub pool_type: PoolType,
}

/// Known Uniswap V3 pool addresses on Ethereum Mainnet
pub fn get_uniswap_v3_pools() -> Vec<PoolInfo> {
    vec![
        // ============================================
        // WETH <-> Stablecoin pairs (multiple fee tiers = arb potential)
        // ============================================
        PoolInfo { address: "0x88e6A0c2dDD26FEEb64F039a2c41296FcB3f5640", token0_symbol: "USDC", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0x8ad599c3A0ff1De082011EFDDc58f1908eb6e6D8", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0xE0554a476A092703abdB3Ef35c80e0D76d32939F", token0_symbol: "USDC", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        
        PoolInfo { address: "0x11b815efB8f581194ae79006d24E0d814B7697F6", token0_symbol: "WETH", token1_symbol: "USDT", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0x4e68Ccd3E89f51C3074ca5072bbAC773960dFa36", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        
        PoolInfo { address: "0x60594a405d53811d3BC4766596EFD80fd545A270", token0_symbol: "DAI", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0xC2e9F25Be6257c210d7Adf0D4Cd6E3E881ba25f8", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        
        // Stablecoin pairs
        PoolInfo { address: "0x3416cF6C708Da44DB2624D63ea0AAef7113527C6", token0_symbol: "USDC", token1_symbol: "USDT", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0x7858E59e0C01EA06Df3aF3D20aC7B0003275D4Bf", token0_symbol: "USDC", token1_symbol: "USDT", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0x5777d92f208679DB4b9778590Fa3CAB3aC9e2168", token0_symbol: "DAI", token1_symbol: "USDC", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0x6c6Bc977E13Df9b0de53b251522280BB72383700", token0_symbol: "DAI", token1_symbol: "USDC", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0x6f48ECa74B38d2936B02ab603FF4e36A6C0E3A77", token0_symbol: "DAI", token1_symbol: "USDT", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        
        // WBTC pairs
        PoolInfo { address: "0xCBCdF9626bC03E24f779434178A73a0B4bad62eD", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0x4585FE77225b41b697C938B018E2Ac67Ac5a20c0", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0x99ac8cA7087fA4A2A1FB6357269965A2014ABc35", token0_symbol: "WBTC", token1_symbol: "USDC", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        
        // DeFi blue chips
        PoolInfo { address: "0xa6Cc3C2531FdaA6Ae1A3CA84c2855806728693e8", token0_symbol: "LINK", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0x1d42064Fc4Beb5F8aAF85F4617AE8b3b5B8Bd801", token0_symbol: "UNI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0xa3f558aebAecAf0e11cA4b2199cC5Ed341edfd74", token0_symbol: "LDO", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0xe8c6c9227491C0a8156A0106A0204d881BB7E531", token0_symbol: "MKR", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0x290A6a7460B308ee3F19023D2D00dE604bcf5B42", token0_symbol: "MATIC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        
        // Memecoins
        PoolInfo { address: "0x11950d141EcB863F01007AdD7D1A342041227b58", token0_symbol: "PEPE", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0x2F62f2B4c5fcd7570a709DeC05D68EA19c82A9ec", token0_symbol: "SHIB", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3 },
    ]
}

/// Known Sushiswap V3 pool addresses on Ethereum Mainnet
/// These use the same interface as Uniswap V3
pub fn get_sushiswap_v3_pools() -> Vec<PoolInfo> {
    vec![
        // Sushiswap V3 pools on Ethereum Mainnet
        // Major pairs with good liquidity
        PoolInfo { address: "0x2e5F635c5B2c2d7FD36e1e4F0C1e2a5b8bF1dE5C", token0_symbol: "WETH", token1_symbol: "USDC", fee: 500, dex: Dex::SushiswapV3, pool_type: PoolType::V3 },
        PoolInfo { address: "0x4c83A7f819A5c37D64B4c5A2f8238Ea082fA1f4e", token0_symbol: "WETH", token1_symbol: "USDT", fee: 500, dex: Dex::SushiswapV3, pool_type: PoolType::V3 },
    ]
}

/// Known Uniswap V2 pool addresses on Ethereum Mainnet  
/// V2 uses constant product formula: x * y = k
pub fn get_uniswap_v2_pools() -> Vec<PoolInfo> {
    vec![
        // Uniswap V2 major pairs - 0.3% fee on all
        PoolInfo { address: "0xB4e16d0168e52d35CaCD2c6185b44281Ec28C9Dc", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0x0d4a11d5EEaaC28EC3F61d100daF4d40471f1852", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xA478c2975Ab1Ea89e8196811F51A7B7Ade33eB11", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xBb2b8038a1640196FbE3e38816F3e67Cba72D940", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xB6909B960DbbE7392D405429eB2b3649752b4838", token0_symbol: "BAT", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xd3d2E2692501A5c9Ca623199D38826e513033a17", token0_symbol: "UNI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xa2107FA5B38d9bbd2C461D6EDf11B11A50F6b974", token0_symbol: "LINK", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xC3D03e4F041Fd4cD388c549Ee2A29a9E5075882f", token0_symbol: "DAI", token1_symbol: "USDC", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0x3041CbD36888bECc7bbCBc0045E3B1f144466f5f", token0_symbol: "USDC", token1_symbol: "USDT", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xCFfDdeD873554F362Ac02f8Fb1f02E5ada10516f", token0_symbol: "COMP", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xC2aDdA861F89bBB333c90c492cB837741916A225", token0_symbol: "MKR", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xDFC14d2Af169B0D36C4EFF567Ada9b2E0CAE044f", token0_symbol: "AAVE", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2 },
    ]
}

/// Known Sushiswap V2 pool addresses on Ethereum Mainnet
/// Uses same interface as Uniswap V2
pub fn get_sushiswap_v2_pools() -> Vec<PoolInfo> {
    vec![
        // Sushiswap major pairs - 0.3% fee on all
        PoolInfo { address: "0x397FF1542f962076d0BFE58eA045FfA2d347ACa0", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0x06da0fd433C1A5d7a4faa01111c044910A184553", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xC3D03e4F041Fd4cD388c549Ee2A29a9E5075882f", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xCEfF51756c56CeFFCA006cD410B03FFC46dd3a58", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xC40D16476380e4037e6b1A2594cAF6a6cc8Da967", token0_symbol: "LINK", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xDafd66636E2561b0284EDdE37e42d192F2844D40", token0_symbol: "UNI", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xBa13afEcda9beB75De5c56BbAF696b880a5A50dD", token0_symbol: "MKR", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0x088ee5007C98a9677165D78dD2109AE4a3D04d0C", token0_symbol: "YFI", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0x795065dCc9f64b5614C407a6EFDC400DA6221FB0", token0_symbol: "SUSHI", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0x001b6450083E531A5a7Bf310BD2c1Af4247E23D4", token0_symbol: "USDC", token1_symbol: "USDT", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0xA1d7b2d891e3A1f9ef4bBC5be20630C2FEB1c470", token0_symbol: "SNX", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2 },
        PoolInfo { address: "0x31503dcb60119A812feE820bb7042752019F2355", token0_symbol: "COMP", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2 },
    ]
}

/// Get all known pools across all DEXes
pub fn get_all_known_pools() -> Vec<PoolInfo> {
    let mut pools = Vec::new();
    pools.extend(get_uniswap_v3_pools());
    pools.extend(get_sushiswap_v3_pools());
    pools.extend(get_uniswap_v2_pools());
    pools.extend(get_sushiswap_v2_pools());
    pools
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

    /// Fetch pool state for a V3 pool (Uniswap V3 or Sushiswap V3)
    #[allow(deprecated)]
    pub async fn fetch_v3_pool(&self, pool_address: Address, dex: Dex) -> Result<PoolState> {
        let provider = ProviderBuilder::new()
            .on_http(self.rpc_url.parse()?);

        let pool = IUniswapV3Pool::new(pool_address, &provider);

        let slot0 = pool.slot0().call().await.map_err(|e| eyre!("Failed to fetch slot0: {}", e))?;
        let liquidity_result = pool.liquidity().call().await.map_err(|e| eyre!("Failed to fetch liquidity: {}", e))?;
        let token0_result = pool.token0().call().await.map_err(|e| eyre!("Failed to fetch token0: {}", e))?;
        let token1_result = pool.token1().call().await.map_err(|e| eyre!("Failed to fetch token1: {}", e))?;
        let fee_result = pool.fee().call().await.map_err(|e| eyre!("Failed to fetch fee: {}", e))?;

        let tick_i32: i32 = slot0.tick.as_i32();

        Ok(PoolState {
            address: pool_address,
            token0: token0_result,
            token1: token1_result,
            sqrt_price_x96: U256::from(slot0.sqrtPriceX96),
            tick: tick_i32,
            liquidity: liquidity_result,
            reserve1: 0,  // Not used for V3
            fee: fee_result.to::<u32>(),
            is_v4: false,
            dex,
            pool_type: PoolType::V3,
        })
    }

    /// Fetch pool state for a V2 pool (Uniswap V2 or Sushiswap V2)
    #[allow(deprecated)]
    pub async fn fetch_v2_pool(&self, pool_address: Address, dex: Dex, fee: u32) -> Result<PoolState> {
        let provider = ProviderBuilder::new()
            .on_http(self.rpc_url.parse()?);

        let pool = IUniswapV2Pair::new(pool_address, &provider);

        let reserves = pool.getReserves().call().await.map_err(|e| eyre!("Failed to fetch reserves: {}", e))?;
        let token0_result = pool.token0().call().await.map_err(|e| eyre!("Failed to fetch token0: {}", e))?;
        let token1_result = pool.token1().call().await.map_err(|e| eyre!("Failed to fetch token1: {}", e))?;

        // For V2, we store reserve0 as "liquidity" and reserve1 as "reserve1"
        // This lets us calculate price as reserve1/reserve0
        Ok(PoolState {
            address: pool_address,
            token0: token0_result,
            token1: token1_result,
            sqrt_price_x96: U256::ZERO,  // Not used for V2
            tick: 0,  // Not used for V2
            liquidity: reserves.reserve0.to::<u128>(),
            reserve1: reserves.reserve1.to::<u128>(),
            fee,
            is_v4: false,
            dex,
            pool_type: PoolType::V2,
        })
    }

    /// Fetch all known pools across all DEXes
    pub async fn fetch_all_pools(&self) -> Result<Vec<PoolState>> {
        info!("Fetching pool data from RPC across multiple DEXes...");
        
        let all_pools = get_all_known_pools();
        let mut pools = Vec::new();
        let mut success_count = 0;
        let mut fail_count = 0;

        for pool_info in &all_pools {
            let address = match Address::from_str(pool_info.address) {
                Ok(a) => a,
                Err(_) => {
                    warn!("Invalid address: {}", pool_info.address);
                    fail_count += 1;
                    continue;
                }
            };

            let result = match pool_info.pool_type {
                PoolType::V3 => self.fetch_v3_pool(address, pool_info.dex).await,
                PoolType::V2 => self.fetch_v2_pool(address, pool_info.dex, pool_info.fee).await,
            };
            
            match result {
                Ok(pool) => {
                    let (t0_dec, t1_dec) = get_decimals(pool_info.token0_symbol, pool_info.token1_symbol);
                    let price = pool.price(t0_dec, t1_dec);
                    
                    let liq_display = match pool.pool_type {
                        PoolType::V3 => format!("liq={}", pool.liquidity),
                        PoolType::V2 => format!("r0={}, r1={}", pool.liquidity, pool.reserve1),
                    };
                    
                    info!(
                        "✓ [{}] {}/{} ({}bps): price={:.6}, {}", 
                        pool.dex,
                        pool_info.token0_symbol, 
                        pool_info.token1_symbol, 
                        pool_info.fee / 100,
                        price, 
                        liq_display
                    );
                    
                    pools.push(pool);
                    success_count += 1;
                }
                Err(e) => {
                    warn!("✗ [{}] Failed to fetch {}/{}: {}", pool_info.dex, pool_info.token0_symbol, pool_info.token1_symbol, e);
                    fail_count += 1;
                }
            }
        }

        // Print summary by DEX
        let v3_uni = pools.iter().filter(|p| p.dex == Dex::UniswapV3).count();
        let v3_sushi = pools.iter().filter(|p| p.dex == Dex::SushiswapV3).count();
        let v2_uni = pools.iter().filter(|p| p.dex == Dex::UniswapV2).count();
        let v2_sushi = pools.iter().filter(|p| p.dex == Dex::SushiswapV2).count();

        info!(
            "Fetched {} pools: UniV3={}, SushiV3={}, UniV2={}, SushiV2={} ({} failed)",
            success_count, v3_uni, v3_sushi, v2_uni, v2_sushi, fail_count
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
            _ => 18, // WETH, DAI, LINK, UNI, PEPE, SHIB, SUSHI, YFI, AAVE, COMP, etc.
        }
    };
    (dec(token0), dec(token1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_price_calculation_v3() {
        let pool = PoolState {
            address: Address::ZERO,
            token0: Address::ZERO,
            token1: Address::ZERO,
            sqrt_price_x96: U256::from(1_500_000_000_000_000_000_000_000_000_u128),
            tick: 0,
            liquidity: 1000000,
            reserve1: 0,
            fee: 3000,
            is_v4: false,
            dex: Dex::UniswapV3,
            pool_type: PoolType::V3,
        };

        let raw_price = pool.raw_price();
        assert!(raw_price > 0.0, "Price should be positive");
    }

    #[test]
    fn test_price_calculation_v2() {
        let pool = PoolState {
            address: Address::ZERO,
            token0: Address::ZERO,
            token1: Address::ZERO,
            sqrt_price_x96: U256::ZERO,
            tick: 0,
            liquidity: 1_000_000,  // reserve0
            reserve1: 2_000_000,   // reserve1
            fee: 3000,
            is_v4: false,
            dex: Dex::UniswapV2,
            pool_type: PoolType::V2,
        };

        let raw_price = pool.raw_price();
        assert!((raw_price - 2.0).abs() < 0.001, "Price should be 2.0");
    }

    #[test]
    fn test_all_pools_exist() {
        let pools = get_all_known_pools();
        assert!(!pools.is_empty(), "Should have known pools");
        assert!(pools.len() >= 30, "Should have at least 30 pools across all DEXes");
    }
}
