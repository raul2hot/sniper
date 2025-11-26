//! Phase 4: The Executor
//!
//! This module handles the actual execution of arbitrage opportunities:
//! - Flash Loan acquisition (Balancer V2 / Aave V3)
//! - Flashbots bundle submission (private, no failed tx costs)
//! - Profit withdrawal and accounting
//!
//! ⚠️  WARNING: This module interacts with real funds!
//! Always test on Goerli testnet first.

mod flash_loan;
mod flashbots;

pub use flash_loan::{
    FlashLoanBuilder,
    FlashLoanTransaction,
    DexType,
    get_executor_contract_source,
};

pub use flashbots::{
    FlashbotsClient,
    FlashbotsBundle,
    BundleBuilder,
    BundleResponse,
    SimulationResult,
    SubmissionStrategy,
    FlashbotsEndpoints,
    submit_arbitrage_bundle,
};

use alloy_primitives::{Address, Bytes, U256};
use eyre::Result;
use tracing::{info, warn, error};

use crate::brain::ArbitrageCycle;
use crate::config::{Config, ExecutionMode};
use crate::simulator::SwapSimulator;

/// The main execution engine
pub struct ExecutionEngine {
    config: Config,
    flash_loan_builder: FlashLoanBuilder,
    flashbots_client: FlashbotsClient,
    bundle_builder: BundleBuilder,
}

impl ExecutionEngine {
    /// Create a new execution engine
    pub fn new(config: Config) -> Self {
        Self {
            flash_loan_builder: FlashLoanBuilder::new(&config),
            flashbots_client: FlashbotsClient::new(&config),
            bundle_builder: BundleBuilder::new(&config),
            config,
        }
    }
    
    /// Check if the engine is ready for production
    pub fn is_production_ready(&self) -> bool {
        self.config.execution_mode == ExecutionMode::Production
            && self.flashbots_client.has_signer()
            && self.config.executor_contract_address.is_some()
            && self.config.profit_wallet_address.is_some()
    }
    
    /// Execute an arbitrage opportunity
    pub async fn execute(
        &self,
        cycle: &ArbitrageCycle,
        simulation: &crate::simulator::swap_simulator::ArbitrageSimulation,
        current_block: u64,
    ) -> Result<ExecutionResult> {
        // Safety checks
        if self.config.emergency_stop {
            return Ok(ExecutionResult::Aborted {
                reason: "Emergency stop is active".to_string(),
            });
        }
        
        if !simulation.is_profitable {
            return Ok(ExecutionResult::Skipped {
                reason: "Simulation shows unprofitable".to_string(),
            });
        }
        
        // Calculate input amount (flash loan amount)
        let input_amount = simulation.input_amount;
        
        // Calculate minimum output (must cover: loan + fee + min_profit)
        let min_profit_wei = U256::from((self.config.min_profit_usd * 1e18 / 3500.0) as u128);
        let min_output = self.flash_loan_builder.calculate_min_output(input_amount, min_profit_wei);
        
        // Build the flash loan transaction
        let flash_loan_tx = self.flash_loan_builder.build_flash_loan_tx(
            cycle,
            input_amount,
            min_output,
        )?;
        
        match self.config.execution_mode {
            ExecutionMode::Simulation => {
                info!("📋 SIMULATION MODE: Would execute arbitrage");
                info!("   Path: {:?}", cycle.path);
                info!("   Input: {} wei", input_amount);
                info!("   Min output: {} wei", min_output);
                info!("   Expected profit: ${:.2}", simulation.profit_usd);
                
                // Log the opportunity if enabled
                if self.config.simulation_log {
                    self.log_opportunity(cycle, simulation)?;
                }
                
                Ok(ExecutionResult::Simulated {
                    expected_profit_usd: simulation.profit_usd,
                    would_execute: true,
                })
            }
            
            ExecutionMode::DryRun => {
                info!("🔬 DRY RUN MODE: Building and simulating bundle...");
                
                // Build a mock signed transaction (in production, this would be properly signed)
                let mock_signed_tx = flash_loan_tx.calldata.clone();
                
                // Build the bundle
                let bundle = self.bundle_builder.build_bundle(
                    &flash_loan_tx,
                    mock_signed_tx,
                    current_block + 1,
                    U256::from((simulation.profit_usd * 1e18 / 3500.0) as u128),
                )?;
                
                // Simulate with Flashbots
                if self.flashbots_client.has_signer() {
                    match self.flashbots_client.simulate_bundle(&bundle).await {
                        Ok(sim_result) => {
                            if sim_result.success {
                                info!("✅ Bundle simulation passed!");
                                info!("   Gas used: {:?}", sim_result.gas_used);
                                info!("   Coinbase diff: {:?}", sim_result.coinbase_diff);
                                
                                Ok(ExecutionResult::DryRun {
                                    simulation_passed: true,
                                    gas_used: sim_result.gas_used,
                                    coinbase_diff: sim_result.coinbase_diff,
                                })
                            } else {
                                warn!("❌ Bundle simulation failed: {:?}", sim_result.error);
                                
                                Ok(ExecutionResult::DryRun {
                                    simulation_passed: false,
                                    gas_used: None,
                                    coinbase_diff: None,
                                })
                            }
                        }
                        Err(e) => {
                            error!("Failed to simulate bundle: {}", e);
                            Ok(ExecutionResult::Failed {
                                reason: format!("Simulation error: {}", e),
                            })
                        }
                    }
                } else {
                    warn!("No Flashbots signer key - skipping bundle simulation");
                    Ok(ExecutionResult::Skipped {
                        reason: "No Flashbots signer key configured".to_string(),
                    })
                }
            }
            
            ExecutionMode::Production => {
                if !self.is_production_ready() {
                    return Ok(ExecutionResult::Aborted {
                        reason: "Production requirements not met (check wallet/executor config)".to_string(),
                    });
                }
                
                info!("🚀 PRODUCTION MODE: Executing arbitrage!");
                warn!("⚠️  This will use real funds!");
                
                // TODO: Implement proper transaction signing
                // For now, we abort with an error to prevent accidental execution
                
                error!("Production execution not yet fully implemented");
                error!("Need to implement:");
                error!("  1. Proper transaction signing with alloy");
                error!("  2. Nonce management");
                error!("  3. Gas price oracle integration");
                
                Ok(ExecutionResult::Aborted {
                    reason: "Production signing not yet implemented - safety abort".to_string(),
                })
            }
        }
    }
    
