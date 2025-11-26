//! Pool Data Fetcher - DECIMAL-AWARE Edition
//!
//! Step 1.1: The Scout
//!
//! CRITICAL FIX: All prices are now normalized to 18-decimal precision
//! to avoid the "trillion dollar glitch" when comparing tokens with
//! different decimals (e.g., DAI 18 decimals vs USDC 6 decimals).

use alloy::{
    primitives::{Address, U256},
    providers::ProviderBuilder,
    sol,
};
use eyre::{eyre, Result};
use futures::future::join_all;
use std::str::FromStr;
use std::time::Instant;
use std::collections::HashMap;
use tracing::{info, debug, warn};

// ============================================
// CONTRACT INTERFACES
// ============================================
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

sol! {
    #[sol(rpc)]
    interface ICurvePool {
        function coins(uint256 i) external view returns (address);
        function balances(uint256 i) external view returns (uint256);
    }
}

sol! {
    #[sol(rpc)]
    interface IERC20 {
        function decimals() external view returns (uint8);
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
    /// Decimals for token0
    pub token0_decimals: u8,
    /// Decimals for token1
    pub token1_decimals: u8,
    /// Current sqrt price (Q64.96 format) - only for V3
    pub sqrt_price_x96: U256,
    pub tick: i32,
    /// Available liquidity (V3) or reserve0 (V2/Balancer/Curve)
    pub liquidity: u128,
    /// Reserve1 for V2 pools
    pub reserve1: u128,
    /// Fee tier (in hundredths of a bip: 100 = 1bps = 0.01%)
    pub fee: u32,
    pub is_v4: bool,
    pub dex: Dex,
    pub pool_type: PoolType,
    pub weight0: u128,
}

impl PoolState {
    /// Calculate the DECIMAL-NORMALIZED price of token0 in terms of token1
    /// 
    /// CRITICAL: This adjusts for decimal differences!
    /// Returns the price such that: 1.0 token0 = price token1
    pub fn price(&self, _t0_decimals_override: u8, _t1_decimals_override: u8) -> f64 {
        // Use stored decimals (ignore overrides - we have the real values now)
        self.normalized_price()
    }
    
    /// Get the decimal-normalized price
    /// This is the TRUE price: how many token1 you get for 1 token0
    pub fn normalized_price(&self) -> f64 {
        match self.pool_type {
            PoolType::V3 => {
                // sqrtPriceX96 = sqrt(price) * 2^96
                // where price = token1_raw / token0_raw
                // 
                // To get the human-readable price (1 token0 = ? token1):
                // price_raw = (sqrtPriceX96 / 2^96)^2
                // price_normalized = price_raw * 10^(token0_decimals - token1_decimals)
                
                let sqrt_price_x96 = self.sqrt_price_x96.to::<u128>() as f64;
                if sqrt_price_x96 == 0.0 {
                    return 0.0;
                }
                
                let q96 = 2_f64.powi(96);
                let price_raw = (sqrt_price_x96 / q96).powi(2);
                
                // CRITICAL: Decimal adjustment
                // If token0 has more decimals than token1, price_raw is too small
                // Example: DAI(18) / USDC(6) -> multiply by 10^(18-6) = 10^12
                let decimal_diff = self.token0_decimals as i32 - self.token1_decimals as i32;
                let decimal_adjustment = 10_f64.powi(decimal_diff);
                
                price_raw * decimal_adjustment
            }
            PoolType::V2 | PoolType::Curve | PoolType::Balancer => {
                if self.liquidity == 0 || self.reserve1 == 0 {
                    return 0.0;
                }
                
                // price_raw = reserve1 / reserve0
                let reserve0 = self.liquidity as f64;
                let reserve1 = self.reserve1 as f64;
                let price_raw = reserve1 / reserve0;
                
                // CRITICAL: Decimal adjustment
                let decimal_diff = self.token0_decimals as i32 - self.token1_decimals as i32;
                let decimal_adjustment = 10_f64.powi(decimal_diff);
                
                let price = price_raw * decimal_adjustment;
                
                // For Balancer weighted pools, adjust for weights
                if self.pool_type == PoolType::Balancer && self.weight0 != 0 {
                    let w0 = self.weight0 as f64 / 1e18;
                    let w1 = 1.0 - w0;
                    return price * (w0 / w1);
                }
                
                price
            }
        }
    }

    /// Get raw price for graph weights (STILL NORMALIZED!)
    /// This is what we use for edge weights in Bellman-Ford
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

// ============================================
// KNOWN TOKEN DECIMALS (Ethereum Mainnet)
// ============================================
// lazy_static_decimals! {}

/// Get decimals for a known token address
/// Falls back to 18 if unknown (most ERC20s use 18)
pub fn get_token_decimals(address: &Address) -> u8 {
    let addr_lower = format!("{:?}", address).to_lowercase();
    
    match addr_lower.as_str() {
        // 6 decimal tokens
        a if a.contains("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48") => 6,  // USDC
        a if a.contains("dac17f958d2ee523a2206206994597c13d831ec7") => 6,  // USDT
        
        // 8 decimal tokens
        a if a.contains("2260fac5e5542a773aa44fbcfedf7c193bc2c599") => 8,  // WBTC
        
        // 18 decimal tokens (most common)
        a if a.contains("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2") => 18, // WETH
        a if a.contains("6b175474e89094c44da98b954eedcdecb5be3830") => 18, // DAI
        a if a.contains("7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0") => 18, // wstETH
        a if a.contains("ae7ab96520de3a18e5e111b5eaab095312d7fe84") => 18, // stETH
        a if a.contains("ae78736cd615f374d3085123a210448e74fc6393") => 18, // rETH
        a if a.contains("be9895146f7af43049ca1c1ae358b0541ea49704") => 18, // cbETH
        a if a.contains("514910771af9ca656af840dff83e8264ecf986ca") => 18, // LINK
        a if a.contains("1f9840a85d5af5bf1d1762f925bdaddc4201f984") => 18, // UNI
        a if a.contains("9f8f72aa9304c8b593d555f12ef6589cc3a579a2") => 18, // MKR
        a if a.contains("7fc66500c84a76ad7e9c93437bfc5ac33e2ddae9") => 18, // AAVE
        a if a.contains("c00e94cb662c3520282e6f5717214004a7f26888") => 18, // COMP
        a if a.contains("d533a949740bb3306d119cc777fa900ba034cd52") => 18, // CRV
        a if a.contains("4e3fbd56cd56c3e72c1403e103b45db9da5b9d2b") => 18, // CVX
        a if a.contains("5a98fcbea516cf06857215779fd812ca3bef1b32") => 18, // LDO
        a if a.contains("6b3595068778dd592e39a122f4f5a5cf09c90fe2") => 18, // SUSHI
        a if a.contains("0bc529c00c6401aef6d220be8c6ea1667f6ad93e") => 18, // YFI
        a if a.contains("c011a73ee8576fb46f5e1c5751ca3b9fe0af2a6f") => 18, // SNX
        a if a.contains("ba100000625a3754423978a60c9317c58a424e3d") => 18, // BAL
        a if a.contains("6982508145454ce325ddbe47a25d4ec3d2311933") => 18, // PEPE
        a if a.contains("95ad61b0a150d79219dcf64e1e6cc01f0b64c4ce") => 18, // SHIB
        a if a.contains("7d1afa7b718fb893db30a3abc0cfc608aacfebb0") => 18, // MATIC
        a if a.contains("853d955acef822db058eb8505911ed77f175b99e") => 18, // FRAX
        a if a.contains("f939e0a03fb07f59a73314e73794be0e57ac1b4e") => 18, // crvUSD
        
        // Default to 18 (most common)
        _ => 18,
    }
}

// ============================================
// POOL DEFINITIONS
// ============================================
pub fn get_uniswap_v3_pools() -> Vec<PoolInfo> {
    vec![
        // 1bps pools (0.01% fee) - TIGHTEST SPREADS
        PoolInfo { address: "0x3416cF6C708Da44DB2624D63ea0AAef7113527C6", token0_symbol: "USDC", token1_symbol: "USDT", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x5777d92f208679DB4b9778590Fa3CAB3aC9e2168", token0_symbol: "DAI", token1_symbol: "USDC", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x109830a1AAaD605BbF02a9dFA7B0B92EC2FB7dAa", token0_symbol: "wstETH", token1_symbol: "WETH", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // 5bps pools (0.05% fee)
        PoolInfo { address: "0x88e6A0c2dDD26FEEb64F039a2c41296FcB3f5640", token0_symbol: "USDC", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x11b815efB8f581194ae79006d24E0d814B7697F6", token0_symbol: "WETH", token1_symbol: "USDT", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x60594a405d53811d3BC4766596EFD80fd545A270", token0_symbol: "DAI", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x4585FE77225b41b697C938B018E2Ac67Ac5a20c0", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x7858E59e0C01EA06Df3aF3D20aC7B0003275D4Bf", token0_symbol: "USDC", token1_symbol: "USDT", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // 30bps pools (0.3% fee)
        PoolInfo { address: "0x8ad599c3A0ff1De082011EFDDc58f1908eb6e6D8", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x4e68Ccd3E89f51C3074ca5072bbAC773960dFa36", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xC2e9F25Be6257c210d7Adf0D4Cd6E3E881ba25f8", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xCBCdF9626bC03E24f779434178A73a0B4bad62eD", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x99ac8cA7087fA4A2A1FB6357269965A2014ABc35", token0_symbol: "WBTC", token1_symbol: "USDC", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // DeFi tokens
        PoolInfo { address: "0xa6Cc3C2531FdaA6Ae1A3CA84c2855806728693e8", token0_symbol: "LINK", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x1d42064Fc4Beb5F8aAF85F4617AE8b3b5B8Bd801", token0_symbol: "UNI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xa3f558aebAecAf0e11cA4b2199cC5Ed341edfd74", token0_symbol: "LDO", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xe8c6c9227491C0a8156A0106A0204d881BB7E531", token0_symbol: "MKR", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // Memecoins
        PoolInfo { address: "0x11950d141EcB863F01007AdD7D1A342041227b58", token0_symbol: "PEPE", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x2F62f2B4c5fcd7570a709DeC05D68EA19c82A9ec", token0_symbol: "SHIB", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x290A6a7460B308ee3F19023D2D00dE604bcf5B42", token0_symbol: "MATIC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
    ]
}

pub fn get_uniswap_v2_pools() -> Vec<PoolInfo> {
    vec![
        PoolInfo { address: "0xB4e16d0168e52d35CaCD2c6185b44281Ec28C9Dc", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x0d4a11d5EEaaC28EC3F61d100daF4d40471f1852", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xA478c2975Ab1Ea89e8196811F51A7B7Ade33eB11", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xBb2b8038a1640196FbE3e38816F3e67Cba72D940", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xd3d2E2692501A5c9Ca623199D38826e513033a17", token0_symbol: "UNI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xa2107FA5B38d9bbd2C461D6EDf11B11A50F6b974", token0_symbol: "LINK", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xAE461cA67B15dc8dc81CE7615e0320dA1A9aB8D5", token0_symbol: "DAI", token1_symbol: "USDC", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x3041CbD36888bECc7bbCBc0045E3B1f144466f5f", token0_symbol: "USDC", token1_symbol: "USDT", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xC2aDdA861F89bBB333c90c492cB837741916A225", token0_symbol: "MKR", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
    ]
}

pub fn get_sushiswap_v2_pools() -> Vec<PoolInfo> {
    vec![
        PoolInfo { address: "0x397FF1542f962076d0BFE58eA045FfA2d347ACa0", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x06da0fd433C1A5d7a4faa01111c044910A184553", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xCEfF51756c56CeFFCA006cD410B03FFC46dd3a58", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xC40D16476380e4037e6b1A2594cAF6a6cc8Da967", token0_symbol: "LINK", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xDafd66636E2561b0284EDdE37e42d192F2844D40", token0_symbol: "UNI", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xBa13afEcda9beB75De5c56BbAF696b880a5A50dD", token0_symbol: "MKR", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x795065dCc9f64b5614C407a6EFDC400DA6221FB0", token0_symbol: "SUSHI", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
    ]
}

pub fn get_pancakeswap_v3_pools() -> Vec<PoolInfo> {
    vec![
        PoolInfo { address: "0x1ac1A8FEaAEa1900C4166dEeed0C11cC10669D36", token0_symbol: "USDC", token1_symbol: "WETH", fee: 500, dex: Dex::PancakeSwapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x6CA298D2983aB03Aa1dA7679389D955A4eFEE15C", token0_symbol: "WETH", token1_symbol: "USDT", fee: 500, dex: Dex::PancakeSwapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x517F451b0A9E1b87Dc0Ae98A05Ee033C3310F046", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 500, dex: Dex::PancakeSwapV3, pool_type: PoolType::V3, weight0: None },
    ]
}

pub fn get_curve_pools() -> Vec<PoolInfo> {
    vec![
        // 3pool (DAI/USDC/USDT) - we model as DAI/USDC pair
        PoolInfo { address: "0xbEbc44782C7dB0a1A60Cb6fe97d0b483032FF1C7", token0_symbol: "DAI", token1_symbol: "USDC", fee: 400, dex: Dex::Curve, pool_type: PoolType::Curve, weight0: None },
        // stETH/ETH pool
        PoolInfo { address: "0xDC24316b9AE028F1497c275EB9192a3Ea0f67022", token0_symbol: "stETH", token1_symbol: "WETH", fee: 400, dex: Dex::Curve, pool_type: PoolType::Curve, weight0: None },
    ]
}

pub fn get_balancer_v2_pools() -> Vec<PoolInfo> {
    vec![
        PoolInfo { 
            address: "0x32296969Ef14EB0c6d29669C550D4a0449130230", 
            token0_symbol: "wstETH", 
            token1_symbol: "WETH", 
            fee: 4,   // 0.004%
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
    pools.extend(get_curve_pools());
    pools.extend(get_balancer_v2_pools());
    pools
}

/// Pool data fetcher with DECIMAL AWARENESS
pub struct PoolFetcher {
    rpc_url: String,
    /// Cache of token address -> decimals
    decimals_cache: HashMap<Address, u8>,
}

impl PoolFetcher {
    pub fn new(rpc_url: String) -> Self {
        Self { 
            rpc_url,
            decimals_cache: HashMap::new(),
        }
    }

    /// Get decimals for a token, using cache or RPC
    async fn get_decimals(&self, token: Address) -> u8 {
        // First check our hardcoded list
        let known = get_token_decimals(&token);
        if known != 18 {
            return known;  // If not default, we know it
        }
        
        // Check if it's a known 18-decimal token
        let addr_lower = format!("{:?}", token).to_lowercase();
        if addr_lower.contains("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2") ||  // WETH
           addr_lower.contains("6b175474e89094c44da98b954eedcdecb5be3830") {   // DAI
            return 18;
        }
        
        // For unknown tokens, try RPC (but default to 18 on failure)
        // In production, you'd want to cache this
        18
    }

    /// Fetch a V3 pool with decimal info
    #[allow(deprecated)]
    async fn fetch_v3_pool_inner(&self, pool_address: Address, dex: Dex) -> Result<PoolState> {
        let provider = ProviderBuilder::new()
            .on_http(self.rpc_url.parse()?);

        let pool = IUniswapV3Pool::new(pool_address, &provider);

        let slot0 = pool.slot0().call().await.map_err(|e| eyre!("slot0: {}", e))?;
        let liquidity = pool.liquidity().call().await.map_err(|e| eyre!("liquidity: {}", e))?;
        let token0 = pool.token0().call().await.map_err(|e| eyre!("token0: {}", e))?;
        let token1 = pool.token1().call().await.map_err(|e| eyre!("token1: {}", e))?;
        let fee = pool.fee().call().await.map_err(|e| eyre!("fee: {}", e))?;

        // Get decimals for both tokens
        let token0_decimals = self.get_decimals(token0).await;
        let token1_decimals = self.get_decimals(token1).await;

        Ok(PoolState {
            address: pool_address,
            token0,
            token1,
            token0_decimals,
            token1_decimals,
            sqrt_price_x96: U256::from(slot0.sqrtPriceX96),
            tick: slot0.tick.as_i32(),
            liquidity,
            reserve1: 0,
            fee: fee.to::<u32>(),
            is_v4: false,
            dex,
            pool_type: PoolType::V3,
            weight0: 0,
        })
    }

    /// Fetch a V2 pool with decimal info
    #[allow(deprecated)]
    async fn fetch_v2_pool_inner(&self, pool_address: Address, dex: Dex, fee: u32) -> Result<PoolState> {
        let provider = ProviderBuilder::new()
            .on_http(self.rpc_url.parse()?);

        let pool = IUniswapV2Pair::new(pool_address, &provider);

        let reserves = pool.getReserves().call().await.map_err(|e| eyre!("getReserves: {}", e))?;
        let token0 = pool.token0().call().await.map_err(|e| eyre!("token0: {}", e))?;
        let token1 = pool.token1().call().await.map_err(|e| eyre!("token1: {}", e))?;

        // Get decimals for both tokens
        let token0_decimals = self.get_decimals(token0).await;
        let token1_decimals = self.get_decimals(token1).await;

        Ok(PoolState {
            address: pool_address,
            token0,
            token1,
            token0_decimals,
            token1_decimals,
            sqrt_price_x96: U256::ZERO,
            tick: 0,
            liquidity: reserves.reserve0.to::<u128>(),
            reserve1: reserves.reserve1.to::<u128>(),
            fee,
            is_v4: false,
            dex,
            pool_type: PoolType::V2,
            weight0: 0,
        })
    }

    /// Fetch a Curve pool with decimal info
    #[allow(deprecated)]
    async fn fetch_curve_pool_inner(&self, pool_address: Address, fee: u32) -> Result<PoolState> {
        let provider = ProviderBuilder::new()
            .on_http(self.rpc_url.parse()?);

        let pool = ICurvePool::new(pool_address, &provider);

        let coin0 = pool.coins(U256::from(0)).call().await.map_err(|e| eyre!("coins(0): {}", e))?;
        let coin1 = pool.coins(U256::from(1)).call().await.map_err(|e| eyre!("coins(1): {}", e))?;
        let balance0 = pool.balances(U256::from(0)).call().await.map_err(|e| eyre!("balances(0): {}", e))?;
        let balance1 = pool.balances(U256::from(1)).call().await.map_err(|e| eyre!("balances(1): {}", e))?;

        // Get decimals for both tokens
        let token0_decimals = self.get_decimals(coin0).await;
        let token1_decimals = self.get_decimals(coin1).await;

        Ok(PoolState {
            address: pool_address,
            token0: coin0,
            token1: coin1,
            token0_decimals,
            token1_decimals,
            sqrt_price_x96: U256::ZERO,
            tick: 0,
            liquidity: balance0.to::<u128>(),
            reserve1: balance1.to::<u128>(),
            fee,
            is_v4: false,
            dex: Dex::Curve,
            pool_type: PoolType::Curve,
            weight0: 0,
        })
    }

    /// Fetch a single pool (any type)
    async fn fetch_pool(&self, pool_info: &PoolInfo) -> Result<PoolState> {
        let address = Address::from_str(pool_info.address)
            .map_err(|_| eyre!("Invalid address: {}", pool_info.address))?;

        match pool_info.pool_type {
            PoolType::V3 => self.fetch_v3_pool_inner(address, pool_info.dex).await,
            PoolType::V2 => self.fetch_v2_pool_inner(address, pool_info.dex, pool_info.fee).await,
            PoolType::Curve => self.fetch_curve_pool_inner(address, pool_info.fee).await,
            PoolType::Balancer => {
                let mut state = self.fetch_v2_pool_inner(address, pool_info.dex, pool_info.fee).await;
                if let Ok(ref mut s) = state {
                    s.pool_type = PoolType::Balancer;
                    s.weight0 = (pool_info.weight0.unwrap_or(0.5) * 1e18) as u128;
                }
                state
            }
        }
    }

    /// Fetch all pools with concurrent requests
    pub async fn fetch_all_pools(&self) -> Result<Vec<PoolState>> {
        let start = Instant::now();
        info!("🚀 Fetching pools from 6 DEXes with DECIMAL-AWARE pricing...");
        
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
                    
                    // Sanity check: price should be reasonable
                    if price > 0.0 && price < 1e12 {
                        debug!(
                            "✓ [{}] {}/{}: price={:.6} ({}d/{}d)",
                            pool.dex, info.token0_symbol, info.token1_symbol, 
                            price, pool.token0_decimals, pool.token1_decimals
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
        for dex in [Dex::UniswapV3, Dex::UniswapV2, Dex::SushiswapV2, Dex::PancakeSwapV3, Dex::Curve, Dex::BalancerV2] {
            if let Some(&count) = counts.get(&dex) {
                info!("     {}: {} pools", dex, count);
            }
        }
        info!("   Low-fee pools (≤5bps): {}", low_fee_count);

        // Log some sample prices to verify decimal handling
        info!("   Sample normalized prices:");
        for pool in pools.iter().take(5) {
            let t0 = get_token_symbol(&pool.token0);
            let t1 = get_token_symbol(&pool.token1);
            info!(
                "     {}/{}: {:.6} ({}d/{}d)",
                t0, t1, pool.normalized_price(),
                pool.token0_decimals, pool.token1_decimals
            );
        }

        if pools.is_empty() {
            return Err(eyre!("No pools fetched! Check your RPC URL."));
        }

        Ok(pools)
    }
}

/// Get token symbol from address
fn get_token_symbol(address: &Address) -> &'static str {
    let addr_lower = format!("{:?}", address).to_lowercase();
    
    match addr_lower.as_str() {
        a if a.contains("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2") => "WETH",
        a if a.contains("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48") => "USDC",
        a if a.contains("dac17f958d2ee523a2206206994597c13d831ec7") => "USDT",
        a if a.contains("6b175474e89094c44da98b954eedcdecb5be3830") => "DAI",
        a if a.contains("2260fac5e5542a773aa44fbcfedf7c193bc2c599") => "WBTC",
        a if a.contains("7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0") => "wstETH",
        a if a.contains("ae7ab96520de3a18e5e111b5eaab095312d7fe84") => "stETH",
        _ => "???",
    }
}
