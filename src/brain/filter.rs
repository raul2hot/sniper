//! Profit Filter - FIXED Edition
//!
//! FIXES:
//! 1. Better suspicious cycle detection
//! 2. More granular profit thresholds
//! 3. Gas cost estimation per DEX type

use alloy_primitives::Address;
use console::style;
use std::collections::HashMap;
use tracing::{debug, info, warn};

use super::ArbitrageCycle;
use crate::cartographer::Dex;

/// Result of profit analysis for a cycle
#[derive(Debug, Clone)]
pub struct ProfitAnalysis {
    pub cycle: ArbitrageCycle,
    pub input_usd: f64,
    pub gross_profit_usd: f64,
    pub gas_cost_usd: f64,
    pub net_profit_usd: f64,
    pub is_profitable: bool,
    pub is_suspicious: bool,
    pub suspicion_reason: Option<String>,
}

impl ProfitAnalysis {
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
}

/// Profit calculator and filter
pub struct ProfitFilter {
    min_profit_usd: f64,
    gas_per_swap_v3: u64,
    gas_per_swap_v2: u64,
    gas_per_swap_balancer: u64,
    gas_per_swap_curve: u64,
    gas_price_gwei: f64,
    eth_price_usd: f64,
    default_input_usd: f64,
    
    // Suspicious thresholds
    max_reasonable_return: f64,
    max_profit_usd: f64,
}

impl ProfitFilter {
    pub fn new(min_profit_usd: f64) -> Self {
        Self {
            min_profit_usd,
            gas_per_swap_v3: 150_000,
            gas_per_swap_v2: 100_000,
            gas_per_swap_balancer: 120_000,
            gas_per_swap_curve: 200_000,
            gas_price_gwei: 0.5,  // Updated for Nov 2025 low-gas environment
            eth_price_usd: 3000.0,
            default_input_usd: 10_000.0,
            
            // Reasonable limits
            max_reasonable_return: 1.10, // 10% return is very high but possible
            max_profit_usd: 10_000.0,    // $10K profit on single arb is suspicious
        }
    }

    pub fn set_eth_price(&mut self, eth_price_usd: f64) {
        self.eth_price_usd = eth_price_usd;
    }

    pub fn set_gas_price(&mut self, gas_price_gwei: f64) {
        // Enforce minimum realistic gas price
        self.gas_price_gwei = gas_price_gwei.max(1.0);
    }
    
    pub fn set_default_input(&mut self, input_usd: f64) {
        self.default_input_usd = input_usd;
    }

    fn calculate_gas_cost(&self, cycle: &ArbitrageCycle) -> f64 {
        let mut total_gas_units: u64 = 0;
        
        for dex in &cycle.dexes {
            let gas = match dex {
                Dex::UniswapV3 | Dex::SushiswapV3 | Dex::PancakeSwapV3 => self.gas_per_swap_v3,
                Dex::UniswapV2 | Dex::SushiswapV2 => self.gas_per_swap_v2,
                Dex::BalancerV2 => self.gas_per_swap_balancer,
                Dex::Curve => self.gas_per_swap_curve,
            };
            total_gas_units += gas;
        }
        
        total_gas_units += 50_000;  // Flash loan overhead
        
        let gas_cost_eth = (total_gas_units as f64) * self.gas_price_gwei * 1e-9;
        gas_cost_eth * self.eth_price_usd
    }
    
    /// Check if a cycle looks suspicious (likely a bug)
    fn check_suspicious(&self, cycle: &ArbitrageCycle, gross_profit_usd: f64) -> Option<String> {
        // 1. Unrealistically high return (> 10%)
        if cycle.expected_return > self.max_reasonable_return {
            return Some(format!(
                "Return {:.2}x exceeds reasonable limit of {:.2}x",
                cycle.expected_return, self.max_reasonable_return
            ));
        }
        
        // 2. Unrealistically high profit
        if gross_profit_usd > self.max_profit_usd {
            return Some(format!(
                "Profit ${:.2} exceeds reasonable limit of ${:.0}",
                gross_profit_usd, self.max_profit_usd
            ));
        }
        
        // 3. Negative or zero expected return (broken math)
        if cycle.expected_return <= 0.0 {
            return Some("Expected return is non-positive".to_string());
        }
        
        // 4. Non-finite expected return
        if !cycle.expected_return.is_finite() {
            return Some("Expected return is not finite".to_string());
        }
        
        // 5. Cycle validation (already done in ArbitrageCycle::is_valid but double-check)
        if !cycle.is_valid() {
            return Some("Cycle failed validation (duplicate nodes or pools)".to_string());
        }
        
        // 6. Too many hops (graph may have found a loop-the-loop)
        if cycle.hop_count() > 6 {
            return Some(format!("Too many hops: {}", cycle.hop_count()));
        }
        
        None
    }

