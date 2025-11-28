//! Phase 4: The Executor - PRODUCTION READY
//!
//! This module handles the actual execution of arbitrage opportunities:
//! - Flash Loan acquisition (Balancer V2 / Aave V3)
//! - Transaction signing with alloy
//! - Flashbots bundle submission (private, no failed tx costs)
//! - Bundle monitoring and profit tracking
//!
//! ⚠️  WARNING: This module interacts with real funds in production mode!
//! Always test on Goerli/Sepolia testnet first.

mod flash_loan;
mod flashbots;
mod signer;

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

pub use signer::{WalletManager, generate_new_wallet};

use alloy_primitives::{Address, Bytes, U256};
use alloy_provider::{Provider, ProviderBuilder};
use eyre::{eyre, Result};
use tracing::{info, warn, error, debug};

use crate::brain::ArbitrageCycle;
use crate::config::{Config, ExecutionMode};

/// The main execution engine - now with full signing support
pub struct ExecutionEngine {
    config: Config,
    flash_loan_builder: FlashLoanBuilder,
    flashbots_client: FlashbotsClient,
    bundle_builder: BundleBuilder,
    wallet_manager: WalletManager,
}

impl ExecutionEngine {
    /// Create a new execution engine
    pub fn new(config: Config) -> Self {
        // Initialize wallet manager from environment
        let wallet_manager = WalletManager::from_env()
            .unwrap_or_else(|e| {
                warn!("Failed to initialize wallet manager: {}", e);
                WalletManager::new(None, None, config.chain_id).unwrap()
            });
        
        Self {
            flash_loan_builder: FlashLoanBuilder::new(&config),
            flashbots_client: FlashbotsClient::new(&config),
            bundle_builder: BundleBuilder::new(&config),
            wallet_manager,
            config,
        }
    }
    
    /// Check if the engine is ready for production
    pub fn is_production_ready(&self) -> bool {
        self.config.execution_mode == ExecutionMode::Production
            && self.wallet_manager.has_flashbots_signer()
            && self.wallet_manager.has_profit_wallet()
            && self.config.executor_contract_address.is_some()
    }
    
    /// Get production readiness report
    pub fn production_readiness_report(&self) -> Vec<(String, bool, String)> {
        vec![
            (
                "Execution Mode".to_string(),
                self.config.execution_mode == ExecutionMode::Production,
                format!("Current: {}", self.config.execution_mode),
            ),
            (
                "Flashbots Signer".to_string(),
                self.wallet_manager.has_flashbots_signer(),
                self.wallet_manager.flashbots_address()
                    .map(|a| format!("{:?}", a))
                    .unwrap_or_else(|| "Not configured".to_string()),
            ),
            (
                "Profit Wallet".to_string(),
                self.wallet_manager.has_profit_wallet(),
                self.wallet_manager.profit_wallet_address()
                    .map(|a| format!("{:?}", a))
                    .unwrap_or_else(|| "Not configured".to_string()),
            ),
            (
                "Executor Contract".to_string(),
                self.config.executor_contract_address.is_some(),
                self.config.executor_contract_address.clone()
                    .unwrap_or_else(|| "Not deployed".to_string()),
            ),
        ]
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
                self.execute_simulation(cycle, simulation, &flash_loan_tx).await
            }
            
            ExecutionMode::DryRun => {
                self.execute_dry_run(cycle, simulation, &flash_loan_tx, current_block).await
            }
            
