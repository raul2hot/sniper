//! Phase 3: The Simulator
//! 
//! Uses alloy Provider's call() for simulation instead of REVM.
//! This is a simpler, more stable approach that many production MEV bots use.
//!
//! Responsible for:
//! - Simulating swaps via eth_call (using Uniswap Quoter contracts)
//! - Calculating actual gas costs and profits
//! - Validating arbitrage cycles

mod quoter;
mod swap_simulator;

pub use quoter::UniV3Quoter;
pub use swap_simulator::SwapSimulator;
