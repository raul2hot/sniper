//! Configuration module for The Sniper
//!
//! Handles environment variables and constants

use eyre::Result;
use std::env;

/// Main configuration struct
#[derive(Debug, Clone)]
pub struct Config {
    /// Ethereum RPC URL
    pub rpc_url: String,

    /// Minimum profit threshold in USD
    pub min_profit_usd: f64,

    /// Maximum number of hops in arbitrage path
    pub max_hops: usize,

    /// Maximum gas price in gwei
    pub max_gas_gwei: u64,

    /// Dry run mode (simulation only)
    pub dry_run: bool,
}

impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self> {
        // Load .env file if it exists
        dotenvy::dotenv().ok();

        Ok(Self {
            rpc_url: env::var("RPC_URL")
                .unwrap_or_else(|_| "https://eth.llamarpc.com".to_string()),

            min_profit_usd: env::var("MIN_PROFIT_USD")
                .unwrap_or_else(|_| "5.0".to_string())
                .parse()
                .unwrap_or(5.0),

            max_hops: env::var("MAX_HOPS")
                .unwrap_or_else(|_| "4".to_string())
                .parse()
                .unwrap_or(4),

            max_gas_gwei: env::var("MAX_GAS_GWEI")
                .unwrap_or_else(|_| "100".to_string())
                .parse()
                .unwrap_or(100),

            dry_run: env::var("DRY_RUN")
                .unwrap_or_else(|_| "true".to_string())
                .parse()
                .unwrap_or(true),
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rpc_url: "https://eth.llamarpc.com".to_string(),
            min_profit_usd: 5.0,
            max_hops: 4,
            max_gas_gwei: 100,
            dry_run: true,
        }
    }
}
