//! The Sniper - Arbitrage Detection Bot (Phase 4: FULL SIMULATION Edition)
//!
//! Run with: cargo run
//!
//! CHANGES:
//! - Batched RPC calls (10 calls instead of 240)
//! - Better console display with PnL table
//! - Full simulation mode that simulates flash loan execution
//! - More verbose logging even when no opportunities

use alloy_primitives::Address;
use color_eyre::eyre::Result;
use console::style;
use std::collections::HashMap;
use std::time::Instant;
use tracing::{info, warn, error, debug};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod brain;
mod cartographer;
mod config;
mod tokens;
mod simulator;
mod executor;

use brain::{BoundedBellmanFord, ProfitFilter, ArbitrageCycle};
use cartographer::{ArbitrageGraph, PoolFetcher, Dex, PoolType};
use config::{Config, ExecutionMode};
use simulator::SwapSimulator;
use executor::ExecutionEngine;

fn print_banner() {
    println!();
    println!(
        "{}",
        style("═══════════════════════════════════════════════════════════════").cyan()
    );
    println!(
        "{}",
        style(" 🎯 THE SNIPER - Arbitrage Detection Bot v0.4").cyan().bold()
    );
    println!(
        "{}",
        style("    Batched RPC | Full Simulation | Live PnL Display").cyan()
    );
    println!(
        "{}",
        style("═══════════════════════════════════════════════════════════════").cyan()
    );
    println!();
}

/// Safely truncate a UTF-8 string to approximately `max_chars` characters
/// Handles multi-byte characters like → properly
fn truncate_utf8(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = chars.into_iter().take(max_chars).collect();
        format!("{}...", truncated)
    }
}

fn build_token_symbols() -> HashMap<Address, &'static str> {
    let mut map = HashMap::new();

    let tokens = [
        ("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", "WETH"),
        ("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48", "USDC"),
        ("0xdAC17F958D2ee523a2206206994597C13D831ec7", "USDT"),
        ("0x6B175474E89094C44Da98b954EedcdeCB5BE3830", "DAI"),
        ("0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599", "WBTC"),
        ("0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0", "wstETH"),
        ("0xae7ab96520DE3A18E5e111B5EaAb095312D7fE84", "stETH"),
        ("0x514910771AF9Ca656af840dff83E8264EcF986CA", "LINK"),
        ("0x1f9840a85d5aF5bf1D1762F925BDADdC4201F984", "UNI"),
        ("0x6982508145454Ce325dDbE47a25d4ec3d2311933", "PEPE"),
        ("0x95aD61b0a150d79219dCF64E1E6Cc01f0B64C4cE", "SHIB"),
        ("0x5A98FcBEA516Cf06857215779Fd812CA3beF1B32", "LDO"),
        ("0x9f8F72aA9304c8B593d555F12eF6589cC3A579A2", "MKR"),
        ("0x7D1AfA7B718fb893dB30A3aBc0Cfc608AaCfeBB0", "MATIC"),
        ("0x6B3595068778DD592e39A122f4f5a5cF09C90fE2", "SUSHI"),
        ("0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DDaE9", "AAVE"),
        ("0xD533a949740bb3306d119CC777fa900bA034cd52", "CRV"),
        ("0xc011a73ee8576Fb46F5E1c5751cA3B9Fe0af2a6F", "SNX"),
        ("0xc00e94Cb662C3520282E6f5717214004A7f26888", "COMP"),
    ];

    for (addr, symbol) in tokens {
        if let Ok(address) = addr.parse() {
            map.insert(address, symbol);
        }
    }

    map
}

fn format_token(addr: &Address, symbols: &HashMap<Address, &str>) -> String {
    if let Some(symbol) = symbols.get(addr) {
        symbol.to_string()
    } else {
        format!("0x{}...", &format!("{:?}", addr)[2..8])
    }
}

fn get_eth_price_from_pools(pools: &[cartographer::PoolState]) -> f64 {
    for pool in pools {
        let price = pool.price(6, 18);
        if price > 1000.0 && price < 10000.0 {
            return price;
        }
        let inverse = pool.price(18, 6);
        if inverse > 1000.0 && inverse < 10000.0 {
            return inverse;
        }
    }
    3500.0
}

