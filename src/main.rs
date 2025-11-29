//! The Sniper v0.8 - ACCURATE GAS PRICING Edition
//!
//! NEW: Uses Etherscan API for real-time gas prices
//! Shows detailed scan info: gas, cycles found, best candidates
//! Only alerts on actual profitable opportunities

use alloy_primitives::Address;
use color_eyre::eyre::Result;
use console::style;
use std::collections::HashMap;
use std::time::Instant;
use tracing::{info, warn, error, debug, trace};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod brain;
mod cartographer;
mod config;
mod tokens;
mod simulator;
mod executor;
mod gas_oracle;

use brain::{BoundedBellmanFord, ProfitFilter, ArbitrageCycle};
use cartographer::{ArbitrageGraph, ExpandedPoolFetcher, Dex, build_expanded_symbol_map};
use config::{Config, ExecutionMode};
use simulator::SwapSimulator;
use executor::ExecutionEngine;
use gas_oracle::{GasOracle, GasPriceInfo};

fn print_banner() {
    println!();
    println!("{}", style("═══════════════════════════════════════════════════").cyan());
    println!("{}", style("  🎯 THE SNIPER v0.8 - Accurate Gas Pricing").cyan().bold());
    println!("{}", style("═══════════════════════════════════════════════════").cyan());
    println!();
}

fn build_token_symbols() -> HashMap<Address, &'static str> {
    // Use the expanded symbol map from cartographer
    build_expanded_symbol_map()
}


fn format_token(addr: &Address, symbols: &HashMap<Address, &str>) -> String {
    symbols.get(addr).map(|s| s.to_string())
        .unwrap_or_else(|| format!("0x{}...", &format!("{:?}", addr)[2..8]))
}

fn format_path_short(cycle: &ArbitrageCycle, symbols: &HashMap<Address, &str>) -> String {
    cycle.path.iter()
        .map(|a| format_token(a, symbols))
        .collect::<Vec<_>>()
        .join("→")
}

fn get_eth_price_from_pools(pools: &[cartographer::PoolState]) -> f64 {
    for pool in pools {
        let price = pool.price(6, 18);
        if price > 1000.0 && price < 10000.0 { return price; }
        let inverse = pool.price(18, 6);
        if inverse > 1000.0 && inverse < 10000.0 { return inverse; }
    }
    3500.0
}

/// Cumulative statistics
struct Stats {
    total_scans: u64,
    total_cycles: u64,
    simulations_run: u64,
    opportunities_found: u64,
    executions_attempted: u64,
    start_time: Instant,
    last_gas_gwei: f64,
    gas_source: String,
    last_eth_price: f64,
    last_best_gross: f64,
    last_best_path: String,
}

impl Stats {
    fn new() -> Self {
        Self {
            total_scans: 0,
            total_cycles: 0,
            simulations_run: 0,
            opportunities_found: 0,
            executions_attempted: 0,
            start_time: Instant::now(),
            last_gas_gwei: 0.0,
            gas_source: "Unknown".to_string(),
            last_eth_price: 0.0,
            last_best_gross: 0.0,
            last_best_path: String::new(),
        }
    }
    
    fn uptime_hours(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64() / 3600.0
    }
    
    fn print_heartbeat(&self, config: &Config) {
        let uptime = self.uptime_hours();
        println!();
        println!("{}", style("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━").dim());
        println!(
            "💓 {} | Scans: {} | Cycles: {} | Sims: {} | {} Opportunities",
            style(format!("Uptime: {:.1}h", uptime)).cyan(),
            self.total_scans,
            self.total_cycles,
            self.simulations_run,
            style(self.opportunities_found).green().bold()
        );
        println!(
            "   ⛽ Gas: {:.2} gwei ({}) | ETH: ${:.0} | Bribe: {:.0}%",
            self.last_gas_gwei,
            self.gas_source,
            self.last_eth_price,
            config.miner_bribe_pct
        );
        if !self.last_best_path.is_empty() {
            println!(
                "   📊 Best seen this session: {} (gross: ${:.2})",
                style(&self.last_best_path).cyan(),
                self.last_best_gross
            );
        }
        println!("{}", style("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━").dim());
    }
}

