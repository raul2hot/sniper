//! Phase 1: The Cartographer
//!
//! Responsible for:
//! - Fetching pool data from RPC across multiple DEXes
//! - Building the token graph with price edges
//!
//! Supported DEXes:
//! - Uniswap V3 (concentrated liquidity)
//! - Uniswap V2 (constant product)
//! - Sushiswap V3 (concentrated liquidity)
//! - Sushiswap V2 (constant product)
//!
//! Submodules:
//! - `fetcher`: RPC calls to get pool state
//! - `graph`: Graph construction from pool data

mod fetcher;
mod graph;

pub use fetcher::{PoolFetcher, PoolState, Dex, PoolType};
pub use graph::{ArbitrageGraph, EdgeData};
