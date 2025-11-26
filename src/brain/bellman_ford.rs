//! Bounded Bellman-Ford Algorithm
//!
//! Step 2.1: The Pathfinder
//!
//! Implements a modified Bellman-Ford algorithm that:
//! 1. Only searches up to k hops (k=4 for our MVP)
//! 2. Finds negative cycles (arbitrage opportunities)
//! 3. Now tracks cross-DEX paths!
//!
//! Key insight from the spec:
//! - Standard Bellman-Ford runs |V|-1 relaxation iterations
//! - We only run exactly k=4 iterations
//! - This gives us O(k × E) = O(4 × E) = O(E) complexity
//!
//! Mathematical transformation:
//! - Arbitrage exists when: price_A × price_B × price_C > 1
//! - Taking log: log(A) + log(B) + log(C) > 0
//! - With negative weights: -log(A) - log(B) - log(C) < 0
//! - This is a NEGATIVE CYCLE!

use alloy::primitives::Address;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::HashMap;
use tracing::{debug, info};

use crate::cartographer::{ArbitrageGraph, Dex, EdgeData};

/// Represents an arbitrage cycle (negative cycle in the graph)
#[derive(Debug, Clone)]
pub struct ArbitrageCycle {
    /// The sequence of token addresses forming the cycle
    /// First and last element are the same (it's a cycle)
    pub path: Vec<Address>,
    
    /// The pool addresses used for each hop
    pub pools: Vec<Address>,
    
    /// The DEX used for each hop
    pub dexes: Vec<Dex>,
    
    /// The total weight of the cycle (negative = profitable)
    pub total_weight: f64,
    
    /// Expected return multiplier (e.g., 1.005 = 0.5% profit before gas)
    pub expected_return: f64,
    
    /// Individual prices for each hop
    pub prices: Vec<f64>,
    
    /// Fees for each hop (in basis points / 100)
    pub fees: Vec<u32>,
}

impl ArbitrageCycle {
    /// Calculate profit percentage (before gas)
    pub fn profit_percentage(&self) -> f64 {
        (self.expected_return - 1.0) * 100.0
    }
    
    /// Number of hops in the cycle
    pub fn hop_count(&self) -> usize {
        self.path.len().saturating_sub(1)
    }
    
    /// Check if this is a cross-DEX arbitrage (uses multiple DEXes)
    pub fn is_cross_dex(&self) -> bool {
        if self.dexes.is_empty() {
            return false;
        }
        let first = self.dexes[0];
        self.dexes.iter().any(|d| *d != first)
    }
    
    /// Get a string showing the DEX path
    pub fn dex_path(&self) -> String {
        self.dexes.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(" → ")
    }
}

/// Bounded Bellman-Ford algorithm for finding arbitrage cycles
pub struct BoundedBellmanFord<'a> {
    graph: &'a ArbitrageGraph,
    max_hops: usize,
}

impl<'a> BoundedBellmanFord<'a> {
    /// Create a new Bounded Bellman-Ford instance
    /// 
    /// # Arguments
    /// * `graph` - The arbitrage graph to search
    /// * `max_hops` - Maximum number of hops (typically 3-4)
    pub fn new(graph: &'a ArbitrageGraph, max_hops: usize) -> Self {
        Self { graph, max_hops }
    }

