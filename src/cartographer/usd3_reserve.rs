//! USD3 + Reserve Protocol Integration - Phase 3 (MULTICALL OPTIMIZED)
//!
//! USD3 is a basket-backed stablecoin that can trade at a premium or discount
//! to its Net Asset Value (NAV). This creates arbitrage when:
//! - DEX price < NAV: Buy USD3, redeem for basket
//! - DEX price > NAV: Mint USD3 from basket, sell on DEX
//!
//! Basket components (yield-bearing):
//! - pyUSD (PayPal USD)
//! - sDAI (Savings DAI)
//! - cUSDC (Compound USDC)
//!
//! OPTIMIZATION: Uses Multicall3 to batch all state fetches into
//! a single RPC call instead of 5-8 individual calls.

use alloy_primitives::{Address, Bytes, U256, address};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{sol, SolCall};
use alloy_rpc_types::TransactionRequest;
use eyre::{eyre, Result};
use tracing::{debug, info, trace, warn};

// ============================================
// USD3 CONTRACT ADDRESSES
// ============================================

/// USD3 Token (Reserve Protocol stablecoin)
pub const USD3_TOKEN: Address = address!("0d86883faf4ffd7aeb116390af37746f45b6f378");

/// USD3 RToken Main contract (for minting/redeeming)
/// Note: This needs to be verified on-chain
pub const USD3_RTOKEN_MAIN: Address = address!("0d86883faf4ffd7aeb116390af37746f45b6f378");

// Basket components
/// pyUSD - PayPal USD
pub const PYUSD_TOKEN: Address = address!("6c3ea9036406852006290770BEdFcAbA0e23A0e8");

/// sDAI - Savings DAI (ERC-4626)
pub const SDAI_TOKEN: Address = address!("83F20F44975D03b1b09e64809B757c47f942BEeA");

/// cUSDC - Compound USDC v3 (comet)
pub const CUSDC_TOKEN: Address = address!("c3d688B66703497DAA19211EEdff47f25384cdc3");

/// USDC - Circle USD
pub const USDC_TOKEN: Address = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");

/// USDT - Tether USD
pub const USDT_TOKEN: Address = address!("dAC17F958D2ee523a2206206994597C13D831ec7");

/// Multicall3 address (same on all EVM chains)
const MULTICALL3: Address = address!("cA11bde05977b3631167028862bE2a173976CA11");

// ============================================
// SOLIDITY INTERFACES
// ============================================

sol! {
    /// Reserve Protocol RToken interface
    interface IRToken {
        // Get basket composition
        function basketsNeeded() external view returns (uint256);
        
        // Issue RToken from basket
        function issue(uint256 amount) external;
        
        // Redeem RToken for basket
        function redeem(uint256 amount) external;
        
        // Current exchange rate (RToken per basket)
        function exchangeRate() external view returns (uint256);
        
        // Total supply
        function totalSupply() external view returns (uint256);
        
        // Check if trading is paused
        function paused() external view returns (bool);
    }
    
    /// Basket Handler interface
    interface IBasketHandler {
        // Get current basket composition
        function quote(uint256 amount, bool roundUp) external view returns (address[] memory, uint256[] memory);
        
        // Check if basket is ready
        function status() external view returns (uint8);
        
        // Get basket nonce (changes when composition changes)
        function nonce() external view returns (uint48);
    }
    
    /// Compound V3 Comet interface (for cUSDC)
    interface IComet {
        // Get underlying balance for an account
        function balanceOf(address account) external view returns (uint256);

        // Get exchange rate
        function exchangeRateStored() external view returns (uint256);

        // Base token
        function baseToken() external view returns (address);

        // Get price (in USD, scaled by 1e8)
        function getPrice(address priceFeed) external view returns (uint256);
    }

    /// ERC-4626 interface for sDAI
    interface IERC4626 {
        function convertToAssets(uint256 shares) external view returns (uint256);
    }

    /// ERC20 interface
    interface IERC20 {
        function totalSupply() external view returns (uint256);
    }

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
}

// ============================================
// USD3 STATE TRACKER
// ============================================

/// Tracks USD3 NAV and basket composition
#[derive(Debug, Clone)]
pub struct USD3State {
    /// USD3 token address
    pub token: Address,
    
    /// Current basket composition (token -> weight)
    pub basket: Vec<BasketComponent>,
    
    /// Net Asset Value per USD3 (in USD, scaled by 1e18)
    pub nav: U256,
    
    /// NAV in USD as float
    pub nav_usd: f64,
    
    /// Current DEX price (if known)
    pub dex_price: Option<f64>,
    