/// Print a fancy table of the top cycles found
fn print_cycle_table(
    cycles: &[ArbitrageCycle], 
    symbols: &HashMap<Address, &str>,
    filter: &ProfitFilter,
    title: &str,
    max_rows: usize,
) {
    if cycles.is_empty() {
        return;
    }

    println!();
    println!("{}", style(format!("┌─────────────────────────────────────────────────────────────────────────────────┐")).dim());
    println!("{}", style(format!("│ {:^79} │", title)).yellow().bold());
    println!("{}", style(format!("├─────────────────────────────────────────────────────────────────────────────────┤")).dim());
    println!("{}", style(format!("│ {:^3} │ {:^30} │ {:^12} │ {:^10} │ {:^12} │", 
        "#", "PATH", "DEXes", "RETURN", "EST. PnL")).dim());
    println!("{}", style(format!("├─────────────────────────────────────────────────────────────────────────────────┤")).dim());

    for (i, cycle) in cycles.iter().take(max_rows).enumerate() {
        let path = cycle.path.iter()
            .map(|a| format_token(a, symbols))
            .collect::<Vec<_>>()
            .join("→");
        
        let path_display = truncate_utf8(&path, 25);

        let dex_str: String = cycle.dexes.iter()
            .map(|d| match d {
                Dex::UniswapV3 => "U3",
                Dex::UniswapV2 => "U2",
                Dex::SushiswapV2 => "S2",
                Dex::PancakeSwapV3 => "P3",
                Dex::BalancerV2 => "B2",
                _ => "??",
            })
            .collect::<Vec<_>>()
            .join("-");

        let analysis = filter.analyze(cycle, None);
        let return_str = format!("{:.4}x", cycle.expected_return);
        let pnl_str = format!("${:+.2}", analysis.net_profit_usd);

        let row_style = if analysis.is_profitable {
            style(format!("│ {:>3} │ {:^30} │ {:^12} │ {:^10} │ {:^12} │",
                i + 1, path_display, dex_str, return_str, pnl_str)).green()
        } else if analysis.net_profit_usd > -10.0 {
            style(format!("│ {:>3} │ {:^30} │ {:^12} │ {:^10} │ {:^12} │",
                i + 1, path_display, dex_str, return_str, pnl_str)).yellow()
        } else {
            style(format!("│ {:>3} │ {:^30} │ {:^12} │ {:^10} │ {:^12} │",
                i + 1, path_display, dex_str, return_str, pnl_str)).dim()
        };

        println!("{}", row_style);
    }

    if cycles.len() > max_rows {
        println!("{}", style(format!("│ {:^79} │", 
            format!("... and {} more cycles", cycles.len() - max_rows))).dim());
    }

    println!("{}", style(format!("└─────────────────────────────────────────────────────────────────────────────────┘")).dim());
}

/// Print market summary
fn print_market_summary(pools: &[cartographer::PoolState], eth_price: f64, gas_gwei: f64) {
    let v3_count = pools.iter().filter(|p| p.pool_type == PoolType::V3).count();
    let v2_count = pools.iter().filter(|p| matches!(p.pool_type, PoolType::V2 | PoolType::Balancer)).count();
    
    let total_liquidity_est: f64 = pools.iter()
        .filter(|p| p.pool_type == PoolType::V3)
        .map(|p| p.liquidity as f64)
        .sum::<f64>() / 1e18 * eth_price;

    println!();
    println!("{}", style("┌─────────────────── MARKET SNAPSHOT ───────────────────┐").cyan());
    println!("{}", style(format!("│  ETH Price: ${:<12.2}  Gas: {:<8.2} gwei          │", eth_price, gas_gwei)).cyan());
    println!("{}", style(format!("│  Pools: {} V3, {} V2           Est. TVL: ${:<.0}M   │", v3_count, v2_count, total_liquidity_est / 1e6)).cyan());
    println!("{}", style("└───────────────────────────────────────────────────────┘").cyan());
}