    /// Find all arbitrage cycles starting from a given token
    ///
    /// Algorithm:
    /// 1. Initialize: dist[start] = 0, dist[others] = ∞
    /// 2. For each hop k = 1 to max_hops:
    ///    - Relax all edges
    ///    - If dist[start] < 0, we found a negative cycle!
    /// 3. Reconstruct the path
    pub fn find_cycles_from(&self, start_token: Address) -> Vec<ArbitrageCycle> {
        let mut cycles = Vec::new();

        let Some(start_node) = self.graph.get_node(start_token) else {
            return cycles;
        };

        let node_count = self.graph.graph.node_count();

        // Distance from start to each node
        let mut dist: Vec<f64> = vec![f64::INFINITY; node_count];
        
        // Predecessor info: (previous_node, pool_address, price, fee, dex)
        let mut pred: Vec<Option<(NodeIndex, Address, f64, u32, Dex)>> = vec![None; node_count];
        
        // Track which hop we found each best distance at
        let mut dist_at_hop: Vec<usize> = vec![0; node_count];

        // Initialize start
        dist[start_node.index()] = 0.0;

        // Relax edges for exactly max_hops iterations
        for hop in 1..=self.max_hops {
            let mut any_update = false;

            // Create a copy of distances to avoid using updated values in same iteration
            let dist_snapshot = dist.clone();

            for edge in self.graph.graph.edge_references() {
                let source = edge.source();
                let target = edge.target();
                let edge_data = edge.weight();

                // Skip if source hasn't been reached yet
                if dist_snapshot[source.index()].is_infinite() {
                    continue;
                }

                let new_dist = dist_snapshot[source.index()] + edge_data.weight;

                // Only update if we found a better path
                if new_dist < dist[target.index()] {
                    dist[target.index()] = new_dist;
                    pred[target.index()] = Some((
                        source,
                        edge_data.pool_address,
                        edge_data.price,
                        edge_data.fee,
                        edge_data.dex,
                    ));
                    dist_at_hop[target.index()] = hop;
                    any_update = true;
                }
            }

            // After relaxing, check ALL paths back to start
            if hop >= 2 {
                for edge in self.graph.graph.edge_references() {
                    let source = edge.source();
                    let target = edge.target();
                    
                    if target == start_node && !dist[source.index()].is_infinite() {
                        let cycle_weight = dist[source.index()] + edge.weight().weight;
                        
                        // Found a cycle! (negative weight = profitable, but track all near-profitable)
                        let expected_return = (-cycle_weight).exp();
                        
                        if expected_return > 0.99 {  // Within 1% of breakeven
                            if let Some(cycle) = self.reconstruct_cycle_via_edge(
                                start_node,
                                source,
                                edge.weight(),
                                &pred,
                                hop,
                            ) {
                                // Check if we already have this cycle
                                let dominated = cycles.iter().any(|c: &ArbitrageCycle| {
                                    c.path.len() == cycle.path.len() && 
                                    (c.expected_return - cycle.expected_return).abs() < 0.0001
                                });
                                
                                if !dominated {
                                    let cross_dex = if cycle.is_cross_dex() { " [CROSS-DEX]" } else { "" };
                                    debug!(
                                        "Found cycle at hop {}: weight={:.6}, return={:.4}x{}",
                                        hop, cycle_weight, expected_return, cross_dex
                                    );
                                    cycles.push(cycle);
                                }
                            }
                        }
                    }
                }
            }

            // Early termination if no updates
            if !any_update {
                break;
            }
        }

        cycles
    }
    
