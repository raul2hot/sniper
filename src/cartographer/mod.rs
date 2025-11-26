//! Phase 1: The Cartographer - Enhanced Edition
//!
//! Supported DEXes (Curve removed for simplicity):
//! - Uniswap V3/V2, Sushiswap V2, PancakeSwap V3, Balancer V2

mod fetcher;
mod graph;

pub use fetcher::{PoolFetcher, PoolState, Dex, PoolType};
pub use graph::{ArbitrageGraph, EdgeData};
