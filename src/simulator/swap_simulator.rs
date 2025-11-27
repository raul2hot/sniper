//! Swap Simulator - Enhanced Logging Edition
//!
//! CHANGES:
//! - More verbose logging even for failed simulations
//! - Better error messages
//! - Progress indicators

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
const MIN_GAS_PRICE_GWEI: f64 = 0.01;

/// Default gas price if we can't fetch it
const DEFAULT_GAS_PRICE_GWEI: f64 = 0.5;

/// Token liquidity tiers for dynamic sizing
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LiquidityTier {
    Major,
    MidCap,
    LongTail,
    Unknown,
}

impl LiquidityTier {
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
    pub liquidity_tier: LiquidityTier,
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
        debug!("Initializing SwapSimulator...");
        
        let provider = ProviderBuilder::new()
            .on_http(rpc_url.parse()?);
        
        let block_number = provider.get_block_number().await?;
        debug!("Connected at block: {}", block_number);
        
        let gas_price_gwei = match provider.get_gas_price().await {
            Ok(gas_price_wei) => {
                let gwei = gas_price_wei as f64 / 1e9;
                gwei.max(MIN_GAS_PRICE_GWEI).min(1000.0)
            }
            Err(_) => DEFAULT_GAS_PRICE_GWEI,
        };
        
        debug!("Gas price: {:.2} gwei", gas_price_gwei);
        
        let quoter = UniV3Quoter::new(rpc_url.to_string());
        
        Ok(Self {
            rpc_url: rpc_url.to_string(),
            quoter,
            gas_price_gwei,
            eth_price_usd: 3500.0,
        })
    }
    
    pub fn set_eth_price(&mut self, eth_price_usd: f64) {
        self.eth_price_usd = eth_price_usd;
    }
    
    pub fn set_gas_price(&mut self, gas_price_gwei: f64) {
        self.gas_price_gwei = gas_price_gwei.max(MIN_GAS_PRICE_GWEI);
    }
    
    pub fn gas_price_gwei(&self) -> f64 {
        self.gas_price_gwei
    }
    
    pub fn get_liquidity_tier(&self, token: &Address) -> LiquidityTier {
        let addr_hex = format!("{:?}", token).to_lowercase();
        
        // Major tokens
        if addr_hex.contains("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2")  // WETH
            || addr_hex.contains("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")  // USDC
            || addr_hex.contains("dac17f958d2ee523a2206206994597c13d831ec7")  // USDT
            || addr_hex.contains("6b175474e89094c44da98b954eedcdecb5be3830")  // DAI
            || addr_hex.contains("2260fac5e5542a773aa44fbcfedf7c193bc2c599")  // WBTC
        {
            return LiquidityTier::Major;
        }
        
        // Mid-cap
        if addr_hex.contains("514910771af9ca656af840dff83e8264ecf986ca")  // LINK
            || addr_hex.contains("7fc66500c84a76ad7e9c93437bfc5ac33e2ddae9")  // AAVE
            || addr_hex.contains("9f8f72aa9304c8b593d555f12ef6589cc3a579a2")  // MKR
            || addr_hex.contains("7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0")  // wstETH
        {
            return LiquidityTier::MidCap;
        }
        
        // Long-tail
        if addr_hex.contains("1f9840a85d5af5bf1d1762f925bdaddc4201f984")  // UNI
            || addr_hex.contains("6982508145454ce325ddbe47a25d4ec3d2311933")  // PEPE
            || addr_hex.contains("95ad61b0a150d79219dcf64e1e6cc01f0b64c4ce")  // SHIB
        {
            return LiquidityTier::LongTail;
        }
        
        LiquidityTier::Unknown
    }
    
    pub fn get_cycle_liquidity_tier(&self, cycle: &ArbitrageCycle) -> LiquidityTier {
        let mut min_tier = LiquidityTier::Major;
        
        for token in &cycle.path {
            let tier = self.get_liquidity_tier(token);
            match (&min_tier, &tier) {
                (LiquidityTier::Major, t) => min_tier = *t,
                (LiquidityTier::MidCap, LiquidityTier::LongTail | LiquidityTier::Unknown) => min_tier = tier,
                (LiquidityTier::LongTail, LiquidityTier::Unknown) => min_tier = tier,
                _ => {}
            }
        }
        
        min_tier
    }
    
    pub fn get_simulation_amount(&self, token: Address, target_usd: f64) -> U256 {
        let decimals = get_token_decimals(&token);
        let addr_hex = format!("{:?}", token).to_lowercase();
        
        let token_price_usd = if addr_hex.contains("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2") {
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
        
        let amount_float = (target_usd / token_price_usd) * 10_f64.powi(decimals as i32);
        
        if amount_float > 1e30 {
            U256::from(10u128.pow(30))
        } else {
            U256::from(amount_float as u128)
        }
    }
    
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
    
    /// Simulate a full arbitrage cycle
    pub async fn simulate_cycle(
        &self,
        cycle: &ArbitrageCycle,
        max_input_usd: f64,
    ) -> ArbitrageSimulation {
        let liquidity_tier = self.get_cycle_liquidity_tier(cycle);
        let target_usd = liquidity_tier.recommended_amount_usd().min(max_input_usd);
        
        let start_token = cycle.path[0];
        let input_amount = self.get_simulation_amount(start_token, target_usd);
        let token_decimals = get_token_decimals(&start_token);
        
        let mut swaps = Vec::new();
        let mut current_amount = input_amount;
        let mut total_gas: u64 = 50_000; // Base overhead
        let mut last_error: Option<String> = None;
        
        debug!(
            "Simulating {}-hop cycle, input: {} wei (${:.0})", 
            cycle.hop_count(), input_amount, target_usd
        );
        
        for i in 0..cycle.pools.len() {
            let pool = cycle.pools[i];
            let token_in = cycle.path[i];
            let token_out = cycle.path[i + 1];
            let dex = cycle.dexes[i];
            let fee = cycle.fees[i];
            
            debug!("  Swap {}: {} -> {} via {:?} ({})", i + 1, token_in, token_out, pool, dex);
            
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
                    debug!("    ✓ Out: {} (gas: {})", swap.amount_out, swap.gas_used);
                    total_gas += swap.gas_used;
                    current_amount = swap.amount_out;
                    swaps.push(swap);
                }
                Err(e) => {
                    let err_msg = format!("Swap {} failed: {}", i + 1, e);
                    debug!("    ✗ {}", err_msg);
                    last_error = Some(err_msg);
                    break;
                }
            }
        }
        
        let simulation_success = last_error.is_none() && swaps.len() == cycle.pools.len();
        
        // Calculate gas cost
        let gas_cost_wei = U256::from((self.gas_price_gwei * 1e9) as u128) * U256::from(total_gas);
        let gas_cost_eth = (total_gas as f64) * self.gas_price_gwei * 1e-9;
        let gas_cost_usd = gas_cost_eth * self.eth_price_usd;
        
        // Calculate profit
        let (profit_usd, profit_wei) = if simulation_success {
            let output_i128: i128 = current_amount.to_string().parse().unwrap_or(0);
            let input_i128: i128 = input_amount.to_string().parse().unwrap_or(0);
            let profit_in_token = output_i128 - input_i128;
            
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
}
