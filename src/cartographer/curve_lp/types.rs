//! Curve LP Token Types, Addresses, and ABIs
//!
//! Contains all contract addresses, interface definitions, and constants
//! for LP token NAV arbitrage functionality.
//!
//! CRITICAL: All addresses are for Ethereum Mainnet.

use alloy_primitives::{address, Address};
use alloy_sol_types::sol;

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
/// Format: (Pool Address, LP Token Address, Name)
pub const LP_POOLS: &[(Address, Address, &str)] = &[
    // 3pool - The most liquid Curve pool
    (
        address!("bEbc44782C7dB0a1A60Cb6fe97d0b483032FF1C7"), // 3pool
        address!("6c3F90f043a72FA612cbac8115EE7e52BDe6E490"), // 3CRV
        "3pool"
    ),
    // FRAX/USDC (FRAXBP) - High volume FRAX pool
    (
        address!("DcEF968d416a41Cdac0ED8702fAC8128A64241A2"), // FRAXBP
        address!("3175Df0976dFA876431C2E9eE6Bc45b65d3473CC"), // crvFRAX
        "FRAXBP"
    ),
    // stETH/ETH - Lido staked ETH pool
    (
        address!("DC24316b9AE028F1497c275EB9192a3Ea0f67022"), // stETH/ETH
        address!("06325440D014e39736583c165C2963BA99fAf14E"), // steCRV
        "stETH"
    ),
    // sUSD pool - Synthetix USD
    (
        address!("A5407eAE9Ba41422680e2e00537571bcC53efBfD"), // sUSD pool
        address!("C25a3A3b969415c80451098fa907EC722572917F"), // sCRV
        "sUSD"
    ),
    // crvUSD/USDC
    (
        address!("4DEcE678ceceb27446b35C672dC7d61F30bAD69E"), // crvUSD/USDC
        address!("4DEcE678ceceb27446b35C672dC7d61F30bAD69E"), // LP = pool for NG
        "crvUSD-USDC"
    ),
    // crvUSD/USDT
    (
        address!("390f3595bCa2Df7d23783dFd126427CCeb997BF4"), // crvUSD/USDT
        address!("390f3595bCa2Df7d23783dFd126427CCeb997BF4"), // LP = pool for NG
        "crvUSD-USDT"
    ),
    // LUSD/3Crv
    (
        address!("Ed279fDD11cA84bEef15AF5D39BB4d4bEE23F0cA"), // LUSD/3Crv
        address!("Ed279fDD11cA84bEef15AF5D39BB4d4bEE23F0cA"), // LUSD3CRV-f
        "LUSD"
    ),
    // MIM/3Crv
    (
        address!("5a6A4D54456819380173272A5E8E9B9904BdF41B"), // MIM/3Crv
        address!("5a6A4D54456819380173272A5E8E9B9904BdF41B"), // MIM-3LP3CRV-f
        "MIM"
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

// ============================================
// CACHE CONFIGURATION
// ============================================

/// Cache duration for pool structure (addresses, coins) - rarely changes
pub const POOL_STRUCTURE_CACHE_SECS: u64 = 300; // 5 minutes

/// Cache duration for virtual_price - slow moving but important for accuracy
pub const VIRTUAL_PRICE_CACHE_SECS: u64 = 60; // 1 minute

/// Cache duration for secondary market discovery
pub const MARKET_CACHE_SECS: u64 = 300; // 5 minutes

/// Discovery throttle - only discover new LP markets every N scans
pub const DISCOVERY_THROTTLE_INTERVAL: u64 = 10;

/// Uniswap V3 fee tiers to check for LP token pairs
pub const UNIV3_FEE_TIERS: &[u32] = &[100, 500, 3000, 10000];

/// Minimum liquidity in USD to consider a market
pub const MIN_MARKET_LIQUIDITY_USD: f64 = 50_000.0;

// ============================================
// NAV CONFIGURATION
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
// SOLIDITY INTERFACES
// ============================================

sol! {
    /// Curve StableSwap Pool Interface
    /// IMPORTANT: StableSwap uses int128 for indices, CryptoSwap uses uint256
    #[allow(missing_docs)]
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
        function calc_token_amount(uint256[] memory amounts, bool is_deposit) external view returns (uint256);

        /// Estimate coins from single-coin withdrawal (INCLUDES fees)
        /// More accurate than calc_token_amount for withdrawals
        function calc_withdraw_one_coin(uint256 lp_amount, int128 i) external view returns (uint256);

        // ============================================
        // EXCHANGE FUNCTIONS (executor can use these)
        // ============================================

        /// Standard swap (requires approval)
        function exchange(int128 i, int128 j, uint256 dx, uint256 min_dy) external returns (uint256);

        /// Approval-free swap (tokens must be transferred first)
        function exchange_received(int128 i, int128 j, uint256 dx, uint256 min_dy, address receiver) external returns (uint256);

        /// Get expected output amount for a swap
        function get_dy(int128 i, int128 j, uint256 dx) external view returns (uint256);
    }

    /// Curve Factory Interface (for pool discovery)
    #[allow(missing_docs)]
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
    #[allow(missing_docs)]
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
    #[allow(missing_docs)]
    interface IUniswapV3Factory {
        /// Get pool address for token pair and fee
        function getPool(address tokenA, address tokenB, uint24 fee) external view returns (address);
    }

    #[allow(missing_docs)]
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
    #[allow(missing_docs)]
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
    #[allow(missing_docs)]
    interface IERC20 {
        function totalSupply() external view returns (uint256);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string memory);
        function balanceOf(address account) external view returns (uint256);
    }
}

// ============================================
// KNOWN STABLECOINS
// ============================================

/// Known stablecoin addresses (assume $1 price)
pub const STABLECOINS: &[Address] = &[
    address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"), // USDC
    address!("dAC17F958D2ee523a2206206994597C13D831ec7"), // USDT
    address!("6B175474E89094C44Da98b954EedcdeCB5BE3830"), // DAI
    address!("853d955aCEf822Db058eb8505911ED77F175b99e"), // FRAX
    address!("f939E0A03FB07F59A73314E73794Be0E57ac1b4E"), // crvUSD
    address!("5f98805A4E8be255a32880FDeC7F6728C6568bA0"), // LUSD
    address!("99D8a9C45b2ecA8864373A26D1459e3Dff1e17F3"), // MIM
    address!("dC035D45d973E3EC169d2276DDab16f1e407384F"), // USDS
];

/// Check if address is a known stablecoin
pub fn is_stablecoin(addr: &Address) -> bool {
    STABLECOINS.contains(addr)
}

/// WETH address for ETH-related pools
pub const WETH: Address = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");

/// stETH address
pub const STETH: Address = address!("ae7ab96520DE3A18E5e111B5EaAb095312D7fE84");

/// wstETH address
pub const WSTETH: Address = address!("7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stablecoin_check() {
        let usdc = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
        let weth = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");

        assert!(is_stablecoin(&usdc));
        assert!(!is_stablecoin(&weth));
    }

    #[test]
    fn test_lp_pools_defined() {
        assert!(!LP_POOLS.is_empty());
        assert!(LP_POOLS.len() >= 5);

        // Check 3pool is included
        let has_3pool = LP_POOLS.iter().any(|(_, _, name)| *name == "3pool");
        assert!(has_3pool);
    }

    #[test]
    fn test_quote_tokens_defined() {
        assert!(QUOTE_TOKENS.len() >= 4);

        // Check WETH is included
        let has_weth = QUOTE_TOKENS.iter().any(|(addr, _, _)| *addr == WETH);
        assert!(has_weth);
    }
}