    /// Log a profitable opportunity to file
    fn log_opportunity(
        &self,
        cycle: &ArbitrageCycle,
        simulation: &crate::simulator::swap_simulator::ArbitrageSimulation,
    ) -> Result<()> {
        use crate::config::OpportunityLog;
        use chrono::Utc;
        
        let log = OpportunityLog {
            timestamp: Utc::now(),
            path: cycle.path.iter().map(|a| format!("{:?}", a)).collect(),
            dexes: cycle.dexes.iter().map(|d| d.to_string()).collect(),
            input_usd: self.config.default_simulation_usd,
            gross_profit_usd: simulation.profit_usd + (simulation.total_gas_used as f64 * 20.0 * 1e-9 * 3500.0),
            gas_cost_usd: simulation.total_gas_used as f64 * 20.0 * 1e-9 * 3500.0,
            net_profit_usd: simulation.profit_usd,
            gas_price_gwei: 20.0,
            eth_price_usd: 3500.0,
            block_number: 0, // Would be filled from provider
        };
        
        log.append_to_file(&self.config.simulation_log_path)?;
        
        info!("📝 Logged opportunity to {}", self.config.simulation_log_path);
        
        Ok(())
    }
}

/// Result of an execution attempt
#[derive(Debug)]
pub enum ExecutionResult {
    /// Simulated successfully (simulation mode)
    Simulated {
        expected_profit_usd: f64,
        would_execute: bool,
    },
    
    /// Dry run completed
    DryRun {
        simulation_passed: bool,
        gas_used: Option<u64>,
        coinbase_diff: Option<String>,
    },
    
    /// Bundle submitted (production mode)
    Submitted {
        bundle_hash: String,
        target_block: u64,
        expected_profit_usd: f64,
    },
    
    /// Bundle included in block!
    Included {
        bundle_hash: String,
        block_number: u64,
        actual_profit_wei: U256,
    },
    
    /// Skipped (not profitable or other reason)
    Skipped {
        reason: String,
    },
    
    /// Aborted (safety/config issue)
    Aborted {
        reason: String,
    },
    
    /// Failed
    Failed {
        reason: String,
    },
}

impl ExecutionResult {
    pub fn is_success(&self) -> bool {
        matches!(self, 
            ExecutionResult::Simulated { would_execute: true, .. } |
            ExecutionResult::DryRun { simulation_passed: true, .. } |
            ExecutionResult::Submitted { .. } |
            ExecutionResult::Included { .. }
        )
    }
}
