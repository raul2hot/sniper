//! Phase 2: The Brain
//!
//! Responsible for:
//! - Finding negative cycles (arbitrage opportunities) using Bellman-Ford
//! - Filtering out dust profits that won't cover gas
//!
//! Submodules:
//! - `bellman_ford`: Bounded Bellman-Ford implementation (max 4 hops)
//! - `filter`: Profit calculation and filtering

mod bellman_ford;
mod filter;

pub use bellman_ford::{BoundedBellmanFord, ArbitrageCycle};
pub use filter::{ProfitFilter, ProfitAnalysis};