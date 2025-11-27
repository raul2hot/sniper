//! Profit Filter - QUIET Edition

use alloy_primitives::Address;
use std::collections::HashMap;
use tracing::{debug, trace};  // No info!

use super::ArbitrageCycle;
use crate::cartographer::Dex;

#[derive(Debug, Clone)]
pub struct ProfitAnalysis {
    pub cycle: ArbitrageCycle,
    pub input_usd: f64,
    pub gross_profit_usd: f64,
    pub gas_cost_usd: f64,
    pub net_profit_usd: f64,
    pub is_candidate: bool,
    pub is_suspicious: bool,
    pub suspicion_reason: Option<String>,
}

impl ProfitAnalysis {
    pub fn format_path(&self, symbols: &HashMap<Address, &str>) -> String {
        self.cycle.path.iter()
            .map(|addr| {
                symbols.get(addr)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("0x{}...", &format!("{:?}", addr)[2..8]))
            })
            .collect::<Vec<_>>()
            .join(" → ")
    }
}

pub struct ProfitFilter {
    min_profit_usd: f64,
    gas_per_swap_v3: u64,
    gas_per_swap_v2: u64,
    gas_per_swap_balancer: u64,
    gas_per_swap_curve: u64,
    gas_price_gwei: f64,
    eth_price_usd: f64,
    default_input_usd: f64,
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
            gas_price_gwei: 0.5,
            eth_price_usd: 3000.0,
            default_input_usd: 10_000.0,
            max_reasonable_return: 1.10,
            max_profit_usd: 10_000.0,
        }
    }

    pub fn set_eth_price(&mut self, eth_price_usd: f64) {
        self.eth_price_usd = eth_price_usd;
    }

    pub fn set_gas_price(&mut self, gas_price_gwei: f64) {
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
        total_gas_units += 50_000;
        let gas_cost_eth = (total_gas_units as f64) * self.gas_price_gwei * 1e-9;
        gas_cost_eth * self.eth_price_usd
    }
    
    fn check_suspicious(&self, cycle: &ArbitrageCycle, gross_profit_usd: f64) -> Option<String> {
        if cycle.expected_return > self.max_reasonable_return {
            return Some(format!("Return {:.2}x too high", cycle.expected_return));
        }
        if gross_profit_usd > self.max_profit_usd {
            return Some(format!("Profit ${:.2} too high", gross_profit_usd));
        }
        if cycle.expected_return <= 0.0 || !cycle.expected_return.is_finite() {
            return Some("Invalid return".to_string());
        }
        if !cycle.is_valid() {
            return Some("Invalid cycle".to_string());
        }
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
        let is_candidate = net_profit_usd >= self.min_profit_usd;
        let suspicion_reason = self.check_suspicious(cycle, gross_profit_usd);
        let is_suspicious = suspicion_reason.is_some();

        ProfitAnalysis {
            cycle: cycle.clone(),
            input_usd: input,
            gross_profit_usd,
            gas_cost_usd,
            net_profit_usd,
            is_candidate,
            is_suspicious,
            suspicion_reason,
        }
    }

    pub fn filter_candidates(
        &self,
        cycles: &[ArbitrageCycle],
        _symbols: &HashMap<Address, &str>,
    ) -> Vec<ProfitAnalysis> {
        let mut candidates = Vec::new();

        for cycle in cycles {
            let analysis = self.analyze(cycle, None);
            if analysis.is_suspicious { continue; }
            if analysis.is_candidate {
                candidates.push(analysis);
            }
        }

        candidates.sort_by(|a, b| {
            b.net_profit_usd.partial_cmp(&a.net_profit_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        candidates
    }
    
    #[deprecated(note = "Use filter_candidates instead")]
    pub fn filter_profitable(
        &self,
        cycles: &[ArbitrageCycle],
        symbols: &HashMap<Address, &str>,
    ) -> Vec<ProfitAnalysis> {
        self.filter_candidates(cycles, symbols)
    }
}

impl Default for ProfitFilter {
    fn default() -> Self {
        Self::new(5.0)
    }
}