/// Result of a single scan
struct ScanResult {
    cycles_found: usize,
    candidates_simulated: usize,
    best_gross_profit: f64,
    best_net_profit: f64,
    best_path: String,
    profitable_count: usize,
    gas_gwei: f64,
    eth_price: f64,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    // Default to WARN level, use RUST_LOG=sniper=info for scan details
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_target(false).compact())
        .with(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("sniper=warn".parse()?)
                .add_directive("warn".parse()?),
        )
        .init();

    print_banner();

    let config = Config::from_env()?;
    if let Err(e) = config.validate() {
        error!("Config error: {}", e);
        return Err(e.into());
    }

    // Initialize gas oracle
    let gas_oracle = GasOracle::new(
        config.etherscan_api_key.clone(),
        config.chain_id,
        config.rpc_url.clone(),
    );

    let token_symbols = build_token_symbols();
    let engine = ExecutionEngine::new(config.clone());
    let mut stats = Stats::new();
    let mut consecutive_failures = 0u32;

    // Show config summary
    let gas_source = if config.etherscan_api_key.is_some() {
        "Etherscan API ✓"
    } else {
        "RPC (less accurate)"
    };

    println!("┌─────────────────────────────────────────────────────────┐");
    println!("│ {:^55} │", format!("Mode: {} | Min profit: ${:.2}", config.execution_mode, config.min_profit_usd));
    println!("│ {:^55} │", format!("Gas source: {} | Bribe: {:.0}%", gas_source, config.miner_bribe_pct));
    println!("│ {:^55} │", format!("Flash loan: ${:.0} | Interval: {}s", config.default_simulation_usd, config.scan_interval_secs));
    println!("└─────────────────────────────────────────────────────────┘");
    println!();
    println!("Starting continuous scan loop...");
    println!("DEBUG: About to start first scan...");
    println!();

    loop {
        // Emergency stop check
        if std::env::var("EMERGENCY_STOP").unwrap_or_default() == "true" || config.emergency_stop {
            warn!("🛑 Emergency stop active - pausing 60s");
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            continue;
        }

        let scan_start = Instant::now();
        
        match run_scan(&config, &token_symbols, &engine, &gas_oracle, &mut stats).await {
            Ok(result) => {
                consecutive_failures = 0;
                
                // Update best seen
                if result.best_gross_profit > stats.last_best_gross {
                    stats.last_best_gross = result.best_gross_profit;
                    stats.last_best_path = result.best_path.clone();
                }
                
                // Print scan summary (compact, one line)
                let scan_time = scan_start.elapsed();
                print_scan_summary(&result, &stats, scan_time, &config);
            }
            Err(e) => {
                consecutive_failures += 1;
                println!(
                    "  {} Scan #{} failed: {} (failure {}/{})",
                    style("✗").red(),
                    stats.total_scans,
                    e,
                    consecutive_failures,
                    config.max_consecutive_failures
                );
                
                if consecutive_failures >= config.max_consecutive_failures {
                    println!(
                        "  {} Too many failures, backing off {}s...",
                        style("⏸").yellow(),
                        config.failure_pause_secs
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(config.failure_pause_secs)).await;
                    consecutive_failures = 0;
                }
            }
        }

        // Detailed heartbeat every 50 scans
        if stats.total_scans % 50 == 0 && stats.total_scans > 0 {
            stats.print_heartbeat(&config);
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(config.scan_interval_secs)).await;
    }
}

/// Print compact scan summary
fn print_scan_summary(result: &ScanResult, stats: &Stats, elapsed: std::time::Duration, config: &Config) {
    let profit_indicator = if result.profitable_count > 0 {
        style(format!("💰 {} PROFITABLE!", result.profitable_count)).green().bold()
    } else if result.best_net_profit > 0.0 {
        style(format!("~${:.2} best", result.best_net_profit)).yellow()
    } else if result.best_gross_profit > 0.0 {
        style(format!("-${:.2} (gas>{:.2})", 
            result.best_gross_profit - result.best_net_profit,
            result.best_gross_profit
        )).red().dim()
    } else {
        style("no candidates".to_string()).dim()
    };

    // Calculate what user would keep after bribe
    let after_bribe = if result.best_net_profit > 0.0 {
        result.best_net_profit * (1.0 - config.miner_bribe_pct / 100.0)
    } else {
        0.0
    };

    println!(
        "#{:<4} ⛽{:>5.3}gwei │ {} cycles │ {} sims │ {} │ {:.1}s",
        stats.total_scans,
        result.gas_gwei,
        result.cycles_found,
        result.candidates_simulated,
        profit_indicator,
        elapsed.as_secs_f64()
    );
    
    // If there's a best path, show it on next line
    if !result.best_path.is_empty() && result.candidates_simulated > 0 {
        let keep_str = if after_bribe > 0.0 {
            format!(" (you'd keep ${:.2})", after_bribe)
        } else {
            String::new()
        };
        println!(
            "      └─ Best: {} │ gross ${:.2} │ net ${:.2}{}",
            style(&result.best_path).cyan(),
            result.best_gross_profit,
            result.best_net_profit,
            style(keep_str).dim()
        );
    }
}

