//! Phase 2: The Brain
//!
//! Responsible for:
//! - Finding negative cycles (arbitrage opportunities) using Bellman-Ford
//! - Filtering out dust profits that won't cover gas

mod bellman_ford;
mod filter;

pub use bellman_ford::{BoundedBellmanFord, ArbitrageCycle};
pub use filter::ProfitFilter;