/// Statistics tracking across scans
struct ScanStats {
    total_scans: u64,
    profitable_found: u64,
    total_cycles_analyzed: u64,
    last_profitable_scan: Option<u64>,
    simulations_run: u64,
    flash_loan_sims: u64,
}

impl ScanStats {
    fn new() -> Self {
        Self {
            total_scans: 0,
            profitable_found: 0,
            total_cycles_analyzed: 0,
            last_profitable_scan: None,
            simulations_run: 0,
            flash_loan_sims: 0,
        }
    }
    
    fn record_scan(&mut self, cycles: usize, profitable: usize, simulations: usize, flash_sims: usize) {
        self.total_scans += 1;
        self.total_cycles_analyzed += cycles as u64;
        self.profitable_found += profitable as u64;
        self.simulations_run += simulations as u64;
        self.flash_loan_sims += flash_sims as u64;
        if profitable > 0 {
            self.last_profitable_scan = Some(self.total_scans);
        }
    }
    
    fn print_summary(&self) {
        println!();
        println!("{}", style("━━━━━━━━━━━━ CUMULATIVE STATISTICS ━━━━━━━━━━━━").yellow().bold());
        println!("  Total scans completed: {}", self.total_scans);
        println!("  Cycles analyzed: {}", self.total_cycles_analyzed);
        println!("  Simulations run: {}", self.simulations_run);
        println!("  Flash loan simulations: {}", self.flash_loan_sims);
        println!("  Profitable opportunities: {}", self.profitable_found);
        if let Some(last) = self.last_profitable_scan {
            println!("  Last profitable: scan #{}", last);
        } else {
            println!("  Last profitable: None yet (market is efficient)");
        }
        println!("{}", style("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━").yellow());
    }
}

/// Simulation result for a cycle
#[derive(Debug)]
struct CycleSimResult {
    cycle: ArbitrageCycle,
    sim: simulator::swap_simulator::ArbitrageSimulation,
    flash_loan_simulated: bool,
    execution_would_succeed: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("sniper=info".parse()?),
        )
        .init();

    print_banner();

    // Load configuration
    let config = Config::from_env()?;
    
    // Validate configuration
    if let Err(e) = config.validate() {
        error!("Configuration validation failed: {}", e);
        error!("Please check your .env file");
        return Err(e.into());
    }
    
    // Print configuration summary (only once at startup)
    config.print_summary();
    println!();

    let token_symbols = build_token_symbols();
    
    // Initialize execution engine once
    let engine = ExecutionEngine::new(config.clone());
    
    // Initialize stats tracking
    let mut stats = ScanStats::new();
    
    // Track consecutive failures for backoff
    let mut consecutive_failures = 0u32;
    
    info!("🚀 Starting continuous scanning loop...");
    info!("   Mode: {} (will simulate full trades)", config.execution_mode);
    info!("   Scan interval: {} seconds", config.scan_interval_secs);
    info!("   Press Ctrl+C to stop");
    println!();

    // ========================================
    // MAIN CONTINUOUS LOOP
    // ========================================
    loop {
        let scan_number = stats.total_scans + 1;
        
        // Check emergency stop
        if std::env::var("EMERGENCY_STOP").unwrap_or_default() == "true" || config.emergency_stop {
            warn!("🛑 Emergency stop is active. Waiting 60 seconds...");
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            continue;
        }
        
        println!();
        println!("{}", style(format!(
            "━━━━━━━━━━━━━━━━━━ SCAN #{} @ {} ━━━━━━━━━━━━━━━━━━",
            scan_number, 
            chrono::Utc::now().format("%H:%M:%S UTC")
        )).cyan().bold());
        
        let scan_start = Instant::now();
        
        // Run the scan
        match run_single_scan(&config, &token_symbols, &engine).await {
            Ok((cycles_count, profitable_count, sim_count, flash_sim_count)) => {
                consecutive_failures = 0;
                stats.record_scan(cycles_count, profitable_count, sim_count, flash_sim_count);
                
                let scan_duration = scan_start.elapsed();
                
                println!();
                if profitable_count > 0 {
                    println!(
                        "{}",
                        style(format!(
                            "🎯 SCAN #{} COMPLETE: {} PROFITABLE! ({} cycles, {} sims, {:?})",
                            scan_number, profitable_count, cycles_count, sim_count, scan_duration
                        )).green().bold()
                    );
                } else {
                    println!(
                        "✅ Scan #{} complete: {} cycles, {} simulations, 0 profitable ({:?})",
                        scan_number, cycles_count, sim_count, scan_duration
                    );
                    println!("   {} - Market is efficient, waiting for volatility...", 
                        style("No arbitrage opportunities").dim());
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                error!("❌ Scan #{} failed: {}", scan_number, e);
                
                stats.record_scan(0, 0, 0, 0);
                
                if consecutive_failures >= config.max_consecutive_failures {
                    warn!(
                        "⚠️ {} consecutive failures. Backing off for {} seconds...",
                        consecutive_failures, config.failure_pause_secs
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(config.failure_pause_secs)).await;
                    consecutive_failures = 0;
                }
            }
        }
        
        // Print cumulative stats every 10 scans
        if stats.total_scans % 10 == 0 {
            stats.print_summary();
        }
        
        // Wait for next scan
        println!();
        println!("💤 Next scan in {} seconds...", config.scan_interval_secs);
        tokio::time::sleep(tokio::time::Duration::from_secs(config.scan_interval_secs)).await;
    }
}