/// Run a single scan and return results
async fn run_scan(
    config: &Config,
    token_symbols: &HashMap<Address, &'static str>,
    engine: &ExecutionEngine,
    gas_oracle: &GasOracle,
    stats: &mut Stats,
) -> Result<ScanResult> {
    stats.total_scans += 1;
    println!("DEBUG: Scan #{} - fetching gas price...", stats.total_scans);
    
    let gas_info = gas_oracle.get_gas_price().await;
    println!("DEBUG: Gas price fetched: {:.2} gwei", gas_info.gas_price_gwei);
    let gas_gwei = gas_info.gas_price_gwei;
    
    // Update stats
    stats.last_gas_gwei = gas_gwei;
    stats.gas_source = gas_info.source.to_string();
    
    // Check if gas is too high BEFORE doing expensive operations
    if gas_gwei > config.max_gas_gwei as f64 {
        return Ok(ScanResult {
            cycles_found: 0,
            candidates_simulated: 0,
            best_gross_profit: 0.0,
            best_net_profit: f64::NEG_INFINITY,
            best_path: format!("SKIPPED: gas {:.1} > {} max", gas_gwei, config.max_gas_gwei),
            profitable_count: 0,
            gas_gwei,
            eth_price: stats.last_eth_price,
        });
    }
    
    // Fetch pools
    println!("DEBUG: About to fetch pools...");
    let fetcher = ExpandedPoolFetcher::new(config.rpc_url.clone());
    println!("DEBUG: Fetching all pools (this may take a while)...");
    let result = fetcher.fetch_all_pools().await?;
    println!("DEBUG: Fetched {} pools", result.pool_states.len());
    let pools = result.pool_states;
    
    let eth_price = get_eth_price_from_pools(&pools);
    stats.last_eth_price = eth_price;

    // Build graph
    let graph = ArbitrageGraph::from_pools(&pools);
    // Debug: List tokens in graph
    let symbol_map = build_token_symbols(); // or build_expanded_symbol_map()
    println!("\n=== TOKENS IN GRAPH ({}) ===", graph.node_count());
    for (addr, _) in &graph.token_to_node {
        let sym = symbol_map.get(addr).copied().unwrap_or("???");
        println!("  {}: {:?}", sym, addr);
    }
    println!("===========================\n");
    // Find cycles
    let bellman_ford = BoundedBellmanFord::new(&graph, config.max_hops);
    let base_tokens = config.base_token_addresses();
    // Add expanded base tokens for cycle search
    let mut expanded_bases = base_tokens.clone();
    // Add USDS, sUSDS, crvUSD as cycle starting points
    if let Ok(addr) = "0xdC035D45d973E3EC169d2276DDab16f1e407384F".parse() {
        expanded_bases.push(addr); // USDS
    }
    if let Ok(addr) = "0xa3931d71877C0E7a3148CB7Eb4463524FEc27fbD".parse() {
        expanded_bases.push(addr); // sUSDS  
    }
    if let Ok(addr) = "0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E".parse() {
        expanded_bases.push(addr); // crvUSD
    }
    let cycles: Vec<_> = bellman_ford.find_all_cycles(&expanded_bases)
        .into_iter()
        .filter(|c| !config.is_cycle_blacklisted(&c.path))
        .collect();

    let cycles_found = cycles.len();
    stats.total_cycles += cycles_found as u64;
    println!("DEBUG: Found {} raw cycles", cycles_found);

    if cycles.is_empty() {
        return Ok(ScanResult {
            cycles_found: 0,
            candidates_simulated: 0,
            best_gross_profit: 0.0,
            best_net_profit: f64::NEG_INFINITY,
            best_path: String::new(),
            profitable_count: 0,
            gas_gwei,
            eth_price,
        });
    }

    // Sort by expected return and take top candidates
    let mut sorted = cycles;
    sorted.sort_by(|a, b| b.expected_return.partial_cmp(&a.expected_return).unwrap_or(std::cmp::Ordering::Equal));

    // Detailed cycle debug
    println!("\n=== TOP 3 CYCLES DETAILED ===");
    for (i, cycle) in sorted.iter().take(3).enumerate() {
        println!("\nCycle #{} (return: {:.6})", i, cycle.expected_return);
        
        let mut cumulative = 1.0_f64;
        for j in 0..cycle.pools.len() {
            let from = symbol_map.get(&cycle.path[j]).copied().unwrap_or("???");
            let to = symbol_map.get(&cycle.path[j + 1]).copied().unwrap_or("???");
            let price = cycle.prices[j];
            let fee = cycle.fees[j];
            let fee_mult = 1.0 - (fee as f64 / 1_000_000.0);
            let step_return = price * fee_mult;
            cumulative *= step_return;
            
            println!("  Step {}: {} → {}", j + 1, from, to);
            println!("         price={:.8}, fee={}bps, step_return={:.8}, cumulative={:.8}",
                price, fee, step_return, cumulative);
        }
    }
    println!("=== END DETAILED ===\n");

    // Show top 3 expected returns before filtering
    for (i, c) in sorted.iter().take(3).enumerate() {
        println!("DEBUG: Cycle {} expected_return: {:.6}", i, c.expected_return);
    }
    
    let candidates: Vec<_> = sorted.into_iter()
        .filter(|c| c.expected_return > 1.0001)
        .take(5)
        .collect();

    println!("DEBUG: {} candidates after filter (expected_return > 1.0001)", candidates.len());

    if candidates.is_empty() {
        return Ok(ScanResult {
            cycles_found,
            candidates_simulated: 0,
            best_gross_profit: 0.0,
            best_net_profit: f64::NEG_INFINITY,
            best_path: String::new(),
            profitable_count: 0,
            gas_gwei,
            eth_price,
        });
    }
    // let specials = check_special_opportunities(&result, 50.0).await;
    // for opp in specials {
    //     match opp {
    //         SpecialOpportunity::YieldDrift { symbol, spread_pct, .. } => {
    //             info!("🎯 Yield drift: {} spread={:.2}%", symbol, spread_pct);
    //         }
    //         SpecialOpportunity::NAVArb { symbol, spread_pct, .. } => {
    //             info!("🎯 NAV arb: {} spread={:.2}%", symbol, spread_pct);
    //         }
    //         SpecialOpportunity::ImbalancedPool { base_fee, effective_fee, .. } => {
    //             info!("🎯 Imbalanced pool: fee {} -> {}", base_fee, effective_fee);
    //         }
    //     }
    // }
    // Create simulator with REAL gas price
    let swap_sim = SwapSimulator::new(&config.rpc_url).await?;
    // Note: We calculate gas cost separately using gas_info for accuracy

    let mut best_gross_profit = 0.0f64;
    let mut best_net_profit = f64::NEG_INFINITY;
    let mut best_path = String::new();
    let mut profitable_count = 0;
    let mut candidates_simulated = 0;

    // === SIMULATE EACH CANDIDATE ===
    for cycle in &candidates {
        candidates_simulated += 1;
        stats.simulations_run += 1;
        
        let tier = swap_sim.get_cycle_liquidity_tier(cycle);
        let target_usd = tier.recommended_amount_usd().min(config.default_simulation_usd);
        
        // Run simulation
        let sim = swap_sim.simulate_cycle(cycle, target_usd).await;

        if !sim.simulation_success {
            continue;
        }

        // Calculate ACCURATE gas cost with REAL gas price
        let gas_cost_usd = gas_info.estimate_cost_usd(sim.total_gas_used, eth_price);
        
        // Calculate profits
        let gross_return = sim.return_multiplier();
        let gross_profit_usd = target_usd * (gross_return - 1.0);
        let net_profit_usd = gross_profit_usd - gas_cost_usd;
        
        let path_str = format_path_short(cycle, token_symbols);
        
        // Track best
        if gross_profit_usd > best_gross_profit {
            best_gross_profit = gross_profit_usd;
        }
        if net_profit_usd > best_net_profit {
            best_net_profit = net_profit_usd;
            best_path = path_str.clone();
        }

        // === ONLY EXECUTE/LOG IF ACTUALLY PROFITABLE ===
        if net_profit_usd >= config.min_profit_usd {
            profitable_count += 1;
            stats.opportunities_found += 1;
            
            // Calculate after-bribe profit
            let after_bribe_profit = net_profit_usd * (1.0 - config.miner_bribe_pct / 100.0);
            
            let dex_str = cycle.dexes.iter()
                .map(|d| match d {
                    Dex::UniswapV3 => "U3",
                    Dex::UniswapV2 => "U2",
                    Dex::SushiswapV2 => "S2",
                    Dex::PancakeSwapV3 => "P3",
                    _ => "??",
                })
                .collect::<Vec<_>>()
                .join("-");

            // Big alert for profitable opportunity!
            println!();
            println!("{}", style("╔════════════════════════════════════════════════════════════════╗").green().bold());
            println!("{}", style(format!(
                "║  💰 PROFITABLE!  Net: ${:.2}  │  You keep: ${:.2} after {:.0}% bribe  ║", 
                net_profit_usd, after_bribe_profit, config.miner_bribe_pct
            )).green().bold());
            println!("{}", style("╠════════════════════════════════════════════════════════════════╣").green());
            println!("║  Path: {} ({})", style(&path_str).cyan(), style(&dex_str).magenta());
            println!("║  Return: {:.4}x │ Gross: ${:.2} │ Gas: ${:.2} @ {:.2} gwei",
                gross_return, gross_profit_usd, gas_cost_usd, gas_gwei);
            println!("║  Input: ${:.0} │ Gas units: {} │ ETH: ${:.0}",
                target_usd, sim.total_gas_used, eth_price);
            println!("{}", style("╚════════════════════════════════════════════════════════════════╝").green().bold());

            // Execute
            stats.executions_attempted += 1;
            
            match engine.execute(cycle, &sim, 0).await {
                Ok(result) => {
                    match &result {
                        executor::ExecutionResult::Simulated { expected_profit_usd, would_execute } => {
                            println!("   {} Simulation mode: profit ${:.2}, execute={}", 
                                style("📋").dim(), expected_profit_usd, would_execute);
                        }
                        executor::ExecutionResult::DryRun { simulation_passed, gas_used, .. } => {
                            println!("   {} Dry run: passed={}, gas={:?}", 
                                style("🔬").dim(), simulation_passed, gas_used);
                        }
                        executor::ExecutionResult::Submitted { bundle_hash, target_block, .. } => {
                            println!("   {} SUBMITTED to block {}: {}", 
                                style("🚀").green().bold(), target_block, bundle_hash);
                        }
                        executor::ExecutionResult::Included { block_number, actual_profit_wei, .. } => {
                            println!("   {} INCLUDED in block {}! Profit: {} wei", 
                                style("✅").green().bold(), block_number, actual_profit_wei);
                        }
                        executor::ExecutionResult::Skipped { reason } => {
                            println!("   {} Skipped: {}", style("⏭").yellow(), reason);
                        }
                        executor::ExecutionResult::Aborted { reason } => {
                            println!("   {} Aborted: {}", style("⛔").red(), reason);
                        }
                        executor::ExecutionResult::Failed { reason } => {
                            println!("   {} Failed: {}", style("✗").red(), reason);
                        }
                    }
                }
                Err(e) => {
                    println!("   {} Execution error: {}", style("✗").red(), e);
                }
            }

            // Log to file with accurate data
            if config.simulation_log {
                let _ = log_opportunity(
                    config, cycle, gross_profit_usd, gas_cost_usd, 
                    net_profit_usd, gas_gwei, eth_price, token_symbols
                );
            }
            
            println!();
        }
    }

    Ok(ScanResult {
        cycles_found,
        candidates_simulated,
        best_gross_profit,
        best_net_profit,
        best_path,
        profitable_count,
        gas_gwei,
        eth_price,
    })
}

