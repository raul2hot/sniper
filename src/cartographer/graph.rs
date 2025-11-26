//! Graph Construction
//!
//! Step 1.2: The Map Maker
//!
//! Converts raw pool data into a directed graph structure suitable
//! for negative cycle detection (arbitrage finding).
//!
//! Key insight: We use -log(price) as edge weights.
//! This transforms: A × B × C > 1 (profit)
//! Into: log(A) + log(B) + log(C) > 0
//! Which becomes: -log(A) + -log(B) + -log(C) < 0 (negative cycle!)
//!
//! Success Criteria:
//! - Console logs: "Graph built: 54 Nodes, 124 Edges"
//! - Edge(WETH -> USDC) has weight X, Edge(USDC -> WETH) has weight ~-X

use alloy::primitives::Address;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef; // Required for .source() and .target() methods
use std::collections::HashMap;
use tracing::info;

use super::PoolState;

/// Edge data in our arbitrage graph
#[derive(Debug, Clone)]
pub struct EdgeData {
    /// Pool address (for executing the trade)
    pub pool_address: Address,
    /// The -log(effective_price) weight for Bellman-Ford
    /// effective_price = price * (1 - fee_rate)
    pub weight: f64,
    /// Original raw price (for profit calculation)
    pub price: f64,
    /// Fee tier of the pool (in hundredths of a bip, e.g., 3000 = 0.3%)
    pub fee: u32,
    /// Is this a V4 pool?
    pub is_v4: bool,
}

/// The arbitrage graph
///
/// Nodes: Token addresses
/// Edges: Trading paths with -log(price) weights
pub struct ArbitrageGraph {
    /// The underlying petgraph directed graph
    pub graph: DiGraph<Address, EdgeData>,
    /// Mapping from token address to node index
    pub token_to_node: HashMap<Address, NodeIndex>,
    /// Mapping from node index to token address
    pub node_to_token: HashMap<NodeIndex, Address>,
}

impl ArbitrageGraph {
    /// Create a new empty graph
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            token_to_node: HashMap::new(),
            node_to_token: HashMap::new(),
        }
    }

    /// Build the graph from a list of pool states
    pub fn from_pools(pools: &[PoolState]) -> Self {
        let mut graph = Self::new();

        for pool in pools {
            graph.add_pool(pool);
        }

        info!(
            "Graph built: {} Nodes, {} Edges",
            graph.graph.node_count(),
            graph.graph.edge_count()
        );

        graph
    }

    /// Add a pool to the graph (creates two directional edges)
    /// 
    /// For a pool with token0/token1:
    /// - Edge token0 -> token1: you sell token0 to get token1
    /// - Edge token1 -> token0: you sell token1 to get token0
    pub fn add_pool(&mut self, pool: &PoolState) {
        // Skip pools with zero liquidity
        if pool.liquidity == 0 {
            return;
        }

        // Get or create nodes for both tokens
        let node0 = self.get_or_create_node(pool.token0);
        let node1 = self.get_or_create_node(pool.token1);

        // Calculate raw price (token1 per token0)
        let raw_price = pool.raw_price();
        
        // Fee rate (e.g., 3000 = 0.3% = 0.003)
        let fee_rate = pool.fee as f64 / 1_000_000.0;
        
        // Effective prices after fees
        // When selling token0 for token1: you get price * (1 - fee)
        let effective_price_0_to_1 = raw_price * (1.0 - fee_rate);
        // When selling token1 for token0: you get (1/price) * (1 - fee)
        let effective_price_1_to_0 = (1.0 / raw_price) * (1.0 - fee_rate);

        // Edge: token0 -> token1 (selling token0 for token1)
        // Weight = -log(effective_price)
        if effective_price_0_to_1 > 0.0 {
            self.graph.add_edge(
                node0,
                node1,
                EdgeData {
                    pool_address: pool.address,
                    weight: -effective_price_0_to_1.ln(),
                    price: raw_price,
                    fee: pool.fee,
                    is_v4: pool.is_v4,
                },
            );
        }

        // Edge: token1 -> token0 (selling token1 for token0)
        if effective_price_1_to_0 > 0.0 {
            self.graph.add_edge(
                node1,
                node0,
                EdgeData {
                    pool_address: pool.address,
                    weight: -effective_price_1_to_0.ln(),
                    price: 1.0 / raw_price,
                    fee: pool.fee,
                    is_v4: pool.is_v4,
                },
            );
        }
    }

    /// Get the node index for a token, creating it if it doesn't exist
    fn get_or_create_node(&mut self, token: Address) -> NodeIndex {
        if let Some(&node) = self.token_to_node.get(&token) {
            return node;
        }

        let node = self.graph.add_node(token);
        self.token_to_node.insert(token, node);
        self.node_to_token.insert(node, token);
        node
    }

    /// Get node index for a token (if it exists)
    pub fn get_node(&self, token: Address) -> Option<NodeIndex> {
        self.token_to_node.get(&token).copied()
    }

    /// Get token address for a node
    pub fn get_token(&self, node: NodeIndex) -> Option<Address> {
        self.node_to_token.get(&node).copied()
    }

    /// Get the number of nodes (tokens)
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Get the number of edges (trading pairs, both directions)
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
    
    /// Get all token addresses in the graph
    pub fn all_tokens(&self) -> Vec<Address> {
        self.token_to_node.keys().copied().collect()
    }
    
    /// Print graph summary with token symbols
    pub fn print_summary(&self, token_symbols: &HashMap<Address, &str>) {
        info!("=== Graph Summary ===");
        info!("Nodes (tokens): {}", self.node_count());
        info!("Edges (trading paths): {}", self.edge_count());
        
        // Print some sample edges
        let mut edge_count = 0;
        for edge in self.graph.edge_references() {
            if edge_count >= 5 {
                info!("  ... and {} more edges", self.edge_count() - 5);
                break;
            }
            
            let from = self.get_token(edge.source()).unwrap();
            let to = self.get_token(edge.target()).unwrap();
            let data = edge.weight();
            
            let from_sym = token_symbols.get(&from).unwrap_or(&"???");
            let to_sym = token_symbols.get(&to).unwrap_or(&"???");
            
            info!(
                "  {} -> {}: weight={:.4}, price={:.6}, fee={}bps",
                from_sym, to_sym, data.weight, data.price, data.fee / 100
            );
            
            edge_count += 1;
        }
    }
}

