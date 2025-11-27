//! Phase 3: The Simulator - Enhanced Edition
//! 
//! Uses alloy Provider's call() for simulation.

mod quoter;
pub mod swap_simulator;

pub use quoter::UniV3Quoter;
pub use swap_simulator::{SwapSimulator, ArbitrageSimulation, SwapResult, LiquidityTier};
