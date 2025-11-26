//! Swap Simulator - Full Arbitrage Simulation
//!
//! Simulates complete arbitrage cycles through multiple DEXes
//! using Provider's eth_call and calculates actual profits after gas costs.

use alloy_primitives::{Address, U256};
use alloy_provider::{Provider, ProviderBuilder};
use eyre::Result;
use tracing::{debug, info};

use super::UniV3Quoter;
use crate::brain::ArbitrageCycle;
use crate::cartographer::Dex;

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
}

impl ArbitrageSimulation {
    /// Calculate the return multiplier (output/input)
    pub fn return_multiplier(&self) -> f64 {
        if self.input_amount == U256::ZERO {
            return 0.0;
        }
        let input_f64: f64 = self.input_amount.to_string().parse().unwrap_or(0.0);
        let output_f64: f64 = self.output_amount.to_string().parse().unwrap_or(0.0);
        output_f64 / input_f64
    }
    
    /// Calculate gross profit percentage (before gas)
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
        
        // Get gas price
        let gas_price = provider.get_gas_price().await?;
        let gas_price_gwei = gas_price as f64 / 1e9;
        info!("Current gas price: {:.2} gwei", gas_price_gwei);
        
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
        self.gas_price_gwei = gas_price_gwei;
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
        
        Ok(SwapResult {
            pool,
            token_in,
            token_out,
            amount_in: quote.amount_in,
            amount_out: quote.amount_out,
            gas_used: quote.gas_estimate,
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
            gas_used: quote.gas_estimate,
            dex,
        })
    }
    
    /// Simulate a full arbitrage cycle
    pub async fn simulate_cycle(
        &self,
        cycle: &ArbitrageCycle,
        input_amount: U256,
    ) -> ArbitrageSimulation {
        let mut swaps = Vec::new();
        let mut current_amount = input_amount;
        let mut total_gas: u64 = 50_000; // Base overhead for flash loan
        let mut last_error: Option<String> = None;
        
        debug!("Simulating cycle with {} hops, input: {}", cycle.hop_count(), input_amount);
        
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
                    // For Balancer/Curve, use V2-style quote (simplified)
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
        
        // Calculate profit
        let gas_cost_wei = U256::from((self.gas_price_gwei * 1e9) as u128) * U256::from(total_gas);
        
        let profit_wei: i128 = if simulation_success {
            let output_i128: i128 = current_amount.to_string().parse().unwrap_or(0);
            let input_i128: i128 = input_amount.to_string().parse().unwrap_or(0);
            let gas_i128: i128 = gas_cost_wei.to_string().parse().unwrap_or(0);
            output_i128 - input_i128 - gas_i128
        } else {
            i128::MIN
        };
        
        // Convert to USD
        let profit_eth = profit_wei as f64 / 1e18;
        let profit_usd = profit_eth * self.eth_price_usd;
        
        let is_profitable = profit_wei > 0;
        
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
        }
    }
}
