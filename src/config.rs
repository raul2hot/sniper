//! Production Configuration for The Sniper
//!
//! This module contains all configuration parameters for running the bot
//! in both Simulation and Production modes with proper guardrails.

use alloy_primitives::Address;
use eyre::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::Path;
use std::str::FromStr;

// ============================================
// EXECUTION MODE
// ============================================

/// Execution mode determines how the bot operates
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    /// Simulation mode - finds opportunities but never executes
    /// Safe for testing and monitoring
    Simulation,
    
    /// DryRun mode - simulates execution through Flashbots but doesn't submit
    /// Good for validating the full pipeline
    DryRun,
    
    /// Production mode - actually submits bundles to Flashbots
    /// CAUTION: This uses real funds!
    Production,
}

impl Default for ExecutionMode {
    fn default() -> Self {
        ExecutionMode::Simulation
    }
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionMode::Simulation => write!(f, "SIMULATION"),
            ExecutionMode::DryRun => write!(f, "DRY_RUN"),
            ExecutionMode::Production => write!(f, "PRODUCTION"),
        }
    }
}

// ============================================
// FLASH LOAN PROVIDER
// ============================================

/// Available Flash Loan providers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlashLoanProvider {
    /// Aave V3 - 0.05% fee, very reliable
    AaveV3,
    
    /// Balancer V2 - 0% fee, but lower liquidity on some tokens
    BalancerV2,
    
    /// Uniswap V3 - Flash swap (pay with output token)
    UniswapV3,
}

impl Default for FlashLoanProvider {
    fn default() -> Self {
        FlashLoanProvider::BalancerV2 // 0% fee is ideal
    }
}

impl std::fmt::Display for FlashLoanProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlashLoanProvider::AaveV3 => write!(f, "Aave V3 (0.05% fee)"),
            FlashLoanProvider::BalancerV2 => write!(f, "Balancer V2 (0% fee)"),
            FlashLoanProvider::UniswapV3 => write!(f, "Uniswap V3 (Flash Swap)"),
        }
    }
}

// ============================================
// MAIN CONFIGURATION
// ============================================

/// Main configuration struct for The Sniper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // ========== Network Settings ==========
    /// Primary RPC URL (Alchemy/Infura recommended)
    pub rpc_url: String,
    
    /// Backup RPC URLs for failover
    pub backup_rpc_urls: Vec<String>,
    
    /// Chain ID (1 = Ethereum Mainnet)
    pub chain_id: u64,
    
    // ========== Execution Settings ==========
    /// Current execution mode
    pub execution_mode: ExecutionMode,
    
    /// Enable/disable simulation logging
    pub simulation_log: bool,
    
    /// Path to save profitable opportunity logs
    pub simulation_log_path: String,
    
    // ========== Profit Thresholds ==========
    /// Minimum net profit in USD to consider an opportunity
    /// Set to $20+ to cover gas spikes and leave margin
    pub min_profit_usd: f64,
    
    /// Minimum gross profit percentage (before gas)
    /// Set to 0.3%+ to ensure real opportunity exists
    pub min_gross_profit_pct: f64,
    
    /// Maximum acceptable gas price in gwei
    /// Abort if gas exceeds this (prevents executing during spikes)
    pub max_gas_gwei: u64,
    
    /// Maximum slippage tolerance (0.01 = 1%)
    pub max_slippage: f64,
    
    // ========== Path Finding Settings ==========
    /// Maximum hops in arbitrage cycle (3-4 recommended)
    pub max_hops: usize,
    
    /// Minimum liquidity in USD for a pool to be considered
    pub min_pool_liquidity_usd: f64,
    
    // ========== Token Filters ==========
    /// Tokens to ALWAYS start arbitrage from (high liquidity)
    pub base_tokens: Vec<String>,
    
    /// Token pairs to BLACKLIST (e.g., stablecoin-to-stablecoin)
    pub blacklisted_pairs: Vec<(String, String)>,
    
    /// Individual tokens to blacklist (known scam/honeypot tokens)
    pub blacklisted_tokens: Vec<String>,
    
    /// Only trade tokens in this whitelist (if non-empty)
    pub whitelisted_tokens: Vec<String>,
    
    // ========== Flash Loan Settings ==========
    /// Preferred flash loan provider
    pub flash_loan_provider: FlashLoanProvider,
    
    /// Maximum flash loan amount in USD
    pub max_flash_loan_usd: f64,
    
    /// Default flash loan amount for simulation
    pub default_simulation_usd: f64,
    
    // ========== Flashbots Settings ==========
    /// Flashbots RPC endpoint
    pub flashbots_rpc_url: String,
    
    /// Flashbots bundle signing key (KEEP SECRET!)
    /// This is separate from your profit wallet
    pub flashbots_signer_key: Option<String>,
    
    /// Miner bribe percentage (of profit to give to miner)
    /// 90% = keep 10% profit, give 90% to miner for inclusion
    pub miner_bribe_pct: f64,
    
    // ========== Wallet Settings ==========
    /// Ethereum wallet address to receive profits
    pub profit_wallet_address: Option<String>,
    
    /// Executor contract address (deployed flash loan executor)
    pub executor_contract_address: Option<String>,
    
    // ========== Rate Limiting ==========
    /// Minimum seconds between scans
    pub scan_interval_secs: u64,
    
    /// Maximum RPC calls per second
    pub max_rpc_calls_per_sec: u32,
    
    // ========== Safety Settings ==========
    /// Kill switch - immediately stop all operations
    pub emergency_stop: bool,
    
    /// Maximum consecutive failures before pausing
    pub max_consecutive_failures: u32,
    
    /// Pause duration after max failures (seconds)
    pub failure_pause_secs: u64,
    
    // ========== API Keys ==========
    /// Etherscan API key for accurate gas prices
    pub etherscan_api_key: Option<String>,
}