/// Log opportunity to file with accurate gas pricing
fn log_opportunity(
    config: &Config,
    cycle: &ArbitrageCycle,
    gross_profit_usd: f64,
    gas_cost_usd: f64,
    net_profit_usd: f64,
    gas_gwei: f64,
    eth_price: f64,
    symbols: &HashMap<Address, &'static str>,
) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    if let Some(parent) = std::path::Path::new(&config.simulation_log_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let path_str = cycle.path.iter()
        .map(|a| format_token(a, symbols))
        .collect::<Vec<_>>()
        .join(" → ");

    let after_bribe = net_profit_usd * (1.0 - config.miner_bribe_pct / 100.0);

    let entry = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "path": path_str,
        "dexes": cycle.dexes.iter().map(|d| d.to_string()).collect::<Vec<_>>(),
        "gross_profit_usd": gross_profit_usd,
        "gas_cost_usd": gas_cost_usd,
        "net_profit_usd": net_profit_usd,
        "after_bribe_usd": after_bribe,
        "bribe_pct": config.miner_bribe_pct,
        "gas_gwei": gas_gwei,
        "eth_price_usd": eth_price,
        "return": cycle.expected_return,
    });

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config.simulation_log_path)?;

    writeln!(file, "{}", serde_json::to_string(&entry)?)?;
    Ok(())
}