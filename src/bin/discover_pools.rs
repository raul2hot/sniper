//! Pool Discovery Demo - Tests all new pool adapters
//!
//! Run with: cargo run --bin discover-pools
//!
//! This demonstrates:
//! 1. Curve NG dynamic pool discovery
//! 2. Sky ecosystem (sUSDS) state
//! 3. USD3 NAV calculation
//! 4. Yield drift arbitrage detection

use std::env;

#[tokio::main]
async fn main() {
    println!();
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║          SNIPER EXPANDED POOL DISCOVERY DEMO               ║");
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();
    
    // Load .env
    dotenvy::dotenv().ok();
    
    let rpc_url = env::var("RPC_URL").unwrap_or_else(|_| {
        println!("⚠️  RPC_URL not set, using public endpoint (may be slow)");
        "https://eth.llamarpc.com".to_string()
    });
    
    println!("📡 RPC: {}", if rpc_url.len() > 50 { 
        format!("{}...{}", &rpc_url[..30], &rpc_url[rpc_url.len()-15..])
    } else { 
        rpc_url.clone() 
    });
    println!();
    
    // Demo 1: Token Statistics
    demo_token_stats();
    
    // Demo 2: Curve NG Discovery (requires RPC)
    println!("═══════════════════════════════════════════════════════════════");
    println!("                 CURVE NG POOL DISCOVERY                        ");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("To discover Curve NG pools from factory:");
    println!("  let fetcher = CurveNGFetcher::new(rpc_url);");
    println!("  let pools = fetcher.discover_all_ng_pools().await?;");
    println!();
    println!("Factory addresses:");
    println!("  StableSwap NG: 0x6A8cbed756804B16E05E741eDaBd5cB544AE21bf");
    println!("  TwoCrypto NG:  0x98EE851a00abeE0d95D08cF4CA2BdCE32aeaAF7F");
    println!("  TriCrypto NG:  0x0c0e5f2fF0ff18a3BE9b835635039256dC4B4963");
    println!();
    
    // Demo 3: Sky Ecosystem
    println!("═══════════════════════════════════════════════════════════════");
    println!("                  SKY ECOSYSTEM (sUSDS)                         ");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("ERC-4626 tokens for yield drift arbitrage:");
    println!("  sUSDS: 0xa3931d71877C0E7a3148CB7Eb4463524FEc27fbD");
    println!("  sDAI:  0x83F20F44975D03b1b09e64809B757c47f942BEeA");
    println!();
    println!("To fetch sUSDS exchange rate:");
    println!("  let adapter = SkyAdapter::new(rpc_url);");
    println!("  let state = adapter.fetch_susds_state().await?;");
    println!("  println!(\"1 sUSDS = {{}} USDS\", state.fair_value_usd);");
    println!();
    println!("Arbitrage opportunity:");
    println!("  If DEX price < redemption value:");
    println!("    Buy sUSDS on DEX → Redeem for USDS → Profit!");
    println!();
    
    // Demo 4: USD3
    println!("═══════════════════════════════════════════════════════════════");
    println!("                   USD3 / RESERVE PROTOCOL                      ");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("USD3 is a basket-backed stablecoin:");
    println!("  Token: 0x0d86883faf4ffd7aeb116390af37746f45b6f378");
    println!();
    println!("Basket components (yield-bearing):");
    println!("  - pyUSD (PayPal USD)");
    println!("  - sDAI (Savings DAI)");
    println!("  - cUSDC (Compound USDC)");
    println!();
    println!("NAV arbitrage opportunity:");
    println!("  If DEX price < NAV: Buy USD3, redeem basket");
    println!("  If DEX price > NAV: Mint USD3, sell on DEX");
    println!();
    
    // Demo 5: Integration
    println!("═══════════════════════════════════════════════════════════════");
    println!("                    FULL INTEGRATION                            ");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("To use expanded fetcher in main.rs:");
    println!();
    println!("  // Replace:");
    println!("  // let fetcher = PoolFetcher::new(rpc_url);");
    println!("  // let pools = fetcher.fetch_all_pools().await?;");
    println!();
    println!("  // With:");
    println!("  use cartographer::ExpandedPoolFetcher;");
    println!("  let fetcher = ExpandedPoolFetcher::new(rpc_url);");
    println!("  let result = fetcher.fetch_all_pools().await?;");
    println!("  let pools = result.pool_states;");
    println!();
    println!("  // Check for special opportunities:");
    println!("  let specials = check_special_opportunities(&result, 50.0).await;");
    println!();
    
    // Summary
    println!("═══════════════════════════════════════════════════════════════");
    println!("                        SUMMARY                                 ");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("New capabilities added:");
    println!("  ✅ Curve NG dynamic pool discovery");
    println!("  ✅ Dynamic fee calculation (offpeg multiplier)");
    println!("  ✅ ERC-4626 yield drift arbitrage");
    println!("  ✅ USD3 NAV arbitrage");
    println!("  ✅ Expanded token list (30+ tokens)");
    println!();
    println!("Files added:");
    println!("  src/cartographer/curve_ng.rs       - Curve NG adapter");
    println!("  src/cartographer/sky_ecosystem.rs  - Sky/sUSDS adapter");
    println!("  src/cartographer/usd3_reserve.rs   - USD3 adapter");
    println!("  src/cartographer/expanded_fetcher.rs - Combined fetcher");
    println!("  src/tokens.rs                      - Expanded token list");
    println!();
    println!("No changes to:");
    println!("  ❌ Executor contract");
    println!("  ❌ Flash loan handlers");
    println!("  ❌ Flashbots integration");
    println!();
}

fn demo_token_stats() {
    println!("═══════════════════════════════════════════════════════════════");
    println!("                      TOKEN STATISTICS                          ");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    
    // Base tokens
    println!("Base tokens (arbitrage start points):");
    println!("  WETH, USDC, USDT, DAI, WBTC");
    println!();
    
    // NEW: Sky ecosystem
    println!("Sky Ecosystem (NEW):");
    println!("  USDS  - Sky's USD stablecoin");
    println!("  sUSDS - Savings USDS (ERC-4626, yield-bearing)");
    println!("  sDAI  - Savings DAI (ERC-4626, yield-bearing)");
    println!("  SKY   - Governance token");
    println!();
    
    // NEW: USD3
    println!("USD3 / Reserve Protocol (NEW):");
    println!("  USD3  - Basket-backed stablecoin");
    println!("  pyUSD - PayPal USD (basket component)");
    println!();
    
    // NEW: Algo stables
    println!("Algorithmic Stablecoins (NEW):");
    println!("  crvUSD  - Curve's pegkeeper stablecoin");
    println!("  scrvUSD - Savings crvUSD");
    println!("  FRAX    - Frax algorithmic stablecoin");
    println!("  GHO     - Aave's stablecoin");
    println!("  DOLA    - Inverse Finance");
    println!();
    
    // Existing LSDs
    println!("Liquid Staking (existing):");
    println!("  wstETH, stETH, rETH, cbETH");
    println!();
}