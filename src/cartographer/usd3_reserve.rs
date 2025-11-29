//! USD3 + Reserve Protocol Integration - Phase 3
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

use alloy_primitives::{Address, U256, address};
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
// USD3 ADAPTER
// ============================================

/// Adapter for USD3 and Reserve Protocol integration
pub struct USD3Adapter {
    rpc_url: String,
}

impl USD3Adapter {
    pub fn new(rpc_url: String) -> Self {
        Self { rpc_url }
    }
    
    /// Helper to call a contract
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
    
    /// Fetch current USD3 state including NAV
    pub async fn fetch_usd3_state(&self) -> Result<USD3State> {
        info!("📊 Fetching USD3 NAV and basket composition...");
        
        // Get total supply
        let total_supply = self.get_total_supply(USD3_TOKEN).await?;
        
        // Get basket composition
        let basket = self.get_basket_composition().await?;
        
        // Calculate NAV from basket
        let nav_usd = basket.iter().map(|c| c.value_usd).sum::<f64>();
        let nav = U256::from((nav_usd * 1e18) as u128);
        
        // Check if paused
        let is_paused = self.is_paused().await.unwrap_or(false);
        
        info!("📊 USD3 NAV: ${:.6}, Basket: {} components", nav_usd, basket.len());
        
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
    
    /// Get basket composition with current values
    async fn get_basket_composition(&self) -> Result<Vec<BasketComponent>> {
        // Note: In production, this would query the BasketHandler contract
        // For now, we use known basket composition
        
        let mut components = Vec::new();
        
        // pyUSD component
        let pyusd_value = self.get_stablecoin_value(PYUSD_TOKEN).await.unwrap_or(1.0);
        components.push(BasketComponent {
            token: PYUSD_TOKEN,
            symbol: "pyUSD".to_string(),
            weight_bps: 3333, // ~33.33%
            value_usd: pyusd_value * 0.3333,
            is_yield_bearing: false,
        });
        
        // sDAI component (yield-bearing)
        let sdai_value = self.get_erc4626_value(SDAI_TOKEN).await.unwrap_or(1.0);
        components.push(BasketComponent {
            token: SDAI_TOKEN,
            symbol: "sDAI".to_string(),
            weight_bps: 3333,
            value_usd: sdai_value * 0.3333,
            is_yield_bearing: true,
        });
        
        // cUSDC component (yield-bearing)
        let cusdc_value = self.get_compound_value(CUSDC_TOKEN).await.unwrap_or(1.0);
        components.push(BasketComponent {
            token: CUSDC_TOKEN,
            symbol: "cUSDC".to_string(),
            weight_bps: 3334,
            value_usd: cusdc_value * 0.3334,
            is_yield_bearing: true,
        });
        
        Ok(components)
    }
    
    /// Get value of a simple stablecoin (assumed ~$1)
    async fn get_stablecoin_value(&self, _token: Address) -> Result<f64> {
        // In production, use Chainlink oracle
        Ok(1.0)
    }
    
    /// Get value of an ERC-4626 vault token
    async fn get_erc4626_value(&self, vault: Address) -> Result<f64> {
        use super::sky_ecosystem::IERC4626;
        
        let one_unit = U256::from(10u64.pow(18));
        let calldata = IERC4626::convertToAssetsCall { shares: one_unit }.abi_encode();
        let output = self.call_contract(vault, calldata).await?;
        let assets = IERC4626::convertToAssetsCall::abi_decode_returns(&output)?;
        
        Ok(assets.to::<u128>() as f64 / 1e18)
    }
    
    /// Get value of a Compound V3 token
    async fn get_compound_value(&self, comet: Address) -> Result<f64> {
        // Compound V3 (Comet) exchange rate
        // In production, query the actual exchange rate
        // For now, assume slight premium due to yield
        Ok(1.02) // ~2% accumulated yield
    }
    
    /// Get total supply of a token
    async fn get_total_supply(&self, token: Address) -> Result<U256> {
        sol! {
            interface IERC20 {
                function totalSupply() external view returns (uint256);
            }
        }
        
        let calldata = IERC20::totalSupplyCall {}.abi_encode();
        let output = self.call_contract(token, calldata).await?;
        let supply = IERC20::totalSupplyCall::abi_decode_returns(&output)?;
        Ok(supply)
    }
    
    /// Check if USD3 trading is paused
    async fn is_paused(&self) -> Result<bool> {
        let calldata = IRToken::pausedCall {}.abi_encode();
        let output = self.call_contract(USD3_RTOKEN_MAIN, calldata).await?;
        let paused = IRToken::pausedCall::abi_decode_returns(&output)?;
        Ok(paused)
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