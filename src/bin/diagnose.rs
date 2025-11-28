//! Diagnostic tool - Check system status
//!
//! Run with: cargo run --bin diagnose

use std::env;

fn main() {
    println!("🔍 SNIPER DIAGNOSTIC CHECK\n");
    
    // Load .env
    dotenvy::dotenv().ok();
    
    println!("═══════════════════════════════════════════════════");
    println!("                  CONFIGURATION                     ");
    println!("═══════════════════════════════════════════════════\n");
    
    // Key settings
    let checks = [
        ("EXECUTION_MODE", "simulation", "What mode are we in?"),
        ("MIN_PROFIT_USD", "20.0", "Minimum profit threshold"),
        ("MAX_HOPS", "4", "Maximum hops in cycle"),
        ("SCAN_INTERVAL_SECS", "12", "Seconds between scans"),
        ("MAX_GAS_GWEI", "50", "Maximum gas price"),
        ("SIMULATION_LOG", "true", "Log opportunities?"),
    ];
    
    for (key, default, desc) in checks {
        let value = env::var(key).unwrap_or_else(|_| default.to_string());
        let is_default = env::var(key).is_err();
        let marker = if is_default { "(default)" } else { "(from .env)" };
        println!("  {}: {} {}", key, value, marker);
        println!("    └─ {}\n", desc);
    }
    
    // RPC check
    let rpc = env::var("RPC_URL").unwrap_or_else(|_| "NOT SET".to_string());
    let rpc_display = if rpc.len() > 50 { 
        format!("{}...{}", &rpc[..30], &rpc[rpc.len()-15..])
    } else { 
        rpc.clone() 
    };
    println!("  RPC_URL: {}", rpc_display);
    
    // Flashbots check
    let fb_key = env::var("FLASHBOTS_SIGNER_KEY").is_ok();
    let profit_addr = env::var("PROFIT_WALLET_ADDRESS").is_ok();
    let executor = env::var("EXECUTOR_CONTRACT_ADDRESS").is_ok();
    
    println!("\n═══════════════════════════════════════════════════");
    println!("                PRODUCTION READINESS                ");
    println!("═══════════════════════════════════════════════════\n");
    
    println!("  FLASHBOTS_SIGNER_KEY:      {}", if fb_key { "✅ Set" } else { "❌ Not set" });
    println!("  PROFIT_WALLET_ADDRESS:     {}", if profit_addr { "✅ Set" } else { "❌ Not set" });
    println!("  EXECUTOR_CONTRACT_ADDRESS: {}", if executor { "✅ Set" } else { "❌ Not set" });
    
    let mode = env::var("EXECUTION_MODE").unwrap_or_else(|_| "simulation".to_string());
    
    println!("\n═══════════════════════════════════════════════════");
    println!("                     STATUS                         ");
    println!("═══════════════════════════════════════════════════\n");
    
    match mode.to_lowercase().as_str() {
        "simulation" => {
            println!("  📋 SIMULATION MODE");
            println!("     → Bot finds opportunities but does NOT execute");
            println!("     → Flash loans: NOT used");
            println!("     → Flashbots: NOT used");
            println!("     → Your money: SAFE");
        }
        "dry_run" | "dryrun" => {
            println!("  🔬 DRY RUN MODE");
            println!("     → Bot builds Flashbots bundles but does NOT submit");
            println!("     → Flash loans: Simulated only");
            println!("     → Flashbots: Bundle simulation only");
            println!("     → Your money: SAFE");
        }
        "production" => {
            println!("  🚀 PRODUCTION MODE");
            println!("     → Bot WILL submit real transactions!");
            println!("     → Flash loans: ACTIVE");
            println!("     → Flashbots: ACTIVE");
            println!("     → Your money: AT RISK");
            
            if !fb_key || !profit_addr || !executor {
                println!("\n  ⚠️  WARNING: Production mode but missing required keys!");
                println!("     Bot will abort executions until configured.");
            }
        }
        _ => {
            println!("  ❓ Unknown mode: {}", mode);
        }
    }
    
    println!("\n═══════════════════════════════════════════════════");
    println!("                  WHAT TO EXPECT                    ");
    println!("═══════════════════════════════════════════════════\n");
    
    let min_profit: f64 = env::var("MIN_PROFIT_USD")
        .unwrap_or_else(|_| "20.0".to_string())
        .parse()
        .unwrap_or(20.0);
    
    println!("  With MIN_PROFIT_USD = ${:.2}:", min_profit);
    println!("  • In calm markets: 0-2 opportunities per day");
    println!("  • During volatility: 5-20+ opportunities per day");
    println!("  • Gas at 0.5 gwei: ~$0.50 per trade");
    println!("  • Gas at 20 gwei: ~$20 per trade");
    
    if min_profit < 2.0 {
        println!("\n  ⚠️  Low threshold! You'll see many opportunities that");
        println!("     may not be profitable after real-world slippage.");
    }
    
    println!("\n✅ Diagnostic complete!\n");
}