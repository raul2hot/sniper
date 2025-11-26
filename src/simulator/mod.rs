//! Phase 3: The Simulator
//! 
//! Responsible for:
//! - Spinning up a local EVM instance (REVM)
//! - Checking V4 hooks for compatibility
//! - Simulating the full arbitrage transaction
//! 
//! Submodules:
//! - `revm_setup`: REVM initialization with RPC forking
//! - `hook_checker`: V4 hook analysis

mod revm_setup;
mod hook_checker;

pub use revm_setup::*;
pub use hook_checker::*;