    pub fn analyze(&self, cycle: &ArbitrageCycle, input_usd: Option<f64>) -> ProfitAnalysis {
        let input = input_usd.unwrap_or(self.default_input_usd);
        let gross_profit_usd = input * (cycle.expected_return - 1.0);
        let gas_cost_usd = self.calculate_gas_cost(cycle);
        let net_profit_usd = gross_profit_usd - gas_cost_usd;
        let is_profitable = net_profit_usd >= self.min_profit_usd;
        
        let suspicion_reason = self.check_suspicious(cycle, gross_profit_usd);
        let is_suspicious = suspicion_reason.is_some();

        ProfitAnalysis {
            cycle: cycle.clone(),
            input_usd: input,
            gross_profit_usd,
            gas_cost_usd,
            net_profit_usd,
            is_profitable,
            is_suspicious,
            suspicion_reason,
        }
    }

    pub fn filter_profitable(
        &self,
        cycles: &[ArbitrageCycle],
        symbols: &HashMap<Address, &str>,
    ) -> Vec<ProfitAnalysis> {
        let mut profitable = Vec::new();
        let mut filtered_count = 0;
        let mut suspicious_count = 0;

        for cycle in cycles {
            let analysis = self.analyze(cycle, None);
            
            if analysis.is_suspicious {
                suspicious_count += 1;
                let path = analysis.format_path(symbols);
                let reason = analysis.suspicion_reason.as_deref().unwrap_or("Unknown");
                debug!(
                    "⚠️ SUSPICIOUS: {} | Reason: {}",
                    path, reason
                );
                continue;
            }

            if analysis.is_profitable {
                let path = analysis.format_path(symbols);
                
                let mut tags = Vec::new();
                if cycle.is_cross_dex() {
                    tags.push(style("[CROSS-DEX]").magenta().bold().to_string());
                }
                if cycle.has_low_fee_pools() {
                    tags.push(style("[LOW-FEE]").cyan().bold().to_string());
                }
                if cycle.unique_dex_count() >= 3 {
                    tags.push(style("[MULTI-DEX]").yellow().bold().to_string());
                }
                let tags_str = tags.join(" ");
                
                info!(
                    "{}",
                    style(format!(
                        "💰 PROFITABLE: {} {} | Gross: ${:.2} | Gas: ${:.2} | Net: ${:.2}",
                        path,
                        tags_str,
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
                    "Filtered: {} | Return: {:.4}x | Net: ${:.2}",
                    path, cycle.expected_return, analysis.net_profit_usd
                );
            }
        }

        if suspicious_count > 0 {
            warn!(
                "⚠️ Flagged {} cycles as SUSPICIOUS (check debug logs for details)",
                suspicious_count
            );
        }

        if filtered_count > 0 {
            info!(
                "Filtered out {} cycles below ${:.2} profit threshold",
                filtered_count, self.min_profit_usd
            );
        }

        profitable.sort_by(|a, b| {
            b.net_profit_usd
                .partial_cmp(&a.net_profit_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        profitable
    }

    pub fn print_summary(
        &self,
        cycles: &[ArbitrageCycle],
        symbols: &HashMap<Address, &str>,
    ) {
        if cycles.is_empty() {
            warn!("No cycles found to analyze");
            return;
        }

        // Separate valid from suspicious
        let mut valid_cycles = Vec::new();
        let mut suspicious_cycles = Vec::new();
        
        for c in cycles {
            let analysis = self.analyze(c, None);
            if analysis.is_suspicious {
                suspicious_cycles.push((c, analysis.suspicion_reason));
            } else {
                valid_cycles.push(c);
            }
        }

        let cross_dex: Vec<_> = valid_cycles.iter().filter(|c| c.is_cross_dex()).collect();
        let single_dex: Vec<_> = valid_cycles.iter().filter(|c| !c.is_cross_dex()).collect();
        let low_fee: Vec<_> = valid_cycles.iter().filter(|c| c.has_low_fee_pools()).collect();
        let multi_dex: Vec<_> = valid_cycles.iter().filter(|c| c.unique_dex_count() >= 3).collect();

        println!();
        println!("{}", style("═══ CYCLE ANALYSIS ═══").yellow().bold());
        println!();
        println!(
            "Analysis parameters: Input=${:.0}, Gas={:.1} gwei, ETH=${:.0}",
            self.default_input_usd, self.gas_price_gwei, self.eth_price_usd
        );
        println!(
            "Minimum profit threshold: ${:.2}",
            self.min_profit_usd
        );
        println!();
        
        if !suspicious_cycles.is_empty() {
            println!(
                "{} {} cycles with invalid returns (bugs filtered out)",
                style("⚠️ Excluded").red(),
                suspicious_cycles.len()
            );
            
            // Show top 3 suspicious ones for debugging
            for (cycle, reason) in suspicious_cycles.iter().take(3) {
                let path: Vec<_> = cycle.path.iter()
                    .map(|addr| {
                        symbols
                            .get(addr)
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("0x{}...", &format!("{:?}", addr)[2..8]))
                    })
                    .collect();
                debug!(
                    "  - {} | Return: {:.2}x | Reason: {}",
                    path.join(" → "),
                    cycle.expected_return,
                    reason.as_deref().unwrap_or("Unknown")
                );
            }
            println!();
        }
        
        println!("Found {} valid cycles:", valid_cycles.len());
        println!("  • {} cross-DEX (using 2+ DEXes)", cross_dex.len());
        println!("  • {} multi-DEX (using 3+ DEXes)", multi_dex.len());
        println!("  • {} single-DEX", single_dex.len());
        println!("  • {} using low-fee pools (≤5bps)", low_fee.len());

        if !cross_dex.is_empty() {
            println!();
            println!("{}", style("=== CROSS-DEX CYCLES ===").magenta().bold());
            println!();

            for (i, cycle) in cross_dex.iter().take(10).enumerate() {
                let analysis = self.analyze(cycle, None);
                let path = analysis.format_path(symbols);
                
                let status = if analysis.is_profitable {
                    style("✓ PROFITABLE").green()
                } else if analysis.net_profit_usd > -50.0 {
                    style("○ marginal").yellow()
                } else {
                    style("✗ unprofitable").red()
                };

                let fee_indicator = if cycle.has_low_fee_pools() {
                    style(" [LOW-FEE]").cyan().to_string()
                } else {
                    String::new()
                };

                println!(
                    "  {}. {} | {:.4}x return ({:+.3}%){}",
                    i + 1, status, cycle.expected_return, cycle.profit_percentage(), fee_indicator
                );
                println!("     Path: {}", style(path).cyan());
                println!("     DEXes: {} ({} unique)", style(cycle.dex_path()).magenta(), cycle.unique_dex_count());
                println!(
                    "     Gross: ${:+.2} | Gas: ${:.2} | Net: ${:+.2}",
                    analysis.gross_profit_usd, analysis.gas_cost_usd, analysis.net_profit_usd
                );
                println!();
            }
        }

        if !single_dex.is_empty() {
            println!();
            println!("{}", style("=== SINGLE-DEX CYCLES ===").blue().bold());
            println!();

            let to_show = single_dex.len().min(5);
            for (i, cycle) in single_dex.iter().take(to_show).enumerate() {
                let analysis = self.analyze(cycle, None);
                let path = analysis.format_path(symbols);
                
                let status = if analysis.is_profitable {
                    style("✓ PROFITABLE").green()
                } else if analysis.net_profit_usd > -50.0 {
                    style("○ marginal").yellow()
                } else {
                    style("✗ unprofitable").red()
                };

                println!(
                    "  {}. {} | {:.4}x return ({:+.3}%)",
                    i + 1, status, cycle.expected_return, cycle.profit_percentage()
                );
                println!("     Path: {} [{}]", style(path).cyan(), cycle.dexes[0]);
                println!(
                    "     Gross: ${:+.2} | Gas: ${:.2} | Net: ${:+.2}",
                    analysis.gross_profit_usd, analysis.gas_cost_usd, analysis.net_profit_usd
                );
                println!();
            }

            if single_dex.len() > to_show {
                println!("  ... and {} more single-DEX cycles", single_dex.len() - to_show);
            }
        }
    }
}

impl Default for ProfitFilter {
    fn default() -> Self {
        Self::new(5.0)
    }
}