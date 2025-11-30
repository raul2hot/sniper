//! Sky Ecosystem Adapter - Phase 2 (MULTICALL OPTIMIZED)
//!
//! Integration with Sky Protocol (formerly MakerDAO) for:
//! - USDS/DAI migration paths (1:1 swap)
//! - sUSDS (ERC-4626 savings token) yield arbitrage
//! - Sky Savings Rate integration
//!
//! Key arbitrage opportunity:
//! sUSDS's value DRIFTS upward continuously due to yield accrual.
//! When DEX price lags behind the true redemption value, arbitrage exists.
//!
//! OPTIMIZATION: Uses Multicall3 to batch all vault state fetches into
//! a single RPC call instead of 10+ individual calls.

use alloy_primitives::{Address, Bytes, U256, address};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{sol, SolCall};
use alloy_rpc_types::TransactionRequest;
use eyre::{eyre, Result};
use tracing::{debug, info, trace, warn};

// ============================================
// SKY ECOSYSTEM CONTRACT ADDRESSES
// ============================================

/// USDS - Sky's USD stablecoin (1:1 with DAI)
pub const USDS_TOKEN: Address = address!("dC035D45d973E3EC169d2276DDab16f1e407384F");

/// sUSDS - Savings USDS (ERC-4626 vault)
pub const SUSDS_TOKEN: Address = address!("a3931d71877C0E7a3148CB7Eb4463524FEc27fbD");

/// SKY - Governance token
pub const SKY_TOKEN: Address = address!("56072C95FAA701256059aa122697B133aDEd9279");

/// DAI - Original MakerDAO stablecoin
pub const DAI_TOKEN: Address = address!("6B175474E89094C44Da98b954EedcdeCB5BE3830");

/// sDAI - Savings DAI (Spark Protocol ERC-4626)
pub const SDAI_TOKEN: Address = address!("83F20F44975D03b1b09e64809B757c47f942BEeA");

/// DAI-USDS Migration/Upgrade Module
pub const DAI_USDS_CONVERTER: Address = address!("3225737a9Bbb6473CB4a45b7244ACa2BeFdB276A");

/// Sky Savings Rate Module (SSR)
pub const SSR_MODULE: Address = address!("a3931d71877C0E7a3148CB7Eb4463524FEc27fbD"); // sUSDS is the module

/// Multicall3 address (same on all EVM chains)
const MULTICALL3: Address = address!("cA11bde05977b3631167028862bE2a173976CA11");

// ============================================
// SOLIDITY INTERFACES
// ============================================