impl Config {
    /// Load configuration from environment variables and .env file
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        Ok(Self {
            // Network
            rpc_url: env::var("RPC_URL")
                .unwrap_or_else(|_| "https://eth.llamarpc.com".to_string()),
            backup_rpc_urls: env::var("BACKUP_RPC_URLS")
                .map(|s| s.split(',').map(String::from).collect())
                .unwrap_or_default(),
            chain_id: env::var("CHAIN_ID")
                .unwrap_or_else(|_| "1".to_string())
                .parse()
                .unwrap_or(1),
            
            // Execution
            execution_mode: match env::var("EXECUTION_MODE")
                .unwrap_or_else(|_| "simulation".to_string())
                .to_lowercase()
                .as_str()
            {
                "production" => ExecutionMode::Production,
                "dry_run" | "dryrun" => ExecutionMode::DryRun,
                _ => ExecutionMode::Simulation,
            },
            simulation_log: env::var("SIMULATION_LOG")
                .unwrap_or_else(|_| "true".to_string())
                .parse()
                .unwrap_or(true),
            simulation_log_path: env::var("SIMULATION_LOG_PATH")
                .unwrap_or_else(|_| "./logs/profitable_opportunities.log".to_string()),
            
            // Profit thresholds
            min_profit_usd: env::var("MIN_PROFIT_USD")
                .unwrap_or_else(|_| "20.0".to_string())
                .parse()
                .unwrap_or(20.0),
            min_gross_profit_pct: env::var("MIN_GROSS_PROFIT_PCT")
                .unwrap_or_else(|_| "0.3".to_string())
                .parse()
                .unwrap_or(0.3),
            max_gas_gwei: env::var("MAX_GAS_GWEI")
                .unwrap_or_else(|_| "50".to_string())
                .parse()
                .unwrap_or(50),
            max_slippage: env::var("MAX_SLIPPAGE")
                .unwrap_or_else(|_| "0.005".to_string())
                .parse()
                .unwrap_or(0.005),
            
            // Path finding
            max_hops: env::var("MAX_HOPS")
                .unwrap_or_else(|_| "4".to_string())
                .parse()
                .unwrap_or(4),
            min_pool_liquidity_usd: env::var("MIN_POOL_LIQUIDITY_USD")
                .unwrap_or_else(|_| "50000.0".to_string())
                .parse()
                .unwrap_or(50000.0),
            
            // Token filters
            base_tokens: env::var("BASE_TOKENS")
                .map(|s| s.split(',').map(String::from).collect())
                .unwrap_or_else(|_| Self::default_base_tokens()),
            blacklisted_pairs: Self::parse_blacklisted_pairs(),
            blacklisted_tokens: env::var("BLACKLISTED_TOKENS")
                .map(|s| s.split(',').map(String::from).collect())
                .unwrap_or_default(),
            whitelisted_tokens: env::var("WHITELISTED_TOKENS")
                .map(|s| s.split(',').map(String::from).collect())
                .unwrap_or_default(),
            
            // Flash loan
            flash_loan_provider: match env::var("FLASH_LOAN_PROVIDER")
                .unwrap_or_else(|_| "balancer".to_string())
                .to_lowercase()
                .as_str()
            {
                "aave" | "aavev3" => FlashLoanProvider::AaveV3,
                "uniswap" | "uniswapv3" => FlashLoanProvider::UniswapV3,
                _ => FlashLoanProvider::BalancerV2,
            },
            max_flash_loan_usd: env::var("MAX_FLASH_LOAN_USD")
                .unwrap_or_else(|_| "100000.0".to_string())
                .parse()
                .unwrap_or(100000.0),
            default_simulation_usd: env::var("DEFAULT_SIMULATION_USD")
                .unwrap_or_else(|_| "10000.0".to_string())
                .parse()
                .unwrap_or(10000.0),
            
            // Flashbots
            flashbots_rpc_url: env::var("FLASHBOTS_RPC_URL")
                .unwrap_or_else(|_| "https://relay.flashbots.net".to_string()),
            flashbots_signer_key: env::var("FLASHBOTS_SIGNER_KEY").ok(),
            miner_bribe_pct: env::var("MINER_BRIBE_PCT")
                .unwrap_or_else(|_| "90.0".to_string())
                .parse()
                .unwrap_or(90.0),
            
            // Wallet
            profit_wallet_address: env::var("PROFIT_WALLET_ADDRESS").ok(),
            executor_contract_address: env::var("EXECUTOR_CONTRACT_ADDRESS").ok(),
            
            // Rate limiting
            scan_interval_secs: env::var("SCAN_INTERVAL_SECS")
                .unwrap_or_else(|_| "12".to_string()) // ~1 block
                .parse()
                .unwrap_or(12),
            max_rpc_calls_per_sec: env::var("MAX_RPC_CALLS_PER_SEC")
                .unwrap_or_else(|_| "25".to_string())
                .parse()
                .unwrap_or(25),
            
            // Safety
            emergency_stop: env::var("EMERGENCY_STOP")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .unwrap_or(false),
            max_consecutive_failures: env::var("MAX_CONSECUTIVE_FAILURES")
                .unwrap_or_else(|_| "5".to_string())
                .parse()
                .unwrap_or(5),
            failure_pause_secs: env::var("FAILURE_PAUSE_SECS")
                .unwrap_or_else(|_| "60".to_string())
                .parse()
                .unwrap_or(60),
            
            // API Keys
            etherscan_api_key: env::var("ETHERSCAN_API_KEY").ok(),
        })
    }
    
    /// Load configuration from a TOML file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }
    
    /// Save configuration to a TOML file
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }
    
    /// Default base tokens (high liquidity)
    fn default_base_tokens() -> Vec<String> {
        vec![
            "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".to_string(), // WETH
            "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string(), // USDC
            "0xdAC17F958D2ee523a2206206994597C13D831ec7".to_string(), // USDT
            "0x6B175474E89094C44Da98b954EedcdeCB5BE3830".to_string(), // DAI
            "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599".to_string(), // WBTC
        ]
    }
    
    /// Parse blacklisted pairs from environment
    fn parse_blacklisted_pairs() -> Vec<(String, String)> {
        // Default: Block stablecoin-to-stablecoin arbitrage
        // These almost never profit due to tight pegs
        vec![
            // USDC <-> USDT (always ~$0.0001 spread, loses to gas)
            (
                "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string(), // USDC
                "0xdAC17F958D2ee523a2206206994597C13D831ec7".to_string(), // USDT
            ),
            // USDC <-> DAI
            (
                "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string(), // USDC
                "0x6B175474E89094C44Da98b954EedcdeCB5BE3830".to_string(), // DAI
            ),
            // USDT <-> DAI
            (
                "0xdAC17F958D2ee523a2206206994597C13D831ec7".to_string(), // USDT
                "0x6B175474E89094C44Da98b954EedcdeCB5BE3830".to_string(), // DAI
            ),
        ]
    }
    
    /// Check if a token pair is blacklisted
    pub fn is_pair_blacklisted(&self, token_a: &Address, token_b: &Address) -> bool {
        let a_str = format!("{:?}", token_a);
        let b_str = format!("{:?}", token_b);
        
        for (bl_a, bl_b) in &self.blacklisted_pairs {
            let bl_a_lower = bl_a.to_lowercase();
            let bl_b_lower = bl_b.to_lowercase();
            let a_lower = a_str.to_lowercase();
            let b_lower = b_str.to_lowercase();
            
            // Check both directions
            if (a_lower.contains(&bl_a_lower[2..]) && b_lower.contains(&bl_b_lower[2..]))
                || (a_lower.contains(&bl_b_lower[2..]) && b_lower.contains(&bl_a_lower[2..]))
            {
                return true;
            }
        }
        false
    }
    
    /// Check if a cycle contains any blacklisted pairs
    pub fn is_cycle_blacklisted(&self, path: &[Address]) -> bool {
        for i in 0..path.len().saturating_sub(1) {
            if self.is_pair_blacklisted(&path[i], &path[i + 1]) {
                return true;
            }
        }
        false
    }
    
    /// Check if a token is blacklisted
    pub fn is_token_blacklisted(&self, token: &Address) -> bool {
        let token_str = format!("{:?}", token).to_lowercase();
        
        for blacklisted in &self.blacklisted_tokens {
            if token_str.contains(&blacklisted.to_lowercase()[2..]) {
                return true;
            }
        }
        false
    }
    
    /// Validate configuration for production use
    pub fn validate(&self) -> Result<()> {
        // Check RPC URL
        if self.rpc_url.is_empty() || self.rpc_url.contains("YOUR_API_KEY") {
            return Err(eyre::eyre!("Invalid RPC_URL - please set a valid Alchemy/Infura URL"));
        }
        
        // Production mode requires additional settings
        if self.execution_mode == ExecutionMode::Production {
            if self.flashbots_signer_key.is_none() {
                return Err(eyre::eyre!(
                    "Production mode requires FLASHBOTS_SIGNER_KEY"
                ));
            }
            if self.profit_wallet_address.is_none() {
                return Err(eyre::eyre!(
                    "Production mode requires PROFIT_WALLET_ADDRESS"
                ));
            }
            if self.executor_contract_address.is_none() {
                return Err(eyre::eyre!(
                    "Production mode requires EXECUTOR_CONTRACT_ADDRESS (deploy the executor first)"
                ));
            }
            if self.min_profit_usd < 2.0 {
                return Err(eyre::eyre!(
                    "Production mode requires MIN_PROFIT_USD >= $2 (currently ${:.2})",
                    self.min_profit_usd
                ));
            }
        }
        
        // Sanity checks
        if self.max_hops > 6 {
            return Err(eyre::eyre!(
                "MAX_HOPS > 6 will cause exponential gas costs"
            ));
        }
        if self.miner_bribe_pct < 50.0 || self.miner_bribe_pct > 99.0 {
            return Err(eyre::eyre!(
                "MINER_BRIBE_PCT should be between 50-99% (currently {:.1}%)",
                self.miner_bribe_pct
            ));
        }
        
        Ok(())
    }
    
    /// Get base token addresses as Address type
    pub fn base_token_addresses(&self) -> Vec<Address> {
        self.base_tokens
            .iter()
            .filter_map(|s| Address::from_str(s).ok())
            .collect()
    }
    
    /// Print configuration summary
    pub fn print_summary(&self) {
        println!("╔════════════════════════════════════════════════════════════╗");
        println!("║              THE SNIPER - CONFIGURATION                    ║");
        println!("╠════════════════════════════════════════════════════════════╣");
        println!("║ Execution Mode:    {:^40} ║", self.execution_mode);
        println!("║ Chain ID:          {:^40} ║", self.chain_id);
        println!("╠════════════════════════════════════════════════════════════╣");
        println!("║ PROFIT THRESHOLDS                                          ║");
        println!("║ • Min Net Profit:  ${:<38.2} ║", self.min_profit_usd);
        println!("║ • Min Gross %:     {:<38.2}% ║", self.min_gross_profit_pct);
        println!("║ • Max Gas:         {:>38} gwei ║", self.max_gas_gwei);
        println!("║ • Max Slippage:    {:>38.2}% ║", self.max_slippage * 100.0);
        println!("╠════════════════════════════════════════════════════════════╣");
        println!("║ PATH FINDING                                               ║");
        println!("║ • Max Hops:        {:^40} ║", self.max_hops);
        println!("║ • Base Tokens:     {:^40} ║", self.base_tokens.len());
        println!("║ • Blacklisted Pairs: {:^38} ║", self.blacklisted_pairs.len());
        println!("╠════════════════════════════════════════════════════════════╣");
        println!("║ FLASH LOAN                                                 ║");
        println!("║ • Provider:        {:^40} ║", self.flash_loan_provider);
        println!("║ • Max Amount:      ${:<38.0} ║", self.max_flash_loan_usd);
        println!("╠════════════════════════════════════════════════════════════╣");
        println!("║ FLASHBOTS                                                  ║");
        println!("║ • Miner Bribe:     {:>38.0}% ║", self.miner_bribe_pct);
        println!("║ • Signer Key:      {:^40} ║", 
            if self.flashbots_signer_key.is_some() { "✓ Configured" } else { "✗ Not Set" }
        );
        println!("╠════════════════════════════════════════════════════════════╣");
        println!("║ GAS ORACLE                                                 ║");
        println!("║ • Etherscan API:   {:^40} ║",
            if self.etherscan_api_key.is_some() { "✓ Configured" } else { "✗ Using RPC" }
        );
        println!("╠════════════════════════════════════════════════════════════╣");
        println!("║ SAFETY                                                     ║");
        println!("║ • Emergency Stop:  {:^40} ║",
            if self.emergency_stop { "🛑 ACTIVE" } else { "✓ Inactive" }
        );
        println!("║ • Simulation Log:  {:^40} ║",
            if self.simulation_log { "✓ Enabled" } else { "✗ Disabled" }
        );
        println!("╚════════════════════════════════════════════════════════════╝");
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rpc_url: "https://eth.llamarpc.com".to_string(),
            backup_rpc_urls: vec![],
            chain_id: 1,
            execution_mode: ExecutionMode::Simulation,
            simulation_log: true,
            simulation_log_path: "./logs/profitable_opportunities.log".to_string(),
            min_profit_usd: 20.0,
            min_gross_profit_pct: 0.3,
            max_gas_gwei: 50,
            max_slippage: 0.005,
            max_hops: 4,
            min_pool_liquidity_usd: 50000.0,
            base_tokens: Self::default_base_tokens(),
            blacklisted_pairs: Self::parse_blacklisted_pairs(),
            blacklisted_tokens: vec![],
            whitelisted_tokens: vec![],
            flash_loan_provider: FlashLoanProvider::BalancerV2,
            max_flash_loan_usd: 100000.0,
            default_simulation_usd: 10000.0,
            flashbots_rpc_url: "https://relay.flashbots.net".to_string(),
            flashbots_signer_key: None,
            miner_bribe_pct: 90.0,
            profit_wallet_address: None,
            executor_contract_address: None,
            scan_interval_secs: 12,
            max_rpc_calls_per_sec: 25,
            emergency_stop: false,
            max_consecutive_failures: 5,
            failure_pause_secs: 60,
            etherscan_api_key: None,
        }
    }
}

