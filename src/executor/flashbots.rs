//! Flashbots Bundle Submission - Phase 4 (PRODUCTION READY)
//!
//! This module handles the submission of arbitrage bundles to Flashbots.
//! Key benefits:
//! 1. No failed transaction costs - if the bundle fails, you pay nothing
//! 2. Frontrunning protection - hidden from public mempool
//! 3. MEV-Share for additional revenue
//!
//! NOW WITH: Proper ECDSA signing via WalletManager

use alloy_primitives::{Address, Bytes, B256, U256};
use eyre::{eyre, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

use crate::config::Config;
use super::flash_loan::FlashLoanTransaction;
use super::signer::WalletManager;

// ============================================
// FLASHBOTS ENDPOINTS
// ============================================

/// Flashbots relay endpoints
pub struct FlashbotsEndpoints;

impl FlashbotsEndpoints {
    /// Mainnet relay
    pub const MAINNET: &'static str = "https://relay.flashbots.net";
    
    /// Goerli testnet relay (for testing)
    pub const GOERLI: &'static str = "https://relay-goerli.flashbots.net";
    
    /// Sepolia testnet relay
    pub const SEPOLIA: &'static str = "https://relay-sepolia.flashbots.net";
    
    /// MEV-Share endpoint (for builders)
    pub const MEV_SHARE: &'static str = "https://mev-share.flashbots.net";
    
    /// Bundle simulation endpoint
    pub const SIMULATE: &'static str = "https://relay.flashbots.net/simulate";
}

// ============================================
// BUNDLE TYPES
// ============================================

/// A Flashbots bundle ready for submission
#[derive(Debug, Clone)]
pub struct FlashbotsBundle {
    /// Signed transactions in the bundle
    pub transactions: Vec<Bytes>,
    
    /// Target block number
    pub block_number: u64,
    
    /// Minimum timestamp (optional)
    pub min_timestamp: Option<u64>,
    
    /// Maximum timestamp (optional)
    pub max_timestamp: Option<u64>,
    
    /// Reverting transaction hashes to allow (optional)
    pub reverting_tx_hashes: Vec<B256>,
}

/// Response from bundle submission
#[derive(Debug, Clone, Deserialize)]
pub struct BundleResponse {
    pub bundle_hash: Option<String>,
    pub error: Option<BundleError>,
}

/// Error from Flashbots
#[derive(Debug, Clone, Deserialize)]
pub struct BundleError {
    pub code: i64,
    pub message: String,
}

/// Simulation result
#[derive(Debug, Clone, Deserialize)]
pub struct SimulationResult {
    pub success: bool,
    pub state_block: Option<u64>,
    pub gas_used: Option<u64>,
    pub coinbase_diff: Option<String>,
    pub error: Option<String>,
}

// ============================================
// FLASHBOTS CLIENT
// ============================================

/// Client for interacting with Flashbots relay
pub struct FlashbotsClient {
    http_client: Client,
    relay_url: String,
    chain_id: u64,
}

impl FlashbotsClient {
    /// Create a new Flashbots client from config
    pub fn new(config: &Config) -> Self {
        Self {
            http_client: Client::new(),
            relay_url: config.flashbots_rpc_url.clone(),
            chain_id: config.chain_id,
        }
    }
    
    /// Create a client for testing on Goerli
    pub fn goerli() -> Self {
        Self {
            http_client: Client::new(),
            relay_url: FlashbotsEndpoints::GOERLI.to_string(),
            chain_id: 5,
        }
    }
    
    /// Create a client for Sepolia testnet
    pub fn sepolia() -> Self {
        Self {
            http_client: Client::new(),
            relay_url: FlashbotsEndpoints::SEPOLIA.to_string(),
            chain_id: 11155111,
        }
    }
    
    /// Check if the client has a signing key configured (legacy check)
    pub fn has_signer(&self) -> bool {
        // This is now checked via WalletManager
        true
    }
    
    /// Send a bundle to the Flashbots relay
    pub async fn send_bundle(
        &self, 
        bundle: &FlashbotsBundle,
        wallet: &WalletManager,
    ) -> Result<BundleResponse> {
        if !wallet.has_flashbots_signer() {
            return Err(eyre!("Flashbots signer key not configured"));
        }
        
        // Build the bundle params
        let tx_strings: Vec<String> = bundle.transactions
            .iter()
            .map(|tx| format!("0x{}", hex::encode(tx)))
            .collect();
        
        let mut params = json!({
            "txs": tx_strings,
            "blockNumber": format!("0x{:x}", bundle.block_number),
        });
        
        if let Some(min_ts) = bundle.min_timestamp {
            params["minTimestamp"] = json!(min_ts);
        }
        if let Some(max_ts) = bundle.max_timestamp {
            params["maxTimestamp"] = json!(max_ts);
        }
        if !bundle.reverting_tx_hashes.is_empty() {
            let hashes: Vec<String> = bundle.reverting_tx_hashes
                .iter()
                .map(|h| format!("0x{}", hex::encode(h)))
                .collect();
            params["revertingTxHashes"] = json!(hashes);
        }
        
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_sendBundle",
            "params": [params]
        });
        
        // Sign the request using WalletManager
        let body = serde_json::to_string(&request)?;
        let signature = wallet.sign_flashbots_request(&body).await?;
        
        debug!("Sending bundle to {}", self.relay_url);
        
        // Send to relay
        let response = self.http_client
            .post(&self.relay_url)
            .header("Content-Type", "application/json")
            .header("X-Flashbots-Signature", &signature)
            .body(body)
            .send()
            .await?;
        
        let status = response.status();
        let response_body: Value = response.json().await?;
        
        debug!("Flashbots response status: {}", status);
        debug!("Flashbots response: {:?}", response_body);
        
        if let Some(error) = response_body.get("error") {
            return Ok(BundleResponse {
                bundle_hash: None,
                error: Some(BundleError {
                    code: error.get("code").and_then(|v| v.as_i64()).unwrap_or(-1),
                    message: error.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error").to_string(),
                }),
            });
        }
        
        let bundle_hash = response_body
            .get("result")
            .and_then(|r| r.get("bundleHash"))
            .and_then(|h| h.as_str())
            .map(String::from);
        
        Ok(BundleResponse {
            bundle_hash,
            error: None,
        })
    }
    
    /// Simulate a bundle without submitting
    pub async fn simulate_bundle(
        &self, 
        bundle: &FlashbotsBundle,
        wallet: &WalletManager,
    ) -> Result<SimulationResult> {
        if !wallet.has_flashbots_signer() {
            return Err(eyre!("Flashbots signer key not configured"));
        }
        
        let tx_strings: Vec<String> = bundle.transactions
            .iter()
            .map(|tx| format!("0x{}", hex::encode(tx)))
            .collect();
        
        let params = json!({
            "txs": tx_strings,
            "blockNumber": format!("0x{:x}", bundle.block_number),
            "stateBlockNumber": "latest"
        });
        
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_callBundle",
            "params": [params]
        });
        
        let body = serde_json::to_string(&request)?;
        let signature = wallet.sign_flashbots_request(&body).await?;
        
        debug!("Simulating bundle at {}", self.relay_url);
        
        let response = self.http_client
            .post(&self.relay_url)
            .header("Content-Type", "application/json")
            .header("X-Flashbots-Signature", &signature)
            .body(body)
            .send()
            .await?;
        
        let response_body: Value = response.json().await?;
        
        if let Some(error) = response_body.get("error") {
            return Ok(SimulationResult {
                success: false,
                state_block: None,
                gas_used: None,
                coinbase_diff: None,
                error: Some(error.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown").to_string()),
            });
        }
        
        let result = response_body.get("result");
        
        // Check for simulation errors in the results
        if let Some(results) = result.and_then(|r| r.get("results")).and_then(|r| r.as_array()) {
            for tx_result in results {
                if let Some(error) = tx_result.get("error") {
                    return Ok(SimulationResult {
                        success: false,
                        state_block: None,
                        gas_used: None,
                        coinbase_diff: None,
                        error: Some(error.as_str().unwrap_or("Transaction error").to_string()),
                    });
                }
            }
        }
        
        Ok(SimulationResult {
            success: true,
            state_block: result.and_then(|r| r.get("stateBlockNumber")).and_then(|s| {
                s.as_str().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            }),
            gas_used: result.and_then(|r| r.get("totalGasUsed")).and_then(|g| g.as_u64()),
            coinbase_diff: result.and_then(|r| r.get("coinbaseDiff")).and_then(|c| c.as_str()).map(String::from),
            error: None,
        })
    }
    
    /// Get the current bundle stats
    pub async fn get_bundle_stats(&self, bundle_hash: &str) -> Result<Value> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "flashbots_getBundleStats",
            "params": [{ "bundleHash": bundle_hash }]
        });
        
        let response = self.http_client
            .post(&self.relay_url)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;
        
        Ok(response.json().await?)
    }
    
    /// Get user stats from Flashbots
    pub async fn get_user_stats(&self, wallet: &WalletManager) -> Result<Value> {
        if !wallet.has_flashbots_signer() {
            return Err(eyre!("Flashbots signer key not configured"));
        }
        
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "flashbots_getUserStats",
            "params": [{ "blockNumber": "latest" }]
        });
        
        let body = serde_json::to_string(&request)?;
        let signature = wallet.sign_flashbots_request(&body).await?;
        
        let response = self.http_client
            .post(&self.relay_url)
            .header("Content-Type", "application/json")
            .header("X-Flashbots-Signature", &signature)
            .body(body)
            .send()
            .await?;
        
        Ok(response.json().await?)
    }
}

