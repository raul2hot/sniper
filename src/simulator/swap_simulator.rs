//! Swap Simulator - FIXED Edition
//!
//! FIXES:
//! 1. Proper gas price conversion (wei -> gwei)
//! 2. Dynamic simulation sizing based on token liquidity
//! 3. Better error handling and logging
//! 4. Minimum gas price floor (network can't really be 0.05 gwei)

use alloy_primitives::{Address, U256};
use alloy_provider::{Provider, ProviderBuilder};
use eyre::Result;
use tracing::{debug, info, warn};

use super::UniV3Quoter;
use crate::brain::ArbitrageCycle;
use crate::cartographer::{Dex, PoolState, get_token_decimals};

/// Maximum gas estimate per swap to prevent unrealistic values
const MAX_GAS_PER_SWAP: u64 = 500_000;

/// Minimum realistic gas price on mainnet (gwei)
/// As of Nov 2025, mainnet gas is extremely low (0.05-0.5 gwei typical)
/// due to L2 adoption and EIP-4844
const MIN_GAS_PRICE_GWEI: f64 = 0.01;

/// Default gas price if we can't fetch it (based on Nov 2025 conditions)
const DEFAULT_GAS_PRICE_GWEI: f64 = 0.5;

/// Token liquidity tiers for dynamic sizing
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LiquidityTier {
    /// Major pairs (WETH/USDC, WETH/USDT) - can handle $50K+
    Major,
    /// Mid-cap tokens (LINK, UNI, AAVE) - $5K-$20K
    MidCap,
    /// Long-tail tokens (PEPE, SHIB, etc.) - $500-$2K
    LongTail,
    /// Unknown/new tokens - $100-$500
    Unknown,
}

impl LiquidityTier {
    /// Get recommended simulation amount in USD
    pub fn recommended_amount_usd(&self) -> f64 {
        match self {
            LiquidityTier::Major => 10_000.0,
            LiquidityTier::MidCap => 2_000.0,
            LiquidityTier::LongTail => 500.0,
            LiquidityTier::Unknown => 200.0,
        }
    }
}

/// Result of a single swap simulation
#[derive(Debug, Clone)]
pub struct SwapResult {
    pub pool: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub amount_out: U256,
    pub gas_used: u64,
    pub dex: Dex,
}

/// Result of a full arbitrage simulation
#[derive(Debug, Clone)]
pub struct ArbitrageSimulation {
    pub cycle: ArbitrageCycle,
    pub swaps: Vec<SwapResult>,
    pub input_amount: U256,
    pub output_amount: U256,
    pub total_gas_used: u64,
    pub gas_cost_wei: U256,
    pub profit_wei: i128,
    pub profit_usd: f64,
    pub is_profitable: bool,
    pub simulation_success: bool,
    pub revert_reason: Option<String>,
    pub token_decimals: u8,
    /// The liquidity tier used for this simulation
    pub liquidity_tier: LiquidityTier,
    /// Actual input amount in USD
    pub input_usd: f64,
}

impl ArbitrageSimulation {
    pub fn return_multiplier(&self) -> f64 {
        if self.input_amount == U256::ZERO {
            return 0.0;
        }
        let input_f64: f64 = self.input_amount.to_string().parse().unwrap_or(0.0);
        let output_f64: f64 = self.output_amount.to_string().parse().unwrap_or(0.0);
        output_f64 / input_f64
    }
    
    pub fn gross_profit_pct(&self) -> f64 {
        (self.return_multiplier() - 1.0) * 100.0
    }
}

/// Swap simulator using Provider-based eth_call
pub struct SwapSimulator {
    rpc_url: String,
    quoter: UniV3Quoter,
    gas_price_gwei: f64,
    eth_price_usd: f64,
}

