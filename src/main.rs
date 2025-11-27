//! The Sniper v0.7 - QUIET PRODUCTION MODE
//!
//! Only speaks when there's something worth saying:
//! - Startup banner (once)
//! - Heartbeat every 50 scans
//! - PROFITABLE opportunities (immediately)
//! - Errors
//!
//! All verbose logging moved to DEBUG level

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

use brain::{BoundedBellmanFord, ProfitFilter, ArbitrageCycle};
use cartographer::{ArbitrageGraph, PoolFetcher, Dex};
use config::{Config, ExecutionMode};
use simulator::SwapSimulator;
use executor::ExecutionEngine;

fn print_banner() {
    println!();
    println!("{}", style("═══════════════════════════════════════════════════").cyan());
    println!("{}", style("  🎯 THE SNIPER v0.7 - Quiet Production Mode").cyan().bold());
    println!("{}", style("═══════════════════════════════════════════════════").cyan());
    println!();
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
        ("0x514910771AF9Ca656af840dff83E8264EcF986CA", "LINK"),
        ("0x1f9840a85d5aF5bf1D1762F925BDADdC4201F984", "UNI"),
        ("0x6982508145454Ce325dDbE47a25d4ec3d2311933", "PEPE"),
    ];
    for (addr, symbol) in tokens {
        if let Ok(address) = addr.parse() {
            map.insert(address, symbol);
        }
    }
    map
}

fn format_token(addr: &Address, symbols: &HashMap<Address, &str>) -> String {
    symbols.get(addr).map(|s| s.to_string())
        .unwrap_or_else(|| format!("0x{}...", &format!("{:?}", addr)[2..8]))
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
        }
    }
    
    fn uptime_hours(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64() / 3600.0
    }
    
    fn print_heartbeat(&self) {
        let uptime = self.uptime_hours();
        println!();
        println!("{}", style("─────────────────────────────────────────────────").dim());
        println!(
            "💓 {} | Scans: {} | Cycles: {} | Sims: {} | Opportunities: {}",
            style(format!("Uptime: {:.1}h", uptime)).cyan(),
            self.total_scans,
            self.total_cycles,
            self.simulations_run,
            style(self.opportunities_found).green().bold()
        );
        println!("{}", style("─────────────────────────────────────────────────").dim());
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    // Default to WARN level, DEBUG for sniper module
    // Use RUST_LOG=sniper=debug for verbose output
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_target(false).compact())
        .with(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("sniper=warn".parse()?)  // Quiet by default
                .add_directive("warn".parse()?),
        )
        .init();

    print_banner();

    let config = Config::from_env()?;
    if let Err(e) = config.validate() {
        error!("Config error: {}", e);
        return Err(e.into());
    }

    let token_symbols = build_token_symbols();
    let engine = ExecutionEngine::new(config.clone());
    let mut stats = Stats::new();
    let mut consecutive_failures = 0u32;

    println!("Mode: {} | Min profit: ${:.2} | Interval: {}s",
        style(format!("{}", config.execution_mode)).yellow(),
        config.min_profit_usd,
        config.scan_interval_secs
    );
    println!("Scanning... (quiet mode, will alert on opportunities)");
    println!();

    loop {
        // Emergency stop check
        if std::env::var("EMERGENCY_STOP").unwrap_or_default() == "true" || config.emergency_stop {
            warn!("🛑 Emergency stop");
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            continue;
        }

        match run_scan(&config, &token_symbols, &engine, &mut stats).await {
            Ok(_) => {
                consecutive_failures = 0;
            }
            Err(e) => {
                consecutive_failures += 1;
                error!("Scan failed: {}", e);
                if consecutive_failures >= config.max_consecutive_failures {
                    warn!("Backing off {} seconds...", config.failure_pause_secs);
                    tokio::time::sleep(tokio::time::Duration::from_secs(config.failure_pause_secs)).await;
                    consecutive_failures = 0;
                }
            }
        }

        // Heartbeat every 50 scans
        if stats.total_scans % 50 == 0 && stats.total_scans > 0 {
            stats.print_heartbeat();
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(config.scan_interval_secs)).await;
    }
}

