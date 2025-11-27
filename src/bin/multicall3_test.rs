//! Multicall3 Test Script
//!
//! Run with: cargo run --bin multicall3_test
//!
//! This tests the Multicall3 integration independently

use alloy_primitives::{Address, Bytes, U256, address};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{sol, SolCall};
use alloy_rpc_types::TransactionRequest;
use eyre::Result;
use std::time::Instant;

sol! {
    interface IMulticall3 {
        struct Call3 {
            address target;
            bool allowFailure;
            bytes callData;
        }
        
        struct Result {
            bool success;
            bytes returnData;
        }
        
        function aggregate3(Call3[] calldata calls) 
            external payable returns (Result[] memory returnData);
    }
    
    interface IUniswapV3Pool {
        function slot0() external view returns (
            uint160 sqrtPriceX96, int24 tick, uint16 observationIndex,
            uint16 observationCardinality, uint16 observationCardinalityNext,
            uint8 feeProtocol, bool unlocked
        );
        function liquidity() external view returns (uint128);
        function token0() external view returns (address);
        function token1() external view returns (address);
        function fee() external view returns (uint24);
    }
}

const MULTICALL3: Address = address!("cA11bde05977b3631167028862bE2a173976CA11");

// Test pools (USDC/WETH on various fee tiers)
const TEST_POOLS: &[&str] = &[
    "0x88e6A0c2dDD26FEEb64F039a2c41296FcB3f5640", // USDC/WETH 0.05%
    "0x8ad599c3A0ff1De082011EFDDc58f1908eb6e6D8", // USDC/WETH 0.3%
    "0x4e68Ccd3E89f51C3074ca5072bbAC773960dFa36", // WETH/USDT 0.3%
    "0xCBCdF9626bC03E24f779434178A73a0B4bad62eD", // WBTC/WETH 0.3%
];

#[tokio::main]
async fn main() -> Result<()> {
    println!("🧪 Multicall3 Test Script");
    println!("========================\n");
    
    // Get RPC URL from environment
    let rpc_url = std::env::var("RPC_URL")
        .unwrap_or_else(|_| "https://eth.llamarpc.com".to_string());
    
    println!("📡 RPC: {}\n", &rpc_url[..50.min(rpc_url.len())]);
    
    let provider = ProviderBuilder::new()
        .on_http(rpc_url.parse()?);
    
    // ============================================
    // TEST 1: Individual Calls (Old Method)
    // ============================================
    println!("📊 Test 1: Individual RPC Calls");
    println!("--------------------------------");
    
    let start = Instant::now();
    let mut individual_results = Vec::new();
    
    for pool_addr in TEST_POOLS {
        let addr: Address = pool_addr.parse()?;
        
        // slot0
        let tx = TransactionRequest::default()
            .to(addr)
            .input(IUniswapV3Pool::slot0Call {}.abi_encode().into());
        let result = provider.call(tx).await?;
        let slot0 = IUniswapV3Pool::slot0Call::abi_decode_returns(&result)?;
        
        // liquidity
        let tx = TransactionRequest::default()
            .to(addr)
            .input(IUniswapV3Pool::liquidityCall {}.abi_encode().into());
        let result = provider.call(tx).await?;
        let liquidity = IUniswapV3Pool::liquidityCall::abi_decode_returns(&result)?;
        
        individual_results.push((addr, slot0.sqrtPriceX96, liquidity));
    }
    
    let individual_time = start.elapsed();
    println!("   Fetched {} pools in {:?}", TEST_POOLS.len(), individual_time);
    println!("   RPC calls: {}", TEST_POOLS.len() * 2);
    
    // ============================================
    // TEST 2: Multicall3 Batched Calls (New Method)
    // ============================================
    println!("\n📊 Test 2: Multicall3 Batched Calls");
    println!("------------------------------------");
    
    let start = Instant::now();
    
    // Build batch
    let mut calls: Vec<IMulticall3::Call3> = Vec::new();
    for pool_addr in TEST_POOLS {
        let addr: Address = pool_addr.parse()?;
        
        // slot0
        calls.push(IMulticall3::Call3 {
            target: addr,
            allowFailure: true,
            callData: IUniswapV3Pool::slot0Call {}.abi_encode().into(),
        });
        
        // liquidity
        calls.push(IMulticall3::Call3 {
            target: addr,
            allowFailure: true,
            callData: IUniswapV3Pool::liquidityCall {}.abi_encode().into(),
        });
    }
    
    // Execute batch
    let calldata = IMulticall3::aggregate3Call { calls }.abi_encode();
    let tx = TransactionRequest::default()
        .to(MULTICALL3)
        .input(calldata.into());
    
    let result = provider.call(tx).await?;
    let decoded = IMulticall3::aggregate3Call::abi_decode_returns(&result)?;
    
    let multicall_time = start.elapsed();
    println!("   Fetched {} pools in {:?}", TEST_POOLS.len(), multicall_time);
    println!("   RPC calls: 1");
    
    // Parse results
    let mut multicall_results = Vec::new();
    for (i, pool_addr) in TEST_POOLS.iter().enumerate() {
        let addr: Address = pool_addr.parse()?;
        let offset = i * 2;
        
        let slot0 = if decoded.returnData[offset].success {
            IUniswapV3Pool::slot0Call::abi_decode_returns(&decoded.returnData[offset].returnData)
                .ok()
                .map(|s| s.sqrtPriceX96)
        } else {
            None
        };
        
        let liquidity = if decoded.returnData[offset + 1].success {
            IUniswapV3Pool::liquidityCall::abi_decode_returns(&decoded.returnData[offset + 1].returnData)
                .ok()
        } else {
            None
        };
        
        multicall_results.push((addr, slot0, liquidity));
    }
    
    // ============================================
    // RESULTS COMPARISON
    // ============================================
    println!("\n📈 Results Comparison");
    println!("---------------------");
    
    let speedup = individual_time.as_millis() as f64 / multicall_time.as_millis() as f64;
    println!("   Individual: {:?}", individual_time);
    println!("   Multicall3: {:?}", multicall_time);
    println!("   Speedup:    {:.1}x faster", speedup);
    println!("   RPC reduction: {}x fewer calls", TEST_POOLS.len() * 2);
    
    // Verify data matches
    println!("\n✅ Data Verification");
    println!("--------------------");
    
    let mut all_match = true;
    for (i, ((addr1, price1, liq1), (addr2, price2, liq2))) in 
        individual_results.iter().zip(multicall_results.iter()).enumerate() 
    {
        let price_match = price2.map(|p| p == *price1).unwrap_or(false);
        let liq_match = liq2.map(|l| l == *liq1).unwrap_or(false);
        
        let status = if price_match && liq_match { "✓" } else { "✗" };
        println!("   Pool {}: {} (price: {}, liq: {})", i, status, price_match, liq_match);
        
        if !price_match || !liq_match {
            all_match = false;
        }
    }
    
    println!("\n{}", if all_match {
        "✅ All data matches! Multicall3 is working correctly."
    } else {
        "❌ Some data mismatches. Check the implementation."
    });
    
    // ============================================
    // COST ANALYSIS
    // ============================================
    println!("\n💰 Cost Analysis (Alchemy Pricing)");
    println!("-----------------------------------");
    println!("   Individual calls: {} × 25 CU = {} CU", TEST_POOLS.len() * 2, TEST_POOLS.len() * 2 * 25);
    println!("   Multicall3:       1 × 25 CU = 25 CU");
    println!("   Savings:          {}x fewer compute units", TEST_POOLS.len() * 2);
    
    println!("\n🎉 Test complete!");
    
    Ok(())
}