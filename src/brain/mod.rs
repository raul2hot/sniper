//! Phase 2: The Brain - FIXED Edition
//!
//! Responsible for:
//! - Finding negative cycles (arbitrage opportunities) using Bellman-Ford
//! - Filtering out dust profits that won't cover gas
//! - Validating cycle structure (no duplicate nodes)

mod bellman_ford;
mod filter;

pub use bellman_ford::{BoundedBellmanFord, ArbitrageCycle, format_cycle_path};
pub use filter::{ProfitFilter, ProfitAnalysis};