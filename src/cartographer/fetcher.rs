//! Pool Data Fetcher - EXPANDED Edition (80+ Pools)
//!
//! Step 1.1: The Scout
//!
//! This expanded version covers:
//! - Core pairs (WETH, USDC, USDT, DAI, WBTC) across multiple DEXes
//! - Long-tail tokens (PEPE, SHIB, LINK, UNI, LDO, MKR, AAVE, etc.)
//! - Multiple fee tiers for maximum arbitrage opportunity detection
//! - Cross-DEX coverage for the same pairs

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
        // Stablecoins (6 decimals)
        a if a.contains("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48") => 6,  // USDC
        a if a.contains("dac17f958d2ee523a2206206994597c13d831ec7") => 6,  // USDT
        
        // WBTC (8 decimals)
        a if a.contains("2260fac5e5542a773aa44fbcfedf7c193bc2c599") => 8,  // WBTC
        
        // Standard 18 decimals tokens
        a if a.contains("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2") => 18, // WETH
        a if a.contains("6b175474e89094c44da98b954eedcdecb5be3830") => 18, // DAI
        a if a.contains("7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0") => 18, // wstETH
        a if a.contains("ae7ab96520de3a18e5e111b5eaab095312d7fe84") => 18, // stETH
        a if a.contains("514910771af9ca656af840dff83e8264ecf986ca") => 18, // LINK
        a if a.contains("1f9840a85d5af5bf1d1762f925bdaddc4201f984") => 18, // UNI
        a if a.contains("6982508145454ce325ddbe47a25d4ec3d2311933") => 18, // PEPE
        a if a.contains("95ad61b0a150d79219dcf64e1e6cc01f0b64c4ce") => 18, // SHIB
        a if a.contains("5a98fcbea516cf06857215779fd812ca3bef1b32") => 18, // LDO
        a if a.contains("9f8f72aa9304c8b593d555f12ef6589cc3a579a2") => 18, // MKR
        a if a.contains("7fc66500c84a76ad7e9c93437bfc5ac33e2ddae9") => 18, // AAVE
        a if a.contains("d533a949740bb3306d119cc777fa900ba034cd52") => 18, // CRV
        a if a.contains("c011a73ee8576fb46f5e1c5751ca3b9fe0af2a6f") => 18, // SNX
        a if a.contains("c00e94cb662c3520282e6f5717214004a7f26888") => 18, // COMP
        a if a.contains("c18360217d8f7ab5e7c516566761ea12ce7f9d72") => 18, // ENS
        a if a.contains("d33526068d116ce69f19a9ee46f0bd304f21a51f") => 18, // RPL
        a if a.contains("4d224452801aced8b2f0aebe155379bb5d594381") => 18, // APE
        a if a.contains("5283d291dbcf85356a21ba090e6db59121208b44") => 18, // BLUR
        a if a.contains("7d1afa7b718fb893db30a3abc0cfc608aacfebb0") => 18, // MATIC
        a if a.contains("6b3595068778dd592e39a122f4f5a5cf09c90fe2") => 18, // SUSHI
        a if a.contains("0x111111111117dc0aa78b770fa6a738034120c302") => 18, // 1INCH
        a if a.contains("0bc529c00c6401aef6d220be8c6ea1667f6ad93e") => 18, // YFI
        a if a.contains("ba100000625a3754423978a60c9317c58a424e3d") => 18, // BAL
        a if a.contains("0f5d2fb29fb7d3cfee444a200298f468908cc942") => 18, // MANA
        a if a.contains("4e3fbd56cd56c3e72c1403e103b45db9da5b9d2b") => 18, // CVX
        a if a.contains("853d955acef822db058eb8505911ed77f175b99e") => 18, // FRAX
        a if a.contains("3432b6a60d23ca0dfca7761b7ab56459d9c964d0") => 18, // FXS
        a if a.contains("45804880de22913dafe09f4980848ece6ecbaf78") => 18, // PAXG
        _ => 18,
    }
}

// ============================================
// EXPANDED POOL DEFINITIONS
// ============================================

