//! Bounded Bellman-Ford Algorithm - Enhanced Edition
//!
//! Step 2.1: The Pathfinder

use alloy_primitives::Address;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info};

use crate::cartographer::{ArbitrageGraph, Dex, EdgeData};

/// Represents an arbitrage cycle (negative cycle in the graph)
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
        self.path.len().saturating_sub(1)
    }
    
    pub fn is_cross_dex(&self) -> bool {
        if self.dexes.is_empty() {
            return false;
        }
        let first = self.dexes[0];
        self.dexes.iter().any(|d| *d != first)
    }
    
    pub fn dex_path(&self) -> String {
        self.dexes.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(" → ")
    }
    
    pub fn avg_fee_bps(&self) -> f64 {
        if self.fees.is_empty() {
            return 0.0;
        }
        self.fees.iter().map(|&f| f as f64).sum::<f64>() / self.fees.len() as f64 / 100.0
    }
    
    pub fn has_low_fee_pools(&self) -> bool {
        self.fees.iter().any(|&f| f <= 500)
    }
    
    pub fn unique_dex_count(&self) -> usize {
        let unique: HashSet<_> = self.dexes.iter().collect();
        unique.len()
    }
}

/// Bounded Bellman-Ford algorithm for finding arbitrage cycles
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

        let node_count = self.graph.graph.node_count();

        let mut dist: Vec<f64> = vec![f64::INFINITY; node_count];
        let mut pred: Vec<Option<(NodeIndex, Address, f64, u32, Dex)>> = vec![None; node_count];

        dist[start_node.index()] = 0.0;

        for hop in 1..=self.max_hops {
            let dist_snapshot = dist.clone();

            for edge in self.graph.graph.edge_references() {
                let source = edge.source();
                let target = edge.target();
                let edge_data = edge.weight();

                if dist_snapshot[source.index()].is_infinite() {
                    continue;
                }

                let new_dist = dist_snapshot[source.index()] + edge_data.weight;

                if new_dist < dist[target.index()] {
                    dist[target.index()] = new_dist;
                    pred[target.index()] = Some((
                        source,
                        edge_data.pool_address,
                        edge_data.price,
                        edge_data.fee,
                        edge_data.dex,
                    ));
                }
            }

            if hop >= 2 {
                for edge in self.graph.graph.edge_references() {
                    let source = edge.source();
                    let target = edge.target();
                    
                    if target == start_node && !dist[source.index()].is_infinite() {
                        let cycle_weight = dist[source.index()] + edge.weight().weight;
                        let expected_return = (-cycle_weight).exp();
                        
                        if expected_return > 0.98 {
                            if let Some(cycle) = self.reconstruct_cycle_via_edge(
                                start_node,
                                source,
                                edge.weight(),
                                &pred,
                            ) {
                                let dominated = cycles.iter().any(|c: &ArbitrageCycle| {
                                    c.path.len() == cycle.path.len() && 
                                    (c.expected_return - cycle.expected_return).abs() < 0.0001
                                });
                                
                                if !dominated {
                                    let cross_dex = if cycle.is_cross_dex() { " [CROSS-DEX]" } else { "" };
                                    let low_fee = if cycle.has_low_fee_pools() { " [LOW-FEE]" } else { "" };
                                    debug!(
                                        "Found cycle at hop {}: return={:.4}x{}{}",
                                        hop, expected_return, cross_dex, low_fee
                                    );
                                    cycles.push(cycle);
                                }
                            }
                        }
                    }
                }
            }
        }

        cycles
    }
    
    fn reconstruct_cycle_via_edge(
        &self,
        start_node: NodeIndex,
        last_node: NodeIndex,
        final_edge: &EdgeData,
        pred: &[Option<(NodeIndex, Address, f64, u32, Dex)>],
    ) -> Option<ArbitrageCycle> {
        let mut path = Vec::new();
        let mut pools = Vec::new();
        let mut dexes = Vec::new();
        let mut prices = Vec::new();
        let mut fees = Vec::new();
        let mut total_weight = final_edge.weight;

        path.push(self.graph.get_token(start_node)?);
        pools.push(final_edge.pool_address);
        dexes.push(final_edge.dex);
        prices.push(final_edge.price);
        fees.push(final_edge.fee);
        
        let mut current = last_node;
        let mut steps = 0;
        let max_steps = self.max_hops + 1;

        while current != start_node && steps < max_steps {
            let token = self.graph.get_token(current)?;
            path.push(token);

            if let Some((prev_node, pool, price, fee, dex)) = pred[current.index()] {
                pools.push(pool);
                dexes.push(dex);
                prices.push(price);
                fees.push(fee);
                
                for edge in self.graph.graph.edge_references() {
                    if edge.source() == prev_node && edge.target() == current {
                        total_weight += edge.weight().weight;
                        break;
                    }
                }
                
                current = prev_node;
            } else {
                break;
            }
            steps += 1;
        }

        path.push(self.graph.get_token(start_node)?);

        path.reverse();
        pools.reverse();
        dexes.reverse();
        prices.reverse();
        fees.reverse();

        if path.len() < 3 {
            return None;
        }

        let expected_return = (-total_weight).exp();

        Some(ArbitrageCycle {
            path,
            pools,
            dexes,
            total_weight,
            expected_return,
            prices,
            fees,
        })
    }

    pub fn find_all_cycles(&self, base_tokens: &[Address]) -> Vec<ArbitrageCycle> {
        let mut all_cycles = Vec::new();
        let mut seen_paths: HashSet<String> = HashSet::new();

        for &token in base_tokens {
            let cycles = self.find_cycles_from(token);
            
            for cycle in cycles {
                let path_key = format!("{:?}", cycle.path);
                if !seen_paths.contains(&path_key) {
                    seen_paths.insert(path_key);
                    all_cycles.push(cycle);
                }
            }
        }

        all_cycles.sort_by(|a, b| {
            b.expected_return
                .partial_cmp(&a.expected_return)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let cross_dex_count = all_cycles.iter().filter(|c| c.is_cross_dex()).count();
        let single_dex_count = all_cycles.len() - cross_dex_count;
        let low_fee_count = all_cycles.iter().filter(|c| c.has_low_fee_pools()).count();

        info!(
            "Found {} unique arbitrage cycles:",
            all_cycles.len()
        );
        info!("  • {} cross-DEX cycles", cross_dex_count);
        info!("  • {} single-DEX cycles", single_dex_count);
        info!("  • {} using low-fee pools (≤5bps)", low_fee_count);

        all_cycles
    }
}

pub fn format_cycle_path(cycle: &ArbitrageCycle, symbols: &HashMap<Address, &str>) -> String {
    cycle
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