impl Default for ArbitrageGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;

    fn make_test_pool(token0: &str, token1: &str, sqrt_price_x96: u128, fee: u32) -> PoolState {
        PoolState {
            address: Address::ZERO,
            token0: token0.parse().unwrap_or(Address::ZERO),
            token1: token1.parse().unwrap_or(Address::ZERO),
            sqrt_price_x96: U256::from(sqrt_price_x96),
            tick: 0,
            liquidity: 1000000,
            fee,
            is_v4: false,
        }
    }

    #[test]
    fn test_graph_construction() {
        // sqrt(1.0) * 2^96 ≈ 79228162514264337593543950336
        let pools = vec![
            make_test_pool(
                "0x0000000000000000000000000000000000000001",
                "0x0000000000000000000000000000000000000002",
                79228162514264337593543950336,
                3000, // 0.3% fee
            ),
            make_test_pool(
                "0x0000000000000000000000000000000000000002",
                "0x0000000000000000000000000000000000000003",
                79228162514264337593543950336,
                3000,
            ),
        ];

        let graph = ArbitrageGraph::from_pools(&pools);

        // 3 tokens
        assert_eq!(graph.node_count(), 3);
        // 2 pools × 2 directions = 4 edges
        assert_eq!(graph.edge_count(), 4);
    }

    #[test]
    fn test_fee_impact_on_weights() {
        // Same price, different fees
        let pool_low_fee = make_test_pool(
            "0x0000000000000000000000000000000000000001",
            "0x0000000000000000000000000000000000000002",
            79228162514264337593543950336, // sqrt(1) * 2^96
            500, // 0.05% fee
        );
        
        let pool_high_fee = make_test_pool(
            "0x0000000000000000000000000000000000000003",
            "0x0000000000000000000000000000000000000004",
            79228162514264337593543950336, // sqrt(1) * 2^96
            10000, // 1% fee
        );

        let mut graph = ArbitrageGraph::new();
        graph.add_pool(&pool_low_fee);
        graph.add_pool(&pool_high_fee);

        // Higher fee should result in higher weight (less favorable)
        let edges: Vec<_> = graph.graph.edge_references().collect();
        assert_eq!(edges.len(), 4);
    }
}