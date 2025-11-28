//! Gas Price Oracle - Etherscan API Integration
//!
//! Fetches real-time gas prices from Etherscan for accurate profit calculations.
//! Falls back to RPC provider if Etherscan fails.
//!
//! API: https://api.etherscan.io/v2/api?chainid=1&module=proxy&action=eth_gasPrice

use eyre::{eyre, Result};
use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn, trace};

// ============================================
// CONSTANTS
// ============================================

/// Etherscan API base URL (v2 supports multiple chains)
const ETHERSCAN_API_URL: &str = "https://api.etherscan.io/v2/api";

/// Cache duration for gas prices (avoid hitting rate limits)
const CACHE_DURATION_SECS: u64 = 10;

/// Timeout for API calls
const API_TIMEOUT_SECS: u64 = 5;

/// Minimum sane gas price (0.1 gwei)
const MIN_GAS_GWEI: f64 = 0.01;

/// Maximum sane gas price (1000 gwei - during extreme congestion)
const MAX_GAS_GWEI: f64 = 1000.0;

/// Default fallback gas price if all sources fail
const FALLBACK_GAS_GWEI: f64 = 20.0;

// ============================================
// API RESPONSE TYPES
// ============================================

#[derive(Debug, Deserialize)]
struct EtherscanResponse {
    jsonrpc: Option<String>,
    id: Option<u64>,
    result: Option<String>,
    error: Option<EtherscanError>,
}

#[derive(Debug, Deserialize)]
struct EtherscanError {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct GasTrackerResponse {
    status: String,
    message: String,
    result: Option<GasTrackerResult>,
}

#[derive(Debug, Deserialize)]
struct GasTrackerResult {
    #[serde(rename = "LastBlock")]
    last_block: Option<String>,
    #[serde(rename = "SafeGasPrice")]
    safe_gas_price: Option<String>,
    #[serde(rename = "ProposeGasPrice")]
    propose_gas_price: Option<String>,
    #[serde(rename = "FastGasPrice")]
    fast_gas_price: Option<String>,
    #[serde(rename = "suggestBaseFee")]
    suggest_base_fee: Option<String>,
    #[serde(rename = "gasUsedRatio")]
    gas_used_ratio: Option<String>,
}

// ============================================
// CACHED GAS PRICE
// ============================================

#[derive(Debug, Clone)]
pub struct GasPriceInfo {
    /// Current gas price in gwei
    pub gas_price_gwei: f64,
    
    /// Safe (slow) gas price in gwei
    pub safe_gwei: f64,
    
    /// Proposed (standard) gas price in gwei
    pub standard_gwei: f64,
    
    /// Fast gas price in gwei
    pub fast_gwei: f64,
    
    /// Base fee in gwei (for EIP-1559)
    pub base_fee_gwei: f64,
    
    /// When this was fetched
    pub fetched_at: Instant,
    
    /// Source of the data
    pub source: GasSource,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GasSource {
    Etherscan,
    RpcProvider,
    Fallback,
}

impl std::fmt::Display for GasSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GasSource::Etherscan => write!(f, "Etherscan"),
            GasSource::RpcProvider => write!(f, "RPC"),
            GasSource::Fallback => write!(f, "Fallback"),
        }
    }
}

impl GasPriceInfo {
    pub fn is_stale(&self) -> bool {
        self.fetched_at.elapsed() > Duration::from_secs(CACHE_DURATION_SECS)
    }
    
    /// Get recommended gas price for MEV (fast + 10% buffer)
    pub fn mev_gas_price_gwei(&self) -> f64 {
        self.fast_gwei * 1.1
    }
    
    /// Estimate gas cost in USD for a given gas amount
    pub fn estimate_cost_usd(&self, gas_units: u64, eth_price_usd: f64) -> f64 {
        let gas_eth = (gas_units as f64) * self.gas_price_gwei * 1e-9;
        gas_eth * eth_price_usd
    }
}

// ============================================
// GAS ORACLE
// ============================================

pub struct GasOracle {
    http_client: Client,
    api_key: Option<String>,
    chain_id: u64,
    rpc_url: String,
    cache: Arc<RwLock<Option<GasPriceInfo>>>,
}

