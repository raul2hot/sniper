//! The Sniper - Arbitrage Detection Bot (Phase 4: FIXED Edition)
//!
//! Run with: cargo run
//!
//! FIXES:
//! - Dynamic simulation sizing based on token liquidity
//! - Proper gas price handling
//! - Multiple size attempts to find profitable amount

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
        style(" 🎯 THE SNIPER - Arbitrage Detection Bot (FIXED Edition)").cyan().bold()
    );
    println!(
        "{}",
        style("    5 DEXes | Dynamic Sizing | Proper Gas Handling").cyan()
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
    
    // Print configuration summary
    config.print_summary();
    println!();

    let token_symbols = build_token_symbols();

    // =============================================
    // PHASE 1: THE CARTOGRAPHER
    // =============================================
    println!();
    println!(
        "{}",
        style("═══ PHASE 1: THE CARTOGRAPHER ═══").blue().bold()
    );
    println!();

    println!("{}", style("Step 1.1: Fetching pool data from 5 DEXes...").blue());
    let start = Instant::now();

    let fetcher = PoolFetcher::new(config.rpc_url.clone());
    let pools = fetcher.fetch_all_pools().await?;

    let fetch_time = start.elapsed();
    
    let mut dex_counts: HashMap<Dex, usize> = HashMap::new();
    for pool in &pools {
        *dex_counts.entry(pool.dex).or_insert(0) += 1;
    }
    
    let low_fee_count = pools.iter()
        .filter(|p| p.pool_type == PoolType::V3 && p.fee <= 500)
        .count();
    
    println!(
        "{} Fetched {} pools in {:?}",
        style("✓").green(),
        pools.len(),
        fetch_time
    );
    
    println!("   DEX breakdown:");
    for (dex, count) in &dex_counts {
        println!("     {}: {} pools", dex, count);
    }
    println!("   Low-fee pools (≤5bps): {}", low_fee_count);

    let eth_price = get_eth_price_from_pools(&pools);
    println!("{} ETH price: ${:.2}", style("✓").green(), eth_price);

    // Step 1.2: Build the graph
    println!();
    println!("{}", style("Step 1.2: Building cross-DEX arbitrage graph...").blue());
    let start = Instant::now();

    let graph = ArbitrageGraph::from_pools(&pools);

    let build_time = start.elapsed();
    println!(
        "{} Graph built in {:?}: {} nodes, {} edges",
        style("✓").green(),
        build_time,
        graph.node_count(),
        graph.edge_count()
    );

    // Find cross-DEX price differences
    println!();
    println!("{}", style("Step 1.3: Scanning for cross-DEX price differences...").blue());
    let opportunities = graph.find_cross_dex_opportunities(&token_symbols);
    println!(
        "{} Found {} token pairs with cross-DEX price differences",
        style("✓").green(),
        opportunities.len()
    );

    // =============================================
    // PHASE 2: THE BRAIN
    // =============================================
    println!();
    println!(
        "{}",
        style("═══ PHASE 2: THE BRAIN ═══").magenta().bold()
    );
    println!();

    println!(
        "{}",
        style("Step 2.1: Running Bellman-Ford algorithm...").magenta()
    );
    let start = Instant::now();

    let bellman_ford = BoundedBellmanFord::new(&graph, config.max_hops);
    let base_tokens = config.base_token_addresses();
    let all_cycles = bellman_ford.find_all_cycles(&base_tokens);

    // Filter out blacklisted cycles
    let cycles: Vec<_> = all_cycles
        .into_iter()
        .filter(|c| !config.is_cycle_blacklisted(&c.path))
        .collect();

    let algo_time = start.elapsed();
    
    let cross_dex_count = cycles.iter().filter(|c| c.is_cross_dex()).count();
    let low_fee_cycle_count = cycles.iter().filter(|c| c.has_low_fee_pools()).count();
    
    println!(
        "{} Found {} cycles in {:?} (after filtering blacklisted pairs)",
        style("✓").green(),
        cycles.len(),
        algo_time,
    );
    println!("   {} cross-DEX cycles", cross_dex_count);
    println!("   {} using low-fee pools", low_fee_cycle_count);

    // Step 2.2: Filter for profitable cycles (graph-based estimate)
    println!();
    println!(
        "{}",
        style("Step 2.2: Analyzing profitability (graph estimates)...").magenta()
    );

    let mut filter = ProfitFilter::new(config.min_profit_usd);
    filter.set_eth_price(eth_price);
    filter.set_gas_price(0.5); // Nov 2025: gas is ~0.05-0.5 gwei due to L2 adoption

    filter.print_summary(&cycles, &token_symbols);

    let profitable_candidates = filter.filter_profitable(&cycles, &token_symbols);

    // =============================================
    // PHASE 3: THE SIMULATOR
    // =============================================
    println!();
    println!(
        "{}",
        style("═══ PHASE 3: THE SIMULATOR ═══").green().bold()
    );
    println!();

    let mut simulated_profitable = Vec::new();

    if cycles.is_empty() {
        println!("{}", style("No cycles to simulate.").yellow());
    } else {
        println!(
            "{}",
            style("Step 3.1: Initializing simulator with dynamic sizing...").green()
        );
        
        match SwapSimulator::new(&config.rpc_url).await {
            Ok(mut swap_sim) => {
                swap_sim.set_eth_price(eth_price);
                // Don't override gas price - let it use the fetched/minimum value
                
                let actual_gas = swap_sim.gas_price_gwei();
                println!(
                    "{} Simulator initialized (gas: {:.2} gwei, ETH: ${:.0})",
                    style("✓").green(),
                    actual_gas,
                    eth_price
                );
                
                // Take top candidates based on graph analysis
                let cycles_to_simulate: Vec<_> = if !profitable_candidates.is_empty() {
                    profitable_candidates.iter()
                        .take(15)  // Simulate more candidates
                        .map(|p| &p.cycle)
                        .cloned()
                        .collect()
                } else {
                    cycles.iter().take(10).cloned().collect()
                };
                
                if !cycles_to_simulate.is_empty() {
                    println!();
                    println!(
                        "{}",
                        style(format!(
                            "Step 3.2: Simulating {} cycles with dynamic sizing...",
                            cycles_to_simulate.len()
                        )).green()
                    );
                    println!();
                    
                    for (i, cycle) in cycles_to_simulate.iter().enumerate() {
                        let path = cycle.path.iter()
                            .map(|a| format_token(a, &token_symbols))
                            .collect::<Vec<_>>()
                            .join(" → ");
                        
                        // Get the liquidity tier
                        let tier = swap_sim.get_cycle_liquidity_tier(cycle);
                        
                        // Try to find profitable size
                        if let Some(sim) = simulate_with_optimal_size(&swap_sim, cycle, &token_symbols).await {
                            let status = if sim.is_profitable {
                                style("💰 PROFITABLE").green().bold()
                            } else {
                                style("○ unprofitable").yellow()
                            };
                            
                            println!(
                                "  {}. {} | {} | ${:.0} input ({:?}) | Net: ${:.2}",
                                i + 1,
                                status,
                                style(&path).cyan(),
                                sim.input_usd,
                                tier,
                                sim.profit_usd
                            );
                            
                            if sim.is_profitable && sim.profit_usd >= config.min_profit_usd {
                                simulated_profitable.push((cycle.clone(), sim));
                            }
                        } else {
                            println!(
                                "  {}. {} | {} | Simulation failed",
                                i + 1,
                                style("✗ FAILED").red(),
                                style(&path).cyan()
                            );
                        }
                    }
                    
                    println!();
                    println!(
                        "{} Simulation complete: {} profitable cycles found",
                        style("✓").green(),
                        simulated_profitable.len()
                    );
                    
                    if simulated_profitable.is_empty() && !profitable_candidates.is_empty() {
                        println!();
                        println!("{}", style("📊 Analysis: Graph showed profit, simulation showed loss").yellow());
                        println!("   This is due to SLIPPAGE - the actual price impact when trading.");
                        println!("   The market is efficient; others have already captured the opportunity.");
                        println!();
                        println!("   The bot is working correctly! Opportunities appear during:");
                        println!("   • High volatility (ETH pumps/dumps 5%+)");
                        println!("   • Large whale trades");
                        println!("   • New pool launches");
                    }
                }
            }
            Err(e) => {
                println!(
                    "{} Failed to initialize simulator: {}",
                    style("✗").red(),
                    e
                );
            }
        }
    }

    // =============================================
    // PHASE 4: THE EXECUTOR
    // =============================================
    println!();
    println!(
        "{}",
        style("═══ PHASE 4: THE EXECUTOR ═══").yellow().bold()
    );
    println!();

    let engine = ExecutionEngine::new(config.clone());
    
    match config.execution_mode {
        ExecutionMode::Simulation => {
            println!(
                "{} Mode: {} - Finding opportunities only",
                style("📋").cyan(),
                style("SIMULATION").cyan().bold()
            );
            
            if simulated_profitable.is_empty() {
                println!();
                println!(
                    "{}",
                    style("No profitable opportunities confirmed by simulation.").yellow()
                );
                println!("This is normal in calm markets. The bot is working correctly!");
            } else {
                println!();
                println!(
                    "{}",
                    style(format!("Found {} PROFITABLE opportunities!", simulated_profitable.len())).green().bold()
                );
                
                for (i, (cycle, sim)) in simulated_profitable.iter().enumerate() {
                    let path = cycle.path.iter()
                        .map(|a| format_token(a, &token_symbols))
                        .collect::<Vec<_>>()
                        .join(" → ");
                    
                    println!();
                    println!("{}. {}", i + 1, style(&path).cyan());
                    println!("   DEXes: {}", cycle.dex_path());
                    println!("   Input: ${:.0} ({:?} tier)", sim.input_usd, sim.liquidity_tier);
                    println!("   Expected profit: ${:.2}", sim.profit_usd);
                    println!("   Gas estimate: {} units", sim.total_gas_used);
                    
                    // Execute (in simulation mode, this just logs)
                    match engine.execute(cycle, sim, 0).await {
                        Ok(result) => {
                            if result.is_success() {
                                println!("   Status: {} Would execute!", style("✓").green());
                            }
                        }
                        Err(e) => {
                            warn!("Execution error: {}", e);
                        }
                    }
                }
                
                if config.simulation_log {
                    println!();
                    println!(
                        "{} Opportunities logged to: {}",
                        style("📝").cyan(),
                        config.simulation_log_path
                    );
                }
            }
        }
        
        ExecutionMode::DryRun => {
            println!(
                "{} Mode: {} - Building bundles, not submitting",
                style("🔬").yellow(),
                style("DRY RUN").yellow().bold()
            );
            
            if simulated_profitable.is_empty() {
                println!("No profitable opportunities to test.");
            } else {
                println!("Testing {} opportunities with Flashbots...", simulated_profitable.len());
            }
        }
        
        ExecutionMode::Production => {
            if !engine.is_production_ready() {
                error!("Production mode enabled but requirements not met!");
                error!("Please configure:");
                error!("  - FLASHBOTS_SIGNER_KEY");
                error!("  - PROFIT_WALLET_ADDRESS");
                error!("  - EXECUTOR_CONTRACT_ADDRESS");
            } else {
                println!(
                    "{} Mode: {} - LIVE EXECUTION",
                    style("🚀").red(),
                    style("PRODUCTION").red().bold()
                );
                warn!("⚠️  This mode uses real funds!");
            }
        }
    }

    // =============================================
    // SUMMARY
    // =============================================
    println!();
    println!(
        "{}",
        style("═══════════════════════════════════════════════════════════════").green()
    );
    println!(
        "{}",
        style(" ✅ SCAN COMPLETE").green().bold()
    );
    println!(
        "{}",
        style("═══════════════════════════════════════════════════════════════").green()
    );
    println!();
    println!("Summary:");
    println!("  • Pools scanned: {} across {} DEXes", pools.len(), dex_counts.len());
    println!("  • Cycles analyzed: {} ({} cross-DEX)", cycles.len(), cross_dex_count);
    println!("  • Graph-profitable: {} (before simulation)", profitable_candidates.len());
    println!("  • Simulation-profitable: {}", simulated_profitable.len());
    println!("  • Execution mode: {}", config.execution_mode);
    println!();
    
    if simulated_profitable.is_empty() {
        println!("{}", style("💡 The bot found price differences but slippage makes them unprofitable.").cyan());
        println!("{}", style("   This is normal - run continuously for opportunities during volatility!").cyan());
    }

    Ok(())
}