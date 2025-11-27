//! The Sniper - Arbitrage Detection Bot (Phase 4: CONTINUOUS LOOP Edition)
//!
//! Run with: cargo run
//!
//! CHANGES:
//! - Continuous loop instead of one-shot execution
//! - Proper error handling with retry logic
//! - Reduced RPC costs by not restarting

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
        style(" 🎯 THE SNIPER - Arbitrage Detection Bot (CONTINUOUS Edition)").cyan().bold()
    );
    println!(
        "{}",
        style("    5 DEXes | Dynamic Sizing | Continuous Scanning").cyan()
    );
    println!(
        "{}",
        style("═══════════════════════════════════════════════════════════════").cyan()
    );
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

/// Simulate a cycle with multiple input sizes to find the profitable amount
async fn simulate_with_optimal_size(
    simulator: &SwapSimulator,
    cycle: &ArbitrageCycle,
    symbols: &HashMap<Address, &str>,
) -> Option<simulator::swap_simulator::ArbitrageSimulation> {
    // Get the liquidity tier for this cycle
    let tier = simulator.get_cycle_liquidity_tier(cycle);
    let base_amount = tier.recommended_amount_usd();
    
    // Try multiple sizes: 100%, 50%, 25%, 10%
    let size_multipliers = [1.0, 0.5, 0.25, 0.1];
    let mut best_sim: Option<simulator::swap_simulator::ArbitrageSimulation> = None;
    
    for &mult in &size_multipliers {
        let target_usd = base_amount * mult;
        
        // Skip very small amounts
        if target_usd < 50.0 {
            continue;
        }
        
        let sim = simulator.simulate_cycle(cycle, target_usd).await;
        
        if sim.simulation_success {
            let path = cycle.path.iter()
                .map(|a| format_token(a, symbols))
                .collect::<Vec<_>>()
                .join(" → ");
            
            debug!(
                "  Size ${:.0} ({:?}): Return {:.4}x, Net ${:.2}",
                target_usd, tier, sim.return_multiplier(), sim.profit_usd
            );
            
            // If profitable, we found our answer
            if sim.is_profitable {
                info!(
                    "✓ Found profitable size: ${:.0} for {} → Net: ${:.2}",
                    target_usd, path, sim.profit_usd
                );
                return Some(sim);
            }
            
            // Keep track of the best (least bad) simulation
            match &best_sim {
                None => best_sim = Some(sim),
                Some(prev) if sim.profit_usd > prev.profit_usd => best_sim = Some(sim),
                _ => {}
            }
        }
    }
    
    best_sim
}

/// Statistics tracking across scans
struct ScanStats {
    total_scans: u64,
    profitable_found: u64,
    total_cycles_analyzed: u64,
    last_profitable_scan: Option<u64>,
}

impl ScanStats {
    fn new() -> Self {
        Self {
            total_scans: 0,
            profitable_found: 0,
            total_cycles_analyzed: 0,
            last_profitable_scan: None,
        }
    }
    
    fn record_scan(&mut self, cycles: usize, profitable: usize) {
        self.total_scans += 1;
        self.total_cycles_analyzed += cycles as u64;
        self.profitable_found += profitable as u64;
        if profitable > 0 {
            self.last_profitable_scan = Some(self.total_scans);
        }
    }
    
    fn print_summary(&self) {
        info!("━━━━━━━━━━━━ CUMULATIVE STATS ━━━━━━━━━━━━");
        info!("Total scans: {}", self.total_scans);
        info!("Cycles analyzed: {}", self.total_cycles_analyzed);
        info!("Profitable opportunities: {}", self.profitable_found);
        if let Some(last) = self.last_profitable_scan {
            info!("Last profitable: scan #{}", last);
        }
        info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    }
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
    info!("   Scan interval: {} seconds", config.scan_interval_secs);
    info!("   Press Ctrl+C to stop");
    println!();