impl GasOracle {
    /// Create a new GasOracle
    pub fn new(api_key: Option<String>, chain_id: u64, rpc_url: String) -> Self {
        let http_client = Client::builder()
            .timeout(Duration::from_secs(API_TIMEOUT_SECS))
            .build()
            .expect("Failed to create HTTP client");
        
        Self {
            http_client,
            api_key,
            chain_id,
            rpc_url,
            cache: Arc::new(RwLock::new(None)),
        }
    }
    
    /// Create from environment variables
    pub fn from_env(rpc_url: String) -> Self {
        let api_key = std::env::var("ETHERSCAN_API_KEY").ok();
        let chain_id = std::env::var("CHAIN_ID")
            .unwrap_or_else(|_| "1".to_string())
            .parse()
            .unwrap_or(1);
        
        Self::new(api_key, chain_id, rpc_url)
    }
    
    /// Get current gas price (with caching)
    pub async fn get_gas_price(&self) -> GasPriceInfo {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(ref info) = *cache {
                if !info.is_stale() {
                    trace!("Using cached gas price: {:.2} gwei", info.gas_price_gwei);
                    return info.clone();
                }
            }
        }
        
        // Fetch fresh data
        let info = self.fetch_gas_price().await;
        
        // Update cache
        {
            let mut cache = self.cache.write().await;
            *cache = Some(info.clone());
        }
        