/// Run a single scan - quiet unless something interesting happens
async fn run_scan(
    config: &Config,
    token_symbols: &HashMap<Address, &'static str>,
    engine: &ExecutionEngine,
    stats: &mut Stats,
) -> Result<()> {
    stats.total_scans += 1;
    
    // Fetch pools (quiet)
    let fetcher = PoolFetcher::new(config.rpc_url.clone());
    let pools = fetcher.fetch_all_pools().await?;
    let eth_price = get_eth_price_from_pools(&pools);
    let gas_gwei = 0.5;

    // Build graph (quiet)
    let graph = ArbitrageGraph::from_pools(&pools);
    
    // Find cycles (quiet)
    let bellman_ford = BoundedBellmanFord::new(&graph, config.max_hops);
    let base_tokens = config.base_token_addresses();
    let cycles: Vec<_> = bellman_ford.find_all_cycles(&base_tokens)
        .into_iter()
        .filter(|c| !config.is_cycle_blacklisted(&c.path))
        .collect();

    stats.total_cycles += cycles.len() as u64;

    if cycles.is_empty() {
        return Ok(());
    }

    // Filter candidates (quiet)
    let mut filter = ProfitFilter::new(config.min_profit_usd);
    filter.set_eth_price(eth_price);
    filter.set_gas_price(gas_gwei);

    // Get top candidates
    let mut sorted = cycles;
    sorted.sort_by(|a, b| b.expected_return.partial_cmp(&a.expected_return).unwrap_or(std::cmp::Ordering::Equal));
    
    let candidates: Vec<_> = sorted.into_iter()
        .filter(|c| c.expected_return > 1.0001)
        .take(5)
        .collect();

    if candidates.is_empty() {
        return Ok(());
    }

    // Simulate candidates
    let swap_sim = match SwapSimulator::new(&config.rpc_url).await {
        Ok(mut s) => {
            s.set_eth_price(eth_price);
            s.set_gas_price(gas_gwei);
            s
        }
        Err(_) => return Ok(()),
    };

    for cycle in &candidates {
        stats.simulations_run += 1;
        
        let tier = swap_sim.get_cycle_liquidity_tier(cycle);
        let target_usd = tier.recommended_amount_usd().min(config.default_simulation_usd);
        let sim = swap_sim.simulate_cycle(cycle, target_usd).await;

        if !sim.simulation_success {
            continue;
        }

        let actual_pnl = sim.profit_usd;

        // === ONLY PRINT IF ACTUALLY PROFITABLE ===
        if actual_pnl >= config.min_profit_usd {
            stats.opportunities_found += 1;
            
            let path_str = cycle.path.iter()
                .map(|a| format_token(a, token_symbols))
                .collect::<Vec<_>>()
                .join(" → ");
            
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

            println!();
            println!("{}", style("╔════════════════════════════════════════════════════════════╗").green().bold());
            println!("{}", style(format!("║  💰 PROFITABLE OPPORTUNITY FOUND!  ${:.2}               ║", actual_pnl)).green().bold());
            println!("{}", style("╠════════════════════════════════════════════════════════════╣").green());
            println!("║  Path: {}", style(&path_str).cyan());
            println!("║  DEXes: {}", style(&dex_str).magenta());
            println!("║  Return: {:.4}x | Gas: {} units", sim.return_multiplier(), sim.total_gas_used);
            println!("{}", style("╚════════════════════════════════════════════════════════════╝").green().bold());

            // Execute
            stats.executions_attempted += 1;
            
            match engine.execute(cycle, &sim, 0).await {
                Ok(result) => {
                    match &result {
                        executor::ExecutionResult::Simulated { expected_profit_usd, would_execute } => {
                            println!("   {} Sim mode: Would profit ${:.2}, execute: {}", 
                                style("→").dim(), expected_profit_usd, would_execute);
                        }
                        executor::ExecutionResult::DryRun { simulation_passed, .. } => {
                            println!("   {} Dry run: passed={}", 
                                style("→").dim(), simulation_passed);
                        }
                        executor::ExecutionResult::Submitted { bundle_hash, .. } => {
                            println!("   {} SUBMITTED: {}", 
                                style("🚀").green().bold(), bundle_hash);
                        }
                        executor::ExecutionResult::Included { block_number, actual_profit_wei, .. } => {
                            println!("   {} INCLUDED in block {}! Profit: {} wei", 
                                style("✅").green().bold(), block_number, actual_profit_wei);
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    println!("   {} Execution error: {}", style("✗").red(), e);
                }
            }

            // Log to file
            if config.simulation_log {
                let _ = log_opportunity(config, cycle, &sim, token_symbols);
            }
            
            println!();
        }
    }

    Ok(())
}

/// Log opportunity to file
fn log_opportunity(
    config: &Config,
    cycle: &ArbitrageCycle,
    sim: &simulator::swap_simulator::ArbitrageSimulation,
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

    let entry = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "path": path_str,
        "dexes": cycle.dexes.iter().map(|d| d.to_string()).collect::<Vec<_>>(),
        "profit_usd": sim.profit_usd,
        "return": sim.return_multiplier(),
        "gas_used": sim.total_gas_used,
    });

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config.simulation_log_path)?;

    writeln!(file, "{}", serde_json::to_string(&entry)?)?;
    Ok(())
}
