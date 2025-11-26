//! The Sniper - Arbitrage Detection Bot
//!
//! This is the entry point. Run with: cargo run
//!
//! Phase 1: The Cartographer (Data Ingest) ✅
//! Phase 2: The Brain (Cycle Detection) ← WE ARE HERE
//! Phase 3: The Simulator (V4 Hook Integration)

use alloy::primitives::Address;
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

use brain::{BoundedBellmanFord, ProfitFilter};
use cartographer::{ArbitrageGraph, PoolFetcher};

/// Prints the Sniper banner
fn print_banner() {
    println!();
    println!(
        "{}",
        style("=============================================").cyan()
    );
    println!(
        "{}",
        style(" 🎯 THE SNIPER - Arbitrage Detection Bot").cyan().bold()
    );
    println!(
        "{}",
        style("=============================================").cyan()
    );
    println!();
}

/// Build a map of token address -> symbol for pretty printing
fn build_token_symbols() -> HashMap<Address, &'static str> {
    let mut map = HashMap::new();

    // Known token addresses on Ethereum Mainnet
    let tokens = [
        ("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", "WETH"),
        ("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48", "USDC"),
        ("0xdAC17F958D2ee523a2206206994597C13D831ec7", "USDT"),
        ("0x6B175474E89094C44Da98b954EescdeCB5BE3830", "DAI"),
        ("0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599", "WBTC"),
        ("0x514910771AF9Ca656af840dff83E8264EcF986CA", "LINK"),
        ("0x1f9840a85d5aF5bf1D1762F925BDADdC4201F984", "UNI"),
        ("0x6982508145454Ce325dDbE47a25d4ec3d2311933", "PEPE"),
        ("0x95aD61b0a150d79219dCF64E1E6Cc01f0B64C4cE", "SHIB"),
        ("0x5A98FcBEA516Cf06857215779Fd812CA3beF1B32", "LDO"),
        ("0x9f8F72aA9304c8B593d555F12eF6589cC3A579A2", "MKR"),
        ("0x7D1AfA7B718fb893dB30A3aBc0Cfc608AaCfeBB0", "MATIC"),
    ];

    for (addr, symbol) in tokens {
        if let Ok(address) = addr.parse() {
            map.insert(address, symbol);
        }
    }

    map
}

/// Get base token addresses for starting arbitrage search
fn get_base_tokens() -> Vec<Address> {
    let addrs = [
        "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", // WETH
        "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48", // USDC
        "0xdAC17F958D2ee523a2206206994597C13D831ec7", // USDT
        "0x6B175474E89094C44Da98b954EescdeCB5BE3830", // DAI
        "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599", // WBTC
    ];

    addrs
        .iter()
        .filter_map(|a| a.parse().ok())
        .collect()
}

/// Format an address with symbol if known
fn format_token(addr: &Address, symbols: &HashMap<Address, &str>) -> String {
    if let Some(symbol) = symbols.get(addr) {
        symbol.to_string()
    } else {
        format!("0x{}...", &format!("{:?}", addr)[2..8])
    }
}