impl SwapSimulator {
    /// Create a new simulator connected to the given RPC URL
    pub async fn new(rpc_url: &str) -> Result<Self> {
        info!("Initializing Provider-based simulator with RPC: {}", rpc_url);
        
        let provider = ProviderBuilder::new()
            .on_http(rpc_url.parse()?);
        
        // Get current block for logging
        let block_number = provider.get_block_number().await?;
        info!("Connected to block: {}", block_number);
        
        // Get gas price with proper handling
        let gas_price_gwei = match provider.get_gas_price().await {
            Ok(gas_price_wei) => {
                // Gas price is returned in wei, convert to gwei
                let gwei = gas_price_wei as f64 / 1e9;
                
                // Sanity check - if it's unrealistically low, use a floor
                if gwei < MIN_GAS_PRICE_GWEI {
                    warn!(
                        "Gas price {:.4} gwei seems too low, using minimum of {:.1} gwei",
                        gwei, MIN_GAS_PRICE_GWEI
                    );
                    MIN_GAS_PRICE_GWEI
                } else if gwei > 1000.0 {
                    warn!(
                        "Gas price {:.1} gwei seems too high, using default of {:.1} gwei",
                        gwei, DEFAULT_GAS_PRICE_GWEI
                    );
                    DEFAULT_GAS_PRICE_GWEI
                } else {
                    gwei
                }
            }
            Err(e) => {
                warn!("Failed to get gas price: {}, using default {:.1} gwei", e, DEFAULT_GAS_PRICE_GWEI);
                DEFAULT_GAS_PRICE_GWEI
            }
        };
        
        info!("Using gas price: {:.2} gwei", gas_price_gwei);
        
        let quoter = UniV3Quoter::new(rpc_url.to_string());
        
        Ok(Self {
            rpc_url: rpc_url.to_string(),
            quoter,
            gas_price_gwei,
            eth_price_usd: 3500.0, // Default, should be updated
        })
    }
    
    /// Set the ETH price in USD
    pub fn set_eth_price(&mut self, eth_price_usd: f64) {
        self.eth_price_usd = eth_price_usd;
    }
    
    /// Set the gas price in gwei
    pub fn set_gas_price(&mut self, gas_price_gwei: f64) {
        // Apply minimum floor
        self.gas_price_gwei = gas_price_gwei.max(MIN_GAS_PRICE_GWEI);
    }
    
    /// Get the current gas price in gwei
    pub fn gas_price_gwei(&self) -> f64 {
        self.gas_price_gwei
    }
    
    /// Determine the liquidity tier for a token
    pub fn get_liquidity_tier(&self, token: &Address) -> LiquidityTier {
        let addr_hex = format!("{:?}", token).to_lowercase();
        
        // Major tokens - highest liquidity
        if addr_hex.contains("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2")  // WETH
            || addr_hex.contains("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")  // USDC
            || addr_hex.contains("dac17f958d2ee523a2206206994597c13d831ec7")  // USDT
            || addr_hex.contains("6b175474e89094c44da98b954eedcdecb5be3830")  // DAI
            || addr_hex.contains("2260fac5e5542a773aa44fbcfedf7c193bc2c599")  // WBTC
        {
            return LiquidityTier::Major;
        }
        
        // Mid-cap DeFi tokens
        if addr_hex.contains("514910771af9ca656af840dff83e8264ecf986ca")  // LINK
            || addr_hex.contains("7fc66500c84a76ad7e9c93437bfc5ac33e2ddae9")  // AAVE
            || addr_hex.contains("9f8f72aa9304c8b593d555f12ef6589cc3a579a2")  // MKR
            || addr_hex.contains("c011a73ee8576fb46f5e1c5751ca3b9fe0af2a6f")  // SNX
            || addr_hex.contains("c00e94cb662c3520282e6f5717214004a7f26888")  // COMP
            || addr_hex.contains("7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0")  // wstETH
            || addr_hex.contains("ae7ab96520de3a18e5e111b5eaab095312d7fe84")  // stETH
        {
            return LiquidityTier::MidCap;
        }
        
        // Long-tail tokens (meme coins, smaller DeFi)
        if addr_hex.contains("1f9840a85d5af5bf1d1762f925bdaddc4201f984")  // UNI
            || addr_hex.contains("6982508145454ce325ddbe47a25d4ec3d2311933")  // PEPE
            || addr_hex.contains("95ad61b0a150d79219dcf64e1e6cc01f0b64c4ce")  // SHIB
            || addr_hex.contains("5a98fcbea516cf06857215779fd812ca3bef1b32")  // LDO
            || addr_hex.contains("d533a949740bb3306d119cc777fa900ba034cd52")  // CRV
            || addr_hex.contains("4d224452801aced8b2f0aebe155379bb5d594381")  // APE
            || addr_hex.contains("5283d291dbcf85356a21ba090e6db59121208b44")  // BLUR
        {
            return LiquidityTier::LongTail;
        }
        
        LiquidityTier::Unknown
    }
    
