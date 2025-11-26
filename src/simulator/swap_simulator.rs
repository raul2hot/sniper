//! Swap Simulator - Full Arbitrage Simulation (FIXED)
//!
//! Simulates complete arbitrage cycles through multiple DEXes
//! using Provider's eth_call and calculates actual profits after gas costs.
//!
//! KEY FIX: Uses token-aware input amounts (not 0.1 ETH for everything!)

use alloy_primitives::{Address, U256};
use alloy_provider::{Provider, ProviderBuilder};
use eyre::Result;
use tracing::{debug, info, warn};

use super::UniV3Quoter;
use crate::brain::ArbitrageCycle;
use crate::cartographer::{Dex, get_token_decimals};

/// Maximum gas estimate per swap to prevent unrealistic values
const MAX_GAS_PER_SWAP: u64 = 500_000;

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
    /// The token decimals for proper profit calculation
    pub token_decimals: u8,
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
    
    /// Get appropriate simulation input amount for a token (in USD equivalent)
    /// Returns the amount in the token's smallest units
    pub fn get_simulation_amount(&self, token: Address, target_usd: f64) -> U256 {
        let decimals = get_token_decimals(&token);
        let addr_hex = format!("{:?}", token).to_lowercase();
        
        // Determine the token's USD price
        let token_price_usd = if addr_hex.contains("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2") {
            // WETH
            self.eth_price_usd
        } else if addr_hex.contains("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48") 
               || addr_hex.contains("dac17f958d2ee523a2206206994597c13d831ec7")
               || addr_hex.contains("6b175474e89094c44da98b954eedcdecb5be3830") {
            // USDC, USDT, DAI - stablecoins
            1.0
        } else if addr_hex.contains("2260fac5e5542a773aa44fbcfedf7c193bc2c599") {
            // WBTC - approximate price
            95000.0
        } else if addr_hex.contains("7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0") {
            // wstETH - slightly higher than ETH
            self.eth_price_usd * 1.15
        } else {
            // Default: assume 18 decimals, use ETH price as reference
            self.eth_price_usd
        };
        
        // Calculate the amount in token units
        // amount = target_usd / token_price * 10^decimals
        let amount_float = (target_usd / token_price_usd) * 10_f64.powi(decimals as i32);
        
        // Convert to U256, handling potential overflow
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
        
        // Cap gas estimate to prevent unrealistic values
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
    
    /// Simulate a full arbitrage cycle with token-aware input amounts
    pub async fn simulate_cycle(
        &self,
        cycle: &ArbitrageCycle,
        target_usd: f64,
    ) -> ArbitrageSimulation {
        // Get the starting token and calculate appropriate input amount
        let start_token = cycle.path[0];
        let input_amount = self.get_simulation_amount(start_token, target_usd);
        let token_decimals = get_token_decimals(&start_token);
        
        let mut swaps = Vec::new();
        let mut current_amount = input_amount;
        let mut total_gas: u64 = 50_000; // Base overhead for flash loan
        let mut last_error: Option<String> = None;
        
        debug!(
            "Simulating cycle with {} hops, input: {} (decimals: {})", 
            cycle.hop_count(), input_amount, token_decimals
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
        
        // Calculate gas cost in ETH terms
        let gas_cost_wei = U256::from((self.gas_price_gwei * 1e9) as u128) * U256::from(total_gas);
        
        // Calculate profit in the starting token's units
        let profit_in_token: i128 = if simulation_success {
            let output_i128: i128 = current_amount.to_string().parse().unwrap_or(0);
            let input_i128: i128 = input_amount.to_string().parse().unwrap_or(0);
            output_i128 - input_i128
        } else {
            i128::MIN
        };
        
        // Convert token profit to USD
        let profit_usd = if simulation_success {
            let decimal_factor = 10_f64.powi(token_decimals as i32);
            let profit_tokens = profit_in_token as f64 / decimal_factor;
            
            // Get token USD price (same logic as get_simulation_amount)
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
            
            profit_tokens * token_price
        } else {
            f64::NEG_INFINITY
        };
        
        // Subtract gas cost (convert gas from ETH to USD)
        let gas_cost_eth = gas_cost_wei.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
        let gas_cost_usd = gas_cost_eth * self.eth_price_usd;
        let net_profit_usd = profit_usd - gas_cost_usd;
        
        let is_profitable = simulation_success && net_profit_usd > 0.0;
        
        // Convert for legacy field (approximate)
        let profit_wei: i128 = if simulation_success {
            let net_profit_eth = net_profit_usd / self.eth_price_usd;
            (net_profit_eth * 1e18) as i128
        } else {
            i128::MIN
        };
        
        ArbitrageSimulation {
            cycle: cycle.clone(),
            swaps,
            input_amount,
            output_amount: current_amount,
            total_gas_used: total_gas,
            gas_cost_wei,
            profit_wei,
            profit_usd: net_profit_usd,
            is_profitable,
            simulation_success,
            revert_reason: last_error,
            token_decimals,
        }
    }
}
