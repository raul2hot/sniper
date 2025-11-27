//! Phase 3: The Simulator - FIXED Edition
//! 
//! Uses alloy Provider's call() for simulation.
//!
//! FIXES:
//! - Proper gas price handling with minimum floor
//! - Dynamic simulation sizing based on token liquidity
//! - Better error reporting

mod quoter;
pub mod swap_simulator;

pub use quoter::UniV3Quoter;
pub use swap_simulator::{SwapSimulator, ArbitrageSimulation, SwapResult, LiquidityTier};