    // ========================================
    // MAIN CONTINUOUS LOOP
    // ========================================
    loop {
        let scan_number = stats.total_scans + 1;
        
        // Check emergency stop (re-read from env for hot reload)
        if std::env::var("EMERGENCY_STOP").unwrap_or_default() == "true" || config.emergency_stop {
            warn!("🛑 Emergency stop is active. Waiting 60 seconds...");
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            continue;
        }
        
        info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        info!("🔄 SCAN #{} starting at {}", scan_number, chrono::Utc::now().format("%H:%M:%S UTC"));
        info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        
        let scan_start = Instant::now();
        
        // Run the scan
        match run_single_scan(&config, &token_symbols, &engine).await {
            Ok((cycles_count, profitable_count)) => {
                consecutive_failures = 0;
                stats.record_scan(cycles_count, profitable_count);
                
                let scan_duration = scan_start.elapsed();
                
                if profitable_count > 0 {
                    info!(
                        "{}",
                        style(format!(
                            "💰 SCAN #{} COMPLETE: {} profitable opportunities found! (took {:?})",
                            scan_number, profitable_count, scan_duration
                        )).green().bold()
                    );
                } else {
                    info!(
                        "✅ Scan #{} complete: {} cycles analyzed, 0 profitable (took {:?})",
                        scan_number, cycles_count, scan_duration
                    );
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                error!("❌ Scan #{} failed: {}", scan_number, e);
                
                // Record failed scan in stats
                stats.record_scan(0, 0);
                
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
        debug!(
            "💤 Sleeping {} seconds until next scan...",
            config.scan_interval_secs
        );
        tokio::time::sleep(tokio::time::Duration::from_secs(config.scan_interval_secs)).await;
    }
}

/// Run a single scan iteration
/// Returns (cycles_analyzed, profitable_count)
async fn run_single_scan(
    config: &Config,
    token_symbols: &HashMap<Address, &'static str>,
    engine: &ExecutionEngine,
) -> Result<(usize, usize)> {
    
    // =============================================
    // PHASE 1: THE CARTOGRAPHER
    // =============================================
    debug!("Phase 1: Fetching pool data...");
    let fetch_start = Instant::now();

    let fetcher = PoolFetcher::new(config.rpc_url.clone());
    let pools = fetcher.fetch_all_pools().await?;

    let fetch_time = fetch_start.elapsed();
    debug!("Fetched {} pools in {:?}", pools.len(), fetch_time);

    let eth_price = get_eth_price_from_pools(&pools);
    debug!("ETH price: ${:.2}", eth_price);

    // Build the graph
    let graph = ArbitrageGraph::from_pools(&pools);
    debug!(
        "Graph: {} nodes, {} edges",
        graph.node_count(),
        graph.edge_count()
    );

    // =============================================
    // PHASE 2: THE BRAIN
    // =============================================
    debug!("Phase 2: Running Bellman-Ford...");

    let bellman_ford = BoundedBellmanFord::new(&graph, config.max_hops);
    let base_tokens = config.base_token_addresses();
    let all_cycles = bellman_ford.find_all_cycles(&base_tokens);

    // Filter out blacklisted cycles
    let cycles: Vec<_> = all_cycles
        .into_iter()
        .filter(|c| !config.is_cycle_blacklisted(&c.path))
        .collect();

    let cycles_count = cycles.len();
    debug!("Found {} valid cycles", cycles_count);

    // Quick profit filter
    let mut filter = ProfitFilter::new(config.min_profit_usd);
    filter.set_eth_price(eth_price);
    filter.set_gas_price(0.5);

    let profitable_candidates = filter.filter_profitable(&cycles, token_symbols);

    // =============================================
    // PHASE 3: THE SIMULATOR
    // =============================================
    let mut simulated_profitable = Vec::new();

    if !profitable_candidates.is_empty() {
        debug!("Phase 3: Simulating {} candidates...", profitable_candidates.len());
        
        match SwapSimulator::new(&config.rpc_url).await {
            Ok(mut swap_sim) => {
                swap_sim.set_eth_price(eth_price);
                
                // Take top 10 candidates for simulation
                let cycles_to_simulate: Vec<_> = profitable_candidates
                    .iter()
                    .take(10)
                    .map(|p| &p.cycle)
                    .cloned()
                    .collect();
                
                for cycle in &cycles_to_simulate {
                    if let Some(sim) = simulate_with_optimal_size(&swap_sim, cycle, token_symbols).await {
                        if sim.is_profitable && sim.profit_usd >= config.min_profit_usd {
                            simulated_profitable.push((cycle.clone(), sim));
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Simulator init failed: {}", e);
            }
        }
    }

    // =============================================
    // PHASE 4: THE EXECUTOR
    // =============================================
    if !simulated_profitable.is_empty() {
        info!(
            "{}",
            style(format!("🎯 {} PROFITABLE OPPORTUNITIES FOUND!", simulated_profitable.len()))
                .green()
                .bold()
        );
        
        for (i, (cycle, sim)) in simulated_profitable.iter().enumerate() {
            let path = cycle.path.iter()
                .map(|a| format_token(a, token_symbols))
                .collect::<Vec<_>>()
                .join(" → ");
            
            info!(
                "  {}. {} | DEXes: {} | Input: ${:.0} | Profit: ${:.2}",
                i + 1,
                style(&path).cyan(),
                cycle.dex_path(),
                sim.input_usd,
                sim.profit_usd
            );
            
            // Execute based on mode
            match config.execution_mode {
                ExecutionMode::Simulation => {
                    // Just log it
                    if config.simulation_log {
                        if let Err(e) = log_opportunity(config, cycle, sim) {
                            warn!("Failed to log opportunity: {}", e);
                        }
                    }
                }
                ExecutionMode::DryRun | ExecutionMode::Production => {
                    match engine.execute(cycle, sim, 0).await {
                        Ok(result) => {
                            info!("   Execution result: {:?}", result);
                        }
                        Err(e) => {
                            warn!("   Execution failed: {}", e);
                        }
                    }
                }
            }
        }
    }

    Ok((cycles_count, simulated_profitable.len()))
}

/// Log a profitable opportunity to file
fn log_opportunity(
    config: &Config,
    cycle: &ArbitrageCycle,
    sim: &simulator::swap_simulator::ArbitrageSimulation,
) -> Result<()> {
    use config::OpportunityLog;
    use chrono::Utc;
    
    let log = OpportunityLog {
        timestamp: Utc::now(),
        path: cycle.path.iter().map(|a| format!("{:?}", a)).collect(),
        dexes: cycle.dexes.iter().map(|d| d.to_string()).collect(),
        input_usd: sim.input_usd,
        gross_profit_usd: sim.profit_usd + (sim.total_gas_used as f64 * 0.5 * 1e-9 * 3500.0),
        gas_cost_usd: sim.total_gas_used as f64 * 0.5 * 1e-9 * 3500.0,
        net_profit_usd: sim.profit_usd,
        gas_price_gwei: 0.5,
        eth_price_usd: 3500.0,
        block_number: 0,
    };
    
    log.append_to_file(&config.simulation_log_path)?;
    
    Ok(())
}
