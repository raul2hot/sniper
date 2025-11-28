//! Wallet Generation Utility
//!
//! Run with: cargo run --bin generate-wallet
//!
//! This generates a new Ethereum wallet for use as:
//! - Flashbots bundle signer
//! - Profit wallet (NOT RECOMMENDED - use hardware wallet for large amounts)

use alloy_signer_local::PrivateKeySigner;

fn main() {
    println!();
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║          SNIPER WALLET GENERATOR                           ║");
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();
    
    // Generate a new random wallet
    let signer = PrivateKeySigner::random();
    let address = signer.address();
    
    // Get private key bytes
    let key_bytes = signer.credential().to_bytes();
    let private_key = format!("0x{}", hex::encode(key_bytes));
    
    println!("🔑 NEW WALLET GENERATED");
    println!();
    println!("   Address:     {:?}", address);
    println!("   Private Key: {}", private_key);
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("⚠️  SECURITY WARNINGS:");
    println!();
    println!("   1. NEVER share your private key with anyone");
    println!("   2. NEVER commit it to git or any public repository");
    println!("   3. Store it securely (password manager, encrypted file)");
    println!("   4. For large amounts, use a hardware wallet instead");
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("📝 TO USE THIS WALLET:");
    println!();
    println!("   For Flashbots signing (add to .env):");
    println!("   FLASHBOTS_SIGNER_KEY={}", private_key);
    println!();
    println!("   For profit wallet (add to .env - NOT RECOMMENDED):");
    println!("   PROFIT_WALLET_PRIVATE_KEY={}", private_key);
    println!("   PROFIT_WALLET_ADDRESS={:?}", address);
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("💡 RECOMMENDED SETUP:");
    println!();
    println!("   1. Use this wallet ONLY for Flashbots signing");
    println!("   2. Use a SEPARATE hardware wallet for profits");
    println!("   3. The Flashbots signer doesn't need any ETH");
    println!("   4. Profits go to your executor contract, then you withdraw");
    println!();
}