sol! {
    /// ERC-4626 Vault interface (sUSDS, sDAI, etc.)
    interface IERC4626 {
        // Asset -> Shares conversion (how many shares for X assets)
        function convertToShares(uint256 assets) external view returns (uint256);
        
        // Shares -> Asset conversion (how many assets for X shares)
        function convertToAssets(uint256 shares) external view returns (uint256);
        
        // Preview functions (accounting for fees, slippage)
        function previewDeposit(uint256 assets) external view returns (uint256);
        function previewMint(uint256 shares) external view returns (uint256);
        function previewWithdraw(uint256 assets) external view returns (uint256);
        function previewRedeem(uint256 shares) external view returns (uint256);
        
        // Actual operations
        function deposit(uint256 assets, address receiver) external returns (uint256);
        function mint(uint256 shares, address receiver) external returns (uint256);
        function withdraw(uint256 assets, address receiver, address owner) external returns (uint256);
        function redeem(uint256 shares, address receiver, address owner) external returns (uint256);
        
        // View functions
        function asset() external view returns (address);
        function totalAssets() external view returns (uint256);
        function totalSupply() external view returns (uint256);
        function maxDeposit(address receiver) external view returns (uint256);
        function maxMint(address receiver) external view returns (uint256);
        function maxWithdraw(address owner) external view returns (uint256);
        function maxRedeem(address owner) external view returns (uint256);
    }
    
    /// DAI-USDS Converter interface
    interface IDaiUsdsConverter {
        // DAI -> USDS conversion (1:1)
        function daiToUsds(address usr, uint256 wad) external;
        
        // USDS -> DAI conversion (1:1)
        function usdsToDai(address usr, uint256 wad) external;
    }
    
    /// Sky Savings Rate view functions
    interface ISSR {
        // Current savings rate (ray precision = 1e27)
        function ssr() external view returns (uint256);

        // Rate accumulator (chi)
        function chi() external view returns (uint256);

        // Last update timestamp
        function rho() external view returns (uint256);
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
// ERC-4626 VALUE TRACKER
// ============================================

/// Tracks ERC-4626 vault exchange rates for arbitrage detection
#[derive(Debug, Clone)]
pub struct ERC4626State {
    pub vault_address: Address,
    pub underlying_asset: Address,
    pub symbol: String,
    pub underlying_symbol: String,
    
    /// Exchange rate: assets per share (scaled by 1e18)
    /// For sUSDS: how much USDS you get per 1 sUSDS
    pub assets_per_share: U256,
    
    /// Exchange rate: shares per asset (scaled by 1e18)
    /// For sUSDS: how much sUSDS you get per 1 USDS
    pub shares_per_asset: U256,
    
    /// Total assets under management
    pub total_assets: U256,
    
    /// Total shares outstanding
    pub total_supply: U256,
    
    /// Current DEX price (if known) - for comparison
    pub dex_price: Option<f64>,
    
    /// Fair value in USD based on underlying
    pub fair_value_usd: f64,
}

impl ERC4626State {
    /// Calculate the expected return from deposit + redeem cycle
    /// If this is significantly different from DEX price, arbitrage exists
    pub fn deposit_redeem_ratio(&self) -> f64 {
        if self.shares_per_asset == U256::ZERO || self.assets_per_share == U256::ZERO {
            return 1.0;
        }
        
        // Deposit 1e18 assets -> get shares_per_asset shares
        // Redeem shares_per_asset shares -> get X assets
        // X / 1e18 = ratio
        
        let shares = self.shares_per_asset.to::<u128>() as f64;
        let back = (shares * self.assets_per_share.to::<u128>() as f64) / 1e18;
        
        back / 1e18
    }
    
    /// Check if DEX price creates arbitrage opportunity
    pub fn check_arb_opportunity(&self, min_profit_bps: f64) -> Option<YieldDriftArb> {
        let dex_price = self.dex_price?;
        
        // True value = assets_per_share / 1e18
        let true_value = self.assets_per_share.to::<u128>() as f64 / 1e18;
        
        // Calculate spread
        let spread_pct = (true_value - dex_price) / true_value * 100.0;
        
        if spread_pct.abs() > min_profit_bps / 100.0 {
            let direction = if spread_pct > 0.0 {
                // True value > DEX price: Buy on DEX, redeem for underlying
                ArbDirection::BuyAndRedeem
            } else {
                // True value < DEX price: Deposit underlying, sell on DEX
                ArbDirection::DepositAndSell
            };
            
            return Some(YieldDriftArb {
                vault: self.vault_address,
                underlying: self.underlying_asset,
                direction,
                spread_pct: spread_pct.abs(),
                true_value,
                dex_price,
            });
        }
        
        None
    }
}

/// Direction of yield drift arbitrage
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArbDirection {
    /// DEX price < true value: Buy vault token on DEX, redeem for underlying
    BuyAndRedeem,
    /// DEX price > true value: Deposit underlying, sell vault token on DEX
    DepositAndSell,
}

/// Yield drift arbitrage opportunity
#[derive(Debug, Clone)]
pub struct YieldDriftArb {
    pub vault: Address,
    pub underlying: Address,
    pub direction: ArbDirection,
    pub spread_pct: f64,
    pub true_value: f64,
    pub dex_price: f64,
}

// ============================================
// SKY ECOSYSTEM ADAPTER (MULTICALL OPTIMIZED)
// ============================================

/// Adapter for Sky Protocol integration
/// OPTIMIZED: Uses Multicall3 to fetch all vault data in 1 RPC call
pub struct SkyAdapter {
    rpc_url: String,
}

impl SkyAdapter {
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

    /// OPTIMIZED: Fetch ALL vaults in a SINGLE multicall (1 RPC call instead of 10+)
    pub async fn fetch_all_vaults(&self) -> Result<Vec<ERC4626State>> {
        let vaults_to_fetch: Vec<(Address, &str, &str, Address)> = vec![
            (SUSDS_TOKEN, "sUSDS", "USDS", USDS_TOKEN),
            (SDAI_TOKEN, "sDAI", "DAI", DAI_TOKEN),
        ];

        let one_unit = U256::from(10u64.pow(18));

        // Build ALL calls for ALL vaults in one batch
        // Per vault: convertToAssets, convertToShares, totalAssets, totalSupply = 4 calls
        // 2 vaults × 4 calls = 8 calls in 1 multicall (was 10 individual RPC calls)
        let mut calls: Vec<IMulticall3::Call3> = Vec::new();

        for (vault, _, _, _) in &vaults_to_fetch {
            // convertToAssets(1e18) - assets per share
            calls.push(IMulticall3::Call3 {
                target: *vault,
                allowFailure: true,
                callData: IERC4626::convertToAssetsCall { shares: one_unit }.abi_encode().into(),
            });
            // convertToShares(1e18) - shares per asset
            calls.push(IMulticall3::Call3 {
                target: *vault,
                allowFailure: true,
                callData: IERC4626::convertToSharesCall { assets: one_unit }.abi_encode().into(),
            });
            // totalAssets
            calls.push(IMulticall3::Call3 {
                target: *vault,
                allowFailure: true,
                callData: IERC4626::totalAssetsCall {}.abi_encode().into(),
            });
            // totalSupply
            calls.push(IMulticall3::Call3 {
                target: *vault,
                allowFailure: true,
                callData: IERC4626::totalSupplyCall {}.abi_encode().into(),
            });
        }

        debug!("Sky ecosystem: fetching {} vaults with {} calls in 1 multicall", vaults_to_fetch.len(), calls.len());

        let results = self.execute_multicall(calls).await?;

        // Parse results (4 calls per vault)
        let mut vault_states = Vec::new();

        for (i, (vault, symbol, underlying_symbol, underlying)) in vaults_to_fetch.iter().enumerate() {
            let offset = i * 4;

            if offset + 3 >= results.len() {
                warn!("Insufficient results for vault {}", symbol);
                continue;
            }

            // Parse convertToAssets
            let assets_per_share = if results[offset].success {
                IERC4626::convertToAssetsCall::abi_decode_returns(&results[offset].returnData)
                    .unwrap_or(U256::from(10u64.pow(18)))
            } else {
                warn!("Failed to fetch assets_per_share for {}", symbol);
                continue;
            };

            // Parse convertToShares
            let shares_per_asset = if results[offset + 1].success {
                IERC4626::convertToSharesCall::abi_decode_returns(&results[offset + 1].returnData)
                    .unwrap_or(U256::from(10u64.pow(18)))
            } else {
                warn!("Failed to fetch shares_per_asset for {}", symbol);
                continue;
            };

            // Parse totalAssets
            let total_assets = if results[offset + 2].success {
                IERC4626::totalAssetsCall::abi_decode_returns(&results[offset + 2].returnData)
                    .unwrap_or(U256::ZERO)
            } else {
                U256::ZERO
            };

            // Parse totalSupply
            let total_supply = if results[offset + 3].success {
                IERC4626::totalSupplyCall::abi_decode_returns(&results[offset + 3].returnData)
                    .unwrap_or(U256::ZERO)
            } else {
                U256::ZERO
            };

            let fair_value_usd = assets_per_share.to::<u128>() as f64 / 1e18;

            debug!(
                "📊 {} exchange rate: 1 {} = {:.6} {}",
                symbol, symbol, fair_value_usd, underlying_symbol
            );

            vault_states.push(ERC4626State {
                vault_address: *vault,
                underlying_asset: *underlying,
                symbol: symbol.to_string(),
                underlying_symbol: underlying_symbol.to_string(),
                assets_per_share,
                shares_per_asset,
                total_assets,
                total_supply,
                dex_price: None,
                fair_value_usd,
            });
        }

        info!("✅ Sky ecosystem: fetched {} vaults in 1 RPC call", vault_states.len());
        Ok(vault_states)
    }

    /// Check for yield drift arbitrage across all vaults
    pub fn check_yield_drift_arbs(
        &self,
        vault_states: &[ERC4626State],
        min_profit_bps: f64,
    ) -> Vec<YieldDriftArb> {
        let mut arbs = Vec::new();

        for state in vault_states {
            if let Some(arb) = state.check_arb_opportunity(min_profit_bps) {
                info!(
                    "🎯 Yield drift arb found: {} spread={:.2}% direction={:?}",
                    state.symbol, arb.spread_pct, arb.direction
                );
                arbs.push(arb);
            }
        }

        arbs
    }

    /// Check DAI -> USDS conversion rate (should be 1:1)
    pub fn get_dai_usds_rate(&self) -> f64 {
        // DAI-USDS conversion is always 1:1 in the Sky protocol
        1.0
    }
}

// ============================================
// VIRTUAL POOLS FOR GRAPH INTEGRATION
// ============================================

/// Creates virtual "pools" for ERC-4626 deposit/redeem operations
/// These appear as edges in the arbitrage graph
pub fn create_erc4626_virtual_pools(state: &ERC4626State) -> Vec<VirtualERC4626Pool> {
    vec![
        // Deposit direction: underlying -> vault token
        VirtualERC4626Pool {
            vault: state.vault_address,
            underlying: state.underlying_asset,
            direction: ERC4626Direction::Deposit,
            rate: state.shares_per_asset,
            fee_bps: 0, // No fee for deposit (usually)
        },
        // Redeem direction: vault token -> underlying
        VirtualERC4626Pool {
            vault: state.vault_address,
            underlying: state.underlying_asset,
            direction: ERC4626Direction::Redeem,
            rate: state.assets_per_share,
            fee_bps: 0, // No fee for redeem (usually)
        },
    ]
}

/// Direction for ERC-4626 operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ERC4626Direction {
    Deposit, // underlying -> shares
    Redeem,  // shares -> underlying
}

/// Virtual pool representing an ERC-4626 deposit/redeem
#[derive(Debug, Clone)]
pub struct VirtualERC4626Pool {
    pub vault: Address,
    pub underlying: Address,
    pub direction: ERC4626Direction,
    pub rate: U256,
    pub fee_bps: u32,
}

impl VirtualERC4626Pool {
    /// Calculate output for input amount
    pub fn get_output(&self, input: U256) -> U256 {
        // output = input * rate / 1e18
        (input * self.rate) / U256::from(10u64.pow(18))
    }
}

// ============================================
// TOKEN HELPER FUNCTIONS
// ============================================

/// Check if token is a known ERC-4626 vault
pub fn is_sky_ecosystem_token(address: &Address) -> bool {
    *address == USDS_TOKEN ||
    *address == SUSDS_TOKEN ||
    *address == DAI_TOKEN ||
    *address == SDAI_TOKEN ||
    *address == SKY_TOKEN
}

/// Get symbol for Sky ecosystem tokens
pub fn get_sky_token_symbol(address: &Address) -> Option<&'static str> {
    if *address == USDS_TOKEN { return Some("USDS"); }
    if *address == SUSDS_TOKEN { return Some("sUSDS"); }
    if *address == DAI_TOKEN { return Some("DAI"); }
    if *address == SDAI_TOKEN { return Some("sDAI"); }
    if *address == SKY_TOKEN { return Some("SKY"); }
    None
}