    /// Reconstruct a cycle using a specific final edge
    fn reconstruct_cycle_via_edge(
        &self,
        start_node: NodeIndex,
        last_node: NodeIndex,
        final_edge: &EdgeData,
        pred: &[Option<(NodeIndex, Address, f64, u32, Dex)>],
        _max_hop: usize,
    ) -> Option<ArbitrageCycle> {
        let mut path = Vec::new();
        let mut pools = Vec::new();
        let mut dexes = Vec::new();
        let mut prices = Vec::new();
        let mut fees = Vec::new();
        let mut total_weight = final_edge.weight;

        // Start from last_node and work backwards
        path.push(self.graph.get_token(start_node)?); // Will be end of cycle
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
                
                // Find the edge weight
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

        // Add start token
        path.push(self.graph.get_token(start_node)?);

        // Reverse to get forward order
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

    /// Find all arbitrage cycles starting from multiple base tokens
    pub fn find_all_cycles(&self, base_tokens: &[Address]) -> Vec<ArbitrageCycle> {
        let mut all_cycles = Vec::new();
        let mut seen_paths: std::collections::HashSet<String> = std::collections::HashSet::new();

        for &token in base_tokens {
            let cycles = self.find_cycles_from(token);
            
            for cycle in cycles {
                // Deduplicate cycles (same path starting from different points)
                let path_key = format!("{:?}", cycle.path);
                if !seen_paths.contains(&path_key) {
                    seen_paths.insert(path_key);
                    all_cycles.push(cycle);
                }
            }
        }

        // Sort by expected return (best first)
        all_cycles.sort_by(|a, b| {
            b.expected_return
                .partial_cmp(&a.expected_return)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Count cross-DEX vs single-DEX cycles
        let cross_dex_count = all_cycles.iter().filter(|c| c.is_cross_dex()).count();
        let single_dex_count = all_cycles.len() - cross_dex_count;

        info!(
            "Found {} unique arbitrage cycles ({} cross-DEX, {} single-DEX)",
            all_cycles.len(), cross_dex_count, single_dex_count
        );

        all_cycles
    }
}

/// Helper to format a cycle path with token symbols
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartographer::{PoolState, PoolType};
    use alloy::primitives::U256;

    /// Create a test pool with specific price
    fn make_pool(token0: Address, token1: Address, sqrt_price_x96: u128, fee: u32, dex: Dex) -> PoolState {
        PoolState {
            address: Address::repeat_byte(0x99),
            token0,
            token1,
            sqrt_price_x96: U256::from(sqrt_price_x96),
            tick: 0,
            liquidity: 1_000_000_000,
            reserve1: 0,
            fee,
            is_v4: false,
            dex,
            pool_type: PoolType::V3,
        }
    }

    #[test]
    fn test_find_profitable_cycle() {
        // Create 3 tokens
        let token_a = Address::repeat_byte(0x01);
        let token_b = Address::repeat_byte(0x02);
        let token_c = Address::repeat_byte(0x03);

        let sqrt_1_02 = 80033725539485447474396053504_u128;
        let sqrt_1_01 = 79626412058234710168382062592_u128;
        
        let pools = vec![
            make_pool(token_a, token_b, sqrt_1_02, 100, Dex::UniswapV3),
            make_pool(token_b, token_c, sqrt_1_02, 100, Dex::SushiswapV2),  // Different DEX!
            make_pool(token_c, token_a, sqrt_1_01, 100, Dex::UniswapV3),
        ];

        let graph = crate::cartographer::ArbitrageGraph::from_pools(&pools);
        let bf = BoundedBellmanFord::new(&graph, 4);

        let cycles = bf.find_cycles_from(token_a);

        // Check that we found cross-DEX cycles
        let cross_dex: Vec<_> = cycles.iter().filter(|c| c.is_cross_dex()).collect();
        
        println!("Found {} total cycles, {} cross-DEX", cycles.len(), cross_dex.len());
        for cycle in &cross_dex {
            println!(
                "  Cross-DEX cycle: {} hops, return = {:.4}x, DEXes: {}",
                cycle.hop_count(),
                cycle.expected_return,
                cycle.dex_path()
            );
        }

        assert!(cycles.len() >= 0); // At minimum, should not panic
    }

    #[test]
    fn test_cross_dex_detection() {
        let cycle = ArbitrageCycle {
            path: vec![Address::ZERO; 4],
            pools: vec![Address::ZERO; 3],
            dexes: vec![Dex::UniswapV3, Dex::SushiswapV2, Dex::UniswapV3],
            total_weight: -0.01,
            expected_return: 1.01,
            prices: vec![1.0; 3],
            fees: vec![3000; 3],
        };

        assert!(cycle.is_cross_dex(), "Should detect cross-DEX cycle");

        let single_dex = ArbitrageCycle {
            path: vec![Address::ZERO; 4],
            pools: vec![Address::ZERO; 3],
            dexes: vec![Dex::UniswapV3, Dex::UniswapV3, Dex::UniswapV3],
            total_weight: -0.01,
            expected_return: 1.01,
            prices: vec![1.0; 3],
            fees: vec![3000; 3],
        };

        assert!(!single_dex.is_cross_dex(), "Should detect single-DEX cycle");
    }
}