    /// Get the minimum liquidity tier for a cycle (bottleneck)
    pub fn get_cycle_liquidity_tier(&self, cycle: &ArbitrageCycle) -> LiquidityTier {
        let mut min_tier = LiquidityTier::Major;
        
        for token in &cycle.path {
            let tier = self.get_liquidity_tier(token);
            // Use the most restrictive tier
            match (&min_tier, &tier) {
                (LiquidityTier::Major, t) => min_tier = *t,
                (LiquidityTier::MidCap, LiquidityTier::LongTail | LiquidityTier::Unknown) => min_tier = tier,
                (LiquidityTier::LongTail, LiquidityTier::Unknown) => min_tier = tier,
                _ => {}
            }
        }
        
        min_tier
    }
    
    /// Get appropriate simulation input amount for a token (in USD equivalent)
    pub fn get_simulation_amount(&self, token: Address, target_usd: f64) -> U256 {
        let decimals = get_token_decimals(&token);
        let addr_hex = format!("{:?}", token).to_lowercase();
        
        // Determine the token's USD price
        let token_price_usd = if addr_hex.contains("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2") {
            self.eth_price_usd
        } else if addr_hex.contains("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48") 
               || addr_hex.contains("dac17f958d2ee523a2206206994597c13d831ec7")
               || addr_hex.contains("6b175474e89094c44da98b954eedcdecb5be3830") {
            1.0 // Stablecoins
        } else if addr_hex.contains("2260fac5e5542a773aa44fbcfedf7c193bc2c599") {
            95000.0 // WBTC
        } else if addr_hex.contains("7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0") {
            self.eth_price_usd * 1.15 // wstETH
        } else {
            self.eth_price_usd // Default to ETH price for unknown tokens
        };
        
        let amount_float = (target_usd / token_price_usd) * 10_f64.powi(decimals as i32);
        
        if amount_float > 1e30 {
            warn!("Calculated amount too large, capping at 1e30");
            U256::from(10u128.pow(30))
        } else {
            U256::from(amount_float as u128)
        }
    }
    
    /// Simulate a single V3 swap
    pub async fn simulate_v3_swap(
        &self,
        pool: Address,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
        fee: u32,
        dex: Dex,
    ) -> Result<SwapResult> {
        let quote = self.quoter.quote_v3(pool, token_in, token_out, amount_in, fee).await?;
        
        let gas_used = quote.gas_estimate.min(MAX_GAS_PER_SWAP);
        
        if quote.gas_estimate > MAX_GAS_PER_SWAP {
            warn!(
                "Gas estimate {} exceeded cap, using {} for pool {:?}",
                quote.gas_estimate, MAX_GAS_PER_SWAP, pool
            );
        }
        
        Ok(SwapResult {
            pool,
            token_in,
            token_out,
            amount_in: quote.amount_in,
            amount_out: quote.amount_out,
            gas_used,
            dex,
        })
    }
    
    /// Simulate a single V2 swap
    pub async fn simulate_v2_swap(
        &self,
        pool: Address,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
        dex: Dex,
    ) -> Result<SwapResult> {
        let quote = self.quoter.quote_v2(pool, token_in, amount_in).await?;
        
        Ok(SwapResult {
            pool,
            token_in,
            token_out,
            amount_in: quote.amount_in,
            amount_out: quote.amount_out,
            gas_used: quote.gas_estimate.min(MAX_GAS_PER_SWAP),
            dex,
        })
    }
    