/// All known ERC-4626 vaults for yield arbitrage
pub fn get_all_erc4626_vaults() -> Vec<(Address, &'static str, &'static str)> {
    vec![
        (SUSDS_TOKEN, "sUSDS", "USDS"),
        (SDAI_TOKEN, "sDAI", "DAI"),
        // Add more vaults as they're discovered
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_erc4626_state_ratio() {
        let state = ERC4626State {
            vault_address: Address::ZERO,
            underlying_asset: Address::ZERO,
            symbol: "sUSDS".to_string(),
            underlying_symbol: "USDS".to_string(),
            assets_per_share: U256::from(1_050_000_000_000_000_000u128), // 1.05
            shares_per_asset: U256::from(952_380_952_380_952_380u128),   // ~0.952
            total_assets: U256::from(10u64.pow(24)),
            total_supply: U256::from(10u64.pow(24)),
            dex_price: Some(1.04),
            fair_value_usd: 1.05,
        };
        
        // Check arb opportunity (true value 1.05 vs DEX 1.04 = 0.95% spread)
        let arb = state.check_arb_opportunity(50.0); // 0.5% min
        assert!(arb.is_some());
        
        let arb = arb.unwrap();
        assert_eq!(arb.direction, ArbDirection::BuyAndRedeem);
        assert!(arb.spread_pct > 0.5);
    }
    
    #[test]
    fn test_virtual_pool_output() {
        let pool = VirtualERC4626Pool {
            vault: Address::ZERO,
            underlying: Address::ZERO,
            direction: ERC4626Direction::Redeem,
            rate: U256::from(1_050_000_000_000_000_000u128), // 1.05
            fee_bps: 0,
        };
        
        // 100 shares -> 105 underlying
        let input = U256::from(100u64 * 10u64.pow(18));
        let output = pool.get_output(input);
        
        assert_eq!(output, U256::from(105u64 * 10u64.pow(18)));
    }
}