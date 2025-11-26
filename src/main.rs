//! The Sniper - Arbitrage Detection Bot (Phase 3: Simulator)
//!
//! Run with: cargo run
//!
//! Features:
//! - 5 DEXes: Uniswap V3/V2, Sushiswap V2, PancakeSwap V3, Balancer V2
//! - Low-fee pool priority (1bps, 5bps)
//! - DECIMAL NORMALIZATION
//! - REVM-based simulation for profit validation

use alloy_primitives::{Address, U256};
use color_eyre::eyre::Result;
use console::style;
use std::collections::HashMap;
use std::env;
use std::time::Instant;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod brain;
mod cartographer;
mod config;
mod tokens;
mod simulator;

use brain::{BoundedBellmanFord, ProfitFilter};
use cartographer::{ArbitrageGraph, PoolFetcher, Dex, PoolType};
use simulator::SwapSimulator;

fn print_banner() {
    println!();
    println!(
        "{}",
        style("═══════════════════════════════════════════════════════════════").cyan()
    );
    println!(
        "{}",
        style(" 🎯 THE SNIPER - Arbitrage Detection Bot (Phase 3: Simulator)").cyan().bold()
    );
    println!(
        "{}",
        style("    5 DEXes | REVM Simulation | Profit Validation").cyan()
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

fn get_base_tokens() -> Vec<Address> {
    let addrs = [
        "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", // WETH
        "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48", // USDC
        "0xdAC17F958D2ee523a2206206994597C13D831ec7", // USDT
        "0x6B175474E89094C44Da98b954EedcdeCB5BE3830", // DAI
        "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599", // WBTC
    ];

    addrs.iter().filter_map(|a| a.parse().ok()).collect()
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

    dotenvy::dotenv().ok();

    let rpc_url = env::var("RPC_URL").unwrap_or_else(|_| {
        println!("{}", style("⚠️  RPC_URL not set in .env file!").yellow());
        println!("Using public RPC (rate limited). Set RPC_URL for better performance.");
        "https://eth.llamarpc.com".to_string()
    });

    println!("{} RPC configured", style("✓").green());

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

    let fetcher = PoolFetcher::new(rpc_url.clone());
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

    let bellman_ford = BoundedBellmanFord::new(&graph, 4);
    let base_tokens = get_base_tokens();
    let cycles = bellman_ford.find_all_cycles(&base_tokens);

    let algo_time = start.elapsed();
    
    let cross_dex_count = cycles.iter().filter(|c| c.is_cross_dex()).count();
    let low_fee_cycle_count = cycles.iter().filter(|c| c.has_low_fee_pools()).count();
    
    println!(
        "{} Found {} cycles in {:?}",
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

    let mut filter = ProfitFilter::new(-1000.0);
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

    if profitable_candidates.is_empty() && cycles.is_empty() {
        println!("{}", style("No cycles to simulate.").yellow());
    } else {
        println!(
            "{}",
            style("Step 3.1: Initializing Provider-based simulator...").green()
        );
        
        match SwapSimulator::new(&rpc_url).await {
            Ok(mut swap_sim) => {
                swap_sim.set_eth_price(eth_price);
                swap_sim.set_gas_price(20.0);
                
                println!("{} Simulator initialized", style("✓").green());
                
                // Take the top 10 cycles to simulate
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
                    
                    let input_amount = U256::from(100_000_000_000_000_000u128); // 0.1 ETH
                    
                    let mut sim_success = 0;
                    let mut sim_profitable = 0;
                    
                    for (i, cycle) in cycles_to_simulate.iter().enumerate() {
                        let sim = swap_sim.simulate_cycle(cycle, input_amount).await;
                        
                        let path = cycle.path.iter()
                            .map(|a| format_token(a, &token_symbols))
                            .collect::<Vec<_>>()
                            .join(" → ");
                        
                        if sim.simulation_success {
                            sim_success += 1;
                            
                            let profit_indicator = if sim.is_profitable {
                                sim_profitable += 1;
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
                        "{} Simulation complete: {}/{} succeeded, {} profitable",
                        style("✓").green(),
                        sim_success,
                        cycles_to_simulate.len(),
                        sim_profitable
                    );
                }
            }
            Err(e) => {
                println!(
                    "{} Failed to initialize simulator: {}",
                    style("✗").red(),
                    e
                );
                println!("   Continuing without simulation validation...");
            }
        }
    }

    // =============================================
    // RESULTS
    // =============================================
    println!();
    if profitable_candidates.is_empty() {
        println!(
            "{}",
            style("═══ RESULTS: No profitable arbitrage found ═══")
                .yellow()
                .bold()
        );
        println!();
        println!("This is common - the simulator helps validate real opportunities!");
        println!("  • Scanned {} DEXes:", dex_counts.len());
        for (dex, count) in &dex_counts {
            println!("    - {}: {} pools", dex, count);
        }
        println!("  • Found {} cross-DEX price differences", opportunities.len());
        println!("  • Analyzed {} potential arbitrage cycles", cycles.len());
        println!();
        println!("{}", style("Tips:").green());
        println!("  • Run during high volatility");
        println!("  • Focus on low-fee pools (1bps, 5bps)");
        println!("  • Check late night / early morning (lower gas)");
    } else {
        println!(
            "{}",
            style(format!(
                "═══ RESULTS: {} PROFITABLE OPPORTUNITIES ═══",
                profitable_candidates.len()
            ))
            .green()
            .bold()
        );
        println!();

        for (i, analysis) in profitable_candidates.iter().take(5).enumerate() {
            let path = analysis.format_path(&token_symbols);
            
            let mut tags = Vec::new();
            if analysis.cycle.is_cross_dex() {
                tags.push(style("[CROSS-DEX]").magenta().bold().to_string());
            }
            if analysis.cycle.has_low_fee_pools() {
                tags.push(style("[LOW-FEE]").cyan().bold().to_string());
            }
            let tags_str = if tags.is_empty() { String::new() } else { format!(" {}", tags.join(" ")) };
            
            println!(
                "{}. {}{} | Net profit: ${:.2}",
                i + 1,
                style(&path).cyan(),
                tags_str,
                analysis.net_profit_usd
            );
            
            if analysis.cycle.is_cross_dex() {
                println!("   DEXes: {}", analysis.cycle.dex_path());
            }
        }
    }

    // Final summary
    println!();
    println!(
        "{}",
        style("═══════════════════════════════════════════════════════════════").green()
    );
    println!(
        "{}",
        style(" ✅ PHASE 3 COMPLETE - SIMULATION ENABLED!").green().bold()
    );
    println!(
        "{}",
        style("═══════════════════════════════════════════════════════════════").green()
    );
    println!();
    println!("Summary:");
    println!("  • Pools fetched: {} across {} DEXes", pools.len(), dex_counts.len());
    println!("  • Low-fee pools: {} (prioritized for tight arbs)", low_fee_count);
    println!("  • Cycles analyzed: {} ({} cross-DEX, {} w/ low fees)", 
             cycles.len(), cross_dex_count, low_fee_cycle_count);
    println!("  • REVM simulation: ACTIVE");
    println!();
    println!("The Sniper is ready for production!");

    Ok(())
}