/// Extract ETH price from pools (USDC/WETH price)
fn get_eth_price_from_pools(pools: &[cartographer::PoolState]) -> f64 {
    // Find USDC/WETH or WETH/USDT pool
    for pool in pools {
        // Check for USDC/WETH (USDC has 6 decimals, WETH has 18)
        let price = pool.price(6, 18);
        if price > 1000.0 && price < 10000.0 {
            return price;
        }
        let inverse = pool.price(18, 6);
        if inverse > 1000.0 && inverse < 10000.0 {
            return inverse;
        }
    }
    // Default fallback
    2930.0
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize error handling
    color_eyre::install()?;

    // Initialize logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("sniper=info".parse()?),
        )
        .init();

    // Print banner
    print_banner();

    // Load environment variables
    dotenvy::dotenv().ok();

    let rpc_url = env::var("RPC_URL").unwrap_or_else(|_| {
        println!("{}", style("⚠️  RPC_URL not set in .env file!").yellow());
        println!("Using public RPC (rate limited). Set RPC_URL for better performance.");
        "https://eth.llamarpc.com".to_string()
    });

    println!("{} RPC configured", style("✓").green());

    // Build token symbol map
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

    // Step 1.1: Fetch pool data
    println!("{}", style("Step 1.1: Fetching pool data...").blue());
    let start = Instant::now();

    let fetcher = PoolFetcher::new(rpc_url);
    let pools = fetcher.fetch_all_pools().await?;

    let fetch_time = start.elapsed();
    println!(
        "{} Fetched {} pools in {:?}",
        style("✓").green(),
        pools.len(),
        fetch_time
    );

    // Get ETH price for profit calculations
    let eth_price = get_eth_price_from_pools(&pools);
    println!("{} ETH price: ${:.2}", style("✓").green(), eth_price);

    // Step 1.2: Build the graph
    println!();
    println!("{}", style("Step 1.2: Building arbitrage graph...").blue());
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

    // Show some price examples
    println!();
    println!("{}", style("Sample prices:").blue());
    for pool in pools.iter().take(5) {
        let token0_sym = format_token(&pool.token0, &token_symbols);
        let token1_sym = format_token(&pool.token1, &token_symbols);

        let (t0_dec, t1_dec) = match (token0_sym.as_str(), token1_sym.as_str()) {
            (t0, t1) => {
                let d0 = match t0 {
                    "USDC" | "USDT" => 6,
                    "WBTC" => 8,
                    _ => 18,
                };
                let d1 = match t1 {
                    "USDC" | "USDT" => 6,
                    "WBTC" => 8,
                    _ => 18,
                };
                (d0, d1)
            }
        };

        let price = pool.price(t0_dec, t1_dec);
        println!(
            "  {}/{}: {:.6} (fee: {}bps)",
            style(&token0_sym).cyan(),
            style(&token1_sym).cyan(),
            price,
            pool.fee / 100
        );
    }

    // =============================================
    // PHASE 2: THE BRAIN
    // =============================================
    println!();
    println!(
        "{}",
        style("═══ PHASE 2: THE BRAIN ═══").magenta().bold()
    );
    println!();

    // Step 2.1: Find arbitrage cycles using Bellman-Ford
    println!(
        "{}",
        style("Step 2.1: Running Bellman-Ford algorithm...").magenta()
    );
    let start = Instant::now();

    let bellman_ford = BoundedBellmanFord::new(&graph, 4); // Max 4 hops
    let base_tokens = get_base_tokens();
    let cycles = bellman_ford.find_all_cycles(&base_tokens);

    let algo_time = start.elapsed();
    println!(
        "{} Found {} cycles in {:?}",
        style("✓").green(),
        cycles.len(),
        algo_time
    );

    // Step 2.2: Filter for profitable cycles
    println!();
    println!(
        "{}",
        style("Step 2.2: Analyzing profitability...").magenta()
    );

    let mut filter = ProfitFilter::new(-1000.0); // Show ALL cycles, even unprofitable
    filter.set_eth_price(eth_price);
    filter.set_gas_price(20.0); // 20 gwei

    // Print detailed analysis
    filter.print_summary(&cycles, &token_symbols);

    // Get profitable cycles
    let profitable = filter.filter_profitable(&cycles, &token_symbols);

    // =============================================
    // RESULTS
    // =============================================
    println!();
    if profitable.is_empty() {
        println!(
            "{}",
            style("═══ RESULTS: No profitable arbitrage found ═══")
                .yellow()
                .bold()
        );
        println!();
        println!("This is expected! Here's why:");
        println!("  • Ethereum mainnet is heavily arbitraged by MEV bots");
        println!("  • Most opportunities last <1 second");
        println!("  • Our data is ~6 seconds old by the time we analyze it");
        println!();
        println!("To find real opportunities, you'd need:");
        println!("  • Direct mempool access");
        println!("  • Sub-100ms execution");
        println!("  • Flashbots integration");
        println!("  • Cross-DEX or CEX-DEX arbitrage");
    } else {
        println!(
            "{}",
            style(format!(
                "═══ RESULTS: {} PROFITABLE OPPORTUNITIES ═══",
                profitable.len()
            ))
            .green()
            .bold()
        );
        println!();

        for (i, analysis) in profitable.iter().enumerate() {
            let path = analysis.format_path(&token_symbols);
            println!(
                "{}. {} | Net profit: ${:.2}",
                i + 1,
                style(&path).cyan(),
                analysis.net_profit_usd
            );
        }
    }

    // Final summary
    println!();
    println!(
        "{}",
        style("=============================================").green()
    );
    println!(
        "{}",
        style(" ✅ PHASE 2 COMPLETE!").green().bold()
    );
    println!(
        "{}",
        style("=============================================").green()
    );
    println!();
    println!("Next: Phase 3 - REVM simulation to validate trades!");

    Ok(())
}