/// Run a single scan iteration
/// Returns (cycles_analyzed, profitable_count, simulations_run, flash_loan_sims)
async fn run_single_scan(
    config: &Config,
    token_symbols: &HashMap<Address, &'static str>,
    engine: &ExecutionEngine,
) -> Result<(usize, usize, usize, usize)> {
    
    // =============================================
    // PHASE 1: THE CARTOGRAPHER (Batched RPC)
    // =============================================
    let fetch_start = Instant::now();

    let fetcher = PoolFetcher::new(config.rpc_url.clone());
    let pools = fetcher.fetch_all_pools().await?;

    let fetch_time = fetch_start.elapsed();
    info!("📊 Fetched {} pools in {:?}", pools.len(), fetch_time);

    let eth_price = get_eth_price_from_pools(&pools);
    let gas_gwei = 0.5; // Low gas environment

    // Print market snapshot
    print_market_summary(&pools, eth_price, gas_gwei);

    // Build the graph
    let graph = ArbitrageGraph::from_pools(&pools);

    // =============================================
    // PHASE 2: THE BRAIN (Cycle Detection)
    // =============================================
    info!("🧠 Running Bellman-Ford algorithm...");

    let bellman_ford = BoundedBellmanFord::new(&graph, config.max_hops);
    let base_tokens = config.base_token_addresses();
    let all_cycles = bellman_ford.find_all_cycles(&base_tokens);

    // Filter out blacklisted cycles
    let cycles: Vec<_> = all_cycles
        .into_iter()
        .filter(|c| !config.is_cycle_blacklisted(&c.path))
        .collect();

    let cycles_count = cycles.len();

    // Create profit filter
    let mut filter = ProfitFilter::new(config.min_profit_usd);
    filter.set_eth_price(eth_price);
    filter.set_gas_price(gas_gwei);

    // Sort cycles by expected return
    let mut sorted_cycles = cycles.clone();
    sorted_cycles.sort_by(|a, b| {
        b.expected_return.partial_cmp(&a.expected_return)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Print top cycles table (even if not profitable)
    if !sorted_cycles.is_empty() {
        print_cycle_table(&sorted_cycles, token_symbols, &filter, "TOP ARBITRAGE CYCLES FOUND", 15);
    }

    // Get candidates for simulation
    let profitable_candidates = filter.filter_profitable(&sorted_cycles, token_symbols);

    // =============================================
    // PHASE 3: THE SIMULATOR (Full Trade Simulation)
    // =============================================
    let mut simulated_results: Vec<CycleSimResult> = Vec::new();
    let mut sim_count = 0usize;
    let mut flash_sim_count = 0usize;

    // Always simulate top candidates (even if filter says unprofitable)
    let candidates_to_simulate: Vec<_> = if profitable_candidates.is_empty() {
        // Simulate top 5 cycles anyway to show activity
        sorted_cycles.iter()
            .filter(|c| c.expected_return > 0.99 && c.expected_return < 1.05)
            .take(5)
            .cloned()
            .collect()
    } else {
        profitable_candidates.iter().take(10).map(|p| p.cycle.clone()).collect()
    };

    if !candidates_to_simulate.is_empty() {
        info!("🔬 Simulating {} candidate cycles...", candidates_to_simulate.len());
        
        match SwapSimulator::new(&config.rpc_url).await {
            Ok(mut swap_sim) => {
                swap_sim.set_eth_price(eth_price);
                swap_sim.set_gas_price(gas_gwei);
                
                for cycle in &candidates_to_simulate {
                    sim_count += 1;
                    
                    let path_str = cycle.path.iter()
                        .map(|a| format_token(a, token_symbols))
                        .collect::<Vec<_>>()
                        .join(" → ");
                    
                    // Determine simulation size based on liquidity tier
                    let tier = swap_sim.get_cycle_liquidity_tier(cycle);
                    let target_usd = tier.recommended_amount_usd().min(config.default_simulation_usd);
                    
                    info!("   Simulating: {} (${:.0}, {:?} tier)", path_str, target_usd, tier);
                    
                    let sim = swap_sim.simulate_cycle(cycle, target_usd).await;
                    
                    if sim.simulation_success {
                        let is_profitable = sim.is_profitable && sim.profit_usd >= config.min_profit_usd;
                        
                        // Log simulation results
                        if is_profitable {
                            info!(
                                "   {} Return: {:.4}x | Net: ${:.2} | Gas: {} units",
                                style("✓ PROFITABLE").green().bold(),
                                sim.return_multiplier(),
                                sim.profit_usd,
                                sim.total_gas_used
                            );
                        } else {
                            info!(
                                "   {} Return: {:.4}x | Net: ${:.2}",
                                style("○ Not profitable").dim(),
                                sim.return_multiplier(),
                                sim.profit_usd
                            );
                        }
                        
                        // =============================================
                        // PHASE 3.5: FLASH LOAN SIMULATION
                        // =============================================
                        // For profitable cycles, simulate the full flash loan execution
                        let mut flash_loan_simulated = false;
                        let mut execution_would_succeed = false;
                        
                        if is_profitable {
                            flash_sim_count += 1;
                            info!("   🔥 Simulating FULL FLASH LOAN execution...");
                            
                            // Simulate the execution flow
                            match engine.execute(cycle, &sim, 0).await {
                                Ok(result) => {
                                    flash_loan_simulated = true;
                                    execution_would_succeed = result.is_success();
                                    
                                    match &result {
                                        executor::ExecutionResult::Simulated { expected_profit_usd, would_execute } => {
                                            info!(
                                                "   {} Flash loan sim: Would profit ${:.2}, execute: {}",
                                                style("💰").green(),
                                                expected_profit_usd,
                                                would_execute
                                            );
                                        }
                                        executor::ExecutionResult::DryRun { simulation_passed, gas_used, .. } => {
                                            info!(
                                                "   {} Dry run: passed={}, gas={:?}",
                                                if *simulation_passed { style("✓").green() } else { style("✗").red() },
                                                simulation_passed,
                                                gas_used
                                            );
                                        }
                                        executor::ExecutionResult::Skipped { reason } => {
                                            info!("   ⏭️ Skipped: {}", reason);
                                        }
                                        executor::ExecutionResult::Aborted { reason } => {
                                            warn!("   ⚠️ Aborted: {}", reason);
                                        }
                                        executor::ExecutionResult::Failed { reason } => {
                                            warn!("   ❌ Failed: {}", reason);
                                        }
                                        _ => {
                                            info!("   Result: {:?}", result);
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("   ❌ Flash loan simulation error: {}", e);
                                }
                            }
                            
                            // Log to file
                            if config.simulation_log {
                                if let Err(e) = log_opportunity(config, cycle, &sim, flash_loan_simulated, execution_would_succeed) {
                                    warn!("Failed to log opportunity: {}", e);
                                } else {
                                    info!("   📝 Logged to {}", config.simulation_log_path);
                                }
                            }
                        }
                        
                        simulated_results.push(CycleSimResult {
                            cycle: cycle.clone(),
                            sim,
                            flash_loan_simulated,
                            execution_would_succeed,
                        });
                    } else {
                        info!(
                            "   {} Reverted: {}",
                            style("✗").red(),
                            sim.revert_reason.as_deref().unwrap_or("Unknown")
                        );
                    }
                }
            }
            Err(e) => {
                warn!("⚠️ Simulator init failed: {}", e);
            }
        }
    }

    // =============================================
    // PHASE 4: RESULTS SUMMARY
    // =============================================
    let profitable_count = simulated_results.iter()
        .filter(|r| r.sim.is_profitable && r.sim.profit_usd >= config.min_profit_usd)
        .count();

    if !simulated_results.is_empty() {
        println!();
        println!("{}", style("┌─────────────────── SIMULATION RESULTS ───────────────────┐").magenta());
        
        for (i, result) in simulated_results.iter().enumerate() {
            let path = result.cycle.path.iter()
                .map(|a| format_token(a, token_symbols))
                .collect::<Vec<_>>()
                .join(" → ");
            
            let status = if result.sim.is_profitable && result.sim.profit_usd >= config.min_profit_usd {
                if result.flash_loan_simulated && result.execution_would_succeed {
                    style("🚀 READY").green().bold()
                } else {
                    style("💰 PROFITABLE").green()
                }
            } else if result.sim.profit_usd > -10.0 {
                style("⚡ MARGINAL").yellow()
            } else {
                style("💸 LOSS").red()
            };
            
            println!(
                "│  {}. {} {} │ Net: {:>8} │ Flash: {} │",
                i + 1,
                status,
                truncate_utf8(&path, 22),
                format!("${:.2}", result.sim.profit_usd),
                if result.flash_loan_simulated { "✓" } else { "-" }
            );
        }
        
        println!("{}", style("└───────────────────────────────────────────────────────────┘").magenta());
    }

    Ok((cycles_count, profitable_count, sim_count, flash_sim_count))
}

/// Log a profitable opportunity to file with full details
fn log_opportunity(
    config: &Config,
    cycle: &ArbitrageCycle,
    sim: &simulator::swap_simulator::ArbitrageSimulation,
    flash_loan_simulated: bool,
    execution_would_succeed: bool,
) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    
    // Create parent directory if needed
    if let Some(parent) = std::path::Path::new(&config.simulation_log_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    let log_entry = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "path": cycle.path.iter().map(|a| format!("{:?}", a)).collect::<Vec<_>>(),
        "dexes": cycle.dexes.iter().map(|d| d.to_string()).collect::<Vec<_>>(),
        "pools": cycle.pools.iter().map(|a| format!("{:?}", a)).collect::<Vec<_>>(),
        "expected_return": cycle.expected_return,
        "input_usd": sim.input_usd,
        "gross_profit_usd": sim.profit_usd + (sim.total_gas_used as f64 * 0.5 * 1e-9 * 3500.0),
        "gas_cost_usd": sim.total_gas_used as f64 * 0.5 * 1e-9 * 3500.0,
        "net_profit_usd": sim.profit_usd,
        "gas_used": sim.total_gas_used,
        "liquidity_tier": format!("{:?}", sim.liquidity_tier),
        "flash_loan_simulated": flash_loan_simulated,
        "execution_would_succeed": execution_would_succeed,
        "simulation_details": {
            "input_amount": sim.input_amount.to_string(),
            "output_amount": sim.output_amount.to_string(),
            "return_multiplier": sim.return_multiplier(),
        }
    });
    
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config.simulation_log_path)?;
    
    writeln!(file, "{}", serde_json::to_string(&log_entry)?)?;
    
    Ok(())
}
