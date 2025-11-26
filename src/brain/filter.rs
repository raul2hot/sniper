//! Profit Filter
//!
//! Step 2.2: The Filter
//!
//! Filters out "dust" profits that won't pay for gas.
//! Now highlights cross-DEX opportunities!
//!
//! Success Criteria:
//! - Console filters out unprofitable cycles
//! - Console highlights: "PROFITABLE CANDIDATE: Expected Return $12.40"

use alloy::primitives::Address;
use console::style;
use std::collections::HashMap;
use tracing::{debug, info, warn};

use super::ArbitrageCycle;

/// Result of profit analysis for a cycle
#[derive(Debug, Clone)]
pub struct ProfitAnalysis {
    /// The original cycle
    pub cycle: ArbitrageCycle,
    
    /// Input amount in USD (how much we'd trade)
    pub input_usd: f64,
    
    /// Gross profit in USD (before gas)
    pub gross_profit_usd: f64,
    
    /// Estimated gas cost in USD
    pub gas_cost_usd: f64,
    
    /// Net profit in USD (gross - gas)
    pub net_profit_usd: f64,
    
    /// Is this profitable after gas?
    pub is_profitable: bool,
}

impl ProfitAnalysis {
    /// Format path with symbols for display
    pub fn format_path(&self, symbols: &HashMap<Address, &str>) -> String {
        self.cycle
            .path
            .iter()
            .map(|addr| {
                symbols
                    .get(addr)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("0x{}...", &format!("{:?}", addr)[2..8]))
            })
            .collect::<Vec<_>>()
            .join(" → ")
    }
    
    /// Format path with DEX info
    pub fn format_path_with_dex(&self, symbols: &HashMap<Address, &str>) -> String {
        let mut parts = Vec::new();
        
        for (i, addr) in self.cycle.path.iter().enumerate() {
            let token = symbols
                .get(addr)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("0x{}...", &format!("{:?}", addr)[2..8]));
            
            if i > 0 && i <= self.cycle.dexes.len() {
                let dex = &self.cycle.dexes[i - 1];
                parts.push(format!("-[{}]-> {}", dex, token));
            } else {
                parts.push(token);
            }
        }
        
        parts.join("")
    }
}

/// Profit calculator and filter
pub struct ProfitFilter {
    /// Minimum net profit threshold in USD
    min_profit_usd: f64,
    
    /// Estimated gas per swap (in gas units)
    gas_per_swap: u64,
    
    /// Current gas price in gwei
    gas_price_gwei: f64,
    
    /// Current ETH price in USD
    eth_price_usd: f64,
    
    /// Default input amount for calculations
    default_input_usd: f64,
}

impl ProfitFilter {
    /// Create a new profit filter
    pub fn new(min_profit_usd: f64) -> Self {
        Self {
            min_profit_usd,
            gas_per_swap: 150_000,     // Conservative estimate for Uniswap V3 swap
            gas_price_gwei: 20.0,       // Moderate gas price
            eth_price_usd: 2930.0,      // Will be updated from actual data
            default_input_usd: 10_000.0, // $10k default trade size
        }
    }

    /// Update ETH price (call this with data from the graph)
    pub fn set_eth_price(&mut self, eth_price_usd: f64) {
        self.eth_price_usd = eth_price_usd;
    }

    /// Update gas price
    pub fn set_gas_price(&mut self, gas_price_gwei: f64) {
        self.gas_price_gwei = gas_price_gwei;
    }

    /// Calculate gas cost for a given number of swaps
    fn calculate_gas_cost(&self, num_swaps: usize) -> f64 {
        // Gas cost = gas_units × gas_price × ETH_price
        // gas_price is in gwei (1 gwei = 10^-9 ETH)
        let total_gas_units = (num_swaps as u64) * self.gas_per_swap;
        let gas_cost_eth = (total_gas_units as f64) * self.gas_price_gwei * 1e-9;
        gas_cost_eth * self.eth_price_usd
    }

