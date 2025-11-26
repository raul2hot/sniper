//! REVM Setup - Phase 3
//!
//! Spins up a local EVM instance that forks the current block.
//! Uses AlloyDB to fetch state on-demand from the RPC provider.

use alloy_primitives::{Address, Bytes, U256, keccak256};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_network::Ethereum;
use alloy_transport_http::{Client, Http};
use eyre::{eyre, Result};
use revm::{
    db::{AlloyDB, CacheDB},
    primitives::{
        AccountInfo, Bytecode, ExecutionResult, Output, TransactTo,
        SpecId,
    },
    Evm,
};
use std::sync::Arc;
use tracing::{debug, info};

/// Type alias for our cached database backed by Alloy RPC
pub type AlloyCacheDB = CacheDB<AlloyDB<Http<Client>, Ethereum, Arc<RootProvider<Http<Client>>>>>;

/// The EVM Simulator - wraps REVM with AlloyDB for mainnet forking
pub struct EvmSimulator {
    cache_db: AlloyCacheDB,
    provider: Arc<RootProvider<Http<Client>>>,
}

impl EvmSimulator {
    /// Create a new simulator connected to the given RPC URL
    pub async fn new(rpc_url: &str) -> Result<Self> {
        info!("Initializing REVM simulator with RPC: {}", rpc_url);
        
        let provider = ProviderBuilder::new()
            .on_http(rpc_url.parse()?);
        let provider = Arc::new(provider);
        
        // Get current block number for logging
        let block_number = provider.get_block_number().await?;
        info!("Forking at block: {}", block_number);
        
        // Create AlloyDB that fetches state on-demand
        let alloy_db = AlloyDB::new(provider.clone(), Default::default());
        let cache_db = CacheDB::new(alloy_db);
        
        Ok(Self {
            cache_db,
            provider,
        })
    }
    
    /// Execute a call without committing state changes
    /// Returns the output bytes on success
    pub fn call(
        &mut self,
        from: Address,
        to: Address,
        calldata: Bytes,
        value: U256,
    ) -> Result<Bytes> {
        let mut evm = Evm::builder()
            .with_db(&mut self.cache_db)
            .with_spec_id(SpecId::CANCUN)
            .modify_tx_env(|tx| {
                tx.caller = from;
                tx.transact_to = TransactTo::Call(to);
                tx.data = calldata;
                tx.value = value;
                tx.gas_limit = 1_000_000;
            })
            .build();
        
        let result = evm.transact()?;
        
        match result.result {
            ExecutionResult::Success { output: Output::Call(value), .. } => Ok(value),
            ExecutionResult::Success { .. } => Err(eyre!("Unexpected success type")),
            ExecutionResult::Revert { output, .. } => {
                Err(eyre!("Call reverted: 0x{}", hex::encode(&output)))
            }
            ExecutionResult::Halt { reason, .. } => {
                Err(eyre!("Call halted: {:?}", reason))
            }
        }
    }
    
    /// Execute a call that we expect to revert (for quoter pattern)
    /// Returns the revert data which contains the output
    pub fn call_expecting_revert(
        &mut self,
        from: Address,
        to: Address,
        calldata: Bytes,
    ) -> Result<Bytes> {
        let mut evm = Evm::builder()
            .with_db(&mut self.cache_db)
            .with_spec_id(SpecId::CANCUN)
            .modify_tx_env(|tx| {
                tx.caller = from;
                tx.transact_to = TransactTo::Call(to);
                tx.data = calldata;
                tx.value = U256::ZERO;
                tx.gas_limit = 1_000_000;
            })
            .build();
        
        let result = evm.transact()?;
        
        match result.result {
            ExecutionResult::Revert { output, .. } => Ok(output),
            ExecutionResult::Success { .. } => {
                Err(eyre!("Expected revert but call succeeded"))
            }
            ExecutionResult::Halt { reason, .. } => {
                Err(eyre!("Call halted: {:?}", reason))
            }
        }
    }
    
    /// Execute a call and commit the state changes
    /// Returns (output bytes, gas used)
    pub fn call_and_commit(
        &mut self,
        from: Address,
        to: Address,
        calldata: Bytes,
        value: U256,
    ) -> Result<(Bytes, u64)> {
        let mut evm = Evm::builder()
            .with_db(&mut self.cache_db)
            .with_spec_id(SpecId::CANCUN)
            .modify_tx_env(|tx| {
                tx.caller = from;
                tx.transact_to = TransactTo::Call(to);
                tx.data = calldata;
                tx.value = value;
                tx.gas_limit = 1_000_000;
            })
            .build();
        
        let result = evm.transact_commit()?;
        
        match result {
            ExecutionResult::Success { output: Output::Call(value), gas_used, .. } => {
                Ok((value, gas_used))
            }
            ExecutionResult::Success { gas_used, .. } => {
                Ok((Bytes::new(), gas_used))
            }
            ExecutionResult::Revert { output, gas_used, .. } => {
                Err(eyre!("Call reverted after {} gas: 0x{}", gas_used, hex::encode(&output)))
            }
            ExecutionResult::Halt { reason, gas_used, .. } => {
                Err(eyre!("Call halted after {} gas: {:?}", gas_used, reason))
            }
        }
    }
    
    /// Insert a mock account with custom bytecode
    /// Useful for deploying custom quoter contracts
    pub fn insert_account_with_bytecode(
        &mut self,
        address: Address,
        bytecode: Bytes,
    ) -> Result<()> {
        let bytecode = Bytecode::new_raw(bytecode);
        let code_hash = bytecode.hash_slow();
        
        let acc_info = AccountInfo {
            balance: U256::ZERO,
            nonce: 0,
            code: Some(bytecode),
            code_hash,
        };
        
        self.cache_db.insert_account_info(address, acc_info);
        Ok(())
    }
    
    /// Insert a storage value for an account
    pub fn insert_storage(
        &mut self,
        address: Address,
        slot: U256,
        value: U256,
    ) -> Result<()> {
        self.cache_db.insert_account_storage(address, slot, value)?;
        Ok(())
    }
    
    /// Insert a mapping storage slot (for ERC20 balances, etc.)
    /// Uses keccak256(abi.encode(key, slot)) as per Solidity storage layout
    pub fn insert_mapping_storage(
        &mut self,
        contract: Address,
        mapping_slot: U256,
        key: Address,
        value: U256,
    ) -> Result<()> {
        // Solidity mapping storage: keccak256(abi.encode(key, slot))
        let mut encoded = [0u8; 64];
        encoded[12..32].copy_from_slice(key.as_slice());
        mapping_slot.to_be_bytes::<32>().iter().enumerate().for_each(|(i, b)| {
            encoded[32 + i] = *b;
        });
        
        let hashed_slot = keccak256(&encoded);
        self.cache_db.insert_account_storage(contract, U256::from_be_bytes(hashed_slot.0), value)?;
        
        Ok(())
    }
    
    /// Get the underlying provider for RPC calls
    pub fn provider(&self) -> &Arc<RootProvider<Http<Client>>> {
        &self.provider
    }
    
    /// Get current gas price in gwei
    pub async fn gas_price_gwei(&self) -> Result<f64> {
        let gas_price = self.provider.get_gas_price().await?;
        Ok(gas_price as f64 / 1e9)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_simulator_creation() {
        // This test requires RPC_URL to be set
        if std::env::var("RPC_URL").is_err() {
            return;
        }
        
        let rpc_url = std::env::var("RPC_URL").unwrap();
        let sim = EvmSimulator::new(&rpc_url).await;
        assert!(sim.is_ok());
    }
}