    /// Simulate a full arbitrage cycle with DYNAMIC sizing based on liquidity
    pub async fn simulate_cycle(
        &self,
        cycle: &ArbitrageCycle,
        max_input_usd: f64,
    ) -> ArbitrageSimulation {
        // Determine liquidity tier and appropriate size
        let liquidity_tier = self.get_cycle_liquidity_tier(cycle);
        let target_usd = liquidity_tier.recommended_amount_usd().min(max_input_usd);
        
        let start_token = cycle.path[0];
        let input_amount = self.get_simulation_amount(start_token, target_usd);
        let token_decimals = get_token_decimals(&start_token);
        
        let mut swaps = Vec::new();
        let mut current_amount = input_amount;
        let mut total_gas: u64 = 50_000; // Base overhead for flash loan
        let mut last_error: Option<String> = None;
        
        debug!(
            "Simulating cycle with {} hops, input: {} ({:?} tier, ${:.0})", 
            cycle.hop_count(), input_amount, liquidity_tier, target_usd
        );
        
        for i in 0..cycle.pools.len() {
            let pool = cycle.pools[i];
            let token_in = cycle.path[i];
            let token_out = cycle.path[i + 1];
            let dex = cycle.dexes[i];
            let fee = cycle.fees[i];
            
            let result = match dex {
                Dex::UniswapV3 | Dex::SushiswapV3 | Dex::PancakeSwapV3 => {
                    self.simulate_v3_swap(pool, token_in, token_out, current_amount, fee, dex).await
                }
                Dex::UniswapV2 | Dex::SushiswapV2 => {
                    self.simulate_v2_swap(pool, token_in, token_out, current_amount, dex).await
                }
                Dex::BalancerV2 | Dex::Curve => {
                    self.simulate_v2_swap(pool, token_in, token_out, current_amount, dex).await
                }
            };
            
            match result {
                Ok(swap) => {
                    total_gas += swap.gas_used;
                    current_amount = swap.amount_out;
                    swaps.push(swap);
                }
                Err(e) => {
                    last_error = Some(format!("Swap {} failed: {}", i + 1, e));
                    break;
                }
            }
        }
        
        let simulation_success = last_error.is_none() && swaps.len() == cycle.pools.len();
        
        // Calculate gas cost in ETH then USD
        let gas_cost_wei = U256::from((self.gas_price_gwei * 1e9) as u128) * U256::from(total_gas);
        let gas_cost_eth = (total_gas as f64) * self.gas_price_gwei * 1e-9;
        let gas_cost_usd = gas_cost_eth * self.eth_price_usd;
        
        // Calculate profit
        let (profit_usd, profit_wei) = if simulation_success {
            let output_i128: i128 = current_amount.to_string().parse().unwrap_or(0);
            let input_i128: i128 = input_amount.to_string().parse().unwrap_or(0);
            let profit_in_token = output_i128 - input_i128;
            
            // Convert to USD
            let decimal_factor = 10_f64.powi(token_decimals as i32);
            let profit_tokens = profit_in_token as f64 / decimal_factor;
            
            let addr_hex = format!("{:?}", start_token).to_lowercase();
            let token_price = if addr_hex.contains("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2") {
                self.eth_price_usd
            } else if addr_hex.contains("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")
                   || addr_hex.contains("dac17f958d2ee523a2206206994597c13d831ec7")
                   || addr_hex.contains("6b175474e89094c44da98b954eedcdecb5be3830") {
                1.0
            } else if addr_hex.contains("2260fac5e5542a773aa44fbcfedf7c193bc2c599") {
                95000.0
            } else {
                self.eth_price_usd
            };
            
            let gross_profit_usd = profit_tokens * token_price;
            let net_profit_usd = gross_profit_usd - gas_cost_usd;
            
            // Convert net profit to wei equivalent
            let net_profit_eth = net_profit_usd / self.eth_price_usd;
            let profit_wei = (net_profit_eth * 1e18) as i128;
            
            (net_profit_usd, profit_wei)
        } else {
            (f64::NEG_INFINITY, i128::MIN)
        };
        
        let is_profitable = simulation_success && profit_usd > 0.0;
        
        ArbitrageSimulation {
            cycle: cycle.clone(),
            swaps,
            input_amount,
            output_amount: current_amount,
            total_gas_used: total_gas,
            gas_cost_wei,
            profit_wei,
            profit_usd,
            is_profitable,
            simulation_success,
            revert_reason: last_error,
            token_decimals,
            liquidity_tier,
            input_usd: target_usd,
        }
    }
    
    /// Simulate with multiple size tiers to find optimal amount
    pub async fn simulate_cycle_optimized(
        &self,
        cycle: &ArbitrageCycle,
    ) -> ArbitrageSimulation {
        let base_tier = self.get_cycle_liquidity_tier(cycle);
        let base_amount = base_tier.recommended_amount_usd();
        
        // Try the recommended amount first
        let base_sim = self.simulate_cycle(cycle, base_amount).await;
        
        // If profitable, try scaling up
        if base_sim.is_profitable && base_sim.profit_usd > 5.0 {
            let scaled_sim = self.simulate_cycle(cycle, base_amount * 2.0).await;
            if scaled_sim.is_profitable && scaled_sim.profit_usd > base_sim.profit_usd {
                return scaled_sim;
            }
        }
        
        // If unprofitable with high slippage, try smaller amount
        if !base_sim.is_profitable && base_sim.simulation_success {
            let small_sim = self.simulate_cycle(cycle, base_amount * 0.5).await;
            if small_sim.is_profitable {
                return small_sim;
            }
        }
        
        base_sim
    }
}