    /// Analyze a single cycle for profitability
    pub fn analyze(&self, cycle: &ArbitrageCycle, input_usd: Option<f64>) -> ProfitAnalysis {
        let input = input_usd.unwrap_or(self.default_input_usd);
        let num_swaps = cycle.hop_count();

        // Gross profit = input × (return - 1)
        let gross_profit_usd = input * (cycle.expected_return - 1.0);

        // Gas cost
        let gas_cost_usd = self.calculate_gas_cost(num_swaps);

        // Net profit
        let net_profit_usd = gross_profit_usd - gas_cost_usd;

        // Is it profitable?
        let is_profitable = net_profit_usd >= self.min_profit_usd;

        ProfitAnalysis {
            cycle: cycle.clone(),
            input_usd: input,
            gross_profit_usd,
            gas_cost_usd,
            net_profit_usd,
            is_profitable,
        }
    }

    /// Filter cycles and return only profitable ones
    pub fn filter_profitable(
        &self,
        cycles: &[ArbitrageCycle],
        symbols: &HashMap<Address, &str>,
    ) -> Vec<ProfitAnalysis> {
        let mut profitable = Vec::new();
        let mut filtered_count = 0;

        for cycle in cycles {
            let analysis = self.analyze(cycle, None);

            if analysis.is_profitable {
                let path = analysis.format_path(symbols);
                let cross_dex_tag = if cycle.is_cross_dex() {
                    style(" [CROSS-DEX]").magenta().bold()
                } else {
                    style("").dim()
                };
                
                info!(
                    "{}",
                    style(format!(
                        "💰 PROFITABLE: {}{} | Gross: ${:.2} | Gas: ${:.2} | Net: ${:.2}",
                        path,
                        cross_dex_tag,
                        analysis.gross_profit_usd,
                        analysis.gas_cost_usd,
                        analysis.net_profit_usd
                    ))
                    .green()
                    .bold()
                );
                profitable.push(analysis);
            } else {
                filtered_count += 1;
                let path = analysis.format_path(symbols);
                debug!(
                    "Filtered: {} | Return: {:.4}x | Gross: ${:.2} | Gas: ${:.2} | Net: ${:.2}",
                    path,
                    cycle.expected_return,
                    analysis.gross_profit_usd,
                    analysis.gas_cost_usd,
                    analysis.net_profit_usd
                );
            }
        }

        if filtered_count > 0 {
            info!(
                "Filtered out {} cycles below ${:.2} profit threshold",
                filtered_count, self.min_profit_usd
            );
        }

        // Sort by net profit (best first)
        profitable.sort_by(|a, b| {
            b.net_profit_usd
                .partial_cmp(&a.net_profit_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        profitable
    }

    /// Print a summary of all analyzed cycles
    pub fn print_summary(
        &self,
        cycles: &[ArbitrageCycle],
        symbols: &HashMap<Address, &str>,
    ) {
        if cycles.is_empty() {
            warn!("No cycles found to analyze");
            return;
        }

        // Separate cross-DEX and single-DEX cycles
        let cross_dex: Vec<_> = cycles.iter().filter(|c| c.is_cross_dex()).collect();
        let single_dex: Vec<_> = cycles.iter().filter(|c| !c.is_cross_dex()).collect();

        println!();
        println!("{}", style("═══ CYCLE ANALYSIS ═══").yellow().bold());
        println!();
        println!(
            "Analysis parameters: Input=${:.0}, Gas={} gwei, ETH=${:.0}",
            self.default_input_usd, self.gas_price_gwei, self.eth_price_usd
        );
        println!(
            "Minimum profit threshold: ${:.2}",
            self.min_profit_usd
        );
        println!();
        println!(
            "Found {} total cycles: {} cross-DEX, {} single-DEX",
            cycles.len(), cross_dex.len(), single_dex.len()
        );

        // Show cross-DEX cycles first (they're more interesting!)
        if !cross_dex.is_empty() {
            println!();
            println!("{}", style("=== CROSS-DEX CYCLES (Most Likely Profitable) ===").magenta().bold());
            println!();

            for (i, cycle) in cross_dex.iter().take(10).enumerate() {
                let analysis = self.analyze(cycle, None);
                let path = analysis.format_path(symbols);
                
                let status = if analysis.is_profitable {
                    style("✓ PROFITABLE").green()
                } else if analysis.net_profit_usd > 0.0 {
                    style("○ marginal").yellow()
                } else {
                    style("✗ unprofitable").red()
                };

                println!(
                    "  {}. {} | {:.4}x return ({:+.3}%)",
                    i + 1,
                    status,
                    cycle.expected_return,
                    cycle.profit_percentage()
                );
                println!("     Path: {}", style(path).cyan());
                println!("     DEXes: {}", style(cycle.dex_path()).magenta());
                println!(
                    "     Gross: ${:+.2} | Gas: ${:.2} | Net: ${:+.2}",
                    analysis.gross_profit_usd,
                    analysis.gas_cost_usd,
                    analysis.net_profit_usd
                );
                println!();
            }
        }

        // Show top single-DEX cycles
        println!();
        println!("{}", style("=== SINGLE-DEX CYCLES ===").blue().bold());
        println!();

        let to_show = single_dex.len().min(5);
        for (i, cycle) in single_dex.iter().take(to_show).enumerate() {
            let analysis = self.analyze(cycle, None);
            let path = analysis.format_path(symbols);
            
            let status = if analysis.is_profitable {
                style("✓ PROFITABLE").green()
            } else if analysis.net_profit_usd > 0.0 {
                style("○ marginal").yellow()
            } else {
                style("✗ unprofitable").red()
            };

            println!(
                "  {}. {} | {:.4}x return ({:+.3}%)",
                i + 1,
                status,
                cycle.expected_return,
                cycle.profit_percentage()
            );
            println!("     Path: {}", style(path).cyan());
            println!(
                "     Gross: ${:+.2} | Gas: ${:.2} | Net: ${:+.2}",
                analysis.gross_profit_usd,
                analysis.gas_cost_usd,
                analysis.net_profit_usd
            );
            println!();
        }

        if single_dex.len() > to_show {
            println!("  ... and {} more single-DEX cycles", single_dex.len() - to_show);
        }
    }
}

impl Default for ProfitFilter {
    fn default() -> Self {
        Self::new(5.0) // $5 minimum profit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartographer::Dex;

    fn make_test_cycle(return_mult: f64, hops: usize, cross_dex: bool) -> ArbitrageCycle {
        let dexes = if cross_dex {
            vec![Dex::UniswapV3, Dex::SushiswapV2, Dex::UniswapV3][..hops].to_vec()
        } else {
            vec![Dex::UniswapV3; hops]
        };
        
        ArbitrageCycle {
            path: vec![Address::ZERO; hops + 1],
            pools: vec![Address::ZERO; hops],
            dexes,
            total_weight: -(return_mult.ln()),
            expected_return: return_mult,
            prices: vec![1.0; hops],
            fees: vec![3000; hops],
        }
    }

    #[test]
    fn test_gas_cost_calculation() {
        let filter = ProfitFilter {
            min_profit_usd: 5.0,
            gas_per_swap: 150_000,
            gas_price_gwei: 20.0,
            eth_price_usd: 3000.0,
            default_input_usd: 10_000.0,
        };

        let gas_cost = filter.calculate_gas_cost(3);
        assert!(
            (gas_cost - 27.0).abs() < 0.1,
            "Expected ~$27, got ${:.2}",
            gas_cost
        );
    }

    #[test]
    fn test_profitable_cross_dex_cycle() {
        let filter = ProfitFilter::new(5.0);

        let cycle = make_test_cycle(1.05, 3, true);
        assert!(cycle.is_cross_dex(), "Should be cross-DEX");
        
        let analysis = filter.analyze(&cycle, Some(10_000.0));
        assert!(analysis.is_profitable);
    }
}
