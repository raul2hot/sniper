//! Deployment Check Utility
//!
//! Run with: cargo run --bin deploy-check
//!
//! This verifies your production setup is complete and ready.

use std::env;
use std::str::FromStr;
use alloy_primitives::Address;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    
    println!();
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║          SNIPER PRODUCTION DEPLOYMENT CHECK                ║");
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();
    
    let mut issues: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    
    // ==========================================
    // CHECK 1: RPC URL
    // ==========================================
    println!("📡 CHECKING RPC CONNECTION...");
    
    let rpc_url = env::var("RPC_URL").unwrap_or_default();
    if rpc_url.is_empty() || rpc_url.contains("YOUR_API_KEY") {
        issues.push("RPC_URL not configured".to_string());
        println!("   ❌ RPC_URL: Not configured");
    } else {
        // Try to connect
        match check_rpc(&rpc_url).await {
            Ok(block) => println!("   ✅ RPC connected, current block: {}", block),
            Err(e) => {
                issues.push(format!("RPC connection failed: {}", e));
                println!("   ❌ RPC connection failed: {}", e);
            }
        }
    }
    println!();
    
    // ==========================================
    // CHECK 2: Flashbots Signer
    // ==========================================
    println!("🔐 CHECKING FLASHBOTS SIGNER...");
    
    let fb_key = env::var("FLASHBOTS_SIGNER_KEY").unwrap_or_default();
    if fb_key.is_empty() {
        issues.push("FLASHBOTS_SIGNER_KEY not set".to_string());
        println!("   ❌ FLASHBOTS_SIGNER_KEY: Not configured");
        println!("   💡 Run: cargo run --bin generate-wallet");
    } else {
        // Validate key format
        let key = fb_key.trim_start_matches("0x");
        if key.len() != 64 {
            issues.push("FLASHBOTS_SIGNER_KEY invalid format".to_string());
            println!("   ❌ FLASHBOTS_SIGNER_KEY: Invalid format (should be 64 hex chars)");
        } else {
            match alloy_signer_local::PrivateKeySigner::from_str(key) {
                Ok(signer) => {
                    println!("   ✅ FLASHBOTS_SIGNER_KEY: {:?}", signer.address());
                }
                Err(e) => {
                    issues.push(format!("FLASHBOTS_SIGNER_KEY parse error: {}", e));
                    println!("   ❌ FLASHBOTS_SIGNER_KEY: Parse error - {}", e);
                }
            }
        }
    }
    println!();
    
    // ==========================================
    // CHECK 3: Executor Contract
    // ==========================================
    println!("📜 CHECKING EXECUTOR CONTRACT...");
    
    let executor_addr = env::var("EXECUTOR_CONTRACT_ADDRESS").unwrap_or_default();
    if executor_addr.is_empty() {
        issues.push("EXECUTOR_CONTRACT_ADDRESS not set".to_string());
        println!("   ❌ EXECUTOR_CONTRACT_ADDRESS: Not deployed");
        println!("   💡 See DEPLOYMENT.md for deployment instructions");
    } else {
        match Address::from_str(&executor_addr) {
            Ok(addr) => {
                // Check if contract exists
                if !rpc_url.is_empty() && !rpc_url.contains("YOUR_API_KEY") {
                    match check_contract(&rpc_url, addr).await {
                        Ok(true) => println!("   ✅ EXECUTOR_CONTRACT: {:?} (code exists)", addr),
                        Ok(false) => {
                            issues.push("Executor contract has no code".to_string());
                            println!("   ❌ EXECUTOR_CONTRACT: {:?} (NO CODE - not deployed?)", addr);
                        }
                        Err(e) => {
                            warnings.push(format!("Could not verify contract: {}", e));
                            println!("   ⚠️  EXECUTOR_CONTRACT: {:?} (verification failed)", addr);
                        }
                    }
                } else {
                    println!("   ⚠️  EXECUTOR_CONTRACT: {:?} (cannot verify without RPC)", addr);
                }
            }
            Err(_) => {
                issues.push("EXECUTOR_CONTRACT_ADDRESS invalid format".to_string());
                println!("   ❌ EXECUTOR_CONTRACT_ADDRESS: Invalid address format");
            }
        }
    }
    println!();
    
    // ==========================================
    // CHECK 4: Profit Wallet
    // ==========================================
    println!("💰 CHECKING PROFIT WALLET...");
    
    let profit_addr = env::var("PROFIT_WALLET_ADDRESS").unwrap_or_default();
    let profit_key = env::var("PROFIT_WALLET_PRIVATE_KEY").unwrap_or_default();
    
    if profit_addr.is_empty() {
        warnings.push("PROFIT_WALLET_ADDRESS not set".to_string());
        println!("   ⚠️  PROFIT_WALLET_ADDRESS: Not configured");
        println!("   💡 Profits will stay in executor contract until withdrawn");
    } else {
        match Address::from_str(&profit_addr) {
            Ok(addr) => println!("   ✅ PROFIT_WALLET_ADDRESS: {:?}", addr),
            Err(_) => {
                issues.push("PROFIT_WALLET_ADDRESS invalid".to_string());
                println!("   ❌ PROFIT_WALLET_ADDRESS: Invalid format");
            }
        }
    }
    
    if profit_key.is_empty() {
        // This is actually fine for most setups
        println!("   ℹ️  PROFIT_WALLET_PRIVATE_KEY: Not set (only needed for auto-withdrawal)");
    } else {
        let key = profit_key.trim_start_matches("0x");
        if key.len() != 64 {
            issues.push("PROFIT_WALLET_PRIVATE_KEY invalid".to_string());
            println!("   ❌ PROFIT_WALLET_PRIVATE_KEY: Invalid format");
        } else {
            println!("   ✅ PROFIT_WALLET_PRIVATE_KEY: Configured (be careful!)");
        }
    }
    println!();
    
    // ==========================================
    // CHECK 5: Gas Oracle
    // ==========================================
    println!("⛽ CHECKING GAS ORACLE...");
    
    let etherscan_key = env::var("ETHERSCAN_API_KEY").unwrap_or_default();
    if etherscan_key.is_empty() {
        warnings.push("ETHERSCAN_API_KEY not set".to_string());
        println!("   ⚠️  ETHERSCAN_API_KEY: Not set (using RPC for gas prices)");
        println!("   💡 Get a free key at https://etherscan.io/apis");
    } else {
        println!("   ✅ ETHERSCAN_API_KEY: Configured");
    }
    println!();
    
    // ==========================================
    // CHECK 6: Execution Mode
    // ==========================================
    println!("🎮 CHECKING EXECUTION MODE...");
    
    let mode = env::var("EXECUTION_MODE").unwrap_or_else(|_| "simulation".to_string());
    match mode.to_lowercase().as_str() {
        "simulation" => {
            println!("   ℹ️  Mode: SIMULATION (safe, no real transactions)");
        }
        "dry_run" | "dryrun" => {
            println!("   ℹ️  Mode: DRY_RUN (builds bundles, doesn't submit)");
        }
        "production" => {
            println!("   ⚠️  Mode: PRODUCTION (REAL TRANSACTIONS!)");
            if !issues.is_empty() {
                issues.push("Production mode with unresolved issues".to_string());
            }
        }
        _ => {
            warnings.push(format!("Unknown mode: {}", mode));
            println!("   ⚠️  Mode: Unknown ({}), defaulting to simulation", mode);
        }
    }
    println!();
    
    // ==========================================
    // CHECK 7: Profit Thresholds
    // ==========================================
    println!("📊 CHECKING THRESHOLDS...");
    
    let min_profit: f64 = env::var("MIN_PROFIT_USD")
        .unwrap_or_else(|_| "20.0".to_string())
        .parse()
        .unwrap_or(20.0);
    
    let max_gas: u64 = env::var("MAX_GAS_GWEI")
        .unwrap_or_else(|_| "50".to_string())
        .parse()
        .unwrap_or(50);
    
    let bribe_pct: f64 = env::var("MINER_BRIBE_PCT")
        .unwrap_or_else(|_| "90.0".to_string())
        .parse()
        .unwrap_or(90.0);
    
    println!("   MIN_PROFIT_USD: ${:.2}", min_profit);
    if min_profit < 2.0 {
        warnings.push("MIN_PROFIT_USD is very low".to_string());
        println!("     ⚠️  This is quite low, you may see false positives");
    }
    
    println!("   MAX_GAS_GWEI: {} gwei", max_gas);
    println!("   MINER_BRIBE_PCT: {:.0}%", bribe_pct);
    if bribe_pct > 95.0 {
        warnings.push("MINER_BRIBE_PCT is very high".to_string());
        println!("     ⚠️  You're keeping less than 5% of profits");
    }
    println!();
    
    // ==========================================
    // SUMMARY
    // ==========================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    
    if issues.is_empty() && warnings.is_empty() {
        println!("✅ ALL CHECKS PASSED!");
        println!();
        println!("   Your system is ready for production.");
        println!("   Set EXECUTION_MODE=production when ready.");
    } else if issues.is_empty() {
        println!("⚠️  READY WITH WARNINGS ({} warnings)", warnings.len());
        println!();
        for w in &warnings {
            println!("   • {}", w);
        }
        println!();
        println!("   You can proceed, but consider fixing the warnings.");
    } else {
        println!("❌ NOT READY ({} issues, {} warnings)", issues.len(), warnings.len());
        println!();
        println!("   MUST FIX:");
        for i in &issues {
            println!("   • {}", i);
        }
        if !warnings.is_empty() {
            println!();
            println!("   WARNINGS:");
            for w in &warnings {
                println!("   • {}", w);
            }
        }
        println!();
        println!("   Fix the issues above before going to production.");
    }
    
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
}

async fn check_rpc(url: &str) -> Result<u64, String> {
    use alloy_provider::{Provider, ProviderBuilder};
    
    let provider = ProviderBuilder::new()
        .on_http(url.parse().map_err(|e| format!("Invalid URL: {}", e))?)
        ;
    
    provider.get_block_number().await
        .map_err(|e| format!("Connection failed: {}", e))
}

async fn check_contract(url: &str, address: Address) -> Result<bool, String> {
    use alloy_provider::{Provider, ProviderBuilder};
    
    let provider = ProviderBuilder::new()
        .on_http(url.parse().map_err(|e| format!("Invalid URL: {}", e))?);
    
    let code = provider.get_code_at(address).await
        .map_err(|e| format!("Failed to get code: {}", e))?;
    
    Ok(!code.is_empty())
}