// ============================================
// BUNDLE BUILDER
// ============================================

/// Builds Flashbots bundles from arbitrage opportunities
pub struct BundleBuilder {
    miner_bribe_pct: f64,
    chain_id: u64,
}

impl BundleBuilder {
    pub fn new(config: &Config) -> Self {
        Self {
            miner_bribe_pct: config.miner_bribe_pct,
            chain_id: config.chain_id,
        }
    }
    
    /// Build a bundle from a flash loan transaction
    pub fn build_bundle(
        &self,
        flash_loan_tx: &FlashLoanTransaction,
        signed_tx: Bytes,
        target_block: u64,
        expected_profit_wei: U256,
    ) -> Result<FlashbotsBundle> {
        // Calculate miner bribe
        let bribe_wei = (expected_profit_wei * U256::from((self.miner_bribe_pct * 100.0) as u64)) 
            / U256::from(10000);
        
        info!(
            "Building bundle for block {} with ${:.2} expected profit, ${:.2} miner bribe ({:.0}%)",
            target_block,
            expected_profit_wei.to::<u128>() as f64 / 1e18 * 3500.0,
            bribe_wei.to::<u128>() as f64 / 1e18 * 3500.0,
            self.miner_bribe_pct
        );
        
        Ok(FlashbotsBundle {
            transactions: vec![signed_tx],
            block_number: target_block,
            min_timestamp: None,
            max_timestamp: None,
            reverting_tx_hashes: vec![], // We don't allow reverts - all or nothing
        })
    }
    