/// Uniswap V3 - Core pairs (multiple fee tiers)
pub fn get_uniswap_v3_core_pools() -> Vec<PoolInfo> {
    vec![
        // === USDC/WETH (High volume, multiple fee tiers) ===
        PoolInfo { address: "0x88e6A0c2dDD26FEEb64F039a2c41296FcB3f5640", token0_symbol: "USDC", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x8ad599c3A0ff1De082011EFDDc58f1908eb6e6D8", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x7BeA39867e4169DBe237d55C8242a8f2fcDcc387", token0_symbol: "USDC", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === WETH/USDT ===
        PoolInfo { address: "0x11b815efB8f581194ae79006d24E0d814B7697F6", token0_symbol: "WETH", token1_symbol: "USDT", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x4e68Ccd3E89f51C3074ca5072bbAC773960dFa36", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === DAI/WETH ===
        PoolInfo { address: "0x60594a405d53811d3BC4766596EFD80fd545A270", token0_symbol: "DAI", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xC2e9F25Be6257c210d7Adf0D4Cd6E3E881ba25f8", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === WBTC/WETH ===
        PoolInfo { address: "0x4585FE77225b41b697C938B018E2Ac67Ac5a20c0", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xCBCdF9626bC03E24f779434178A73a0B4bad62eD", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === Stablecoin pairs ===
        PoolInfo { address: "0x3416cF6C708Da44DB2624D63ea0AAef7113527C6", token0_symbol: "USDC", token1_symbol: "USDT", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x5777d92f208679DB4b9778590Fa3CAB3aC9e2168", token0_symbol: "DAI", token1_symbol: "USDC", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x6c6Bc977E13Df9b0de53b251522280BB72383700", token0_symbol: "DAI", token1_symbol: "USDC", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === wstETH/WETH (Liquid staking) ===
        PoolInfo { address: "0x109830a1AAaD605BbF02a9dFA7B0B92EC2FB7dAa", token0_symbol: "wstETH", token1_symbol: "WETH", fee: 100, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x35218a1cbaC5Bbc3E57fd9Bd38219D37571b3537", token0_symbol: "wstETH", token1_symbol: "WETH", fee: 500, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
    ]
}

/// Uniswap V3 - Long tail meme/volatile tokens (HIGH OPPORTUNITY ZONE)
pub fn get_uniswap_v3_longtail_pools() -> Vec<PoolInfo> {
    vec![
        // === PEPE/WETH (Meme coin, high volatility) ===
        PoolInfo { address: "0x11950d141EcB863F01007AdD7D1A342041227b58", token0_symbol: "PEPE", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xF239009A101B6B930A527DEaB052f3AA3149E93D", token0_symbol: "PEPE", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === SHIB/WETH (Meme coin) ===
        PoolInfo { address: "0x2F62f2B4c5fcd7570a709DeC05D68EA19c82A9ec", token0_symbol: "SHIB", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xFe2C5bDd73E83eC61fA5962a8E4ed86A1B2BfE93", token0_symbol: "SHIB", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === LINK/WETH (Oracle token) ===
        PoolInfo { address: "0xa6Cc3C2531FdaA6Ae1A3CA84c2855806728693e8", token0_symbol: "LINK", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === UNI/WETH (Governance token) ===
        PoolInfo { address: "0x1d42064Fc4Beb5F8aAF85F4617AE8b3b5B8Bd801", token0_symbol: "UNI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0xfaA318479b7755b2dBfDD34dC306cb28B420Ad12", token0_symbol: "UNI", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === LDO/WETH (Lido DAO) ===
        PoolInfo { address: "0xa3f558aebAecAf0e11cA4b2NaB80cDD9AACE2a8D", token0_symbol: "LDO", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === MKR/WETH (MakerDAO) ===
        PoolInfo { address: "0xe8c6c9227491C0a8156A0106A0204d881BB7E531", token0_symbol: "MKR", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === AAVE/WETH ===
        PoolInfo { address: "0x5aB53EE1d50eeF2C1DD3d5402789cd27bB52c1bB", token0_symbol: "AAVE", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === CRV/WETH (Curve DAO) ===
        PoolInfo { address: "0x919Fa96e88d67499339577Fa202345436bcDaf79", token0_symbol: "CRV", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x4c83A7f819A5c37D64B4c5A2f8238Ea082fA1f4e", token0_symbol: "CRV", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === SNX/WETH (Synthetix) ===
        PoolInfo { address: "0x87418B2F7E5c9084f2DFf8b5B7CfA7E9D02D6E3c", token0_symbol: "SNX", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === COMP/WETH (Compound) ===
        PoolInfo { address: "0xea4Ba4CE14fdd287f380b55419B1C5b6c3f22ab6", token0_symbol: "COMP", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === ENS/WETH (ENS Domains) ===
        PoolInfo { address: "0x92560C178cE069CC014138eD3C2F5221Ba71f58a", token0_symbol: "ENS", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === RPL/WETH (Rocket Pool) ===
        PoolInfo { address: "0xe42318eA3b998e8355a3Da364EB9D48eC725Eb45", token0_symbol: "RPL", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === APE/WETH (ApeCoin) ===
        PoolInfo { address: "0xAc4b3DacB91461209Ae9d41EC517c2B9Cb1B7DAF", token0_symbol: "APE", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === BLUR/WETH (Blur NFT) ===
        PoolInfo { address: "0xe1573b9d29e2183B1AF0e743Dc2754979A40D237", token0_symbol: "BLUR", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x108B3D3d55b8C3fAA08Bb4E8D9b3C1e3E4A7D4E1", token0_symbol: "BLUR", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === MATIC/WETH (Polygon) ===
        PoolInfo { address: "0x290A6a7460B308ee3F19023D2D00dE604bcf5B42", token0_symbol: "MATIC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === 1INCH/WETH ===
        PoolInfo { address: "0x9feBc984504356225405e26833608b17719c82Ae", token0_symbol: "1INCH", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === YFI/WETH (Yearn) ===
        PoolInfo { address: "0x04916039B1f59D9745Bf6E0a21f191D1e0A84287", token0_symbol: "YFI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === CVX/WETH (Convex) ===
        PoolInfo { address: "0x2E4784446A0a06dF3D1A040b03E1680Ee266c35a", token0_symbol: "CVX", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === BAL/WETH (Balancer) ===
        PoolInfo { address: "0xDC2c21F1B54dDaF39e944689a8f90c0e8AB00d3d", token0_symbol: "BAL", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === FXS/WETH (Frax Share) ===
        PoolInfo { address: "0xCD8286b48936cDAC544D0eFc53941F7ECFb0CD20", token0_symbol: "FXS", token1_symbol: "WETH", fee: 10000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
    ]
}

/// Uniswap V3 - USDC pairs for triangular arbitrage
pub fn get_uniswap_v3_usdc_pairs() -> Vec<PoolInfo> {
    vec![
        // === LINK/USDC ===
        PoolInfo { address: "0xFAD57d2039C21811C8F2B5D5B65308aa99D31559", token0_symbol: "LINK", token1_symbol: "USDC", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === UNI/USDC ===
        PoolInfo { address: "0xD0fC8bA7E267f2bc56044A7715A489d851dC6D78", token0_symbol: "UNI", token1_symbol: "USDC", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === AAVE/USDC ===
        PoolInfo { address: "0x7F8dA88f45E05e93a8C4F10c3eC5E18b3c8f3D81", token0_symbol: "AAVE", token1_symbol: "USDC", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === MKR/USDC ===
        PoolInfo { address: "0xC486Ad2764D55C7dc033487D634195d6e4A6917E", token0_symbol: "MKR", token1_symbol: "USDC", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
        
        // === WBTC/USDC ===
        PoolInfo { address: "0x99ac8cA7087fA4A2A1FB6357269965A2014ABc35", token0_symbol: "WBTC", token1_symbol: "USDC", fee: 3000, dex: Dex::UniswapV3, pool_type: PoolType::V3, weight0: None },
    ]
}

/// Uniswap V2 - Core pairs
pub fn get_uniswap_v2_pools() -> Vec<PoolInfo> {
    vec![
        // Core pairs
        PoolInfo { address: "0xB4e16d0168e52d35CaCD2c6185b44281Ec28C9Dc", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x0d4a11d5EEaaC28EC3F61d100daF4d40471f1852", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xA478c2975Ab1Ea89e8196811F51A7B7Ade33eB11", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xBb2b8038a1640196FbE3e38816F3e67Cba72D940", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xAE461cA67B15dc8dc81CE7615e0320dA1A9aB8D5", token0_symbol: "DAI", token1_symbol: "USDC", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        
        // Long tail V2 pairs
        PoolInfo { address: "0xd3d2E2692501A5c9Ca623199D38826e513033a17", token0_symbol: "LINK", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xd3d2E2692501A5c9Ca623199D38826e513033a17", token0_symbol: "UNI", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x811beEd0119b4AfCE20D2583EB608C6F7AF1954f", token0_symbol: "SHIB", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xA43fe16908251ee70EF74718545e4FE6C5cCec9f", token0_symbol: "PEPE", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x9c4Fe5FFD9A9fC5678cFBd93Aa2D4FD684b67C4C", token0_symbol: "MKR", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xDFC14d2Af169B0D36C4EFF567Ada9b2E0CAE044f", token0_symbol: "AAVE", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x3dA1313aE46132A397D90d95B1424A9A7e3e0fCE", token0_symbol: "CRV", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x43AE24960e5534731Fc831386c07755A2dc33D47", token0_symbol: "SNX", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xCFfDdeD873554F362Ac02f8Fb1f02E5ada10516f", token0_symbol: "COMP", token1_symbol: "WETH", fee: 3000, dex: Dex::UniswapV2, pool_type: PoolType::V2, weight0: None },
    ]
}

/// Sushiswap V2 - Core pairs
pub fn get_sushiswap_v2_pools() -> Vec<PoolInfo> {
    vec![
        // Core pairs
        PoolInfo { address: "0x397FF1542f962076d0BFE58eA045FfA2d347ACa0", token0_symbol: "USDC", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x06da0fd433C1A5d7a4faa01111c044910A184553", token0_symbol: "WETH", token1_symbol: "USDT", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xC3D03e4F041Fd4cD388c549Ee2A29a9E5075882f", token0_symbol: "DAI", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xCEfF51756c56CeFFCA006cD410B03FFC46dd3a58", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        
        // Long tail Sushi pairs
        PoolInfo { address: "0xC40D16476380e4037e6b1A2594cAF6a6cc8Da967", token0_symbol: "LINK", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xDafd66636E2561b0284EDdE37e42d192F2844D40", token0_symbol: "UNI", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xBa13afEcda9beB75De5c56BbAF696b880a5A50dD", token0_symbol: "MKR", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xD75EA151a61d06868E31F8988D28DFE5E9df57B4", token0_symbol: "AAVE", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x58Dc5a51fE44589BEb22E8CE67720B5BC5378009", token0_symbol: "CRV", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0xA1d7b2d891e3A1f9ef4bBC5be20630C2FEB1c470", token0_symbol: "SNX", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x31503dcb60119A812feE820bb7042752019F2355", token0_symbol: "COMP", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x0be88ac4b5C81700acF3a606a52a31C261a24A35", token0_symbol: "YFI", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
        PoolInfo { address: "0x795065dCc9f64b5614C407a6EFDC400DA6221FB0", token0_symbol: "SUSHI", token1_symbol: "WETH", fee: 3000, dex: Dex::SushiswapV2, pool_type: PoolType::V2, weight0: None },
    ]
}

/// PancakeSwap V3 - Ethereum mainnet pools
pub fn get_pancakeswap_v3_pools() -> Vec<PoolInfo> {
    vec![
        PoolInfo { address: "0x1ac1A8FEaAEa1900C4166dEeed0C11cC10669D36", token0_symbol: "USDC", token1_symbol: "WETH", fee: 500, dex: Dex::PancakeSwapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x6CA298D2983aB03Aa1dA7679389D955A4eFEE15C", token0_symbol: "WETH", token1_symbol: "USDT", fee: 500, dex: Dex::PancakeSwapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x517F451b0A9E1b87Dc0Ae98A05Ee033C3310F046", token0_symbol: "WBTC", token1_symbol: "WETH", fee: 500, dex: Dex::PancakeSwapV3, pool_type: PoolType::V3, weight0: None },
        PoolInfo { address: "0x5145755c0535198eEc15D127f05Fb5F1D8b3B5CF", token0_symbol: "PEPE", token1_symbol: "WETH", fee: 10000, dex: Dex::PancakeSwapV3, pool_type: PoolType::V3, weight0: None },
    ]
}

/// Balancer V2 pools
pub fn get_balancer_v2_pools() -> Vec<PoolInfo> {
    vec![
        // wstETH/WETH (Liquid staking - tight spread)
        PoolInfo { 
            address: "0x32296969Ef14EB0c6d29669C550D4a0449130230", 
            token0_symbol: "wstETH", 
            token1_symbol: "WETH", 
            fee: 4,
            dex: Dex::BalancerV2, 
            pool_type: PoolType::Balancer, 
            weight0: Some(0.5),
        },
        // BAL/WETH
        PoolInfo { 
            address: "0x5c6Ee304399DBdB9C8Ef030aB642B10820DB8F56", 
            token0_symbol: "BAL", 
            token1_symbol: "WETH", 
            fee: 10,
            dex: Dex::BalancerV2, 
            pool_type: PoolType::Balancer, 
            weight0: Some(0.8),
        },
    ]
}

/// Get ALL pools (80+ pools for comprehensive coverage)
pub fn get_all_known_pools() -> Vec<PoolInfo> {
    let mut pools = Vec::new();
    
    // Uniswap V3 (40+ pools)
    pools.extend(get_uniswap_v3_core_pools());
    pools.extend(get_uniswap_v3_longtail_pools());
    pools.extend(get_uniswap_v3_usdc_pairs());
    
    // Uniswap V2 (13 pools)
    pools.extend(get_uniswap_v2_pools());
    
    // Sushiswap V2 (13 pools)
    pools.extend(get_sushiswap_v2_pools());
    
    // PancakeSwap V3 (4 pools)
    pools.extend(get_pancakeswap_v3_pools());
    
    // Balancer V2 (2 pools)
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
        let slot0_calldata = IUniswapV3Pool::slot0Call {}.abi_encode();
        let slot0_result = self.call_contract(pool_address, slot0_calldata).await?;
        let slot0 = IUniswapV3Pool::slot0Call::abi_decode_returns(&slot0_result)
            .map_err(|e| eyre!("slot0 decode: {}", e))?;
        
        let liq_calldata = IUniswapV3Pool::liquidityCall {}.abi_encode();
        let liq_result = self.call_contract(pool_address, liq_calldata).await?;
        let liquidity = IUniswapV3Pool::liquidityCall::abi_decode_returns(&liq_result)
            .map_err(|e| eyre!("liquidity decode: {}", e))?;
        
        let t0_calldata = IUniswapV3Pool::token0Call {}.abi_encode();
        let t0_result = self.call_contract(pool_address, t0_calldata).await?;
        let token0 = IUniswapV3Pool::token0Call::abi_decode_returns(&t0_result)
            .map_err(|e| eyre!("token0 decode: {}", e))?;
        
        let t1_calldata = IUniswapV3Pool::token1Call {}.abi_encode();
        let t1_result = self.call_contract(pool_address, t1_calldata).await?;
        let token1 = IUniswapV3Pool::token1Call::abi_decode_returns(&t1_result)
            .map_err(|e| eyre!("token1 decode: {}", e))?;
        
        let fee_calldata = IUniswapV3Pool::feeCall {}.abi_encode();
        let fee_result = self.call_contract(pool_address, fee_calldata).await?;
        let fee = IUniswapV3Pool::feeCall::abi_decode_returns(&fee_result)
            .map_err(|e| eyre!("fee decode: {}", e))?;

        let token0_decimals = self.get_decimals(token0);
        let token1_decimals = self.get_decimals(token1);

        let sqrt_price: u128 = slot0.sqrtPriceX96.to();
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
        let res_calldata = IUniswapV2Pair::getReservesCall {}.abi_encode();
        let res_result = self.call_contract(pool_address, res_calldata).await?;
        let reserves = IUniswapV2Pair::getReservesCall::abi_decode_returns(&res_result)
            .map_err(|e| eyre!("reserves decode: {}", e))?;
        
        let t0_calldata = IUniswapV2Pair::token0Call {}.abi_encode();
        let t0_result = self.call_contract(pool_address, t0_calldata).await?;
        let token0 = IUniswapV2Pair::token0Call::abi_decode_returns(&t0_result)
            .map_err(|e| eyre!("token0 decode: {}", e))?;
        
        let t1_calldata = IUniswapV2Pair::token1Call {}.abi_encode();
        let t1_result = self.call_contract(pool_address, t1_calldata).await?;
        let token1 = IUniswapV2Pair::token1Call::abi_decode_returns(&t1_result)
            .map_err(|e| eyre!("token1 decode: {}", e))?;

        let token0_decimals = self.get_decimals(token0);
        let token1_decimals = self.get_decimals(token1);

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
        info!("🚀 Fetching pools from 5 DEXes (EXPANDED - 80+ pools)...");
        
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