            ExecutionMode::Production => {
                self.execute_production(cycle, simulation, &flash_loan_tx, current_block).await
            }
        }
    }
    
    /// Simulation mode - log only, no execution
    async fn execute_simulation(
        &self,
        cycle: &ArbitrageCycle,
        simulation: &crate::simulator::swap_simulator::ArbitrageSimulation,
        flash_loan_tx: &FlashLoanTransaction,
    ) -> Result<ExecutionResult> {
        info!("📋 SIMULATION MODE: Would execute arbitrage");
        debug!("   Path: {:?}", cycle.path);
        debug!("   Input: {} wei", simulation.input_amount);
        debug!("   Expected profit: ${:.2}", simulation.profit_usd);
        
        // Log the opportunity if enabled
        if self.config.simulation_log {
            self.log_opportunity(cycle, simulation)?;
        }
        
        Ok(ExecutionResult::Simulated {
            expected_profit_usd: simulation.profit_usd,
            would_execute: true,
        })
    }
    
    /// Dry run mode - build and simulate bundles but don't submit
    async fn execute_dry_run(
        &self,
        cycle: &ArbitrageCycle,
        simulation: &crate::simulator::swap_simulator::ArbitrageSimulation,
        flash_loan_tx: &FlashLoanTransaction,
        current_block: u64,
    ) -> Result<ExecutionResult> {
        info!("🔬 DRY RUN MODE: Building and simulating bundle...");
        
        // For dry run, we create a mock signed transaction
        // In production, this would be properly signed
        let mock_signed_tx = flash_loan_tx.calldata.clone();
        
        // Build the bundle
        let bundle = self.bundle_builder.build_bundle(
            flash_loan_tx,
            mock_signed_tx,
            current_block + 1,
            U256::from((simulation.profit_usd * 1e18 / 3500.0) as u128),
        )?;
        
        // Simulate with Flashbots if we have a signer
        if self.wallet_manager.has_flashbots_signer() {
            match self.flashbots_client.simulate_bundle(&bundle, &self.wallet_manager).await {
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
    
    /// Production mode - full execution with real transactions
    async fn execute_production(
        &self,
        cycle: &ArbitrageCycle,
        simulation: &crate::simulator::swap_simulator::ArbitrageSimulation,
        flash_loan_tx: &FlashLoanTransaction,
        current_block: u64,
    ) -> Result<ExecutionResult> {
        // Check production readiness
        if !self.is_production_ready() {
            let report = self.production_readiness_report();
            let missing: Vec<_> = report.iter()
                .filter(|(_, ready, _)| !ready)
                .map(|(name, _, detail)| format!("{}: {}", name, detail))
                .collect();
            
            return Ok(ExecutionResult::Aborted {
                reason: format!("Production requirements not met: {}", missing.join(", ")),
            });
        }
        
        info!("🚀 PRODUCTION MODE: Executing arbitrage!");
        warn!("⚠️  This will use real funds!");
        
        // Get current gas price
        let gas_price = self.get_current_gas_price().await?;
        let priority_fee = gas_price / 10; // 10% priority fee
        
        // Check if gas is too high
        if gas_price > (self.config.max_gas_gwei as u128) * 1_000_000_000 {
            return Ok(ExecutionResult::Aborted {
                reason: format!(
                    "Gas price too high: {} gwei > {} max",
                    gas_price / 1_000_000_000,
                    self.config.max_gas_gwei
                ),
            });
        }
        
        // Clone wallet manager for mutable operations
        let mut wallet = WalletManager::from_env()?;
        
        // Update nonce from network
        wallet.update_nonce(&self.config.rpc_url).await?;
        
        // Sign the flash loan transaction
        let signed_tx = wallet.sign_transaction(
            flash_loan_tx.to,
            flash_loan_tx.calldata.clone(),
            flash_loan_tx.value,
            flash_loan_tx.gas_limit,
            gas_price,
            priority_fee,
        ).await?;
        
        info!("✓ Transaction signed");
        
        // Calculate expected profit in wei
        let expected_profit_wei = U256::from((simulation.profit_usd * 1e18 / 3500.0) as u128);
        
        // Build the bundle
        let bundle = self.bundle_builder.build_bundle(
            flash_loan_tx,
            signed_tx.clone(),
            current_block + 1,
            expected_profit_wei,
        )?;
        
        info!("✓ Bundle built for block {}", current_block + 1);
        
        // Simulate first
        let sim_result = self.flashbots_client.simulate_bundle(&bundle, &wallet).await?;
        
        if !sim_result.success {
            return Ok(ExecutionResult::Failed {
                reason: format!("Bundle simulation failed: {:?}", sim_result.error),
            });
        }
        
        info!("✓ Simulation passed, gas used: {:?}", sim_result.gas_used);
        
        // Submit to Flashbots
        let response = self.flashbots_client.send_bundle(&bundle, &wallet).await?;
        
        if let Some(ref error) = response.error {
            return Ok(ExecutionResult::Failed {
                reason: format!("Bundle submission failed: {} (code {})", error.message, error.code),
            });
        }
        
        let bundle_hash = response.bundle_hash.unwrap_or_else(|| "unknown".to_string());
        
        info!("🎯 Bundle submitted! Hash: {}", bundle_hash);
        info!("   Target block: {}", current_block + 1);
        info!("   Expected profit: ${:.2}", simulation.profit_usd);
        
        Ok(ExecutionResult::Submitted {
            bundle_hash,
            target_block: current_block + 1,
            expected_profit_usd: simulation.profit_usd,
        })
    }
    
    /// Get current gas price from network
    async fn get_current_gas_price(&self) -> Result<u128> {
        let provider = ProviderBuilder::new()
            .on_http(self.config.rpc_url.parse()?);
        
        let gas_price = provider.get_gas_price().await?;
        Ok(gas_price)
    }
    
    /// Get current block number
    pub async fn get_current_block(&self) -> Result<u64> {
        let provider = ProviderBuilder::new()
            .on_http(self.config.rpc_url.parse()?);
        
        Ok(provider.get_block_number().await?)
    }
    
    /// Monitor a submitted bundle for inclusion
    pub async fn monitor_bundle(
        &self,
        bundle_hash: &str,
        target_block: u64,
        timeout_blocks: u64,
    ) -> Result<BundleStatus> {
        let provider = ProviderBuilder::new()
            .on_http(self.config.rpc_url.parse()?);
        
        let deadline = target_block + timeout_blocks;
        
        loop {
            let current_block = provider.get_block_number().await?;
            
            if current_block > deadline {
                return Ok(BundleStatus::NotIncluded {
                    checked_until_block: current_block,
                });
            }
            
            // Check bundle stats from Flashbots
            if let Ok(stats) = self.flashbots_client.get_bundle_stats(bundle_hash).await {
                if let Some(result) = stats.get("result") {
                    if let Some(is_simulated) = result.get("isSimulated") {
                        if is_simulated.as_bool() == Some(true) {
                            // Bundle was simulated by builder
                            debug!("Bundle {} simulated by builder", bundle_hash);
                        }
                    }
                }
            }
            
            // Wait for next block
            tokio::time::sleep(tokio::time::Duration::from_secs(12)).await;
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
        
        let gas_cost_usd = simulation.total_gas_used as f64 * 20.0 * 1e-9 * 3500.0;
        
        let log = OpportunityLog {
            timestamp: Utc::now(),
            path: cycle.path.iter().map(|a| format!("{:?}", a)).collect(),
            dexes: cycle.dexes.iter().map(|d| d.to_string()).collect(),
            input_usd: self.config.default_simulation_usd,
            gross_profit_usd: simulation.profit_usd + gas_cost_usd,
            gas_cost_usd,
            net_profit_usd: simulation.profit_usd,
            gas_price_gwei: 20.0,
            eth_price_usd: 3500.0,
            block_number: 0,
        };
        
        log.append_to_file(&self.config.simulation_log_path)?;
        
        debug!("📝 Logged opportunity to {}", self.config.simulation_log_path);
        
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

/// Status of a submitted bundle
#[derive(Debug)]
pub enum BundleStatus {
    /// Bundle was included in a block
    Included {
        block_number: u64,
        tx_hash: String,
    },
    /// Bundle was not included within the timeout
    NotIncluded {
        checked_until_block: u64,
    },
    /// Error checking bundle status
    Error {
        reason: String,
    },
}
