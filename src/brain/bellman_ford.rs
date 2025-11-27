//! Bounded Bellman-Ford Algorithm - QUIET Edition

use alloy_primitives::Address;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::{HashMap, HashSet};
use tracing::{debug, trace, warn};  // Changed from info

use crate::cartographer::{ArbitrageGraph, Dex, EdgeData};

#[derive(Debug, Clone)]
pub struct ArbitrageCycle {
    pub path: Vec<Address>,
    pub pools: Vec<Address>,
    pub dexes: Vec<Dex>,
    pub total_weight: f64,
    pub expected_return: f64,
    pub prices: Vec<f64>,
    pub fees: Vec<u32>,
}

impl ArbitrageCycle {
    pub fn profit_percentage(&self) -> f64 {
        (self.expected_return - 1.0) * 100.0
    }
    
    pub fn hop_count(&self) -> usize {
        self.pools.len()
    }
    
    pub fn is_cross_dex(&self) -> bool {
        if self.dexes.is_empty() { return false; }
        let first = self.dexes[0];
        self.dexes.iter().any(|d| *d != first)
    }
    
    pub fn dex_path(&self) -> String {
        self.dexes.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(" → ")
    }
    
    pub fn avg_fee_bps(&self) -> f64 {
        if self.fees.is_empty() { return 0.0; }
        self.fees.iter().map(|&f| f as f64).sum::<f64>() / self.fees.len() as f64 / 100.0
    }
    
    pub fn has_low_fee_pools(&self) -> bool {
        self.fees.iter().any(|&f| f <= 500)
    }
    
    pub fn unique_dex_count(&self) -> usize {
        let unique: HashSet<_> = self.dexes.iter().collect();
        unique.len()
    }
    
    pub fn is_valid(&self) -> bool {
        if self.path.len() < 3 { return false; }
        if self.path.first() != self.path.last() { return false; }
        if self.path.len() != self.pools.len() + 1 { return false; }
        
        let intermediate: Vec<_> = self.path[1..self.path.len()-1].to_vec();
        let unique_intermediate: HashSet<_> = intermediate.iter().collect();
        if unique_intermediate.len() != intermediate.len() { return false; }
        
        let start = self.path[0];
        if intermediate.contains(&start) { return false; }
        
        let unique_pools: HashSet<_> = self.pools.iter().collect();
        if unique_pools.len() != self.pools.len() { return false; }
        
        if self.expected_return <= 0.0 || !self.expected_return.is_finite() { return false; }
        if self.expected_return > 100.0 { return false; }
        
        true
    }
}

pub struct BoundedBellmanFord<'a> {
    graph: &'a ArbitrageGraph,
    max_hops: usize,
}

impl<'a> BoundedBellmanFord<'a> {
    pub fn new(graph: &'a ArbitrageGraph, max_hops: usize) -> Self {
        Self { graph, max_hops }
    }

    pub fn find_cycles_from(&self, start_token: Address) -> Vec<ArbitrageCycle> {
        let mut cycles = Vec::new();
        let Some(start_node) = self.graph.get_node(start_token) else {
            return cycles;
        };

        self.dfs_find_cycles(
            start_node, start_node,
            Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(),
            HashSet::new(), 0.0, &mut cycles, 1,
        );

        cycles
    }
    
    #[allow(clippy::too_many_arguments)]
    fn dfs_find_cycles(
        &self,
        start_node: NodeIndex,
        current_node: NodeIndex,
        mut path: Vec<Address>,
        mut pools: Vec<Address>,
        mut dexes: Vec<Dex>,
        mut prices: Vec<f64>,
        mut fees: Vec<u32>,
        mut visited: HashSet<NodeIndex>,
        total_weight: f64,
        cycles: &mut Vec<ArbitrageCycle>,
        depth: usize,
    ) {
        let current_token = match self.graph.get_token(current_node) {
            Some(t) => t,
            None => return,
        };
        
        path.push(current_token);
        if depth > 1 { visited.insert(current_node); }
        if depth > self.max_hops { return; }
        
        for edge in self.graph.graph.edges(current_node) {
            let target = edge.target();
            let edge_data = edge.weight();
            let new_weight = total_weight + edge_data.weight;
            
            if target == start_node && depth >= 2 {
                let expected_return = (-new_weight).exp();
                
                if expected_return > 0.95 {
                    let mut final_path = path.clone();
                    final_path.push(self.graph.get_token(start_node).unwrap());
                    
                    let mut final_pools = pools.clone();
                    final_pools.push(edge_data.pool_address);
                    
                    let mut final_dexes = dexes.clone();
                    final_dexes.push(edge_data.dex);
                    
                    let mut final_prices = prices.clone();
                    final_prices.push(edge_data.price);
                    
                    let mut final_fees = fees.clone();
                    final_fees.push(edge_data.fee);
                    
                    let cycle = ArbitrageCycle {
                        path: final_path,
                        pools: final_pools,
                        dexes: final_dexes,
                        total_weight: new_weight,
                        expected_return,
                        prices: final_prices,
                        fees: final_fees,
                    };
                    
                    if cycle.is_valid() {
                        cycles.push(cycle);
                    }
                }
            } else if !visited.contains(&target) && depth < self.max_hops {
                let mut new_pools = pools.clone();
                new_pools.push(edge_data.pool_address);
                
                let mut new_dexes = dexes.clone();
                new_dexes.push(edge_data.dex);
                
                let mut new_prices = prices.clone();
                new_prices.push(edge_data.price);
                
                let mut new_fees = fees.clone();
                new_fees.push(edge_data.fee);
                
                self.dfs_find_cycles(
                    start_node, target,
                    path.clone(), new_pools, new_dexes, new_prices, new_fees,
                    visited.clone(), new_weight, cycles, depth + 1,
                );
            }
        }
    }

    pub fn find_all_cycles(&self, base_tokens: &[Address]) -> Vec<ArbitrageCycle> {
        let mut all_cycles = Vec::new();
        let mut seen_signatures: HashSet<String> = HashSet::new();

        for &token in base_tokens {
            let cycles = self.find_cycles_from(token);
            
            for cycle in cycles {
                let signature = create_cycle_signature(&cycle);
                if !seen_signatures.contains(&signature) {
                    seen_signatures.insert(signature);
                    all_cycles.push(cycle);
                }
            }
        }

        all_cycles.sort_by(|a, b| {
            b.expected_return.partial_cmp(&a.expected_return)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let cross_dex_count = all_cycles.iter().filter(|c| c.is_cross_dex()).count();
        let single_dex_count = all_cycles.len() - cross_dex_count;
        let low_fee_count = all_cycles.iter().filter(|c| c.has_low_fee_pools()).count();

        // Changed from info! to debug!
        debug!(
            "Found {} cycles ({} cross-DEX, {} single-DEX, {} low-fee)",
            all_cycles.len(), cross_dex_count, single_dex_count, low_fee_count
        );

        all_cycles
    }
}

fn create_cycle_signature(cycle: &ArbitrageCycle) -> String {
    let mut pool_strs: Vec<String> = cycle.pools.iter()
        .map(|p| format!("{:?}", p))
        .collect();
    pool_strs.sort();
    pool_strs.join("-")
}

pub fn format_cycle_path(cycle: &ArbitrageCycle, symbols: &HashMap<Address, &str>) -> String {
    cycle.path.iter()
        .map(|addr| {
            symbols.get(addr)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("0x{}...", &format!("{:?}", addr)[2..8]))
        })
        .collect::<Vec<_>>()
        .join(" → ")
}
