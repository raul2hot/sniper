//! Graph Construction - QUIET Edition

use alloy_primitives::Address;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use std::collections::HashMap;
use tracing::{debug, trace, warn};  // Changed from info

use super::{Dex, PoolState, PoolType};

#[derive(Debug, Clone)]
pub struct EdgeData {
    pub pool_address: Address,
    pub weight: f64,
    pub price: f64,
    pub fee: u32,
    pub is_v4: bool,
    pub dex: Dex,
    pub pool_type: PoolType,
}

pub struct ArbitrageGraph {
    pub graph: DiGraph<Address, EdgeData>,
    pub token_to_node: HashMap<Address, NodeIndex>,
    pub node_to_token: HashMap<NodeIndex, Address>,
}

impl ArbitrageGraph {
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            token_to_node: HashMap::new(),
            node_to_token: HashMap::new(),
        }
    }

    pub fn from_pools(pools: &[PoolState]) -> Self {
        let mut graph = Self::new();
        let mut skipped_invalid = 0;

        for pool in pools {
            if !graph.add_pool(pool) {
                skipped_invalid += 1;
            }
        }

        let mut dex_counts: HashMap<Dex, usize> = HashMap::new();
        for edge in graph.graph.edge_references() {
            *dex_counts.entry(edge.weight().dex).or_insert(0) += 1;
        }

        // Changed from info! to debug!
        debug!(
            "Graph: {} nodes, {} edges",
            graph.graph.node_count(),
            graph.graph.edge_count()
        );
        
        if skipped_invalid > 0 {
            trace!("Skipped {} pools with invalid prices", skipped_invalid);
        }
        
        // DEX breakdown at trace level
        for (dex, count) in &dex_counts {
            trace!("  {}: {}", dex, count);
        }

        graph
    }

    pub fn add_pool(&mut self, pool: &PoolState) -> bool {
        if pool.liquidity == 0 {
            return false;
        }

        if matches!(pool.pool_type, PoolType::V2 | PoolType::Balancer) 
            && pool.reserve1 == 0 
        {
            return false;
        }

        let node0 = self.get_or_create_node(pool.token0);
        let node1 = self.get_or_create_node(pool.token1);

        let raw_price = pool.raw_price();
        
        if raw_price <= 0.0 || !raw_price.is_finite() {
            return false;
        }
        
        const MAX_REASONABLE_PRICE: f64 = 1e9;
        const MIN_REASONABLE_PRICE: f64 = 1e-12;
        
        if raw_price > MAX_REASONABLE_PRICE {
            trace!("Price too high ({:.2e}) for pool {:?}", raw_price, pool.address);
            return false;
        }
        
        if raw_price < MIN_REASONABLE_PRICE {
            trace!("Price too low ({:.2e}) for pool {:?}", raw_price, pool.address);
            return false;
        }
        
        let fee_rate = pool.fee as f64 / 1_000_000.0;
        let effective_price_0_to_1 = raw_price * (1.0 - fee_rate);
        let effective_price_1_to_0 = (1.0 / raw_price) * (1.0 - fee_rate);

        if effective_price_0_to_1 > 0.0 && effective_price_0_to_1.ln().is_finite() {
            self.graph.add_edge(
                node0,
                node1,
                EdgeData {
                    pool_address: pool.address,
                    weight: -effective_price_0_to_1.ln(),
                    price: raw_price,
                    fee: pool.fee,
                    is_v4: pool.is_v4,
                    dex: pool.dex,
                    pool_type: pool.pool_type,
                },
            );
        }

        if effective_price_1_to_0 > 0.0 && effective_price_1_to_0.ln().is_finite() {
            self.graph.add_edge(
                node1,
                node0,
                EdgeData {
                    pool_address: pool.address,
                    weight: -effective_price_1_to_0.ln(),
                    price: 1.0 / raw_price,
                    fee: pool.fee,
                    is_v4: pool.is_v4,
                    dex: pool.dex,
                    pool_type: pool.pool_type,
                },
            );
        }
        
        true
    }

    fn get_or_create_node(&mut self, token: Address) -> NodeIndex {
        if let Some(&node) = self.token_to_node.get(&token) {
            return node;
        }
        let node = self.graph.add_node(token);
        self.token_to_node.insert(token, node);
        self.node_to_token.insert(node, token);
        node
    }

    pub fn get_node(&self, token: Address) -> Option<NodeIndex> {
        self.token_to_node.get(&token).copied()
    }

    pub fn get_token(&self, node: NodeIndex) -> Option<Address> {
        self.node_to_token.get(&node).copied()
    }

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

impl Default for ArbitrageGraph {
    fn default() -> Self {
        Self::new()
    }
}
