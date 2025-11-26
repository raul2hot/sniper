//! UniswapV3/V2 Quoter - Provider-based Simulation
//!
//! Uses the official Uniswap QuoterV2 contract via eth_call for V3 quotes.
//! Uses constant product formula for V2 quotes.

use alloy_primitives::{Address, U256, address};
use alloy_sol_types::{sol, SolCall};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types::TransactionRequest;
use eyre::{eyre, Result};
use tracing::debug;

// ============================================
// SOLIDITY INTERFACES
// ============================================

sol! {
    /// Uniswap V3 QuoterV2 interface
    #[derive(Debug)]
    interface IQuoterV2 {
        struct QuoteExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint256 amountIn;
            uint24 fee;
            uint160 sqrtPriceLimitX96;
        }
        
        function quoteExactInputSingle(QuoteExactInputSingleParams memory params)
            external
            returns (
                uint256 amountOut,
                uint160 sqrtPriceX96After,
                uint32 initializedTicksCrossed,
                uint256 gasEstimate
            );
    }
    
    /// Uniswap V3 Pool interface (for fee lookup)
    #[derive(Debug)]
    interface IUniswapV3Pool {
        function fee() external view returns (uint24);
        function token0() external view returns (address);
        function token1() external view returns (address);
    }
    
    /// Uniswap V2 Pair interface
    #[derive(Debug)]
    interface IUniswapV2Pair {
        function getReserves() external view returns (
            uint112 reserve0,
            uint112 reserve1,
            uint32 blockTimestampLast
        );
        function token0() external view returns (address);
        function token1() external view returns (address);
    }
}

/// Quote result from simulation
#[derive(Debug, Clone)]
pub struct QuoteResult {
    pub amount_in: U256,
    pub amount_out: U256,
    pub pool: Address,
    pub zero_for_one: bool,
    pub gas_estimate: u64,
}

/// Official Uniswap V3 QuoterV2 address on mainnet
const QUOTER_V2: Address = address!("61fFE014bA17989E743c5F6cB21bF9697530B21e");

/// UniV3 Quoter using Provider's eth_call
pub struct UniV3Quoter {
    rpc_url: String,
}

impl UniV3Quoter {
    pub fn new(rpc_url: String) -> Self {
        Self { rpc_url }
    }
    
    async fn call_contract(&self, to: Address, calldata: Vec<u8>) -> Result<Vec<u8>> {
        let provider = ProviderBuilder::new()
            .on_http(self.rpc_url.parse()?);
        
        let tx = TransactionRequest::default()
            .to(to)
            .input(calldata.into());
        
        let result = provider.call(tx).await
            .map_err(|e| eyre!("eth_call failed: {}", e))?;
        
        Ok(result.to_vec())
    }
    
    /// Quote a V3 swap using the official QuoterV2 contract
    pub async fn quote_v3(
        &self,
        pool: Address,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
        fee: u32,
    ) -> Result<QuoteResult> {
        debug!(
            "Quoting V3 swap: {} -> {} via {:?}, amount: {}",
            token_in, token_out, pool, amount_in
        );
        
        // Get token0 to determine direction
        let token0 = self.get_pool_token0(pool).await?;
        let zero_for_one = token_in == token0;
        
        // Build the quote call with U160::ZERO for sqrtPriceLimitX96
        let params = IQuoterV2::QuoteExactInputSingleParams {
            tokenIn: token_in,
            tokenOut: token_out,
            amountIn: amount_in,
            fee: fee.try_into().unwrap_or(3000u32).try_into().unwrap(),
            sqrtPriceLimitX96: alloy_primitives::Uint::<160, 3>::ZERO,
        };
        
        let calldata = IQuoterV2::quoteExactInputSingleCall { params }.abi_encode();
        
        match self.call_contract(QUOTER_V2, calldata).await {
            Ok(output) => {
                // Decode the output
                let decoded = IQuoterV2::quoteExactInputSingleCall::abi_decode_returns(&output)
                    .map_err(|e| eyre!("Failed to decode quoter output: {}", e))?;
                
                let gas: u64 = decoded.gasEstimate.to();
                
                Ok(QuoteResult {
                    amount_in,
                    amount_out: decoded.amountOut,
                    pool,
                    zero_for_one,
                    gas_estimate: gas,
                })
            }
            Err(e) => {
                // The quoter might revert if the swap would fail
                Err(eyre!("Quote failed: {}", e))
            }
        }
    }
    
    /// Quote a V2 swap using constant product formula
    pub async fn quote_v2(
        &self,
        pool: Address,
        token_in: Address,
        amount_in: U256,
    ) -> Result<QuoteResult> {
        debug!(
            "Quoting V2 swap: {} via {:?}, amount: {}",
            token_in, pool, amount_in
        );
        
        // Get reserves
        let calldata = IUniswapV2Pair::getReservesCall {}.abi_encode();
        let output = self.call_contract(pool, calldata).await?;
        
        let reserves = IUniswapV2Pair::getReservesCall::abi_decode_returns(&output)
            .map_err(|e| eyre!("Failed to decode reserves: {}", e))?;
        
        // Get token0 to determine direction
        let token0 = self.get_v2_token0(pool).await?;
        let zero_for_one = token_in == token0;
        
        // Convert reserves to u128 then to U256
        let r0: u128 = reserves.reserve0.to();
        let r1: u128 = reserves.reserve1.to();
        
        let (reserve_in, reserve_out) = if zero_for_one {
            (U256::from(r0), U256::from(r1))
        } else {
            (U256::from(r1), U256::from(r0))
        };
        
        // Constant product formula with 0.3% fee
        // amountOut = (amountIn * 997 * reserveOut) / (reserveIn * 1000 + amountIn * 997)
        let amount_in_with_fee = amount_in * U256::from(997);
        let numerator = amount_in_with_fee * reserve_out;
        let denominator = reserve_in * U256::from(1000) + amount_in_with_fee;
        
        if denominator == U256::ZERO {
            return Err(eyre!("Division by zero in V2 quote"));
        }
        
        let amount_out = numerator / denominator;
        
        Ok(QuoteResult {
            amount_in,
            amount_out,
            pool,
            zero_for_one,
            gas_estimate: 100_000, // V2 swaps are cheaper
        })
    }
    
    /// Get token0 for a V3 pool
    async fn get_pool_token0(&self, pool: Address) -> Result<Address> {
        let calldata = IUniswapV3Pool::token0Call {}.abi_encode();
        let output = self.call_contract(pool, calldata).await?;
        
        let decoded = IUniswapV3Pool::token0Call::abi_decode_returns(&output)
            .map_err(|e| eyre!("Failed to decode token0: {}", e))?;
        
        Ok(decoded)
    }
    
    /// Get token0 for a V2 pair
    async fn get_v2_token0(&self, pool: Address) -> Result<Address> {
        let calldata = IUniswapV2Pair::token0Call {}.abi_encode();
        let output = self.call_contract(pool, calldata).await?;
        
        let decoded = IUniswapV2Pair::token0Call::abi_decode_returns(&output)
            .map_err(|e| eyre!("Failed to decode token0: {}", e))?;
        
        Ok(decoded)
    }
    
    /// Get fee tier for a V3 pool
    pub async fn get_pool_fee(&self, pool: Address) -> Result<u32> {
        let calldata = IUniswapV3Pool::feeCall {}.abi_encode();
        let output = self.call_contract(pool, calldata).await?;
        
        let decoded = IUniswapV3Pool::feeCall::abi_decode_returns(&output)
            .map_err(|e| eyre!("Failed to decode fee: {}", e))?;
        
        Ok(decoded.to())
    }
}