    /// Total supply of USD3
    pub total_supply: U256,
    
    /// Is trading paused?
    pub is_paused: bool,
}

/// Single component in USD3's basket
#[derive(Debug, Clone)]
pub struct BasketComponent {
    pub token: Address,
    pub symbol: String,
    pub weight_bps: u32,      // Weight in basis points (10000 = 100%)
    pub value_usd: f64,       // Current value of component per USD3
    pub is_yield_bearing: bool,
}

impl USD3State {
    /// Check if NAV differs significantly from DEX price
    pub fn check_nav_arb(&self, min_spread_bps: f64) -> Option<NAVArbitrage> {
        let dex_price = self.dex_price?;
        
        let spread_pct = (self.nav_usd - dex_price) / self.nav_usd * 100.0;
        
        if spread_pct.abs() > min_spread_bps / 100.0 {
            let direction = if spread_pct > 0.0 {
                // NAV > DEX: Buy USD3 on DEX, redeem for basket
                NAVArbDirection::BuyAndRedeem
            } else {
                // NAV < DEX: Mint USD3 from basket, sell on DEX
                NAVArbDirection::MintAndSell
            };
            
            return Some(NAVArbitrage {
                token: self.token,
                direction,
                spread_pct: spread_pct.abs(),
                nav_usd: self.nav_usd,
                dex_price,
                basket: self.basket.clone(),
            });
        }
        
        None
    }
}

/// Direction for NAV arbitrage
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NAVArbDirection {
    /// DEX price < NAV: Buy on DEX, redeem basket
    BuyAndRedeem,
    /// DEX price > NAV: Mint from basket, sell on DEX
    MintAndSell,
}

/// NAV arbitrage opportunity
#[derive(Debug, Clone)]
pub struct NAVArbitrage {
    pub token: Address,
    pub direction: NAVArbDirection,
    pub spread_pct: f64,
    pub nav_usd: f64,
    pub dex_price: f64,
    pub basket: Vec<BasketComponent>,
}

// ============================================
// USD3 ADAPTER (MULTICALL OPTIMIZED)
// ============================================

/// Adapter for USD3 and Reserve Protocol integration
/// OPTIMIZED: Uses Multicall3 to fetch all state in 1 RPC call
pub struct USD3Adapter {
    rpc_url: String,
}

impl USD3Adapter {
    pub fn new(rpc_url: String) -> Self {
        Self { rpc_url }
    }

    /// Execute a Multicall3 batch - SINGLE RPC CALL for all data
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

    /// OPTIMIZED: Fetch USD3 state in a SINGLE multicall (1 RPC call instead of 5-8)
    pub async fn fetch_usd3_state(&self) -> Result<USD3State> {
        debug!("📊 Fetching USD3 NAV and basket composition (batched)...");

        let one_unit = U256::from(10u64.pow(18));

        // Build ALL calls in one batch:
        // 1. USD3 totalSupply
        // 2. USD3 paused
        // 3. sDAI convertToAssets (for yield-bearing value)
        // Total: 3 calls in 1 multicall (was 5-8 individual calls)
        let calls = vec![
            // USD3 total supply
            IMulticall3::Call3 {
                target: USD3_TOKEN,
                allowFailure: true,
                callData: IERC20::totalSupplyCall {}.abi_encode().into(),
            },
            // USD3 paused status
            IMulticall3::Call3 {
                target: USD3_RTOKEN_MAIN,
                allowFailure: true,
                callData: IRToken::pausedCall {}.abi_encode().into(),
            },
            // sDAI exchange rate (for basket NAV calculation)
            IMulticall3::Call3 {
                target: SDAI_TOKEN,
                allowFailure: true,
                callData: IERC4626::convertToAssetsCall { shares: one_unit }.abi_encode().into(),
            },
        ];

        debug!("USD3: fetching state with {} calls in 1 multicall", calls.len());

        let results = self.execute_multicall(calls).await?;

        // Parse results
        let total_supply = if results.len() > 0 && results[0].success {
            IERC20::totalSupplyCall::abi_decode_returns(&results[0].returnData)
                .unwrap_or(U256::ZERO)
        } else {
            U256::ZERO
        };

        let is_paused = if results.len() > 1 && results[1].success {
            IRToken::pausedCall::abi_decode_returns(&results[1].returnData)
                .unwrap_or(false)
        } else {
            false
        };

        let sdai_value = if results.len() > 2 && results[2].success {
            let assets = IERC4626::convertToAssetsCall::abi_decode_returns(&results[2].returnData)
                .unwrap_or(one_unit);
            assets.to::<u128>() as f64 / 1e18
        } else {
            1.05 // Default sDAI value if fetch fails
        };

        // Build basket composition with fetched values
        let basket = vec![
            BasketComponent {
                token: PYUSD_TOKEN,
                symbol: "pyUSD".to_string(),
                weight_bps: 3333,
                value_usd: 1.0 * 0.3333, // pyUSD is ~$1
                is_yield_bearing: false,
            },
            BasketComponent {
                token: SDAI_TOKEN,
                symbol: "sDAI".to_string(),
                weight_bps: 3333,
                value_usd: sdai_value * 0.3333,
                is_yield_bearing: true,
            },
            BasketComponent {
                token: CUSDC_TOKEN,
                symbol: "cUSDC".to_string(),
                weight_bps: 3334,
                value_usd: 1.02 * 0.3334, // cUSDC ~$1.02 (yield)
                is_yield_bearing: true,
            },
        ];

        // Calculate NAV from basket
        let nav_usd = basket.iter().map(|c| c.value_usd).sum::<f64>();
        let nav = U256::from((nav_usd * 1e18) as u128);

        debug!("✅ USD3 NAV: ${:.6}, Basket: {} components (1 RPC call)", nav_usd, basket.len());

        Ok(USD3State {
            token: USD3_TOKEN,
            basket,
            nav,
            nav_usd,
            dex_price: None,
            total_supply,
            is_paused,
        })
    }
}

