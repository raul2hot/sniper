//! Wallet Signer Module - Production Transaction Signing
//!
//! This module handles:
//! - Loading private keys securely
//! - Signing transactions for flash loan execution
//! - Signing Flashbots bundle requests
//!
//! ⚠️  SECURITY WARNING:
//! - Never log or expose private keys
//! - Use environment variables, not hardcoded keys
//! - Consider hardware wallets for large amounts

use alloy_primitives::{Address, Bytes, B256, U256, keccak256};
use alloy_signer::Signer;
use alloy_signer_local::PrivateKeySigner;
use alloy_consensus::{TxLegacy, TxEip1559, SignableTransaction};
use eyre::{eyre, Result};
use std::str::FromStr;
use tracing::{debug, info, warn};

/// Wallet manager for signing operations
pub struct WalletManager {
    /// Main wallet for profit withdrawal and contract ownership
    profit_wallet: Option<PrivateKeySigner>,
    
    /// Separate wallet for Flashbots bundle signing (can be different)
    flashbots_signer: Option<PrivateKeySigner>,
    
    /// Chain ID for transaction signing
    chain_id: u64,
    
    /// Current nonce for the profit wallet
    current_nonce: u64,
}

impl WalletManager {
    /// Create a new wallet manager from environment variables
    pub fn from_env() -> Result<Self> {
        let chain_id = std::env::var("CHAIN_ID")
            .unwrap_or_else(|_| "1".to_string())
            .parse()
            .unwrap_or(1);
        
        // Load Flashbots signer (required for bundle submission)
        let flashbots_signer = match std::env::var("FLASHBOTS_SIGNER_KEY") {
            Ok(key) => {
                let key = key.trim_start_matches("0x");
                match PrivateKeySigner::from_str(key) {
                    Ok(signer) => {
                        info!("✓ Flashbots signer loaded: {:?}", signer.address());
                        Some(signer)
                    }
                    Err(e) => {
                        warn!("Failed to parse FLASHBOTS_SIGNER_KEY: {}", e);
                        None
                    }
                }
            }
            Err(_) => {
                debug!("FLASHBOTS_SIGNER_KEY not set");
                None
            }
        };
        
        // Load profit wallet (optional - only needed for production)
        let profit_wallet = match std::env::var("PROFIT_WALLET_PRIVATE_KEY") {
            Ok(key) => {
                let key = key.trim_start_matches("0x");
                match PrivateKeySigner::from_str(key) {
                    Ok(signer) => {
                        info!("✓ Profit wallet loaded: {:?}", signer.address());
                        Some(signer)
                    }
                    Err(e) => {
                        warn!("Failed to parse PROFIT_WALLET_PRIVATE_KEY: {}", e);
                        None
                    }
                }
            }
            Err(_) => {
                debug!("PROFIT_WALLET_PRIVATE_KEY not set (optional for simulation)");
                None
            }
        };
        
        Ok(Self {
            profit_wallet,
            flashbots_signer,
            chain_id,
            current_nonce: 0,
        })
    }
    
    /// Create with explicit keys (for testing)
    pub fn new(
        profit_wallet_key: Option<&str>,
        flashbots_signer_key: Option<&str>,
        chain_id: u64,
    ) -> Result<Self> {
        let profit_wallet = profit_wallet_key
            .map(|k| k.trim_start_matches("0x"))
            .map(|k| PrivateKeySigner::from_str(k))
            .transpose()?;
        
        let flashbots_signer = flashbots_signer_key
            .map(|k| k.trim_start_matches("0x"))
            .map(|k| PrivateKeySigner::from_str(k))
            .transpose()?;
        
        Ok(Self {
            profit_wallet,
            flashbots_signer,
            chain_id,
            current_nonce: 0,
        })
    }
    
    /// Check if we have a Flashbots signer configured
    pub fn has_flashbots_signer(&self) -> bool {
        self.flashbots_signer.is_some()
    }
    
    /// Check if we have a profit wallet configured
    pub fn has_profit_wallet(&self) -> bool {
        self.profit_wallet.is_some()
    }
    
    /// Get the Flashbots signer address
    pub fn flashbots_address(&self) -> Option<Address> {
        self.flashbots_signer.as_ref().map(|s| s.address())
    }
    
    /// Get the profit wallet address
    pub fn profit_wallet_address(&self) -> Option<Address> {
        self.profit_wallet.as_ref().map(|s| s.address())
    }
    
    /// Update nonce from the network
    pub async fn update_nonce(&mut self, rpc_url: &str) -> Result<()> {
        use alloy_provider::{Provider, ProviderBuilder};
        
        let wallet = self.profit_wallet.as_ref()
            .ok_or_else(|| eyre!("No profit wallet configured"))?;
        
        let provider = ProviderBuilder::new()
            .on_http(rpc_url.parse()?);
        
        self.current_nonce = provider.get_transaction_count(wallet.address()).await?;
        debug!("Updated nonce to: {}", self.current_nonce);
        
        Ok(())
    }
    
    /// Get and increment nonce
    pub fn get_nonce(&mut self) -> u64 {
        let nonce = self.current_nonce;
        self.current_nonce += 1;
        nonce
    }
    
