//! Phase 1: The Cartographer
//!
//! Responsible for:
//! - Fetching pool data from RPC
//! - Building the token graph with price edges
//!
//! Submodules:
//! - `fetcher`: RPC calls to get pool state
//! - `graph`: Graph construction from pool data

mod fetcher;
mod graph;

pub use fetcher::{PoolFetcher, PoolState};
pub use graph::{ArbitrageGraph, EdgeData};