        info
    }
    
    /// Fetch gas price (tries Etherscan first, then RPC, then fallback)
    async fn fetch_gas_price(&self) -> GasPriceInfo {
        // Try Etherscan first (if we have an API key)
        if let Some(ref api_key) = self.api_key {
            match self.fetch_from_etherscan(api_key).await {
                Ok(info) => {
                    debug!(
                        "⛽ Gas from Etherscan: {:.2} gwei (safe: {:.2}, fast: {:.2})",
                        info.gas_price_gwei, info.safe_gwei, info.fast_gwei
                    );
                    return info;
                }
                Err(e) => {
                    warn!("Etherscan gas fetch failed: {}", e);
                }
            }
        }
        
        // Fallback to RPC provider
        match self.fetch_from_rpc().await {
            Ok(info) => {
                debug!("⛽ Gas from RPC: {:.2} gwei", info.gas_price_gwei);
                return info;
            }
            Err(e) => {
                warn!("RPC gas fetch failed: {}", e);
            }
        }
        
        // Last resort: use fallback
        warn!("Using fallback gas price: {:.2} gwei", FALLBACK_GAS_GWEI);
        GasPriceInfo {
            gas_price_gwei: FALLBACK_GAS_GWEI,
            safe_gwei: FALLBACK_GAS_GWEI * 0.8,
            standard_gwei: FALLBACK_GAS_GWEI,
            fast_gwei: FALLBACK_GAS_GWEI * 1.2,
            base_fee_gwei: FALLBACK_GAS_GWEI * 0.7,
            fetched_at: Instant::now(),
            source: GasSource::Fallback,
        }
    }
    
    /// Fetch gas price from Etherscan API
    async fn fetch_from_etherscan(&self, api_key: &str) -> Result<GasPriceInfo> {
        // Use eth_gasPrice endpoint
        let url = format!(
            "{}?chainid={}&module=proxy&action=eth_gasPrice&apikey={}",
            ETHERSCAN_API_URL,
            self.chain_id,
            api_key
        );
        
        let response: EtherscanResponse = self.http_client
            .get(&url)
            .send()
            .await?
            .json()
            .await?;
        
        if let Some(error) = response.error {
            return Err(eyre!("Etherscan error: {} (code {})", error.message, error.code));
        }
        
        let result = response.result
            .ok_or_else(|| eyre!("No result from Etherscan"))?;
        
        // Parse hex gas price (in wei)
        let gas_wei = u128::from_str_radix(result.trim_start_matches("0x"), 16)
            .map_err(|e| eyre!("Failed to parse gas price: {}", e))?;
        
        let gas_gwei = (gas_wei as f64) / 1e9;
        
        // Validate
        let gas_gwei = gas_gwei.clamp(MIN_GAS_GWEI, MAX_GAS_GWEI);
        
        Ok(GasPriceInfo {
            gas_price_gwei: gas_gwei,
            safe_gwei: gas_gwei * 0.8,
            standard_gwei: gas_gwei,
            fast_gwei: gas_gwei * 1.2,
            base_fee_gwei: gas_gwei * 0.7,
            fetched_at: Instant::now(),
            source: GasSource::Etherscan,
        })
    }
    
    /// Fetch gas price from RPC provider
    async fn fetch_from_rpc(&self) -> Result<GasPriceInfo> {
        use alloy_provider::{Provider, ProviderBuilder};
        
        let provider = ProviderBuilder::new()
            .on_http(self.rpc_url.parse()?);
        
        let gas_price_wei = provider.get_gas_price().await?;
        let gas_gwei = (gas_price_wei as f64) / 1e9;
        
        // Validate
        let gas_gwei = gas_gwei.clamp(MIN_GAS_GWEI, MAX_GAS_GWEI);
        
        Ok(GasPriceInfo {
            gas_price_gwei: gas_gwei,
            safe_gwei: gas_gwei * 0.8,
            standard_gwei: gas_gwei,
            fast_gwei: gas_gwei * 1.2,
            base_fee_gwei: gas_gwei * 0.7,
            fetched_at: Instant::now(),
            source: GasSource::RpcProvider,
        })
    }
    
    /// Get gas tracker data (safe/standard/fast prices)
    /// Note: This uses 1 API call, but gives more detail
    #[allow(dead_code)]
    pub async fn get_gas_tracker(&self) -> Result<GasPriceInfo> {
        let api_key = self.api_key.as_ref()
            .ok_or_else(|| eyre!("No Etherscan API key configured"))?;
        
        let url = format!(
            "{}?chainid={}&module=gastracker&action=gasoracle&apikey={}",
            ETHERSCAN_API_URL,
            self.chain_id,
            api_key
        );
        
        let response: GasTrackerResponse = self.http_client
            .get(&url)
            .send()
            .await?
            .json()
            .await?;
        
        if response.status != "1" {
            return Err(eyre!("Gas tracker failed: {}", response.message));
        }
        
        let result = response.result
            .ok_or_else(|| eyre!("No gas tracker result"))?;
        
        let safe_gwei: f64 = result.safe_gas_price
            .and_then(|s| s.parse().ok())
            .unwrap_or(FALLBACK_GAS_GWEI);
        
        let standard_gwei: f64 = result.propose_gas_price
            .and_then(|s| s.parse().ok())
            .unwrap_or(FALLBACK_GAS_GWEI);
        
        let fast_gwei: f64 = result.fast_gas_price
            .and_then(|s| s.parse().ok())
            .unwrap_or(FALLBACK_GAS_GWEI);
        
        let base_fee_gwei: f64 = result.suggest_base_fee
            .and_then(|s| s.parse().ok())
            .unwrap_or(standard_gwei * 0.7);
        
        Ok(GasPriceInfo {
            gas_price_gwei: standard_gwei,
            safe_gwei: safe_gwei.clamp(MIN_GAS_GWEI, MAX_GAS_GWEI),
            standard_gwei: standard_gwei.clamp(MIN_GAS_GWEI, MAX_GAS_GWEI),
            fast_gwei: fast_gwei.clamp(MIN_GAS_GWEI, MAX_GAS_GWEI),
            base_fee_gwei: base_fee_gwei.clamp(MIN_GAS_GWEI, MAX_GAS_GWEI),
            fetched_at: Instant::now(),
            source: GasSource::Etherscan,
        })
    }
}

// ============================================
// TESTS
// ============================================

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_gas_price_info_stale() {
        let info = GasPriceInfo {
            gas_price_gwei: 20.0,
            safe_gwei: 16.0,
            standard_gwei: 20.0,
            fast_gwei: 24.0,
            base_fee_gwei: 14.0,
            fetched_at: Instant::now(),
            source: GasSource::Fallback,
        };
        
        assert!(!info.is_stale());
    }
    
    #[test]
    fn test_estimate_cost() {
        let info = GasPriceInfo {
            gas_price_gwei: 20.0,
            safe_gwei: 16.0,
            standard_gwei: 20.0,
            fast_gwei: 24.0,
            base_fee_gwei: 14.0,
            fetched_at: Instant::now(),
            source: GasSource::Fallback,
        };
        
        // 200,000 gas at 20 gwei, ETH = $3500
        // = 200000 * 20 * 1e-9 ETH = 0.004 ETH
        // = 0.004 * 3500 = $14
        let cost = info.estimate_cost_usd(200_000, 3500.0);
        assert!((cost - 14.0).abs() < 0.01);
    }
}