    /// Calculate the bribe amount in wei
    pub fn calculate_bribe(&self, profit_wei: U256) -> U256 {
        (profit_wei * U256::from((self.miner_bribe_pct * 100.0) as u64)) / U256::from(10000)
    }
    
    /// Calculate our take after bribe
    pub fn calculate_our_profit(&self, gross_profit_wei: U256) -> U256 {
        let bribe = self.calculate_bribe(gross_profit_wei);
        gross_profit_wei.saturating_sub(bribe)
    }
}

// ============================================
// SUBMISSION STRATEGY
// ============================================

/// Strategy for submitting bundles
pub struct SubmissionStrategy {
    /// Target multiple consecutive blocks
    pub target_blocks: usize,
    
    /// Retry on inclusion failure
    pub retry_on_failure: bool,
    
    /// Maximum retries
    pub max_retries: usize,
    
    /// Use MEV-Share for additional revenue
    pub use_mev_share: bool,
}

impl Default for SubmissionStrategy {
    fn default() -> Self {
        Self {
            target_blocks: 3, // Submit to next 3 blocks
            retry_on_failure: true,
            max_retries: 2,
            use_mev_share: false, // Requires additional setup
        }
    }
}

/// Full bundle submission workflow
pub async fn submit_arbitrage_bundle(
    client: &FlashbotsClient,
    flash_loan_tx: FlashLoanTransaction,
    signed_tx: Bytes,
    current_block: u64,
    expected_profit_wei: U256,
    config: &Config,
    wallet: &WalletManager,
) -> Result<Option<String>> {
    let builder = BundleBuilder::new(config);
    let strategy = SubmissionStrategy::default();
    
    // Submit to multiple consecutive blocks for better inclusion
    let mut bundle_hashes = Vec::new();
    
    for i in 0..strategy.target_blocks {
        let target_block = current_block + 1 + i as u64;
        
        let bundle = builder.build_bundle(
            &flash_loan_tx,
            signed_tx.clone(),
            target_block,
            expected_profit_wei,
        )?;
        
        // First simulate
        info!("Simulating bundle for block {}...", target_block);
        let sim_result = client.simulate_bundle(&bundle, wallet).await?;
        
        if !sim_result.success {
            warn!(
                "Bundle simulation failed for block {}: {:?}",
                target_block, sim_result.error
            );
            continue;
        }
        
        info!(
            "Simulation passed! Gas used: {:?}, Coinbase diff: {:?}",
            sim_result.gas_used, sim_result.coinbase_diff
        );
        
        // Submit the bundle
        match client.send_bundle(&bundle, wallet).await {
            Ok(response) => {
                if let Some(hash) = response.bundle_hash {
                    info!("Bundle submitted for block {}: {}", target_block, hash);
                    bundle_hashes.push(hash);
                } else if let Some(error) = response.error {
                    warn!(
                        "Bundle submission failed for block {}: {} (code {})",
                        target_block, error.message, error.code
                    );
                }
            }
            Err(e) => {
                warn!("Failed to submit bundle for block {}: {}", target_block, e);
            }
        }
    }
    
    // Return the first successful bundle hash
    Ok(bundle_hashes.into_iter().next())
}

// ============================================
// TESTS
// ============================================

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bribe_calculation() {
        let config = Config {
            miner_bribe_pct: 90.0,
            ..Default::default()
        };
        let builder = BundleBuilder::new(&config);
        
        // 100 ETH profit -> 90 ETH bribe
        let profit = U256::from(100u64) * U256::from(10u64).pow(U256::from(18u64));
        let bribe = builder.calculate_bribe(profit);
        let our_profit = builder.calculate_our_profit(profit);
        
        assert_eq!(bribe, U256::from(90u64) * U256::from(10u64).pow(U256::from(18u64)));
        assert_eq!(our_profit, U256::from(10u64) * U256::from(10u64).pow(U256::from(18u64)));
    }
}
