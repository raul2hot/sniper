//! Phase 1: The Cartographer (Data Ingest)
//! 
//! Now with Multicall3 for 80x fewer RPC calls!

mod fetcher;
mod graph;

pub use fetcher::{PoolFetcher, PoolState, Dex, PoolType, get_token_decimals, get_all_known_pools, PoolInfo};
pub use graph::{ArbitrageGraph, EdgeData};