    /// Sign a Flashbots bundle request
    /// Returns: "address:signature" format required by Flashbots
    pub async fn sign_flashbots_request(&self, body: &str) -> Result<String> {
        let signer = self.flashbots_signer.as_ref()
            .ok_or_else(|| eyre!("No Flashbots signer configured"))?;
        
        // Flashbots expects: keccak256(body) signed with private key
        // Format: "0xAddress:0xSignature"
        let message_hash = keccak256(body.as_bytes());
        
        // Sign the hash using async method
        let signature = signer.sign_hash(&message_hash).await
            .map_err(|e| eyre!("Failed to sign Flashbots request: {}", e))?;
        
        // Format as "address:signature"
        let sig_bytes = signature.as_bytes();
        let formatted = format!(
            "{:?}:0x{}",
            signer.address(),
            hex::encode(sig_bytes)
        );
        
        debug!("Signed Flashbots request with address: {:?}", signer.address());
        
        Ok(formatted)
    }
    
    /// Sign a transaction and return the raw signed bytes
    pub async fn sign_transaction(
        &mut self,
        to: Address,
        calldata: Bytes,
        value: U256,
        gas_limit: u64,
        gas_price: u128,
        priority_fee: u128,
    ) -> Result<Bytes> {
        // Check wallet exists first
        if self.profit_wallet.is_none() {
            return Err(eyre!("No profit wallet configured"));
        }
        
        // Get nonce before borrowing signer (to satisfy borrow checker)
        let nonce = self.get_nonce();
        
        // Now get the signer reference
        let signer = self.profit_wallet.as_ref().unwrap();
        
        // Build EIP-1559 transaction
        let tx = TxEip1559 {
            chain_id: self.chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas: gas_price,
            max_priority_fee_per_gas: priority_fee,
            to: alloy_primitives::TxKind::Call(to),
            value,
            input: calldata,
            access_list: Default::default(),
        };
        
        // Get the signing hash
        let sig_hash = tx.signature_hash();
        
        // Sign the hash
        let signature = signer.sign_hash(&sig_hash).await
            .map_err(|e| eyre!("Failed to sign transaction: {}", e))?;
        
        // Create signed transaction envelope
        let signed = alloy_consensus::TxEnvelope::Eip1559(
            alloy_consensus::Signed::new_unchecked(
                tx, 
                signature,
                B256::from(signer.address().into_word())
            )
        );
        
        // RLP encode the signed transaction
        let mut encoded = Vec::new();
        alloy_rlp::Encodable::encode(&signed, &mut encoded);
        
        debug!(
            "Signed EIP-1559 transaction: to={:?}, nonce={}, gas_limit={}, gas_price={}",
            to, nonce, gas_limit, gas_price
        );
        
        Ok(Bytes::from(encoded))
    }
    
    /// Create a simpler legacy transaction (for testing/compatibility)
    pub async fn sign_legacy_transaction(
        &mut self,
        to: Address,
        calldata: Bytes,
        value: U256,
        gas_limit: u64,
        gas_price: u128,
    ) -> Result<Bytes> {
        // Check wallet exists first
        if self.profit_wallet.is_none() {
            return Err(eyre!("No profit wallet configured"));
        }
        
        // Get nonce before borrowing signer (to satisfy borrow checker)
        let nonce = self.get_nonce();
        
        // Now get the signer reference
        let signer = self.profit_wallet.as_ref().unwrap();
        
        // Build legacy transaction
        let tx = TxLegacy {
            chain_id: Some(self.chain_id),
            nonce,
            gas_price,
            gas_limit,
            to: alloy_primitives::TxKind::Call(to),
            value,
            input: calldata,
        };
        
        // Get the signing hash
        let sig_hash = tx.signature_hash();
        
        // Sign the hash
        let signature = signer.sign_hash(&sig_hash).await
            .map_err(|e| eyre!("Failed to sign transaction: {}", e))?;
        
        // Create signed transaction envelope
        let signed = alloy_consensus::TxEnvelope::Legacy(
            alloy_consensus::Signed::new_unchecked(
                tx, 
                signature,
                B256::from(signer.address().into_word())
            )
        );
        
        // RLP encode the signed transaction
        let mut encoded = Vec::new();
        alloy_rlp::Encodable::encode(&signed, &mut encoded);
        
        debug!(
            "Signed legacy transaction: to={:?}, nonce={}, gas_limit={}, gas_price={}",
            to, nonce, gas_limit, gas_price
        );
        
        Ok(Bytes::from(encoded))
    }
}

/// Generate a new random wallet (for testing or creating new Flashbots signer)
pub fn generate_new_wallet() -> Result<(String, Address)> {
    let signer = PrivateKeySigner::random();
    let address = signer.address();
    
    // Get private key bytes
    let key_bytes = signer.credential().to_bytes();
    let private_key = format!("0x{}", hex::encode(key_bytes));
    
    Ok((private_key, address))
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_generate_wallet() {
        let (key, addr) = generate_new_wallet().unwrap();
        assert!(key.starts_with("0x"));
        assert_eq!(key.len(), 66); // 0x + 64 hex chars
        println!("Generated wallet: {:?}", addr);
        println!("Private key: {}", key);
    }
    
    #[tokio::test]
    async fn test_flashbots_signing() {
        // Use a test private key (DO NOT USE IN PRODUCTION)
        let test_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        
        let manager = WalletManager::new(None, Some(test_key), 1).unwrap();
        
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"eth_sendBundle"}"#;
        let signature = manager.sign_flashbots_request(body).await.unwrap();
        
        assert!(signature.contains("0x"));
        assert!(signature.contains(":")); // Format: address:signature
        println!("Signature: {}", signature);
    }
}