// ============================================
// OPPORTUNITY LOGGER
// ============================================

use chrono::{DateTime, Utc};
use std::io::Write;

/// Logs profitable opportunities found during simulation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpportunityLog {
    pub timestamp: DateTime<Utc>,
    pub path: Vec<String>,
    pub dexes: Vec<String>,
    pub input_usd: f64,
    pub gross_profit_usd: f64,
    pub gas_cost_usd: f64,
    pub net_profit_usd: f64,
    pub gas_price_gwei: f64,
    pub eth_price_usd: f64,
    pub block_number: u64,
}

impl OpportunityLog {
    /// Append this log to a file
    pub fn append_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        // Create parent directories if needed
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }
        
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        
        let json = serde_json::to_string(self)?;
        writeln!(file, "{}", json)?;
        
        Ok(())
    }
}

// ============================================
// TESTS
// ============================================

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.execution_mode, ExecutionMode::Simulation);
        assert_eq!(config.min_profit_usd, 20.0);
        assert!(!config.blacklisted_pairs.is_empty());
    }
    
    #[test]
    fn test_blacklist_check() {
        let config = Config::default();
        
        // USDC and USDT should be blacklisted
        let usdc = Address::from_str("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap();
        let usdt = Address::from_str("0xdAC17F958D2ee523a2206206994597C13D831ec7").unwrap();
        
        assert!(config.is_pair_blacklisted(&usdc, &usdt));
        assert!(config.is_pair_blacklisted(&usdt, &usdc)); // Both directions
        
        // WETH and USDC should NOT be blacklisted
        let weth = Address::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap();
        assert!(!config.is_pair_blacklisted(&weth, &usdc));
    }
    
    #[test]
    fn test_cycle_blacklist() {
        let config = Config::default();
        
        let usdc = Address::from_str("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap();
        let usdt = Address::from_str("0xdAC17F958D2ee523a2206206994597C13D831ec7").unwrap();
        let weth = Address::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap();
        
        // USDC -> USDT -> USDC should be blacklisted
        let bad_cycle = vec![usdc, usdt, usdc];
        assert!(config.is_cycle_blacklisted(&bad_cycle));
        
        // WETH -> USDC -> WETH should NOT be blacklisted
        let good_cycle = vec![weth, usdc, weth];
        assert!(!config.is_cycle_blacklisted(&good_cycle));
    }
}