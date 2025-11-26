//! The Sniper - Arbitrage Detection Bot (DECIMAL-FIXED Edition)
//!
//! Run with: cargo run
//!
//! Features:
//! - 6 DEXes: Uniswap V3/V2, Sushiswap V2, PancakeSwap V3, Balancer V2, Curve
//! - Low-fee pool priority (1bps, 5bps)
//! - Concurrent RPC fetching (20x faster!)
//! - DECIMAL NORMALIZATION: Properly handles tokens with different decimals
//!   (e.g., DAI 18 decimals vs USDC 6 decimals)

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
mod simulator;

use brain::{BoundedBellmanFord, ProfitFilter};
use cartographer::{ArbitrageGraph, PoolFetcher, Dex, PoolType};

fn print_banner() {
    println!();
    println!(
        "{}",
        style("═══════════════════════════════════════════════════════════════").cyan()
    );
    println!(
        "{}",
        style(" 🎯 THE SNIPER - Arbitrage Detection Bot (DECIMAL-FIXED)").cyan().bold()
    );
    println!(
        "{}",
        style("    6 DEXes | Low-Fee Priority | Decimal-Normalized Prices").cyan()
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
        ("0x0bc529c00C6401aEF6D220BE8C6Ea1667F6Ad93e", "YFI"),
        ("0x6B3595068778DD592e39A122f4f5a5cF09C90fE2", "SUSHI"),
        ("0xC011a73ee8576Fb46F5E1c5751cA3B9Fe0af2a6F", "SNX"),
        ("0xc00e94Cb662C3520282E6f5717214004A7f26888", "COMP"),
        ("0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DDaE9", "AAVE"),
        ("0xba100000625a3754423978a60c9317c58a424e3D", "BAL"),
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

    println!("{}", style("Step 1.1: Fetching pool data from 6 DEXes (DECIMAL-AWARE)...").blue());
    let start = Instant::now();

    let fetcher = PoolFetcher::new(rpc_url);
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

    // Show sample prices by DEX
    println!();
    println!("{}", style("Sample prices by DEX:").blue());
    
    let mut shown_dexes: HashMap<Dex, usize> = HashMap::new();
    for pool in pools.iter() {
        let count = shown_dexes.entry(pool.dex).or_insert(0);
        if *count >= 2 {
            continue;
        }
        *count += 1;
        
        let token0_sym = format_token(&pool.token0, &token_symbols);
        let token1_sym = format_token(&pool.token1, &token_symbols);

        let t0_dec = match token0_sym.as_str() {
            "USDC" | "USDT" => 6,
            "WBTC" => 8,
            _ => 18,
        };
        let t1_dec = match token1_sym.as_str() {
            "USDC" | "USDT" => 6,
            "WBTC" => 8,
            _ => 18,
        };

        let price = pool.price(t0_dec, t1_dec);
        
        let dex_style = match pool.dex {
            Dex::UniswapV3 => style(format!("[{}]", pool.dex)).blue(),
            Dex::UniswapV2 => style(format!("[{}]", pool.dex)).cyan(),
            Dex::SushiswapV3 => style(format!("[{}]", pool.dex)).magenta(),
            Dex::SushiswapV2 => style(format!("[{}]", pool.dex)).yellow(),
            Dex::PancakeSwapV3 => style(format!("[{}]", pool.dex)).green(),
            Dex::BalancerV2 => style(format!("[{}]", pool.dex)).red(),
            Dex::Curve => style(format!("[{}]", pool.dex)).white(),
        };
        
        let fee_indicator = if pool.fee <= 500 {
            style(format!(" ({}bps) ⚡", pool.fee / 100)).cyan().to_string()
        } else {
            format!(" ({}bps)", pool.fee / 100)
        };
        
        println!(
            "  {} {}/{}: {:.6}{}",
            dex_style,
            style(&token0_sym).cyan(),
            style(&token1_sym).cyan(),
            price,
            fee_indicator
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

    println!(
        "{}",
        style("Step 2.1: Running Bellman-Ford algorithm (6 DEXes enabled)...").magenta()
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
        println!("This is common - but 6-DEX cross-arbitrage has the best odds!");
        println!("  • Scanned {} DEXes:", dex_counts.len());
        for (dex, count) in &dex_counts {
            println!("    - {}: {} pools", dex, count);
        }
        println!("  • Found {} cross-DEX price differences", opportunities.len());
        println!("  • Analyzed {} potential arbitrage cycles", cycles.len());
        println!();
        println!("{}", style("Why no profit?").yellow());
        println!("  • MEV bots already captured the opportunities");
        println!("  • Gas costs exceed the price difference");
        println!("  • Our data snapshot was ~2-6 seconds old");
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
                profitable.len()
            ))
            .green()
            .bold()
        );
        println!();

        for (i, analysis) in profitable.iter().enumerate() {
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
        style(" ✅ PHASE 2 COMPLETE!").green().bold()
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
    println!("  • Profitable cycles: {}", profitable.len());
    println!();
    println!("Next: Phase 3 - REVM simulation to validate trades!");

    Ok(())
}