// ============================================
// OTHER BASKET-BACKED STABLECOINS
// ============================================

/// Other Reserve Protocol RTokens we might want to track
pub fn get_known_rtokens() -> Vec<(Address, &'static str)> {
    vec![
        (USD3_TOKEN, "USD3"),
        // Add more RTokens as discovered
        // (eUSD, "eUSD"),
        // (hyUSD, "hyUSD"),
    ]
}

/// Known yield-bearing tokens in baskets
pub fn get_known_yield_tokens() -> Vec<(Address, &'static str, &'static str)> {
    vec![
        (SDAI_TOKEN, "sDAI", "DAI"),
        (CUSDC_TOKEN, "cUSDC", "USDC"),
        // Aave tokens
        // (AUSDC, "aUSDC", "USDC"),
        // (AUSDT, "aUSDT", "USDT"),
    ]
}

// ============================================
// INTEGRATION WITH CURVE POOLS
// ============================================

/// Known Curve pools containing USD3
pub fn get_usd3_curve_pools() -> Vec<(&'static str, &'static str)> {
    vec![
        ("USD3/sUSDS", "Curve NG pool - yield drift both sides"),
        ("USD3/FRAX", "Curve NG pool - algorithmic stablecoin"),
        ("USD3/crvUSD", "Curve NG pool - pegkeeper dynamics"),
    ]
}

/// Check if a token is part of the USD3 ecosystem
pub fn is_usd3_ecosystem_token(address: &Address) -> bool {
    *address == USD3_TOKEN ||
    *address == PYUSD_TOKEN ||
    *address == SDAI_TOKEN ||
    *address == CUSDC_TOKEN
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_nav_arb_detection() {
        let state = USD3State {
            token: USD3_TOKEN,
            basket: vec![
                BasketComponent {
                    token: PYUSD_TOKEN,
                    symbol: "pyUSD".to_string(),
                    weight_bps: 3333,
                    value_usd: 0.3333,
                    is_yield_bearing: false,
                },
                BasketComponent {
                    token: SDAI_TOKEN,
                    symbol: "sDAI".to_string(),
                    weight_bps: 3333,
                    value_usd: 0.35, // sDAI at premium
                    is_yield_bearing: true,
                },
                BasketComponent {
                    token: CUSDC_TOKEN,
                    symbol: "cUSDC".to_string(),
                    weight_bps: 3334,
                    value_usd: 0.34,
                    is_yield_bearing: true,
                },
            ],
            nav: U256::from(1_023_000_000_000_000_000u128), // ~1.023
            nav_usd: 1.023,
            dex_price: Some(1.015), // Trading below NAV
            total_supply: U256::from(10u64.pow(24)),
            is_paused: false,
        };
        
        // NAV 1.023 vs DEX 1.015 = ~0.78% spread
        let arb = state.check_nav_arb(50.0); // 0.5% min threshold
        assert!(arb.is_some());
        
        let arb = arb.unwrap();
        assert_eq!(arb.direction, NAVArbDirection::BuyAndRedeem);
        assert!(arb.spread_pct > 0.5);
    }
}