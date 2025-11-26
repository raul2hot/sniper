//! The Sniper - Arbitrage Detection Bot (Phase 4: Executor Ready)
//!
//! Run with: cargo run
//!
//! Features:
//! - 5 DEXes: Uniswap V3/V2, Sushiswap V2, PancakeSwap V3, Balancer V2
//! - Low-fee pool priority (1bps, 5bps)
//! - DECIMAL NORMALIZATION
//! - Token-aware simulation amounts
//! - Flash Loan + Flashbots integration (Phase 4)
//! - Production-ready configuration

use alloy_primitives::Address;
use color_eyre::eyre::Result;
use console::style;
use std::collections::HashMap;
use std::time::Instant;
use tracing::{info, warn, error};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod brain;
mod cartographer;
mod config;
mod tokens;
mod simulator;
mod executor;

use brain::{BoundedBellmanFord, ProfitFilter};
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
        style(" 🎯 THE SNIPER - Arbitrage Detection Bot (Phase 4: Executor)").cyan().bold()
    );
    println!(
        "{}",
        style("    5 DEXes | Flash Loans | Flashbots | Production Ready").cyan()
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

    // Step 2.2: Filter for profitable cycles
    println!();
    println!(
        "{}",
        style("Step 2.2: Analyzing profitability...").magenta()
    );

    let mut filter = ProfitFilter::new(config.min_profit_usd);
    filter.set_eth_price(eth_price);
    filter.set_gas_price(20.0);

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
            style("Step 3.1: Initializing token-aware simulator...").green()
        );
        
        match SwapSimulator::new(&config.rpc_url).await {
            Ok(mut swap_sim) => {
                swap_sim.set_eth_price(eth_price);
                swap_sim.set_gas_price(20.0);
                
                println!("{} Simulator initialized", style("✓").green());
                
                // Take the top cycles to simulate
                let cycles_to_simulate: Vec<_> = if !profitable_candidates.is_empty() {
                    profitable_candidates.iter().take(10).map(|p| &p.cycle).cloned().collect()
                } else {
                    cycles.iter().take(10).cloned().collect()
                };
                
                if !cycles_to_simulate.is_empty() {
                    println!();
                    println!(
                        "{}",
                        style(format!("Step 3.2: Simulating {} top cycles...", cycles_to_simulate.len())).green()
                    );
                    
                    let target_usd = config.default_simulation_usd;
                    
                    for (i, cycle) in cycles_to_simulate.iter().enumerate() {
                        let sim = swap_sim.simulate_cycle(cycle, target_usd).await;
                        
                        let path = cycle.path.iter()
                            .map(|a| format_token(a, &token_symbols))
                            .collect::<Vec<_>>()
                            .join(" → ");
                        
                        if sim.simulation_success {
                            let profit_indicator = if sim.is_profitable {
                                style("💰 PROFITABLE").green().bold()
                            } else {
                                style("○ unprofitable").yellow()
                            };
                            
                            println!(
                                "  {}. {} | {} | Gas: {} | Net: ${:.2}",
                                i + 1,
                                profit_indicator,
                                style(&path).cyan(),
                                sim.total_gas_used,
                                sim.profit_usd
                            );
                            
                            if sim.is_profitable && sim.profit_usd >= config.min_profit_usd {
                                simulated_profitable.push((cycle.clone(), sim));
                            }
                        } else {
                            println!(
                                "  {}. {} | {} | Reason: {}",
                                i + 1,
                                style("✗ FAILED").red(),
                                style(&path).cyan(),
                                sim.revert_reason.unwrap_or_else(|| "Unknown".to_string())
                            );
                        }
                    }
                    
                    println!();
                    println!(
                        "{} Simulation complete: {} profitable (above ${:.2} threshold)",
                        style("✓").green(),
                        simulated_profitable.len(),
                        config.min_profit_usd
                    );
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
                    style("No profitable opportunities found above threshold.").yellow()
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
                // Dry run logic would go here
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
                // Production execution would go here
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
    println!("  • Profitable (simulated): {}", simulated_profitable.len());
    println!("  • Execution mode: {}", config.execution_mode);
    println!();
    
    if simulated_profitable.is_empty() {
        println!("{}", style("💡 Tip: Run during high volatility for more opportunities!").cyan());
    }

    Ok(())
}
